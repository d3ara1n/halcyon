//! init：持久 root supervisor 与消息/信号/隧道/Runnel 全通路集成验证负载。
//!
//! 剧本：
//! 1. 建立 services Job 拓扑：全部服务入域；pm_domain 是授给 pm 的显式
//!    委托域（域内预置 Running 靶，JobControl 经 StartupBlock grants 交付，
//!    init 保留复制件作直接收束权）；acceptance 域收容一次性验收自测；
//! 2. IPC 自测：Mailbox 自发自收、badged sender、send-once、流控电平；
//! 3. 数据面：Runnel 隧道 Invitation 经 pm sender 转移，阻塞读 8192 字节
//!    （跨回绕、到达移交唤醒）并校验；与 pm 协作验证发送侧流控唤醒；
//! 4. Job 管理面验收：封口与完成传播、派生兑底、递归 JobKill 组合、
//!    seal 闸门与枚举收敛；acceptance 域用完即收；
//! 5. 监督闭环：全部服务等 REAPABLE|CLOSED → Drain 至 Complete → 终态
//!    快照 → close；pm_domain 由 pm 自行收束，未收束时 init 兜底；
//! 6. 等 pm 退出后的 PEER_CLOSED 终态位，关闭本端（帧归还）；
//! 7. 验证系统复位 capability 的负路径，由 init 显式提交 Shutdown；平台拒绝时
//!    保持 root supervisor 稳态等待。
//!
//! 默认构建运行确定性的 core 验收；`acceptance-stress` feature 在同一用户态
//! 编排器中追加重复压力、最小预算 Drain 与完整竞态矩阵。内核不感知该档位。

#![no_std]

use libbudget::Budget;
use libexecution::{
    ExecutionResource,
    runtime::{
        Advance, Input, RequestFailure, Requests, Runtime, SourceId, SourceKind, Step, Task,
    },
};
use libfal::{
    authority::FalRights,
    client::{Client as FalClient, SubscriptionEvent},
    node::NodeKind,
    protocol::{self, Request as FalRequest, Response as FalResponse},
    route,
    value::{ExportMode, ExportPolicy, Protocol as ValueProtocol, Value},
};
use libfs::{
    client::Transport as FalTransport,
    prefix::{DirectoryGrant, PrefixTable},
};
use libprocess::{
    DEFAULT_SUPERVISION_POLICY, DERIVED_CONTROL_RIGHTS, JobCollector, RequiredLaunchSet,
    SpawnRequest, SuperviseResult, SuperviseSink, SuperviseTask, SupervisionCause,
    SupervisionStage, SupervisionTarget, collect_process, enumerate_members, spawn,
};
use librpc::{
    CallCause, CallError, CallPhase, Caller, FrameRejection, Outbox, OutboxResult, Request,
    RequestContext, RpcMessageKind, RpcPrefix,
};
use librunnel::{ConsumerReady, blocking};
use libservice::{client as service_client, protocol as service_protocol, record::ServiceRecord};
use rinlib::ipc::tunnel as tunnel_sys;
use rinlib::ipc::wait_set::WaitSet;
use rinlib::ipc::{
    capability::Capability,
    message::{Mailbox, MailboxSender, SendOnce, send_once},
    packet::Packet,
};
use rinlib::memory_pool::MemoryPool;
#[cfg(feature = "acceptance-stress")]
use rinlib::shared::proc::ProcessDrainStatus;
use rinlib::time::Deadline;
use rinlib::{
    env,
    ipc::{
        message::{create, discard, make_send_once, mint_sender, receive, send_raw, wait_message},
        notification,
        object::{close, duplicate},
        wait::wait_many,
    },
    memory_object::MemoryObject,
    mm::{MappedRegion, Placement},
    preclude::*,
    process,
    shared::{
        call::SystemCallError,
        mem::MemoryProtection,
        memory_object::MemoryObjectState,
        message::{HandleMove, MAILBOX_CAPACITY},
        object::{Handle, ObjectSignals, Rights},
        proc::{
            ExecutionProfile, HandleGrant, JobMemberKind, JobState, ProcessExitReason, ProcessState,
        },
        reset::{ResetAction, ResetReason},
        startup::initial,
        wait::{WaitItem, WaitReason},
    },
    system,
};

mod supervisor;
use supervisor::RootSupervisor;

mod building;
mod public_ipc;
#[cfg(feature = "acceptance-stress")]
mod race;
mod time_checks;
use building::build_spin_building;
#[cfg(feature = "acceptance-stress")]
use race::race_matrix;

/// 受监督服务：init 保留的 control 与 pid。
struct Supervised {
    pid: u64,
    control: Handle,
}

struct LaunchedServices {
    first_fs: AcceptedFsProvider,
    registry_root_watch: libfal::client::Subscription,
    pm_mailbox: Handle,
    fs_bootstrap_second: Handle,
    fs_release_second: Handle,
    fs_route_second: Handle,
    fs_bootstrap_second_sender: Handle,
    fs_release_second_owner: Handle,
    fs_route_second_owner: Handle,
    fs_second_scope: Handle,
    fs_second_image: &'static [u8],
    test_fal_image: &'static [u8],
    target_image: Option<&'static [u8]>,
    hammer_image: Option<&'static [u8]>,
}

enum RunFailure {
    Message(&'static str),
}

impl From<&'static str> for RunFailure {
    fn from(message: &'static str) -> Self {
        Self::Message(message)
    }
}

/// 拓扑语义名登记：内核无名字概念，init 在建域与启动时记录 jid/pid
/// 到名字的映射，供拓扑打印对照（条目个位数，线性查找）。
struct TopologyNames {
    jobs: alloc::vec::Vec<(u64, alloc::string::String)>,
    processes: alloc::vec::Vec<(u64, alloc::string::String)>,
}

impl TopologyNames {
    fn new() -> Self {
        Self {
            jobs: alloc::vec::Vec::new(),
            processes: alloc::vec::Vec::new(),
        }
    }

    fn register_job(&mut self, handle: Handle, name: &str) {
        if let Ok(snapshot) = process::query_job(handle) {
            self.jobs
                .push((snapshot.jid, alloc::string::String::from(name)));
        }
    }

    fn register_process(&mut self, pid: u64, name: &str) {
        self.processes
            .push((pid, alloc::string::String::from(name)));
    }

    fn job_name(&self, jid: u64) -> &str {
        self.jobs
            .iter()
            .find(|(id, _)| *id == jid)
            .map(|(_, name)| name.as_str())
            .unwrap_or("?")
    }

    fn process_name(&self, pid: u64) -> &str {
        self.processes
            .iter()
            .find(|(id, _)| *id == pid)
            .map(|(_, name)| name.as_str())
            .unwrap_or("?")
    }
}

fn job_state_name(state: u32) -> &'static str {
    match state {
        0 => "Open",
        1 => "Sealed",
        2 => "Dead",
        _ => "unknown",
    }
}

fn process_state_name(state: u32) -> &'static str {
    match state {
        0 => "Building",
        1 => "Running",
        2 => "Terminating",
        3 => "Dead",
        _ => "unknown",
    }
}

fn exit_reason_name(reason: u32) -> &'static str {
    match reason {
        0 => "None",
        1 => "Exited",
        2 => "Fault",
        3 => "Killed",
        4 => "Abandoned",
        _ => "unknown",
    }
}

/// 服务 ProcessControl 的监督基准权利：init 保留 control 用于查询、等待、
/// 终止与受保护收束，并可复制/运输/转授。
const SUPERVISOR_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);

/// JobControl 满权（scratch child Job 请求基准；root Handle 持超集）。
const JOB_FULL_RIGHTS: Rights = Rights::from_raw(
    Rights::CREATE.raw()
        | Rights::MANAGE.raw()
        | Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);

/// 委托域授出的 JobControl 权利：seal/派生 kill 需 MANAGE，枚举与查询
/// 需 READ，等 CLOSED 需 WAIT。不含 CREATE——pm 只管理显式委托的域，
/// 不在域内扩张拓扑。
const DELEGATED_DOMAIN_RIGHTS: Rights =
    Rights::from_raw(Rights::MANAGE.raw() | Rights::READ.raw() | Rights::WAIT.raw());

/// 多页流与生命周期 fixture 各自使用正式自动选址。
const TUNNEL_BYTES: usize = 3 * 4096;
/// 超过多倍环容量，覆盖真实背压、分页与回绕。
const STREAM_LEN: usize = 65536;
#[cfg(feature = "acceptance-stress")]
const CONTROL_STRESS: usize = 128;
const CANCELLATION_PROBES: usize = if cfg!(feature = "acceptance-stress") {
    10
} else {
    0
};
#[cfg(feature = "acceptance-stress")]
const TUNNEL_STRESS: usize = 64;
#[cfg(feature = "acceptance-stress")]
const ACCEPTANCE_WORKLOAD: &str = "stress";
#[cfg(not(feature = "acceptance-stress"))]
const ACCEPTANCE_WORKLOAD: &str = "core";

const REQUIRED_FS: u64 = 1 << 0;
const REQUIRED_PM: u64 = 1 << 1;
const REQUIRED_DRIVER: u64 = 1 << 2;
const REQUIRED_TARGET: u64 = 1 << 3;
const REQUIRED_PM_TARGET: u64 = 1 << 4;
const REQUIRED_FAL: u64 = 1 << 5;
const REQUIRED_IMAGES: u64 =
    REQUIRED_FS | REQUIRED_PM | REQUIRED_DRIVER | REQUIRED_TARGET | REQUIRED_FAL;
const REQUIRED_STARTED: u64 =
    REQUIRED_FS | REQUIRED_PM | REQUIRED_DRIVER | REQUIRED_TARGET | REQUIRED_PM_TARGET;

fn required_image(name: &str) -> Option<u64> {
    match name {
        "bin/srv_fs" => Some(REQUIRED_FS),
        "bin/srv_pm" => Some(REQUIRED_PM),
        "bin/drv_spi_sifive" => Some(REQUIRED_DRIVER),
        "bin/test_target" => Some(REQUIRED_TARGET),
        "bin/test_fal" => Some(REQUIRED_FAL),
        _ => None,
    }
}

/// 从声明式 initfs 政策启动服务拓扑。必选映像缺失或任一必选 spawn 失败使
/// 整个 stage 失败；test_fp 是按 admitted execution domain 决定的可选服务。
fn launch_test_services(
    root: &mut RootSupervisor,
    services: Handle,
    pm_domain: Handle,
    acceptance: Handle,
    names: &mut TopologyNames,
) -> Result<LaunchedServices, &'static str> {
    let pm_mailbox = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::GRANT | Rights::DUPLICATE,
        )
        .map_err(|_| "pm mailbox create failed")?;
    let fs_bootstrap = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::GRANT | Rights::DUPLICATE,
        )
        .map_err(|_| "fs bootstrap mailbox create failed")?;
    let fs_release = root
        .create_notification(Rights::READ | Rights::WAIT | Rights::GRANT, Rights::SIGNAL)
        .map_err(|_| "fs release notification create failed")?;
    let fs_route = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::GRANT | Rights::DUPLICATE,
        )
        .map_err(|_| "fs route mailbox create failed")?;
    let fs_bootstrap_second = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::GRANT,
        )
        .map_err(|_| "second fs bootstrap mailbox create failed")?;
    let fs_release_second = root
        .create_notification(Rights::READ | Rights::WAIT | Rights::GRANT, Rights::SIGNAL)
        .map_err(|_| "second fs release notification create failed")?;
    let fs_route_second = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::GRANT | Rights::DUPLICATE,
        )
        .map_err(|_| "second fs route mailbox create failed")?;
    let fs_registration = root
        .create_mailbox_owner(Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT)
        .map_err(|_| "fs registration mailbox create failed")?;
    // GRANT 是直接跨表安装：授出的源 handle 被消费，先复制保留 init 对
    // 委托域的直接收束权（兜底 job_kill 的 authority 源）。
    let delegated_domain = root
        .duplicate(pm_domain, JOB_FULL_RIGHTS)
        .map_err(|_| "pm domain duplicate failed")?;
    let control_rights = SUPERVISOR_RIGHTS;
    let mut manifest = RequiredLaunchSet::new(REQUIRED_IMAGES, REQUIRED_STARTED);
    let mut stage_failure = None;
    // 按需启动的验收进程直接借用启动包的只读映像，避免复制 ELF 到 init 堆。
    let mut target_image: Option<&'static [u8]> = None;
    let mut hammer_image: Option<&'static [u8]> = None;
    let mut test_fal_image: Option<&'static [u8]> = None;
    let mut fs_second_image: Option<&'static [u8]> = None;
    let mut fs_second_scope = None;
    let mut primary_provider = None;
    let mut registry_root_watch = None;
    let result = tar::walk(env::startup_payload(), |entry| {
        if !entry.name.starts_with("bin/") || entry.name.ends_with('/') {
            return;
        }
        if entry.name == "bin/test_hammer" {
            // 竞态锤不是常驻服务：只留存映像，由剧本按需 spawn。
            hammer_image = Some(entry.data);
            return;
        }
        let required = required_image(entry.name);
        if let Some(bit) = required {
            manifest.mark_present(bit);
        } else if entry.name != "bin/test_fp" {
            debug!("optional initfs image ignored: {}", entry.name);
            return;
        }
        if stage_failure.is_some() {
            return;
        }
        if entry.name == "bin/test_fal" {
            test_fal_image = Some(entry.data);
            return;
        }
        let pm_grants = [
            HandleGrant {
                handle: pm_mailbox.owner,
                rights: Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
            },
            HandleGrant {
                handle: delegated_domain,
                rights: DELEGATED_DOMAIN_RIGHTS,
            },
        ];
        let fs_grants = [
            HandleGrant {
                handle: fs_bootstrap.peer,
                rights: Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
            },
            HandleGrant {
                handle: fs_release.owner,
                rights: Rights::READ | Rights::WAIT,
            },
            HandleGrant {
                handle: fs_route.owner,
                rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
            },
            HandleGrant {
                handle: fs_registration,
                rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
            },
        ];
        // test_target 首实例入 acceptance 域（枚举+派生验收线的靶域）。
        let (job, grants): (Handle, &[HandleGrant]) = if entry.name == "bin/test_target" {
            (acceptance, &[])
        } else if entry.name == "bin/srv_pm" {
            (services, pm_grants.as_slice())
        } else if entry.name == "bin/srv_fs" {
            (services, fs_grants.as_slice())
        } else {
            (services, &[])
        };
        if root.reserve_process().is_err() {
            stage_failure = Some("process supervision admission failed");
            return;
        }
        let spawned = spawn(SpawnRequest {
            memory_pool: root_memory_pool(),
            job,
            image: entry.data,
            payload: &[],
            grants,
            control_rights,
        });
        if entry.name == "bin/srv_pm"
            && (spawned.is_ok()
                || matches!(&spawned, Err(failure) if failure.grants == libprocess::GrantOutcome::Consumed))
        {
            root.transferred(pm_mailbox.owner);
            root.transferred(delegated_domain);
        }
        if entry.name == "bin/srv_fs"
            && (spawned.is_ok()
                || matches!(&spawned, Err(failure) if failure.grants == libprocess::GrantOutcome::Consumed))
        {
            root.transferred(fs_bootstrap.peer);
            root.transferred(fs_release.owner);
            root.transferred(fs_route.owner);
            root.transferred(fs_registration);
        }
        match spawned {
            Ok(process) => {
                if let Some(bit) = required {
                    manifest.mark_started(bit);
                }
                debug!("started {} as pid {}", entry.name, process.pid);
                names.register_process(process.pid, entry.name);
                // 持久 init 保留 control：监督、等待与收束的 authority 源。
                if entry.name == "bin/test_target" {
                    target_image = Some(entry.data);
                    // live kill 正路径改经枚举+派生（Job 管理面验收线 1）：
                    // acceptance Job 枚举可见 test_target 的 pid，派生 MANAGE
                    // control 后 kill——保留 control 在派生接管后关闭。
                    if let Err(error) =
                        test_derive_kill(root, acceptance, process.pid, process.control)
                    {
                        stage_failure = Some(error);
                        return;
                    }
                    // 委托域靶：control 即弃（关闭 control 永不隐式终止），
                    // pm 的派生因此走铸造路径，域内收束权归 pm。
                    match spawn(SpawnRequest {
                        memory_pool: root_memory_pool(),
                        job: pm_domain,
                        image: entry.data,
                        payload: &[],
                        grants: &[],
                        control_rights,
                    }) {
                        Ok(second) => {
                            manifest.mark_started(REQUIRED_PM_TARGET);
                            debug!("pm domain target started as pid {}", second.pid);
                            names.register_process(second.pid, "bin/test_target@pm_domain");
                            let _ = unsafe { close(second.control) };
                        }
                        Err(error) => {
                            debug!("required pm-domain target spawn failed: {:?}", error);
                            stage_failure = Some("required pm-domain target spawn failed");
                        }
                    }
                } else if entry.name == "bin/srv_fs" {
                    root.track_process(Supervised {
                        pid: process.pid,
                        control: process.control,
                    });
                    let prepared = (|| -> Result<_, &'static str> {
                        let primary = accept_fs_provider(
                            fs_bootstrap.owner,
                            fs_release.peer,
                            fs_route.peer,
                            true,
                        )?;
                        if primary.pid != process.pid {
                            return Err("primary provider bootstrap sender mismatch");
                        }
                        let directory = primary
                            .directory
                            .as_ref()
                            .ok_or("primary provider omitted service directory")?;
                        let deadline = rinlib::time::timeout_millis(10_000)
                            .map_err(|_| "registration bootstrap deadline invalid")?;
                        let watch = FalClient::new()
                            .subscribe(
                                directory,
                                "",
                                protocol::WatchMask::CREATE | protocol::WatchMask::DELETE,
                                deadline,
                            )
                            .map_err(|_| "service directory root subscription failed")?;
                        let authority = primary
                            .registration_root
                            .as_ref()
                            .ok_or("primary provider omitted registration authority")?;
                        let scoped =
                            service_client::delegate_name(authority, "fs.secondary", deadline)
                                .map_err(|_| "fs name authority delegation failed")?;
                        let scope_handle = root
                            .own_sender(scoped)
                            .map_err(|_| "fs name authority ownership admission failed")?;
                        Ok((primary, watch, scope_handle))
                    })();
                    let (primary, watch, scope_handle) = match prepared {
                        Ok(prepared) => prepared,
                        Err(error) => {
                            stage_failure = Some(error);
                            return;
                        }
                    };
                    primary_provider = Some(primary);
                    registry_root_watch = Some(watch);
                    fs_second_scope = Some(scope_handle);
                    fs_second_image = Some(entry.data);
                } else {
                    root.track_process(Supervised {
                        pid: process.pid,
                        control: process.control,
                    });
                }
            }
            Err(error) => {
                if required.is_some() {
                    debug!("required service {} spawn failed: {:?}", entry.name, error);
                    stage_failure = Some("required service spawn failed");
                } else {
                    debug!("optional service {} degraded: {:?}", entry.name, error);
                }
            }
        }
    });
    if let Err(error) = result {
        debug!("required initfs parse failed: {:?}", error);
        stage_failure = Some("initfs parse failed");
    }
    if manifest.missing_present() != 0 {
        debug!(
            "required service images missing: missing={:#x}, seen={:#x}",
            manifest.missing_present(),
            manifest.present()
        );
        stage_failure = Some("required service image missing");
    }
    #[cfg(feature = "acceptance-stress")]
    if hammer_image.is_none() {
        debug!("required stress hammer image missing");
        stage_failure = Some("required stress hammer image missing");
    }
    if manifest.missing_started() != 0 {
        debug!(
            "required topology incomplete: missing={:#x}, started={:#x}",
            manifest.missing_started(),
            manifest.started()
        );
        stage_failure = Some("required topology incomplete");
    }
    if let Some(failure) = stage_failure {
        return Err(failure);
    }
    debug!(
        "required service topology complete: images={:#x}, started={:#x}",
        manifest.present(),
        manifest.started()
    );
    Ok(LaunchedServices {
        first_fs: primary_provider.ok_or("primary provider bootstrap incomplete")?,
        registry_root_watch: registry_root_watch.ok_or("service discovery watch missing")?,
        pm_mailbox: pm_mailbox.peer,
        fs_bootstrap_second: fs_bootstrap_second.owner,
        fs_release_second: fs_release_second.peer,
        fs_route_second: fs_route_second.peer,
        fs_bootstrap_second_sender: fs_bootstrap_second.peer,
        fs_release_second_owner: fs_release_second.owner,
        fs_route_second_owner: fs_route_second.owner,
        fs_second_scope: fs_second_scope.ok_or("second fs name scope missing")?,
        fs_second_image: fs_second_image.ok_or("second fs image missing")?,
        test_fal_image: test_fal_image.ok_or("FAL consumer image missing")?,
        target_image,
        hammer_image,
    })
}

fn launch_secondary_fs(
    root: &mut RootSupervisor,
    services: Handle,
    names: &mut TopologyNames,
    image: &'static [u8],
    grants: [Handle; 4],
) -> Result<(), &'static str> {
    let [bootstrap, release, route, scope] = grants;
    let grants = [
        HandleGrant {
            handle: bootstrap,
            rights: Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        },
        HandleGrant {
            handle: release,
            rights: Rights::READ | Rights::WAIT,
        },
        HandleGrant {
            handle: route,
            rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
        },
        HandleGrant {
            handle: scope,
            rights: Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
        },
    ];
    root.reserve_process()
        .map_err(|_| "second fs supervision admission failed")?;
    let spawned = spawn(SpawnRequest {
        memory_pool: root_memory_pool(),
        job: services,
        image,
        payload: &[],
        grants: &grants,
        control_rights: SUPERVISOR_RIGHTS,
    });
    if spawned.is_ok()
        || matches!(&spawned, Err(failure) if failure.grants == libprocess::GrantOutcome::Consumed)
    {
        for grant in grants {
            root.transferred(grant.handle);
        }
    }
    let second = spawned.map_err(|error| {
        debug!("required second fs provider spawn failed: {:?}", error);
        "required second fs provider spawn failed"
    })?;
    debug!("second fs provider started as pid {}", second.pid);
    names.register_process(second.pid, "bin/srv_fs@second");
    root.track_process(Supervised {
        pid: second.pid,
        control: second.control,
    });
    Ok(())
}

struct FalConsumerCoordinator {
    pid: u64,
    control: Handle,
    report_owner: Handle,
    command_signaler: Handle,
}

impl FalConsumerCoordinator {
    fn await_report(&self, expected: u64) -> Result<(), &'static str> {
        let deadline = rinlib::time::timeout_millis(120_000)
            .map_err(|_| "FAL consumer phase deadline invalid")?;
        let mut terminal = None;
        loop {
            let bits = match notification::take(self.report_owner, u64::MAX) {
                Ok(bits) => bits,
                Err(SystemCallError::ObjectNotAvailable) => 0,
                Err(_) => return Err("FAL consumer phase report take failed"),
            };
            if bits != 0 {
                if bits == expected {
                    return Ok(());
                }
                return Err("FAL consumer phase report out of order");
            }
            if let Some(reason) = terminal {
                return Err(reason);
            }
            let result = rinlib::ipc::wait::wait_until(
                &[
                    WaitItem::new(
                        self.report_owner,
                        ObjectSignals::READABLE | ObjectSignals::CLOSED,
                        1,
                    ),
                    WaitItem::new(
                        self.control,
                        ObjectSignals::REAPABLE | ObjectSignals::CLOSED,
                        2,
                    ),
                ],
                deadline,
            )
            .map_err(|_| "FAL consumer phase wait failed")?;
            if WaitReason::from_u32(result.reason) == Some(WaitReason::Timeout) {
                terminal = Some("FAL consumer phase timed out");
            } else if result.item_index == 1 {
                terminal = Some("FAL consumer exited before phase report");
            } else if result.observed.intersects(ObjectSignals::CLOSED) {
                terminal = Some("FAL consumer phase channel closed");
            } else if WaitReason::from_u32(result.reason) != Some(WaitReason::Signaled) {
                terminal = Some("FAL consumer phase wait cancelled");
            }
        }
    }

    fn continue_business(&self) -> Result<(), &'static str> {
        notification::signal(self.command_signaler, test_fal::command::CONTINUE)
            .map_err(|_| "FAL consumer continuation signal failed")
    }

    fn observe_shutdown(&self) -> Result<(), &'static str> {
        notification::signal(self.command_signaler, test_fal::command::OBSERVE_SHUTDOWN)
            .map_err(|_| "FAL consumer shutdown signal failed")
    }

    fn finish(self, root: &mut RootSupervisor) -> Result<(), &'static str> {
        let deadline = rinlib::time::timeout_millis(120_000)
            .map_err(|_| "FAL consumer exit deadline invalid")?;
        let result = rinlib::ipc::wait::wait_until(
            &[WaitItem::new(
                self.control,
                ObjectSignals::REAPABLE | ObjectSignals::CLOSED,
                1,
            )],
            deadline,
        )
        .map_err(|_| "FAL consumer exit wait failed")?;
        if WaitReason::from_u32(result.reason) == Some(WaitReason::Timeout) {
            return Err("FAL consumer exit timed out");
        }
        let exit = process::query(self.control).map_err(|_| "FAL consumer exit query failed")?;
        if exit.reason != ProcessExitReason::Exited as u32 || exit.code != 0 {
            return Err("FAL consumer exited without successful result");
        }
        root.collect_process(self.pid)?;
        root.close_control(self.report_owner)?;
        root.close_control(self.command_signaler)?;
        Ok(())
    }
}

fn launch_fal_consumer(
    root: &mut RootSupervisor,
    services: Handle,
    names: &mut TopologyNames,
    image: &'static [u8],
    primary: &AcceptedFsProvider,
) -> Result<FalConsumerCoordinator, &'static str> {
    let root_grant = primary
        .grant
        .as_ref()
        .ok_or("FAL consumer primary grant missing")?;
    let directory = primary
        .directory
        .as_ref()
        .ok_or("FAL consumer service directory missing")?;
    let root_rights = root_grant
        .description()
        .map_err(|_| "FAL consumer primary rights query failed")?
        .rights;
    let directory_rights = directory
        .description()
        .map_err(|_| "FAL consumer directory rights query failed")?
        .rights;
    let primary_child_rights = root_rights & !Rights::GRANT;
    let directory_child_rights = directory_rights & !Rights::GRANT;
    root.reserve_process()
        .map_err(|_| "FAL consumer supervision admission failed")?;
    let primary_copy = root
        .duplicate(root_grant.as_handle(), root_rights)
        .map_err(|_| "FAL consumer primary grant duplicate failed")?;
    let directory_copy = root
        .duplicate(directory.as_handle(), directory_rights)
        .map_err(|_| "FAL consumer directory grant duplicate failed")?;
    let command = root
        .create_notification(
            Rights::READ | Rights::WAIT | Rights::GRANT,
            Rights::SIGNAL | Rights::GRANT,
        )
        .map_err(|_| "FAL consumer command channel create failed")?;
    let report = root
        .create_notification(
            Rights::READ | Rights::WAIT | Rights::GRANT,
            Rights::SIGNAL | Rights::GRANT,
        )
        .map_err(|_| "FAL consumer report channel create failed")?;
    let grants = [
        HandleGrant {
            handle: primary_copy,
            rights: primary_child_rights,
        },
        HandleGrant {
            handle: directory_copy,
            rights: directory_child_rights,
        },
        HandleGrant {
            handle: command.owner,
            rights: Rights::READ | Rights::WAIT,
        },
        HandleGrant {
            handle: report.peer,
            rights: Rights::SIGNAL,
        },
    ];
    let spawned = spawn(SpawnRequest {
        memory_pool: root_memory_pool(),
        job: services,
        image,
        payload: &[],
        grants: &grants,
        control_rights: SUPERVISOR_RIGHTS,
    });
    if spawned.is_ok()
        || matches!(&spawned, Err(failure) if failure.grants == libprocess::GrantOutcome::Consumed)
    {
        for grant in grants {
            root.transferred(grant.handle);
        }
    }
    let consumer = spawned.map_err(|error| {
        debug!("required FAL consumer spawn failed: {:?}", error);
        "required FAL consumer spawn failed"
    })?;
    names.register_process(consumer.pid, "bin/test_fal");
    root.track_process(Supervised {
        pid: consumer.pid,
        control: consumer.control,
    });
    Ok(FalConsumerCoordinator {
        pid: consumer.pid,
        control: consumer.control,
        report_owner: report.owner,
        command_signaler: command.peer,
    })
}

struct AcceptedFsProvider {
    pid: u64,
    grant: Option<MailboxSender>,
    sender_identity: Option<u64>,
    mailbox_identity: Option<u64>,
    directory: Option<MailboxSender>,
    registration_root: Option<MailboxSender>,
    bootstrap: Handle,
    release: Handle,
    route: Handle,
}

fn accept_fs_provider(
    bootstrap: Handle,
    release: Handle,
    route: Handle,
    expect_root: bool,
) -> Result<AcceptedFsProvider, &'static str> {
    let mut published =
        wait_message(bootstrap).map_err(|_| "fs provider publication receive failed")?;
    let pid = published.header.sender_pid;
    let mut grant = None;
    let mut sender_identity = None;
    let mut mailbox_identity = None;
    if published.header.kind == protocol::ROOT_GRANT_KIND {
        if !expect_root || published.handles.remaining() != 1 {
            return Err("fs root grant hand-off layout invalid");
        }
        let capability = published
            .handles
            .take(0)
            .map_err(|_| "fs root grant hand-off capability missing")?;
        let (root, description) = MailboxSender::from_capability(capability)
            .map_err(|_| "fs root grant hand-off role invalid")?;
        if !description.rights.contains(
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE | Rights::GRANT,
        ) {
            return Err("primary fs root transport rights invalid");
        }
        let mut client = FalClient::new();
        let lookup = client
            .call(
                &root,
                &FalRequest::Lookup { path: "" },
                rinlib::time::Deadline::INFINITE,
            )
            .map_err(|_| "independent FAL2 root lookup failed")?;
        let (_, FalResponse::Node(info)) = protocol::decode_response(&lookup.payload)
            .map_err(|_| "independent FAL2 reply invalid")?
        else {
            return Err("independent FAL2 root lookup shape invalid");
        };
        if info.kind != NodeKind::Directory || !info.rights.contains(FalRights::TRAVERSE) {
            return Err("independent FAL2 root grant metadata invalid");
        }
        let child = client
            .derive(
                &root,
                "",
                FalRights::TRAVERSE | FalRights::ENUMERATE,
                rinlib::time::Deadline::INFINITE,
            )
            .map_err(|_| "independent FAL2 derive failed")?;
        let child_lookup = client
            .call(
                &child,
                &FalRequest::Lookup { path: "" },
                rinlib::time::Deadline::INFINITE,
            )
            .map_err(|_| "independent FAL2 child lookup failed")?;
        let (_, FalResponse::Node(child_info)) =
            protocol::decode_response(&child_lookup.payload)
                .map_err(|_| "independent FAL2 child reply invalid")?
        else {
            return Err("independent FAL2 child lookup shape invalid");
        };
        if child_info.rights != (FalRights::TRAVERSE | FalRights::ENUMERATE) {
            return Err("independent FAL2 child grant attenuation failed");
        }
        drop(child);
        sender_identity = Some(description.object_id);
        mailbox_identity = Some(description.related_object_id);
        grant = Some(root);
        published =
            wait_message(bootstrap).map_err(|_| "fs provider publication receive failed")?;
    } else if expect_root {
        return Err("primary provider omitted root grant");
    }
    let directory = if published.header.kind == service_protocol::DIRECTORY_GRANT_KIND {
        if published.handles.remaining() != 1 {
            return Err("service directory grant layout invalid");
        }
        let capability = published
            .handles
            .take(0)
            .map_err(|_| "service directory grant capability missing")?;
        let (directory, description) = MailboxSender::from_capability(capability)
            .map_err(|_| "service directory grant role invalid")?;
        if !description.rights.contains(
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE | Rights::GRANT,
        ) {
            return Err("service directory root transport rights invalid");
        }
        published = wait_message(bootstrap).map_err(|_| "fs provider ready receive failed")?;
        Some(directory)
    } else {
        None
    };
    let registration_root = if expect_root {
        if directory.is_none()
            || published.header.kind != service_protocol::AUTHORITY_GRANT_KIND
            || published.header.sender_pid != pid
            || published.handles.remaining() != 1
        {
            return Err("primary provider registration root layout invalid");
        }
        let capability = published
            .handles
            .take(0)
            .map_err(|_| "registration root grant missing")?;
        let (root, description) = MailboxSender::from_capability(capability)
            .map_err(|_| "registration root grant role invalid")?;
        if !description.rights.contains(
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE | Rights::GRANT,
        ) {
            return Err("registration root transport rights invalid");
        }
        Some(root)
    } else {
        if directory.is_some()
            || published.header.kind != protocol::PROVIDER_READY_KIND
            || !published.handles.is_empty()
        {
            return Err("secondary provider Ready layout invalid");
        }
        None
    };
    Ok(AcceptedFsProvider {
        pid,
        grant,
        sender_identity,
        mailbox_identity,
        directory,
        registration_root,
        bootstrap,
        release,
        route,
    })
}

fn confirm_primary_fs_ready(provider: &AcceptedFsProvider) -> Result<(), &'static str> {
    let ready =
        wait_message(provider.bootstrap).map_err(|_| "primary provider Ready receive failed")?;
    if ready.header.kind != protocol::PROVIDER_READY_KIND
        || ready.header.sender_pid != provider.pid
        || !ready.handles.is_empty()
    {
        return Err("primary provider Ready layout invalid");
    }
    Ok(())
}

fn discover_registered_fs(
    directory: &MailboxSender,
    primary_mailbox: u64,
    root_watch: &libfal::client::Subscription,
) -> Result<(MailboxSender, u64, u64, libfal::client::Subscription), &'static str> {
    let deadline = rinlib::time::Deadline::INFINITE;
    let mut client = FalClient::new();
    match root_watch
        .wait(deadline)
        .map_err(|_| "service directory publication Watch failed")?
    {
        SubscriptionEvent::Events(events) if events.contains(protocol::WatchMask::CREATE) => {}
        _ => return Err("service directory omitted Ready publication"),
    }
    let watch = client
        .subscribe(
            directory,
            "fs.secondary",
            protocol::WatchMask::MODIFY | protocol::WatchMask::DELETE,
            deadline,
        )
        .map_err(|_| "service discovery subscribe failed")?;
    let mut read = client
        .call(
            directory,
            &FalRequest::Read {
                path: "fs.secondary",
            },
            deadline,
        )
        .map_err(|_| "service discovery Record read failed")?;
    let (header, FalResponse::Value(value)) = protocol::decode_response(&read.payload)
        .map_err(|_| "service discovery Record reply invalid")?
    else {
        return Err("service discovery Record reply shape invalid");
    };
    if header.status != protocol::Status::Ok || read.handles.remaining() != 1 {
        return Err("service discovery Record export incomplete");
    }
    let record = ServiceRecord::decode(value).map_err(|_| "service discovery Record invalid")?;
    if record.protocol != protocol::ID || record.version != protocol::VERSION as u32 {
        return Err("service discovery Record protocol mismatch");
    }
    let capability = read
        .handles
        .take(0)
        .map_err(|_| "service discovery derived grant missing")?;
    let (discovered, description) = MailboxSender::from_capability(capability)
        .map_err(|_| "service discovery derived grant role invalid")?;
    if description.related_object_id == primary_mailbox {
        return Err("service discovery resolved the primary provider");
    }
    let lookup = client
        .call(&discovered, &FalRequest::Lookup { path: "" }, deadline)
        .map_err(|_| "discovered provider invocation failed")?;
    let (_, FalResponse::Node(info)) = protocol::decode_response(&lookup.payload)
        .map_err(|_| "discovered provider reply invalid")?
    else {
        return Err("discovered provider reply shape invalid");
    };
    if info.kind != NodeKind::Directory || !info.rights.contains(FalRights::TRAVERSE) {
        return Err("discovered provider root metadata invalid");
    }
    debug!("service discovery root-watch-read-derive-call passed");
    Ok((
        discovered,
        description.object_id,
        description.related_object_id,
        watch,
    ))
}

fn exercise_open_streams(
    root: &mut RootSupervisor,
    transport: &mut FalTransport,
    position: &libfs::resolve::Position<libfs::client::Grant>,
) -> Result<(), &'static str> {
    let deadline =
        rinlib::time::timeout_millis(20_000).map_err(|_| "FAL2 Open session deadline failed")?;

    let payload_len = 16_384;
    let cancel = transport
        .open_stream(
            position,
            protocol::StreamDirection::Write,
            9,
            Some(4),
            deadline,
        )
        .map_err(|_| "FAL2 Open cancel setup failed")?;
    let info = transport
        .cancel_stream(&cancel, deadline)
        .map_err(|_| "FAL2 Open Cancel failed")?;
    let again = transport
        .finish_stream(&cancel, deadline)
        .map_err(|_| "FAL2 Open cancelled Finish failed")?;
    let queried = transport
        .query_stream(&cancel, deadline)
        .map_err(|_| "FAL2 Open cancelled Query failed")?;
    if info != again
        || info != queried
        || info.outcome != protocol::Status::Cancelled
        || info.reason != protocol::StreamReason::Cancelled
        || info.accepted != 0
    {
        return Err("FAL2 Open Cancel result changed");
    }
    cancel
        .close()
        .map_err(|_| "FAL2 Open Cancel close failed")?;
    let invalid = transport.open_stream(
        position,
        protocol::StreamDirection::Read,
        u64::MAX,
        Some(2),
        deadline,
    );
    if !matches!(
        invalid,
        Err(libfs::client::StreamOpenFailure::Call(
            libfal::client::ClientError::Protocol
                | libfal::client::ClientError::Status(protocol::Status::Invalid)
        ))
    ) {
        return Err("FAL2 Open overflow was not rejected before publication");
    }
    let mut bounded = transport
        .open_stream(
            position,
            protocol::StreamDirection::Write,
            9,
            Some(4),
            deadline,
        )
        .map_err(|_| "FAL2 Open bounded write setup failed")?;
    bounded
        .write_until(b"ABCDEF", deadline)
        .map_err(|_| "FAL2 Open bounded write transport failed")?;
    let _ = bounded.end_write();
    let info = transport
        .finish_stream(&bounded, deadline)
        .map_err(|_| "FAL2 Open bounded write Finish failed")?;
    if info.outcome != protocol::Status::Invalid
        || info.accepted != 4
        || info.transported != 5
        || info.reason != protocol::StreamReason::Backend
    {
        return Err("FAL2 Open bounded write partial result mismatch");
    }
    bounded
        .close()
        .map_err(|_| "FAL2 Open bounded write close failed")?;
    let mut abandoned = transport
        .open_stream(
            position,
            protocol::StreamDirection::Read,
            3,
            Some(6),
            deadline,
        )
        .map_err(|_| "FAL2 Open abandoned data setup failed")?;
    abandoned
        .close_data()
        .map_err(|_| "FAL2 Open abandoned data close failed")?;
    let info = transport
        .finish_stream(&abandoned, deadline)
        .map_err(|_| "FAL2 Open abandoned data Finish failed")?;
    if info.outcome != protocol::Status::Cancelled
        || info.reason != protocol::StreamReason::PeerClosed
        || info.accepted > info.transported
    {
        return Err("FAL2 Open peer close reported success");
    }
    abandoned
        .close()
        .map_err(|_| "FAL2 Open abandoned control close failed")?;
    let mut progress = transport
        .open_stream(
            position,
            protocol::StreamDirection::Read,
            9,
            Some(payload_len),
            deadline,
        )
        .map_err(|_| "FAL2 Open read progress setup failed")?;
    let mut consumed = [0; 4096];
    if transport
        .read_stream_until(&mut progress, &mut consumed, deadline)
        .map_err(|_| "FAL2 Open read progress transport failed")?
        != consumed.len()
    {
        return Err("FAL2 Open read progress short read");
    }
    let partial = transport
        .cancel_stream(&progress, deadline)
        .map_err(|_| "FAL2 Open read progress Cancel failed")?;
    if partial.outcome != protocol::Status::Cancelled
        || partial.accepted != consumed.len() as u64
        || partial.transported < partial.accepted
    {
        return Err("FAL2 Open read Cancel lost consumed progress");
    }
    progress
        .close()
        .map_err(|_| "FAL2 Open read progress close failed")?;
    let waiting = transport
        .open_stream(
            position,
            protocol::StreamDirection::Write,
            9,
            None,
            deadline,
        )
        .map_err(|_| "FAL2 Open concurrent Finish setup failed")?;
    let retried = FalClient::new()
        .call(
            waiting
                .control_sender()
                .ok_or("FAL2 duplicate Start control owner missing")?,
            &FalRequest::Start,
            deadline,
        )
        .map_err(|_| "FAL2 duplicate Start failed")?;
    if !matches!(
        protocol::decode_response(&retried.payload),
        Ok((
            _,
            FalResponse::StreamInfo(protocol::StreamInfo {
                state: protocol::StreamState::Active,
                ..
            })
        ))
    ) {
        return Err("FAL2 duplicate Start changed state");
    }
    let reply_mailbox = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .map_err(|_| "FAL2 Open concurrent Finish reply mailbox failed")?;
    let txid = 0x4653_5452_4649_4e49;
    send_raw_fal_request(
        waiting
            .control_sender()
            .ok_or("FAL2 Open control owner missing")?,
        root.sender(reply_mailbox.peer),
        &FalRequest::FinishStream,
        txid,
        deadline,
    )
    .map_err(|_| "FAL2 Open blocked Finish send failed")?;
    rinlib::time::sleep_until(
        rinlib::time::timeout_millis(20)
            .map_err(|_| "FAL2 Open blocked Finish settle deadline failed")?,
    )
    .map_err(|_| "FAL2 Open blocked Finish settle failed")?;
    if !matches!(
        receive(reply_mailbox.owner),
        Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy)
    ) {
        return Err("FAL2 Open Finish replied before the stream ended");
    }
    let cancelled = transport
        .cancel_stream(&waiting, deadline)
        .map_err(|_| "FAL2 Open concurrent Cancel failed")?;
    let reply = rinlib::ipc::message::wait_message_until(reply_mailbox.owner, deadline)
        .map_err(|_| "FAL2 Open blocked Finish did not wake")?;
    let prefix =
        RpcPrefix::decode(&reply.payload).map_err(|_| "FAL2 Open blocked Finish prefix invalid")?;
    let (header, FalResponse::StreamInfo(finished)) =
        protocol::decode_response(&reply.payload[librpc::PREFIX_LEN..])
            .map_err(|_| "FAL2 Open blocked Finish reply invalid")?
    else {
        return Err("FAL2 Open blocked Finish reply shape invalid");
    };
    if prefix.kind != RpcMessageKind::Response
        || prefix.txid != txid
        || header.status != protocol::Status::Ok
        || cancelled != finished
        || cancelled.outcome != protocol::Status::Cancelled
    {
        return Err("FAL2 Open blocked Finish diverged from Cancel");
    }
    root.close_control(reply_mailbox.peer)?;
    root.close_control(reply_mailbox.owner)?;
    waiting
        .close()
        .map_err(|_| "FAL2 Open concurrent control close failed")?;
    debug!(
        "FAL2 Open control and terminal cases passed after {} bytes",
        payload_len
    );
    Ok(())
}

fn exercise_cancellable_rpc(root: &mut RootSupervisor) -> Result<(), &'static str> {
    let deadline =
        rinlib::time::timeout_millis(5_000).map_err(|_| "FAL2 cancellable RPC deadline failed")?;
    let event = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT,
    )
    .map_err(|_| "FAL2 cancellable RPC Notification failed")?;
    // SAFETY: NotificationCreate 已移交该唯一的读端 owner。
    let cancel = unsafe { Capability::from_raw(event.owner) };
    notification::signal(event.peer, 1).map_err(|_| "FAL2 cancellable RPC signal failed")?;
    let blocked = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .map_err(|_| "FAL2 cancellable RPC blocked mailbox failed")?;
    for index in 0..MAILBOX_CAPACITY {
        root.sender(blocked.peer)
            .send(0x4341_4e43_454c_0000 + index as u64, &[])
            .map_err(|_| "FAL2 cancellable RPC mailbox fill failed")?;
    }
    let mut caller = librpc::Caller::new();
    let request = librpc::Request::new(protocol::ID, b"cancel")
        .map_err(|_| "FAL2 cancellable RPC first request failed")?;
    let first = caller.call_cancelable(
        root.sender(blocked.peer),
        deadline,
        request,
        Some(cancel.as_handle()),
    );
    let Err(mut first) = first else {
        return Err("FAL2 cancellable RPC did not return unsent owner");
    };
    if first.phase != librpc::CallPhase::Unsent
        || first.cause != librpc::CallCause::Cancelled
        || !first.has_unretired_owners()
    {
        return Err("FAL2 cancellable RPC unsent phase invalid");
    }
    drop(
        first
            .take_unsent_request()
            .ok_or("FAL2 cancellable RPC request missing")?,
    );
    if first.has_unretired_owners() {
        return Err("FAL2 cancellable RPC request owner not released");
    }
    root.close_control(blocked.owner)?;
    root.close_control(blocked.peer)?;

    let sent = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .map_err(|_| "FAL2 cancellable RPC sent mailbox failed")?;
    let request = librpc::Request::new(protocol::ID, b"cancel")
        .map_err(|_| "FAL2 cancellable RPC second request failed")?;
    let second = caller.call_cancelable(
        root.sender(sent.peer),
        deadline,
        request,
        Some(cancel.as_handle()),
    );
    let Err(mut second) = second else {
        return Err("FAL2 cancellable RPC did not preserve sent uncertainty");
    };
    if second.phase != librpc::CallPhase::Sent
        || second.cause != librpc::CallCause::Cancelled
        || !second.has_unretired_owners()
    {
        return Err("FAL2 cancellable RPC sent phase invalid");
    }
    second
        .retry_cleanup()
        .map_err(|_| "FAL2 cancellable RPC reply owner close failed")?;
    if second.has_unretired_owners() {
        return Err("FAL2 cancellable RPC reply owner not released");
    }
    root.close_control(sent.owner)?;
    root.close_control(sent.peer)?;
    cancel
        .close()
        .map_err(|_| "FAL2 cancellable RPC owner close failed")?;
    // SAFETY: 本函数仍独占 Notification 的信号端点。
    unsafe { rinlib::ipc::object::close(event.peer) }
        .map_err(|_| "FAL2 cancellable RPC peer close failed")?;
    debug!("FAL2 cancellable RPC phases passed");
    Ok(())
}

fn fal_call(
    client: &mut FalClient,
    grant: &MailboxSender,
    request: &FalRequest<'_>,
) -> Result<librpc::Reply, &'static str> {
    client
        .call(grant, request, rinlib::time::Deadline::INFINITE)
        .map_err(|_| "FAL2 provider operation failed")
}

fn fal_blob(value: &[u8]) -> Result<alloc::vec::Vec<u8>, &'static str> {
    let encoded = libfal::value::Value::Blob(value);
    let mut bytes = alloc::vec![0; encoded.encoded_len().ok_or("FAL2 value length invalid")?];
    let used = encoded
        .encode(&mut bytes)
        .map_err(|_| "FAL2 value encoding failed")?;
    bytes.truncate(used);
    Ok(bytes)
}

fn fal_mailbox_handle(mode: ExportMode) -> Result<alloc::vec::Vec<u8>, &'static str> {
    let value = Value::Handle {
        slot: 1,
        policy: ExportPolicy {
            protocol: ValueProtocol::Mailbox,
            mode,
            transport: Rights::WRITE
                | Rights::WAIT
                | Rights::TRANSIT
                | if mode == ExportMode::Repeatable {
                    Rights::DUPLICATE
                } else {
                    Rights::NONE
                },
            fal_ceiling: FalRights::NONE,
        },
    };
    let mut bytes = alloc::vec![0; value.encoded_len().ok_or("FAL2 handle value length invalid")?];
    let used = value
        .encode(&mut bytes)
        .map_err(|_| "FAL2 handle value encoding failed")?;
    bytes.truncate(used);
    Ok(bytes)
}

fn exercise_fs_provider(grant: &MailboxSender) -> Result<(), &'static str> {
    let deadline = rinlib::time::Deadline::INFINITE;
    let mut client = FalClient::new();
    let initial = fal_blob(b"initial")?;
    let leaf_value = fal_blob(b"leaf")?;
    let delete_value = fal_blob(b"delete")?;

    let created_property = fal_call(
        &mut client,
        grant,
        &FalRequest::Create {
            name: "probe-property",
            kind: NodeKind::Property,
            rights: FalRights::READ_PROPERTY | FalRights::WRITE_PROPERTY | FalRights::WATCH,
            value: &initial,
        },
    )?;
    let (_, FalResponse::Node(property)) = protocol::decode_response(&created_property.payload)
        .map_err(|_| "FAL2 property create reply invalid")?
    else {
        return Err("FAL2 property create shape invalid");
    };

    let repeatable_handle = fal_mailbox_handle(ExportMode::Repeatable)?;
    let repeatable_rights = Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT;
    client
        .call_with_target_rights(
            grant,
            &FalRequest::Create {
                name: "probe-repeatable-handle",
                kind: NodeKind::Property,
                rights: FalRights::READ_PROPERTY | FalRights::ACQUIRE_CAPABILITY,
                value: &repeatable_handle,
            },
            grant,
            repeatable_rights,
            deadline,
        )
        .map_err(|_| "FAL2 repeatable handle property create failed")?;
    let affine_handle = fal_mailbox_handle(ExportMode::Affine)?;
    let handle_property = client
        .call_with_target(
            grant,
            &FalRequest::Create {
                name: "probe-affine-handle",
                kind: NodeKind::Property,
                rights: FalRights::READ_PROPERTY | FalRights::ACQUIRE_CAPABILITY,
                value: &affine_handle,
            },
            grant,
            deadline,
        )
        .map_err(|_| "FAL2 affine handle property create failed")?;
    if !matches!(
        protocol::decode_response(&handle_property.payload)
            .map_err(|_| "FAL2 affine handle property reply invalid")?
            .1,
        FalResponse::Node(_)
    ) {
        return Err("FAL2 affine handle property shape invalid");
    }
    fal_call(
        &mut client,
        grant,
        &FalRequest::Create {
            name: "probe-stream",
            kind: NodeKind::Stream,
            rights: FalRights::READ_STREAM | FalRights::WRITE_STREAM | FalRights::WATCH,
            value: &[],
        },
    )?;
    let written = fal_call(
        &mut client,
        grant,
        &FalRequest::WriteAt {
            path: "probe-stream",
            offset: 3,
            value: b"stream",
        },
    )?;
    let (_, FalResponse::Written(count)) = protocol::decode_response(&written.payload)
        .map_err(|_| "FAL2 stream write reply invalid")?
    else {
        return Err("FAL2 stream write shape invalid");
    };
    if count != 6 {
        return Err("FAL2 stream write count mismatch");
    }

    fal_call(
        &mut client,
        grant,
        &FalRequest::Create {
            name: "move-source",
            kind: NodeKind::Property,
            rights: FalRights::READ_PROPERTY,
            value: &delete_value,
        },
    )?;
    fal_call(
        &mut client,
        grant,
        &FalRequest::Create {
            name: "move-target",
            kind: NodeKind::Directory,
            rights: FalRights::ALL,
            value: &[],
        },
    )?;

    fal_call(
        &mut client,
        grant,
        &FalRequest::Create {
            name: "f2-dir",
            kind: NodeKind::Directory,
            rights: FalRights::ALL,
            value: &[],
        },
    )?;
    let directory = client
        .derive(
            grant,
            "f2-dir",
            FalRights::TRAVERSE
                | FalRights::ENUMERATE
                | FalRights::CREATE
                | FalRights::READ_PROPERTY,
            deadline,
        )
        .map_err(|_| "FAL2 directory derive failed")?;
    let mut child_client = FalClient::new();
    child_client
        .call(
            &directory,
            &FalRequest::Create {
                name: "leaf",
                kind: NodeKind::Property,
                rights: FalRights::READ_PROPERTY,
                value: &leaf_value,
            },
            deadline,
        )
        .map_err(|_| "FAL2 derived directory create failed")?;
    drop(directory);

    fal_call(
        &mut client,
        grant,
        &FalRequest::Link {
            name: "f2-link",
            target: "f2-dir/leaf",
            rights: FalRights::TRAVERSE,
        },
    )?;

    if property.kind != NodeKind::Property {
        return Err("FAL2 property metadata invalid");
    }
    Ok(())
}

fn bind_fs_route(
    route_handle: Handle,
    name: &str,
    target: &MailboxSender,
    rights: FalRights,
) -> Result<(), &'static str> {
    let route_sender = unsafe {
        Capability::from_raw(
            duplicate(route_handle, Rights::WRITE | Rights::WAIT)
                .map_err(|_| "fs route sender duplicate failed")?,
        )
    };
    let (route_sender, _) =
        MailboxSender::from_capability(route_sender).map_err(|_| "fs route sender role invalid")?;
    let bind = route::Bind { name, rights };
    let mut payload = alloc::vec![0; bind.encoded_len().ok_or("fs route binding invalid")?];
    let used = bind
        .encode(&mut payload)
        .ok_or("fs route binding encode failed")?;
    payload.truncate(used);
    let mut request =
        Request::new(route::ID, &payload).map_err(|_| "fs route request creation failed")?;
    let target_rights = Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT;
    let target_copy = unsafe {
        Capability::from_raw(
            duplicate(target.as_handle(), target_rights)
                .map_err(|_| "fs route target duplicate failed")?,
        )
    };
    request
        .push(
            target_copy,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE,
        )
        .map_err(|_| "fs route target attachment failed")?;
    let reply = Caller::new()
        .call(&route_sender, rinlib::time::Deadline::INFINITE, request)
        .map_err(|_| "fs route binding RPC failed")?;
    if !reply.handles.is_empty()
        || route::decode_status(&reply.payload).map_err(|_| "fs route binding reply invalid")?
            != route::Status::Ok
    {
        return Err("fs route binding rejected");
    }
    Ok(())
}

fn send_raw_fal_request(
    grant: &MailboxSender,
    reply_sender: &MailboxSender,
    request: &FalRequest<'_>,
    txid: u64,
    send_deadline: Deadline,
) -> Result<(), SystemCallError> {
    let capacity = protocol::HEADER_LEN
        .checked_add(
            request
                .encoded_len()
                .ok_or(SystemCallError::IllegalArgument)?,
        )
        .ok_or(SystemCallError::IllegalArgument)?;
    let mut payload = alloc::vec![0; librpc::PREFIX_LEN + capacity];
    RpcPrefix::new(RpcMessageKind::Request, txid).encode(&mut payload);
    let used = protocol::encode_request(
        request,
        rinlib::time::Deadline::INFINITE,
        &mut payload[librpc::PREFIX_LEN..],
    )
    .ok_or(SystemCallError::IllegalArgument)?;
    payload.truncate(librpc::PREFIX_LEN + used);
    let mut packet = Packet::new(protocol::ID, &payload)?;
    let reply_rights = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
    let reply = send_once(reply_sender, reply_rights)?;
    packet
        .push_front(reply.into_capability(), reply_rights)
        .map_err(|failure| failure.error)?;
    packet
        .try_send(grant, send_deadline)
        .map_err(|failure| failure.error)
}

fn stage_abandoned_reply(
    root: &mut RootSupervisor,
    grant: &MailboxSender,
) -> Result<Handle, &'static str> {
    let committed = fal_blob(b"committed")?;
    let reply = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .map_err(|_| "FAL2 blocked reply mailbox creation failed")?;
    for index in 0..MAILBOX_CAPACITY {
        root.sender(reply.peer)
            .send(0x4641_4c32_4649_4c4c + index as u64, &[])
            .map_err(|_| "FAL2 blocked reply mailbox fill failed")?;
    }
    send_raw_fal_request(
        grant,
        root.sender(reply.peer),
        &FalRequest::Create {
            name: "abandoned-create",
            kind: NodeKind::Property,
            rights: FalRights::READ_PROPERTY,
            value: &committed,
        },
        0x4142_414e_444f_4e45,
        Deadline::INFINITE,
    )
    .map_err(|_| "FAL2 abandoned request send failed")?;
    root.close_control(reply.peer)?;
    let mut client = FalClient::new();
    let deadline =
        rinlib::time::timeout_millis(5_000).map_err(|_| "FAL2 commit probe deadline invalid")?;
    let lookup = loop {
        match client.call(
            grant,
            &FalRequest::Lookup {
                path: "abandoned-create",
            },
            deadline,
        ) {
            Ok(reply) => break reply,
            Err(libfal::client::ClientError::Status(protocol::Status::NotFound)) => {
                if rinlib::time::expired(deadline).map_err(|_| "FAL2 commit probe clock failed")? {
                    return Err("FAL2 abandoned request did not commit");
                }
            }
            Err(error) => {
                debug!("FAL2 abandoned commit probe failed: {:?}", error);
                return Err("FAL2 abandoned request did not commit");
            }
        }
    };
    let (_, FalResponse::Node(info)) =
        protocol::decode_response(&lookup.payload).map_err(|_| "FAL2 commit probe invalid")?
    else {
        return Err("FAL2 commit probe shape invalid");
    };
    if info.kind != NodeKind::Property {
        return Err("FAL2 committed abandoned node metadata invalid");
    }
    Ok(reply.owner)
}

fn finish_abandoned_reply(root: &mut RootSupervisor, reply: Handle) -> Result<(), &'static str> {
    for _ in 0..MAILBOX_CAPACITY {
        discard(reply).map_err(|_| "FAL2 blocked reply discard failed")?;
    }
    if !matches!(
        receive(reply),
        Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy)
    ) {
        return Err("FAL2 abandoned response was unexpectedly delivered");
    }
    root.close_control(reply)
}

#[cfg(feature = "acceptance-stress")]
fn stress_fal2_admission(
    root: &mut RootSupervisor,
    grant: &MailboxSender,
    authority: &MailboxSender,
) -> Result<(), &'static str> {
    const REQUESTS: usize = 150;
    let mut replies = alloc::vec::Vec::new();
    replies
        .try_reserve_exact(REQUESTS)
        .map_err(|_| "FAL2 admission probe reply storage exhausted")?;
    for index in 0..REQUESTS {
        let reply = root
            .create_mailbox(
                Rights::READ | Rights::WAIT | Rights::MANAGE,
                Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
            )
            .map_err(|_| "FAL2 admission probe mailbox creation failed")?;
        for filler in 0..MAILBOX_CAPACITY {
            root.sender(reply.peer)
                .send(0x4641_4c32_5052_4553 + filler as u64, &[])
                .map_err(|_| "FAL2 admission probe mailbox fill failed")?;
        }
        match send_raw_fal_request(
            grant,
            root.sender(reply.peer),
            &FalRequest::Lookup {
                path: "pressure-missing",
            },
            0x5052_4553_5300_0000 + index as u64,
            rinlib::time::timeout_millis(10_000)
                .map_err(|_| "FAL2 admission probe send deadline invalid")?,
        ) {
            Ok(()) => replies.push(reply),
            Err(SystemCallError::MailboxFull | SystemCallError::DeadlineExpired) => {
                debug!("FAL2 admission send backpressure at request {}", index);
                for _ in 0..MAILBOX_CAPACITY {
                    discard(reply.owner)
                        .map_err(|_| "FAL2 admission unsent filler discard failed")?;
                }
                root.close_control(reply.peer)?;
                root.close_control(reply.owner)?;
                break;
            }
            Err(error) => {
                debug!(
                    "FAL2 admission raw send failed: index={}, error={:?}",
                    index, error
                );
                return Err("FAL2 admission probe send failed");
            }
        }
        if index % 8 == 7 {
            rinlib::time::sleep_until(
                rinlib::time::timeout_millis(100)
                    .map_err(|_| "FAL2 admission pacing deadline invalid")?,
            )
            .map_err(|_| "FAL2 admission pacing failed")?;
        }
    }
    rinlib::time::sleep_until(
        rinlib::time::timeout_millis(2_000)
            .map_err(|_| "FAL2 admission probe settling deadline invalid")?,
    )
    .map_err(|_| "FAL2 admission probe settle failed")?;
    let deadline = rinlib::time::timeout_millis(5_000)
        .map_err(|_| "Registry admission probe deadline invalid")?;
    let request = service_protocol::Request::QueryName {
        name: "fs.secondary",
    };
    let mut payload = alloc::vec![0; service_protocol::HEADER_LEN
        + request
            .encoded_len()
            .ok_or("Registry admission probe size invalid")?];
    let used = service_protocol::encode_request(&request, deadline, &mut payload)
        .ok_or("Registry admission probe encoding failed")?;
    let rpc = Request::new(service_protocol::ID, &payload[..used])
        .map_err(|_| "Registry admission probe RPC invalid")?;
    let reply = Caller::new()
        .call(authority, deadline, rpc)
        .map_err(|_| "Registry admission probe call failed")?;
    let (header, _) = service_protocol::decode_response(&reply.payload)
        .map_err(|_| "Registry admission probe reply invalid")?;
    if header.op != service_protocol::Op::QueryName
        || header.status != service_protocol::Status::Quota
        || !reply.handles.is_empty()
    {
        return Err("Registry admission probe did not reject saturated task pool");
    }
    let receive_deadline = rinlib::time::timeout_millis(30_000)
        .map_err(|_| "FAL2 admission probe receive deadline invalid")?;
    for reply in &replies {
        for _ in 0..MAILBOX_CAPACITY {
            discard(reply.owner).map_err(|_| "FAL2 admission probe filler discard failed")?;
        }
    }
    let mut quota = 0;
    for (index, mailbox) in replies.into_iter().enumerate() {
        let reply = rinlib::ipc::message::wait_message_until(mailbox.owner, receive_deadline)
            .map_err(|_| "FAL2 admission probe response missing")?;
        if reply.header.kind != protocol::ID || !reply.handles.is_empty() {
            return Err("FAL2 admission probe response layout invalid");
        }
        let prefix = RpcPrefix::decode(&reply.payload)
            .map_err(|_| "FAL2 admission probe RPC prefix invalid")?;
        if prefix.kind != RpcMessageKind::Response
            || prefix.txid != 0x5052_4553_5300_0000 + index as u64
        {
            return Err("FAL2 admission probe RPC reply kind invalid");
        }
        let (header, FalResponse::Empty) =
            protocol::decode_response(&reply.payload[librpc::PREFIX_LEN..])
                .map_err(|_| "FAL2 admission probe response invalid")?
        else {
            return Err("FAL2 admission probe response shape invalid");
        };
        match header.status {
            protocol::Status::Quota => quota += 1,
            protocol::Status::NotFound => {}
            _ => return Err("FAL2 admission probe unexpected status"),
        }
        root.close_control(mailbox.peer)?;
        root.close_control(mailbox.owner)?;
    }
    if quota == 0 {
        return Err("FAL2 admission probe did not reach request quota");
    }
    if !matches!(
        FalClient::new().call(
            grant,
            &FalRequest::Lookup {
                path: "pressure-cleared"
            },
            rinlib::time::timeout_millis(5_000)
                .map_err(|_| "FAL2 admission recovery deadline invalid")?,
        ),
        Err(libfal::client::ClientError::Status(
            protocol::Status::NotFound
        ))
    ) {
        return Err("FAL2 admission probe did not recover");
    }
    let deadline = rinlib::time::timeout_millis(5_000)
        .map_err(|_| "Registry admission recovery deadline invalid")?;
    let used = service_protocol::encode_request(&request, deadline, &mut payload)
        .ok_or("Registry admission recovery encoding failed")?;
    let rpc = Request::new(service_protocol::ID, &payload[..used])
        .map_err(|_| "Registry admission recovery RPC invalid")?;
    let reply = Caller::new()
        .call(authority, deadline, rpc)
        .map_err(|_| "Registry admission recovery call failed")?;
    let (header, _) = service_protocol::decode_response(&reply.payload)
        .map_err(|_| "Registry admission recovery reply invalid")?;
    if header.op != service_protocol::Op::QueryName
        || header.status != service_protocol::Status::NotFound
        || !reply.handles.is_empty()
    {
        return Err("Registry admission probe did not recover");
    }
    debug!(
        "FAL2 admission pressure passed: requests={}, quota={}",
        REQUESTS, quota
    );
    Ok(())
}

struct StalledDelegate {
    downstream: rinlib::ipc::message::ReceivedMessage,
    downstream_owner: Handle,
    client_reply: Handle,
}

fn stage_stalled_delegate(
    root: &mut RootSupervisor,
    route: Handle,
    grant: &MailboxSender,
) -> Result<StalledDelegate, &'static str> {
    let downstream = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .map_err(|_| "FAL2 stalled downstream creation failed")?;
    bind_fs_route(
        route,
        "stalled",
        root.sender(downstream.peer),
        FalRights::TRAVERSE | FalRights::ENUMERATE,
    )?;
    let client_reply = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .map_err(|_| "FAL2 stalled client reply creation failed")?;
    send_raw_fal_request(
        grant,
        root.sender(client_reply.peer),
        &FalRequest::Lookup {
            path: "stalled/leaf",
        },
        0x5354_414c_4c45_4401,
        Deadline::INFINITE,
    )
    .map_err(|_| "FAL2 stalled request send failed")?;
    root.close_control(client_reply.peer)?;
    root.close_control(downstream.peer)?;
    let downstream_message =
        wait_message(downstream.owner).map_err(|_| "FAL2 downstream Derive was not committed")?;
    if downstream_message.header.kind != protocol::ID {
        return Err("FAL2 downstream Derive protocol invalid");
    }
    let prefix = RpcPrefix::decode(&downstream_message.payload)
        .map_err(|_| "FAL2 downstream Derive prefix invalid")?;
    if prefix.kind != RpcMessageKind::Request {
        return Err("FAL2 downstream Derive was not a request");
    }
    let (header, request) =
        protocol::decode_request(&downstream_message.payload[librpc::PREFIX_LEN..])
            .map_err(|_| "FAL2 downstream Derive body invalid")?;
    if header.op != protocol::Op::Derive || !matches!(request, FalRequest::Derive { path: "", .. })
    {
        return Err("FAL2 downstream request was not root Derive");
    }
    Ok(StalledDelegate {
        downstream: downstream_message,
        downstream_owner: downstream.owner,
        client_reply: client_reply.owner,
    })
}

fn call_registration(
    authority: &MailboxSender,
    request: &service_protocol::Request<'_>,
    deadline: Deadline,
    endpoint: Option<rinlib::ipc::capability::Capability>,
) -> Result<librpc::Reply, &'static str> {
    let mut payload = alloc::vec![0; service_protocol::HEADER_LEN
        + request.encoded_len().ok_or("registration request size invalid")?];
    let used = service_protocol::encode_request(request, deadline, &mut payload)
        .ok_or("registration request encoding failed")?;
    let mut rpc = Request::new(service_protocol::ID, &payload[..used])
        .map_err(|_| "registration RPC request invalid")?;
    if let Some(endpoint) = endpoint {
        rpc.push(
            endpoint,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
        )
        .map_err(|_| "registration endpoint attachment failed")?;
    }
    Caller::new()
        .call(authority, deadline, rpc)
        .map_err(|_| "registration RPC call failed")
}

fn send_raw_registration_request(
    authority: &MailboxSender,
    reply_sender: &MailboxSender,
    request: &service_protocol::Request<'_>,
    endpoint: Option<Capability>,
    txid: u64,
    deadline: Deadline,
) -> Result<(), SystemCallError> {
    let capacity = service_protocol::HEADER_LEN
        .checked_add(
            request
                .encoded_len()
                .ok_or(SystemCallError::IllegalArgument)?,
        )
        .ok_or(SystemCallError::IllegalArgument)?;
    let mut payload = alloc::vec![0; librpc::PREFIX_LEN + capacity];
    RpcPrefix::new(RpcMessageKind::Request, txid).encode(&mut payload);
    let used =
        service_protocol::encode_request(request, deadline, &mut payload[librpc::PREFIX_LEN..])
            .ok_or(SystemCallError::IllegalArgument)?;
    payload.truncate(librpc::PREFIX_LEN + used);
    let mut packet = Packet::new(service_protocol::ID, &payload)?;
    if let Some(endpoint) = endpoint {
        packet
            .push(
                endpoint,
                Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
            )
            .map_err(|failure| failure.error)?;
    }
    let reply_rights = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
    let reply = send_once(reply_sender, reply_rights)?;
    packet
        .push_front(reply.into_capability(), reply_rights)
        .map_err(|failure| failure.error)?;
    packet
        .try_send(authority, deadline)
        .map_err(|failure| failure.error)
}

fn blocked_registration_reply(
    root: &mut RootSupervisor,
) -> Result<rinlib::shared::object::HandlePair, &'static str> {
    let reply = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .map_err(|_| "registration blocked reply mailbox creation failed")?;
    for index in 0..MAILBOX_CAPACITY {
        root.sender(reply.peer)
            .send(0x5245_4742_4c4f_434b + index as u64, &[])
            .map_err(|_| "registration blocked reply fill failed")?;
    }
    Ok(reply)
}

fn registration_endpoint_sender(
    endpoint: Handle,
    rights: Rights,
) -> Result<Capability, &'static str> {
    // SAFETY: duplicate 创建独立 handle，由返回的 capability 唯一负责关闭。
    Ok(unsafe {
        Capability::from_raw(
            duplicate(endpoint, rights)
                .map_err(|_| "registration endpoint sender duplicate failed")?,
        )
    })
}

fn query_registration_name(
    authority: &MailboxSender,
    name: &str,
    deadline: Deadline,
) -> Result<Option<service_protocol::InstanceInfo>, &'static str> {
    let reply = call_registration(
        authority,
        &service_protocol::Request::QueryName { name },
        deadline,
        None,
    )?;
    let (header, response) = service_protocol::decode_response(&reply.payload)
        .map_err(|_| "registration name query reply invalid")?;
    match (header.status, response) {
        (service_protocol::Status::Ok, service_protocol::Response::Instance(info)) => {
            Ok(Some(info))
        }
        (service_protocol::Status::NotFound, service_protocol::Response::Empty) => Ok(None),
        _ => Err("registration name query status invalid"),
    }
}

fn probe_registration_reply_boundaries(
    root: &mut RootSupervisor,
    directory: &MailboxSender,
    authority: &MailboxSender,
) -> Result<(), &'static str> {
    let deadline = rinlib::time::timeout_millis(15_000)
        .map_err(|_| "registration reply probe deadline invalid")?;
    let name = "probe.reply";
    let scoped = service_client::delegate_name(authority, name, deadline)
        .map_err(|_| "registration reply probe delegation failed")?;
    let endpoint = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .map_err(|_| "registration reply probe endpoint failed")?;
    let transport = Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT;
    let policy = ExportPolicy {
        protocol: ValueProtocol::Mailbox,
        mode: ExportMode::Repeatable,
        transport,
        fal_ceiling: FalRights::NONE,
    };
    let register = service_protocol::Request::Register {
        name,
        protocol: service_protocol::ID,
        version: u32::from(service_protocol::VERSION),
        policy,
        establish_deadline: deadline,
    };
    let abandoned = blocked_registration_reply(root)?;
    send_raw_registration_request(
        &scoped,
        root.sender(abandoned.peer),
        &register,
        Some(registration_endpoint_sender(endpoint.peer, transport)?),
        0x5245_4741_4241_4e44,
        deadline,
    )
    .map_err(|_| "registration abandoned Register send failed")?;
    root.close_control(abandoned.peer)?;
    loop {
        if matches!(
            query_registration_name(&scoped, name, deadline)?,
            Some(info) if info.state == service_protocol::State::Starting
        ) {
            break;
        }
        if rinlib::time::expired(deadline).map_err(|_| "registration Starting clock failed")? {
            return Err("registration abandoned Register never reached Starting");
        }
    }
    root.close_control(abandoned.owner)?;
    loop {
        if query_registration_name(&scoped, name, deadline)?.is_none() {
            break;
        }
        if rinlib::time::expired(deadline).map_err(|_| "registration abandon clock failed")? {
            return Err("registration abandoned establishment retained its name");
        }
    }
    let watch = FalClient::new()
        .subscribe(
            directory,
            "",
            protocol::WatchMask::CREATE | protocol::WatchMask::DELETE,
            deadline,
        )
        .map_err(|_| "registration reply probe root Watch failed")?;
    let mut established = call_registration(
        &scoped,
        &register,
        deadline,
        Some(registration_endpoint_sender(endpoint.peer, transport)?),
    )?;
    let (header, response) = service_protocol::decode_response(&established.payload)
        .map_err(|_| "registration reply probe Register reply invalid")?;
    if header.status != service_protocol::Status::Ok
        || !matches!(response, service_protocol::Response::Instance(info) if info.state == service_protocol::State::Starting)
        || established.handles.remaining() != 1
    {
        return Err("registration reply probe Register refused");
    }
    let control = established
        .handles
        .take(0)
        .map_err(|_| "registration reply probe control missing")?;
    let (control, _) = MailboxSender::from_capability(control)
        .map_err(|_| "registration reply probe control role invalid")?;
    let blocked_ready = blocked_registration_reply(root)?;
    send_raw_registration_request(
        &control,
        root.sender(blocked_ready.peer),
        &service_protocol::Request::PublishReady,
        None,
        0x5245_4752_4541_4459,
        deadline,
    )
    .map_err(|_| "registration blocked Ready send failed")?;
    root.close_control(blocked_ready.peer)?;
    if !matches!(
        watch.wait(deadline).map_err(|_| "registration blocked Ready CREATE missing")?,
        SubscriptionEvent::Events(events) if events.contains(protocol::WatchMask::CREATE)
    ) {
        return Err("registration blocked Ready CREATE invalid");
    }
    let ready = call_registration(&control, &service_protocol::Request::Query, deadline, None)?;
    let (header, response) = service_protocol::decode_response(&ready.payload)
        .map_err(|_| "registration blocked Ready Query invalid")?;
    let service_protocol::Response::Instance(info) = response else {
        return Err("registration blocked Ready Query shape invalid");
    };
    if header.status != service_protocol::Status::Ok || info.state != service_protocol::State::Ready
    {
        return Err("registration blocked Ready did not commit");
    }
    root.close_control(blocked_ready.owner)?;
    let blocked_withdraw = blocked_registration_reply(root)?;
    send_raw_registration_request(
        &scoped,
        root.sender(blocked_withdraw.peer),
        &service_protocol::Request::Withdraw {
            name,
            expected_instance: info.instance,
            expected_generation: info.generation,
        },
        None,
        0x5245_4757_4954_4844,
        deadline,
    )
    .map_err(|_| "registration blocked Withdraw send failed")?;
    root.close_control(blocked_withdraw.peer)?;
    if !matches!(
        watch.wait(deadline).map_err(|_| "registration blocked Withdraw DELETE missing")?,
        SubscriptionEvent::Events(events) if events.contains(protocol::WatchMask::DELETE)
    ) {
        return Err("registration blocked Withdraw DELETE invalid");
    }
    let drained = call_registration(&control, &service_protocol::Request::Query, deadline, None)?;
    let (header, response) = service_protocol::decode_response(&drained.payload)
        .map_err(|_| "registration blocked Withdraw Query invalid")?;
    if header.status != service_protocol::Status::Ok
        || !matches!(response, service_protocol::Response::Instance(info)
            if info.reason == service_protocol::TerminalReason::Withdrawn
                && matches!(info.state, service_protocol::State::Draining | service_protocol::State::Terminal))
    {
        return Err("registration blocked Withdraw did not commit");
    }
    root.close_control(blocked_withdraw.owner)?;
    if query_registration_name(&scoped, name, deadline)?.is_some() {
        return Err("registration blocked Withdraw retained its name");
    }
    root.close_control(endpoint.owner)?;
    root.close_control(endpoint.peer)?;
    drop(control);
    drop(scoped);
    drop(watch);
    debug!("registration abandoned Register and blocked Ready/Withdraw committed consistently");
    Ok(())
}

fn probe_registration_endpoint_retirement(
    root: &mut RootSupervisor,
    directory: &MailboxSender,
    authority: &MailboxSender,
) -> Result<(), &'static str> {
    let deadline = rinlib::time::timeout_millis(10_000)
        .map_err(|_| "registration endpoint probe deadline invalid")?;
    let watch = FalClient::new()
        .subscribe(
            directory,
            "",
            protocol::WatchMask::CREATE | protocol::WatchMask::DELETE,
            deadline,
        )
        .map_err(|_| "registration endpoint probe root Watch failed")?;
    let scoped = service_client::delegate_name(authority, "probe.endpoint", deadline)
        .map_err(|_| "registration endpoint probe delegation failed")?;
    let endpoint = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .map_err(|_| "registration endpoint probe mailbox failed")?;
    let transport = Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE;
    let policy = ExportPolicy {
        protocol: ValueProtocol::Mailbox,
        mode: ExportMode::Repeatable,
        transport,
        fal_ceiling: FalRights::NONE,
    };
    let sender = registration_endpoint_sender(endpoint.peer, transport)?;
    let mut registered = call_registration(
        &scoped,
        &service_protocol::Request::Register {
            name: "probe.endpoint",
            protocol: service_protocol::ID,
            version: u32::from(service_protocol::VERSION),
            policy,
            establish_deadline: deadline,
        },
        deadline,
        Some(sender),
    )?;
    let (header, response) = service_protocol::decode_response(&registered.payload)
        .map_err(|_| "registration endpoint probe Register reply invalid")?;
    if header.status != service_protocol::Status::Ok
        || !matches!(response, service_protocol::Response::Instance(_))
        || registered.handles.remaining() != 1
    {
        return Err("registration endpoint probe Register refused");
    }
    let capability = registered
        .handles
        .take(0)
        .map_err(|_| "registration endpoint probe control missing")?;
    let (control, _) = MailboxSender::from_capability(capability)
        .map_err(|_| "registration endpoint probe control role invalid")?;
    debug!("FAL2 registration endpoint probe registered");
    let published = call_registration(
        &control,
        &service_protocol::Request::PublishReady,
        deadline,
        None,
    )?;
    let (header, response) = service_protocol::decode_response(&published.payload)
        .map_err(|_| "registration endpoint probe Ready reply invalid")?;
    if header.status != service_protocol::Status::Ok
        || !matches!(response, service_protocol::Response::Instance(info) if info.state == service_protocol::State::Ready)
    {
        return Err("registration endpoint probe Ready refused");
    }
    debug!("FAL2 registration endpoint probe ready");
    if !matches!(
        watch.wait(deadline).map_err(|_| "registration endpoint probe CREATE missing")?,
        SubscriptionEvent::Events(events) if events.contains(protocol::WatchMask::CREATE)
    ) {
        return Err("registration endpoint probe CREATE invalid");
    }
    root.close_control(endpoint.owner)?;
    debug!("FAL2 registration endpoint probe endpoint closed");
    if !matches!(
        watch.wait(deadline).map_err(|_| "registration endpoint probe DELETE missing")?,
        SubscriptionEvent::Events(events) if events.contains(protocol::WatchMask::DELETE)
    ) {
        return Err("registration endpoint probe DELETE invalid");
    }
    debug!("FAL2 registration endpoint probe delete observed");
    let queried = call_registration(&control, &service_protocol::Request::Query, deadline, None)?;
    let (header, response) = service_protocol::decode_response(&queried.payload)
        .map_err(|_| "registration endpoint probe Query reply invalid")?;
    if header.status != service_protocol::Status::Ok
        || !matches!(response, service_protocol::Response::Instance(info)
            if info.reason == service_protocol::TerminalReason::EndpointClosed
                && matches!(info.state, service_protocol::State::Draining | service_protocol::State::Terminal))
    {
        return Err("registration endpoint probe control lost endpoint terminal state");
    }
    if !matches!(
        FalClient::new().call(
            directory,
            &FalRequest::Read {
                path: "probe.endpoint"
            },
            deadline,
        ),
        Err(libfal::client::ClientError::Status(
            protocol::Status::NotFound
        ))
    ) {
        return Err("registration endpoint probe remained discoverable");
    }
    root.close_control(endpoint.peer)?;
    drop(control);
    drop(scoped);
    drop(watch);
    debug!("registration endpoint CLOSED withdrew Ready while control remained active");
    Ok(())
}

#[cfg(feature = "acceptance-stress")]
fn reply_to_cancelled_delegate(
    mut downstream: rinlib::ipc::message::ReceivedMessage,
) -> Result<(), &'static str> {
    let request = RpcPrefix::decode(&downstream.payload)
        .map_err(|_| "cancelled Derive reply prefix invalid")?;
    let reply = downstream
        .handles
        .take(0)
        .map_err(|_| "cancelled Derive reply sender missing")?;
    let (reply, _) = rinlib::ipc::message::SendOnce::from_capability(reply)
        .map_err(|_| "cancelled Derive reply sender role invalid")?;
    let returned =
        rinlib::ipc::message::Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)
            .map_err(|_| "cancelled Derive reply capability creation failed")?;
    let rights = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
    let minted = returned
        .mint(0, rights)
        .map_err(|_| "cancelled Derive reply capability mint failed")?;
    let lifetime = minted.lifetime;
    let granted = minted.sender.into_capability();
    let mut body = [0; protocol::HEADER_LEN + 128];
    let used = protocol::encode_response(
        protocol::Op::Derive,
        protocol::Status::Ok,
        Deadline::INFINITE,
        &FalResponse::Node(protocol::NodeInfo {
            identity: 1,
            version: 1,
            kind: libfal::node::NodeKind::Directory,
            rights: FalRights::TRAVERSE,
            size: 0,
        }),
        &mut body,
    )
    .ok_or("cancelled Derive reply body invalid")?;
    let mut payload = alloc::vec![0; librpc::PREFIX_LEN + used];
    RpcPrefix::new(RpcMessageKind::Response, request.txid).encode(&mut payload);
    payload[librpc::PREFIX_LEN..].copy_from_slice(&body[..used]);
    let mut packet =
        Packet::new(protocol::ID, &payload).map_err(|_| "cancelled Derive reply packet invalid")?;
    packet
        .push(granted, rights)
        .map_err(|_| "cancelled Derive reply capability attachment failed")?;
    let deadline = rinlib::time::timeout_millis(5_000)
        .map_err(|_| "cancelled Derive reply deadline invalid")?;
    packet
        .try_reply(reply, deadline)
        .map_err(|_| "cancelled Derive late reply was not accepted")?;
    let observed = wait_many(
        &[WaitItem::new(
            lifetime.as_handle(),
            ObjectSignals::CLOSED,
            0,
        )],
        5_000,
    )
    .map_err(|_| "cancelled Derive late capability was not released")?;
    if !observed.observed.contains(ObjectSignals::CLOSED) {
        return Err("cancelled Derive late capability Lifetime was not closed");
    }
    Ok(())
}

#[cfg(feature = "acceptance-stress")]
fn cancel_stalled_delegate_while_provider_runs(
    root: &mut RootSupervisor,
    route: Handle,
    grant: &MailboxSender,
) -> Result<(), &'static str> {
    let mut stalled = alloc::vec::Vec::new();
    stalled
        .try_reserve_exact(CANCELLATION_PROBES)
        .map_err(|_| "FAL2 cancellation probe storage exhausted")?;
    for index in 0..CANCELLATION_PROBES {
        debug!("FAL2 upstream cancellation staging: index={}", index);
        let pending = stage_stalled_delegate(root, route, grant)?;
        debug!("FAL2 upstream cancellation delivered: index={}", index);
        root.close_control(pending.client_reply)?;
        stalled.push(pending);
        rinlib::time::sleep_until(
            rinlib::time::timeout_millis(50)
                .map_err(|_| "FAL2 upstream cancellation pacing deadline invalid")?,
        )
        .map_err(|_| "FAL2 upstream cancellation pacing failed")?;
    }
    for (index, pending) in stalled.into_iter().enumerate() {
        if index == 0 {
            reply_to_cancelled_delegate(pending.downstream)?;
        } else {
            drop(pending.downstream);
        }
        root.close_control(pending.downstream_owner)?;
    }
    if !matches!(
        FalClient::new().call(
            grant,
            &FalRequest::Lookup {
                path: "cancelled-recovered"
            },
            rinlib::time::timeout_millis(5_000)
                .map_err(|_| "FAL2 cancellation recovery deadline invalid")?,
        ),
        Err(libfal::client::ClientError::Status(
            protocol::Status::NotFound
        ))
    ) {
        return Err("FAL2 cancellation did not recover provider ingress");
    }
    debug!(
        "FAL2 upstream cancellation released pending Dispatcher slots: probes={}",
        CANCELLATION_PROBES
    );
    Ok(())
}

fn finish_stalled_delegate(
    root: &mut RootSupervisor,
    stalled: StalledDelegate,
) -> Result<(), &'static str> {
    drop(stalled.downstream);
    if !matches!(
        receive(stalled.client_reply),
        Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy)
    ) {
        return Err("FAL2 cancelled Delegate produced a client response");
    }
    root.close_control(stalled.client_reply)?;
    root.close_control(stalled.downstream_owner)
}

fn release_fs_provider(
    root: &mut RootSupervisor,
    bootstrap: Handle,
    release: Handle,
    route: Handle,
) -> Result<protocol::ProviderReport, &'static str> {
    notification::signal(release, 1).map_err(|_| "fs provider release signal failed")?;
    collect_fs_provider(root, bootstrap, release, route)
}

fn collect_fs_provider(
    root: &mut RootSupervisor,
    bootstrap: Handle,
    release: Handle,
    route: Handle,
) -> Result<protocol::ProviderReport, &'static str> {
    let stopped =
        wait_message(bootstrap).map_err(|_| "fs provider shutdown report receive failed")?;
    if stopped.header.kind != protocol::PROVIDER_STOPPED_KIND || !stopped.handles.is_empty() {
        return Err("fs provider shutdown report layout invalid");
    }
    let report = protocol::ProviderReport::decode(&stopped.payload)
        .map_err(|_| "fs provider shutdown report invalid")?;
    root.close_control(route)?;
    root.close_control(release)?;
    root.close_control(bootstrap)?;
    Ok(report)
}

fn root_memory_pool() -> Handle {
    env::startup_handle(initial::ROOT_MEMORY_POOL)
        .expect("init must hold root MemoryPool authority")
}

fn root_pool_allocated() -> Result<u64, SystemCallError> {
    let handle = duplicate(root_memory_pool(), Rights::READ)?;
    // SAFETY: duplicate 新建唯一 raw owner，本作用域不保留 alias，Drop 负责关闭。
    let pool = unsafe { MemoryPool::from_handle(handle) };
    Ok(pool.query()?.allocated)
}

#[cfg(not(feature = "acceptance-stress"))]
fn test_process_memory_binding(job: Handle) -> Result<(), &'static str> {
    let baseline = root_pool_allocated().map_err(|_| "root Pool query failed")?;
    let created = process::create(job, SUPERVISOR_RIGHTS).map_err(|_| "shell create failed")?;
    let result = (|| {
        if root_pool_allocated()? != baseline {
            return Err(SystemCallError::InternalError);
        }
        if process::map(
            created.builder,
            0x1000,
            rinlib::shared::proc::PROCESS_PAGE_SIZE,
            rinlib::shared::proc::ProcessMapFlags::READ,
        ) != Err(SystemCallError::ObjectNotAvailable)
        {
            return Err(SystemCallError::InternalError);
        }
        if process::bind_memory(created.builder, created.builder)
            != Err(SystemCallError::IllegalArgument)
        {
            return Err(SystemCallError::InternalError);
        }

        let wrong_kind = duplicate(job, Rights::GRANT)?;
        let wrong_result = process::bind_memory(created.builder, wrong_kind);
        let _ = unsafe { close(wrong_kind) };
        if wrong_result != Err(SystemCallError::WrongObjectType) {
            return Err(SystemCallError::InternalError);
        }
        let no_grant = duplicate(root_memory_pool(), Rights::READ)?;
        let rights_result = process::bind_memory(created.builder, no_grant);
        let _ = unsafe { close(no_grant) };
        if rights_result != Err(SystemCallError::RightsDenied) {
            return Err(SystemCallError::InternalError);
        }

        let binding = duplicate(root_memory_pool(), Rights::GRANT)?;
        process::bind_memory(created.builder, binding)?;
        if root_pool_allocated()? != baseline + 1 {
            return Err(SystemCallError::InternalError);
        }
        let repeat = duplicate(root_memory_pool(), Rights::GRANT)?;
        let repeat_result = process::bind_memory(created.builder, repeat);
        let _ = unsafe { close(repeat) };
        if repeat_result != Err(SystemCallError::ObjectNotAvailable) {
            return Err(SystemCallError::InternalError);
        }
        Ok(())
    })();

    let cleanup = unsafe { process::abandon_to_completion(created) };
    if result.is_err() || cleanup.is_err() {
        return Err("BindMemory contract or cleanup failed");
    }
    if root_pool_allocated().map_err(|_| "root Pool final query failed")? != baseline {
        return Err("BindMemory Pool charge did not refund");
    }
    debug!("process shell and memory binding acceptance passed");
    Ok(())
}

fn main() {
    debug!("Hello, init!");
    let root_job = env::startup_handle(initial::ROOT_JOB).expect("init must hold root JobControl");
    let reset =
        env::startup_handle(initial::SYSTEM_RESET).expect("init must hold SystemReset authority");
    // 尚无服务责任时准备根承载；失败不会遗忘已启动服务。
    let mut root = loop {
        match RootSupervisor::prepare() {
            Ok(root) => break root,
            Err(error) => {
                debug!("root supervisor preparation failed: {:?}", error);
                if let Ok(deadline) = rinlib::time::timeout_millis(100) {
                    let _ = rinlib::time::sleep_until(deadline);
                }
            }
        }
    };
    let services = match root.create_job(root_job, JOB_FULL_RIGHTS) {
        Ok(job) => job,
        Err(error) => {
            debug!("services job create failed: {:?}", error);
            root.serve_forever();
        }
    };
    if let Err(error) = root.bind_services(services) {
        debug!("root standby preparation failed: {:?}", error);
        root.serve_forever();
    }
    debug!("services job established");
    if let Err(failure) = run(&mut root, services) {
        match failure {
            RunFailure::Message(stage) => debug!("init acceptance failed: {}", stage),
        }
        root.serve_forever();
    }
    submit_shutdown(root_job, reset, root);
}

/// 先验证 capability 负路径，再提交唯一的成功路径。
fn submit_shutdown(root_job: Handle, reset: Handle, root: RootSupervisor) -> ! {
    assert!(matches!(
        system::reset(root_job, ResetAction::Shutdown, ResetReason::Requested),
        Err(SystemCallError::WrongObjectType)
    ));
    let attenuated =
        duplicate(reset, Rights::DUPLICATE).expect("SystemReset attenuation must succeed");
    assert!(matches!(
        system::reset(attenuated, ResetAction::Shutdown, ResetReason::Requested),
        Err(SystemCallError::RightsDenied)
    ));
    unsafe { close(attenuated) }.expect("attenuated SystemReset close must succeed");
    debug!("system reset authority checks passed");
    debug!("init: submitting explicit system shutdown");

    match system::reset(reset, ResetAction::Shutdown, ResetReason::Requested) {
        Err(error) => {
            debug!("system reset failed: {:?}", error);
            root.idle_forever()
        }
        Ok(never) => match never {},
    }
}

/// 小集合收束辅助：对保留 control 的服务 kill → 等待收束 → Drain →
/// close（验收线 1 的派生接管路径复用）。
fn kill_and_supervise(
    root: &mut RootSupervisor,
    _job: Handle,
    supervised: alloc::vec::Vec<Supervised>,
) -> Result<(), &'static str> {
    for target in &supervised {
        let _ = process::kill(target.control, 0x1F);
    }
    root.collect_targets(supervised)
}

fn test_memory_mapping() -> Result<(), &'static str> {
    #[cfg(not(feature = "acceptance-stress"))]
    let pool_baseline = root_pool_allocated().map_err(|_| "memory Pool baseline query failed")?;
    let page = rinlib::shared::proc::PROCESS_PAGE_SIZE;
    let region = MappedRegion::map_anonymous(
        3 * page,
        page,
        page,
        MemoryProtection::ReadWrite,
        Placement::Anywhere,
    )
    .map_err(|_| "anonymous guarded Map failed")?;
    let usable = region.usable().ok_or("Map returned no usable range")?;
    // SAFETY: usable 是本线程刚取得的 RW anonymous mapping。
    unsafe {
        (usable.start as *mut u64).write_volatile(0x1122_3344_5566_7788);
        ((usable.end - core::mem::size_of::<u64>()) as *mut u64)
            .write_volatile(0x8877_6655_4433_2211);
    }
    region
        .protect(
            usable.start..usable.start + page,
            MemoryProtection::ReadOnly,
        )
        .map_err(|_| "MemoryProtect RW to R failed")?;
    region
        .protect(
            usable.start..usable.start + page,
            MemoryProtection::ReadWrite,
        )
        .map_err(|_| "MemoryProtect R to RW failed")?;

    let middle = usable.start + page..usable.start + 2 * page;
    let remainder = region
        .unmap_range(middle)
        .map_err(|_| "partial MemoryUnmap failed")?;
    remainder
        .left
        .ok_or("partial Unmap lost left fragment")?
        .unmap()
        .map_err(|_| "left fragment Unmap failed")?;
    remainder
        .right
        .ok_or("partial Unmap lost right fragment")?
        .unmap()
        .map_err(|_| "right fragment Unmap failed")?;

    let remapped = MappedRegion::map_anonymous(
        3 * page,
        page,
        page,
        MemoryProtection::ReadWrite,
        Placement::FixedEmpty {
            usable_start: usable.start,
        },
    )
    .map_err(|_| "fixed remap after Unmap failed")?;
    let remapped_usable = remapped
        .usable()
        .ok_or("fixed remap returned no usable range")?;
    if remapped_usable != usable {
        return Err("fixed remap geometry changed");
    }
    // SAFETY: remapped usable range 是新取得的 RW anonymous mapping。
    let zeroed = unsafe {
        (remapped_usable.start as *const u64).read_volatile() == 0
            && ((remapped_usable.end - core::mem::size_of::<u64>()) as *const u64).read_volatile()
                == 0
    };
    if !zeroed {
        return Err("remapped anonymous backing was not zeroed");
    }
    remapped
        .unmap()
        .map_err(|_| "fixed remap final Unmap failed")?;

    // 公共 MemoryObject：创建 → 快照 → 两个 view → Handle 先关仍可访问 → 撤销 → 守恒。
    let object = MemoryObject::create(4 * page).map_err(|_| "MemoryObject create failed")?;
    let snapshot = object.query().map_err(|_| "MemoryObject query failed")?;
    if snapshot.bytes != 4 * page as u64
        || snapshot.state() != Some(MemoryObjectState::Mutable)
        || snapshot.write_views != 0
        || !snapshot.closes()
    {
        return Err("MemoryObject snapshot geometry or state invalid");
    }
    // 同一对象按不同 offset 与权限映入两个区间：对象 backing 只付一次。
    let writable = MappedRegion::map_object(
        &object,
        page,
        2 * page,
        0,
        0,
        MemoryProtection::ReadWrite,
        Placement::Anywhere,
    )
    .map_err(|_| "MemoryObject writable view Map failed")?;
    let readable = MappedRegion::map_object(
        &object,
        0,
        page,
        0,
        0,
        MemoryProtection::ReadOnly,
        Placement::Anywhere,
    )
    .map_err(|_| "MemoryObject read-only view Map failed")?;
    if object
        .query()
        .map_err(|_| "MemoryObject query after Map failed")?
        .write_views
        != 1
    {
        return Err("MemoryObject did not account its writable view");
    }
    let writable_usable = writable
        .usable()
        .ok_or("MemoryObject writable view returned no usable range")?;
    // SAFETY: 刚取得的 RW object view；backing 由对象拥有。
    unsafe {
        (writable_usable.start as *mut u64).write_volatile(0x1357_9bdf);
    }
    // Handle 关闭不撤销既有 view，也不释放 backing：view 强引用独立保活对象。
    object.close();
    // SAFETY: Handle 已关闭，但 view 仍然有效。
    let survived = unsafe { (writable_usable.start as *const u64).read_volatile() };
    if survived != 0x1357_9bdf {
        return Err("MemoryObject view lost its data after Handle close");
    }
    readable
        .unmap()
        .map_err(|_| "MemoryObject read-only view Unmap failed")?;
    // 部分撤销只消费本 view 的区域，不切分对象数据 backing。
    let remainder = writable
        .unmap_range(writable_usable.start..writable_usable.start + page)
        .map_err(|_| "MemoryObject partial view Unmap failed")?;
    remainder
        .right
        .ok_or("MemoryObject partial Unmap lost its right fragment")?
        .unmap()
        .map_err(|_| "MemoryObject last view Unmap failed")?;

    // Sealing 允许既有 writable view 收缩/切分，但拒绝把只读片段重新升为 writable。
    let sealing = MemoryObject::create(3 * page).map_err(|_| "sealing object create failed")?;
    let sealing_view = MappedRegion::map_object(
        &sealing,
        0,
        3 * page,
        0,
        0,
        MemoryProtection::ReadWrite,
        Placement::Anywhere,
    )
    .map_err(|_| "sealing writable view Map failed")?;
    let sealing_usable = sealing_view
        .usable()
        .ok_or("sealing writable view returned no usable range")?;
    sealing.seal().map_err(|_| "sealing request failed")?;
    sealing_view
        .protect(
            sealing_usable.start + page..sealing_usable.start + 2 * page,
            MemoryProtection::ReadOnly,
        )
        .map_err(|_| "Sealing partial writable downgrade failed")?;
    let snapshot = sealing
        .query()
        .map_err(|_| "sealing object query after downgrade failed")?;
    if snapshot.state() != Some(MemoryObjectState::Sealing) || snapshot.write_views != 2 {
        return Err("Sealing writable successor accounting invalid");
    }
    match sealing_view.protect(
        sealing_usable.start + page..sealing_usable.start + 2 * page,
        MemoryProtection::ReadWrite,
    ) {
        Err(SystemCallError::ObjectBusy) => {}
        Err(_) => return Err("Sealing write re-enable returned the wrong error"),
        Ok(()) => return Err("Sealing admitted a new writable region"),
    }
    sealing_view
        .protect(sealing_usable.clone(), MemoryProtection::ReadOnly)
        .map_err(|_| "Sealing final writable downgrade failed")?;
    let snapshot = sealing
        .query()
        .map_err(|_| "sealing object final query failed")?;
    if snapshot.state() != Some(MemoryObjectState::Executable) || snapshot.write_views != 0 {
        return Err("Sealing did not complete after the last writable successor retired");
    }
    sealing_view
        .unmap()
        .map_err(|_| "sealing view Unmap failed")?;
    sealing.close();

    // RX authority 与对象发布状态正交：完整 capability 在 Mutable 期仍不能执行，
    // Seal 后缺 EXECUTE 的裁剪副本也不能借对象状态绕过 rights。
    let executable = MemoryObject::create(page).map_err(|_| "executable object create failed")?;
    match MappedRegion::map_object(
        &executable,
        0,
        page,
        0,
        0,
        MemoryProtection::ReadExecute,
        Placement::Anywhere,
    ) {
        Err(SystemCallError::ObjectBusy) => {}
        Err(_) => return Err("mutable object RX returned the wrong error"),
        Ok(mapping) => {
            mapping
                .unmap()
                .map_err(|_| "unexpected mutable RX cleanup failed")?;
            return Err("mutable object admitted an RX view");
        }
    }
    let no_execute_handle = duplicate(executable.handle(), Rights::MAP | Rights::READ)
        .map_err(|_| "non-execute object capability derive failed")?;
    let rx_handle = duplicate(
        executable.handle(),
        Rights::MAP | Rights::READ | Rights::EXECUTE,
    )
    .map_err(|_| "execute object capability derive failed")?;
    // SAFETY: duplicate 返回两个新 Handle，本作用域分别建立唯一 typed owner。
    let no_execute = unsafe { MemoryObject::from_handle(no_execute_handle) };
    let executable_view = unsafe { MemoryObject::from_handle(rx_handle) };
    executable
        .seal()
        .map_err(|_| "executable object Seal failed")?;
    executable.close();
    match MappedRegion::map_object(
        &no_execute,
        0,
        page,
        0,
        0,
        MemoryProtection::ReadExecute,
        Placement::Anywhere,
    ) {
        Err(SystemCallError::RightsDenied) => {}
        Err(_) => return Err("RX without EXECUTE returned the wrong error"),
        Ok(mapping) => {
            mapping
                .unmap()
                .map_err(|_| "unexpected unauthorized RX cleanup failed")?;
            return Err("RX view admitted without EXECUTE right");
        }
    }
    let rx = MappedRegion::map_object(
        &executable_view,
        0,
        page,
        0,
        0,
        MemoryProtection::ReadExecute,
        Placement::Anywhere,
    )
    .map_err(|_| "authorized RX view Map failed")?;
    executable_view.close();
    no_execute.close();
    rx.unmap().map_err(|_| "authorized RX view Unmap failed")?;
    #[cfg(not(feature = "acceptance-stress"))]
    if root_pool_allocated().map_err(|_| "memory Pool final query failed")? != pool_baseline {
        return Err("MemoryObject backing Pool charge did not refund");
    }
    debug!("public memory mapping acceptance passed");
    Ok(())
}

fn test_rpc_reject_cleanup() {
    const PROTOCOL: u64 = 0x7270_6301;

    let service = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
    )
    .expect("RPC test service mailbox create failed");
    let rejected = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT,
    )
    .expect("RPC rejected Handle notification create failed");
    let rejected_alias = rejected.peer;
    let worker = rinlib::thread::Builder::new()
        .spawn(move || {
            for attempt in 0..2 {
                let mut request =
                    wait_message(service.owner).expect("RPC test request receive failed");
                if attempt == 0 {
                    let prefix = RpcPrefix::decode(&request.payload)
                        .expect("RPC test request prefix invalid");
                    assert_eq!(prefix.kind, RpcMessageKind::Request);
                    let (reply_once, _) = SendOnce::from_capability(
                        request
                            .handles
                            .take(0)
                            .expect("RPC test reply slot missing"),
                    )
                    .map_err(|failure| failure.error)
                    .expect("RPC test reply slot is not a send-once");
                    let mut response = [0u8; librpc::PREFIX_LEN + 1];
                    RpcPrefix::new(RpcMessageKind::Response, prefix.txid).encode(&mut response);
                    response[librpc::PREFIX_LEN] = attempt;
                    let mut packet = Packet::new(PROTOCOL + 1, &response)
                        .expect("RPC response packet allocation failed");
                    // SAFETY: 新创建的原始 signaler 尚未被其他 owner 接管，alias 仅用于 stale 检查。
                    let capability = unsafe { Capability::from_raw(rejected.peer) };
                    packet
                        .push(capability, Rights::SIGNAL)
                        .expect("RPC response capability preparation failed");
                    packet
                        .try_reply(reply_once, rinlib::time::Deadline::INFINITE)
                        .expect("RPC rejection response publication failed");
                    continue;
                }

                let context = RequestContext::decode(request, PROTOCOL)
                    .map_err(|rejected| rejected.reason)
                    .expect("RPC test request context invalid");
                let mut outbox = Outbox::prepare(context, 1, rinlib::time::Deadline::INFINITE, 1)
                    .map_err(|failure| failure.error)
                    .expect("RPC test Outbox preparation failed");
                outbox
                    .response_mut()
                    .expect("RPC test response remains owned")
                    .body_mut()
                    .expect("RPC test response body unavailable")[0] = attempt;
                outbox
                    .response_mut()
                    .expect("RPC test response remains owned")
                    .finish_body(1)
                    .expect("RPC test response body finalize failed");
                assert_eq!(
                    run_rpc_outbox(outbox),
                    OutboxResult::Sent,
                    "RPC Outbox did not deliver the accepted response"
                );
            }
            unsafe { close(service.owner) }.expect("RPC test service owner close failed");
        })
        .expect("RPC test worker spawn failed");

    // SAFETY: 此 sender 由原始队列工厂创建，本测试在此唯一接管关闭责任。
    let (service_sender, _) =
        MailboxSender::from_capability(unsafe { Capability::from_raw(service.peer) })
            .map_err(|failure| failure.error)
            .expect("RPC test service sender adoption failed");
    let mut caller = Caller::new();
    assert!(matches!(
        caller.call(
            &service_sender,
            rinlib::time::Deadline::INFINITE,
            Request::new(PROTOCOL, b"reject").expect("RPC request preparation failed")
        ),
        Err(CallError {
            phase: CallPhase::Sent,
            cause: CallCause::Frame(FrameRejection::ProtocolMismatch),
            ..
        })
    ));
    assert!(matches!(
        notification::signal(rejected_alias, 1),
        Err(SystemCallError::StaleHandle)
    ));
    let reply = caller
        .call(
            &service_sender,
            rinlib::time::Deadline::INFINITE,
            Request::new(PROTOCOL, b"accept").expect("RPC request preparation failed"),
        )
        .expect("RPC call after rejected reply failed");
    assert_eq!(reply.payload, [1]);
    assert!(reply.handles.is_empty());

    worker.join();
    unsafe { close(rejected.owner) }.expect("RPC rejected Handle owner close failed");
    drop(service_sender);
    debug!("RPC rejected reply cleanup passed");
}

struct RpcOutboxTask {
    outbox: Outbox,
}

#[derive(Default)]
struct RpcOutboxWorld {
    result: Option<OutboxResult>,
}

impl Task<RpcOutboxWorld> for RpcOutboxTask {
    type Family = Self;

    fn advance(
        &mut self,
        _id: u64,
        _world: &mut RpcOutboxWorld,
        requests: &mut Requests<Self>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        let advance = self.outbox.advance(requests, input, budget)?;
        Ok(advance)
    }

    fn refused(&mut self, world: &mut RpcOutboxWorld, failure: RequestFailure<Self>) {
        self.outbox.refused(world, failure);
        world.result = self.outbox.result();
    }

    fn registered(&mut self, world: &mut RpcOutboxWorld, kind: SourceKind, source: SourceId) {
        self.outbox.registered(world, kind, source);
    }

    fn unregistered(&mut self, world: &mut RpcOutboxWorld, kind: SourceKind, source: SourceId) {
        self.outbox.unregistered(world, kind, source);
        world.result = self.outbox.result();
    }

    fn stop(&mut self, world: &mut RpcOutboxWorld) {
        self.outbox.stop(world);
        world.result = self.outbox.result();
    }

    fn deadline(&self) -> rinlib::time::Deadline {
        self.outbox.deadline()
    }
}

fn run_rpc_outbox(outbox: Outbox) -> OutboxResult {
    let budget = Budget::new(&[2, 64 * 1024], 1).expect("RPC Outbox budget creation failed");
    let account = budget
        .account(&[2, 64 * 1024])
        .expect("RPC Outbox account creation failed");
    let account = account
        .view::<ExecutionResource>(&[budget.slot(0).unwrap(), budget.slot(1).unwrap()])
        .expect("RPC Outbox account binding failed");
    let set = WaitSet::create(4).expect("RPC Outbox WaitSet creation failed");
    let mut runtime = Runtime::<RpcOutboxTask, WaitSet>::new(set, 1, 1, &account)
        .expect("RPC Outbox Runtime creation failed");
    runtime
        .spawn(RpcOutboxTask { outbox }, 1)
        .map_err(|failure| failure.error)
        .expect("RPC Outbox task admission failed");
    let mut world = RpcOutboxWorld::default();
    runtime
        .run(&mut world, 1)
        .expect("RPC Outbox Runtime failed");
    let result = world.result.expect("RPC Outbox lost terminal result");
    runtime
        .close()
        .map_err(|(_, error)| error)
        .expect("RPC Outbox Runtime close failed");
    assert_eq!(
        account.usage(ExecutionResource::Task).0,
        0,
        "RPC Outbox task charge did not refund"
    );
    assert_eq!(
        account.usage(ExecutionResource::InputBytes).0,
        0,
        "RPC Outbox input charge did not refund"
    );
    result
}

/// 全部测试剧本。失败只短路后续阶段，交回 main 以 services 整树收束兜底。
fn run(root: &mut RootSupervisor, services: Handle) -> Result<(), RunFailure> {
    debug!("acceptance workload: {}", ACCEPTANCE_WORKLOAD);
    let root_job = env::startup_handle(initial::ROOT_JOB).expect("init must hold root JobControl");
    test_memory_mapping()?;
    #[cfg(not(feature = "acceptance-stress"))]
    test_process_memory_binding(services)?;
    let pm_domain = root
        .create_job(services, JOB_FULL_RIGHTS)
        .map_err(|_| "pm domain job create failed")?;
    let acceptance = root
        .create_job(services, JOB_FULL_RIGHTS)
        .map_err(|_| "acceptance job create failed")?;
    root.verify_startup_capture()?;
    let mut names = TopologyNames::new();
    names.register_job(root_job, "root");
    names.register_job(services, "services");
    names.register_job(pm_domain, "pm_domain");
    names.register_job(acceptance, "acceptance");
    names.register_process(env::pid() as u64, "init");
    let launched = launch_test_services(root, services, pm_domain, acceptance, &mut names)?;
    let pm_mailbox = launched.pm_mailbox;
    let mut first_fs = launched.first_fs;
    let registry_root_watch = launched.registry_root_watch;
    confirm_primary_fs_ready(&first_fs)?;
    let consumer = launch_fal_consumer(
        root,
        services,
        &mut names,
        launched.test_fal_image,
        &first_fs,
    )?;
    consumer.await_report(test_fal::report::DISCOVERY_ARMED)?;
    launch_secondary_fs(
        root,
        services,
        &mut names,
        launched.fs_second_image,
        [
            launched.fs_bootstrap_second_sender,
            launched.fs_release_second_owner,
            launched.fs_route_second_owner,
            launched.fs_second_scope,
        ],
    )?;
    let mut second_fs = accept_fs_provider(
        launched.fs_bootstrap_second,
        launched.fs_release_second,
        launched.fs_route_second,
        false,
    )?;
    let directory = first_fs.directory.as_ref().ok_or(RunFailure::Message(
        "primary provider omitted service directory grant",
    ))?;
    if second_fs.directory.is_some() {
        return Err(RunFailure::Message(
            "secondary provider unexpectedly published a service directory",
        ));
    }
    let primary_mailbox = first_fs.mailbox_identity.ok_or(RunFailure::Message(
        "primary provider root identity missing",
    ))?;
    let (discovered, sender_identity, mailbox_identity, registry_shutdown_watch) =
        discover_registered_fs(directory, primary_mailbox, &registry_root_watch)
            .map_err(RunFailure::Message)?;
    second_fs.grant = Some(discovered);
    second_fs.sender_identity = Some(sender_identity);
    second_fs.mailbox_identity = Some(mailbox_identity);
    if first_fs.sender_identity == second_fs.sender_identity {
        return Err(RunFailure::Message(
            "independent FAL2 providers returned the same sender identity",
        ));
    }
    consumer.await_report(test_fal::report::SECONDARY_DISCOVERED)?;
    exercise_fs_provider(
        first_fs
            .grant
            .as_ref()
            .expect("primary provider grant missing"),
    )?;
    exercise_fs_provider(
        second_fs
            .grant
            .as_ref()
            .expect("discovered secondary provider grant missing"),
    )?;
    bind_fs_route(
        first_fs.route,
        "second",
        second_fs
            .grant
            .as_ref()
            .expect("discovered secondary provider grant missing"),
        FalRights::TRAVERSE | FalRights::ENUMERATE,
    )?;
    consumer.continue_business()?;
    consumer.await_report(test_fal::report::COMPLETE)?;
    let first_grant = alloc::sync::Arc::new(
        first_fs
            .grant
            .take()
            .expect("primary provider grant missing"),
    );
    let mut namespace = PrefixTable::new();
    namespace
        .mount("/", DirectoryGrant::new(first_grant.clone()))
        .map_err(|_| "first FAL2 namespace mount failed")?;
    let second_stream_grant = FalClient::new()
        .derive(
            second_fs
                .grant
                .as_ref()
                .expect("secondary provider grant missing"),
            "",
            FalRights::TRAVERSE
                | FalRights::ENUMERATE
                | FalRights::READ_STREAM
                | FalRights::WRITE_STREAM
                | FalRights::CREATE
                | FalRights::REMOVE,
            rinlib::time::Deadline::INFINITE,
        )
        .map_err(|_| "secondary FAL2 stream grant derive failed")?;
    let second_stream_grant = alloc::sync::Arc::new(second_stream_grant);
    namespace
        .mount("/provider-b", DirectoryGrant::new(second_stream_grant))
        .map_err(|_| "secondary FAL2 stream namespace mount failed")?;
    let mut transport = FalTransport::new(rinlib::time::Deadline::INFINITE);
    let first_stream = libfs::resolve::resolve(
        &mut transport,
        &namespace,
        "/probe-stream",
        libfal::protocol::ResolvePolicy::FollowAll,
    )
    .map_err(|_| "FAL2 Open namespace resolve failed")?;
    exercise_open_streams(root, &mut transport, &first_stream)?;
    exercise_cancellable_rpc(root)?;
    drop(namespace);
    let mut shutdown_watch_client = FalClient::new();
    let shutdown_watch = shutdown_watch_client
        .subscribe(
            second_fs
                .grant
                .as_ref()
                .expect("discovered secondary provider grant missing"),
            "",
            protocol::WatchMask::CREATE,
            rinlib::time::Deadline::INFINITE,
        )
        .map_err(|_| RunFailure::Message("FAL2 shutdown Watch subscription failed"))?;
    let abandoned_reply = stage_abandoned_reply(
        root,
        second_fs
            .grant
            .as_ref()
            .expect("discovered secondary provider grant missing"),
    )?;
    drop(second_fs.grant.take());
    consumer.observe_shutdown()?;
    notification::signal(second_fs.release, 1)
        .map_err(|_| RunFailure::Message("secondary provider release signal failed"))?;
    consumer.await_report(test_fal::report::PROVIDER_CLOSED)?;
    consumer.finish(root)?;
    let second_report = collect_fs_provider(
        root,
        second_fs.bootstrap,
        second_fs.release,
        second_fs.route,
    )?;
    debug!("FAL2 secondary provider collected");
    let registry_shutdown_events = registry_shutdown_watch
        .take()
        .map_err(|_| RunFailure::Message("Registry shutdown Watch take failed"))?;
    if !registry_shutdown_events.contains(protocol::WatchMask::TERMINATED) {
        return Err(RunFailure::Message(
            "Registry automatic withdrawal omitted Watch termination",
        ));
    }
    drop(registry_shutdown_watch);
    debug!("FAL2 registry provider withdrawal observed");
    match registry_root_watch
        .wait(
            rinlib::time::timeout_millis(5_000)
                .map_err(|_| RunFailure::Message("Registry Watch deadline invalid"))?,
        )
        .map_err(|_| RunFailure::Message("Registry root withdrawal Watch failed"))?
    {
        SubscriptionEvent::Events(events) if events.contains(protocol::WatchMask::DELETE) => {}
        _ => {
            return Err(RunFailure::Message(
                "Registry root omitted withdrawal DELETE",
            ));
        }
    }
    match FalClient::new().call(
        first_fs
            .directory
            .as_ref()
            .expect("Registry directory root missing"),
        &FalRequest::Read {
            path: "fs.secondary",
        },
        rinlib::time::Deadline::INFINITE,
    ) {
        Err(libfal::client::ClientError::Status(protocol::Status::NotFound)) => {}
        _ => {
            return Err(RunFailure::Message(
                "withdrawn service remained discoverable",
            ));
        }
    }
    debug!("FAL2 withdrawn provider lookup rejected");
    drop(registry_root_watch);
    let shutdown_events = shutdown_watch
        .take()
        .map_err(|_| RunFailure::Message("FAL2 shutdown Watch take failed"))?;
    if !shutdown_events.contains(protocol::WatchMask::TERMINATED) {
        return Err(RunFailure::Message(
            "FAL2 provider shutdown omitted Watch termination",
        ));
    }
    drop(shutdown_watch);
    debug!("FAL2 provider shutdown watch observed");
    finish_abandoned_reply(root, abandoned_reply)?;
    debug!("FAL2 abandoned reply drained");
    debug!("FAL2 registration reply boundary probe starting");
    probe_registration_reply_boundaries(
        root,
        first_fs
            .directory
            .as_ref()
            .ok_or(RunFailure::Message("service directory root missing"))?,
        first_fs
            .registration_root
            .as_ref()
            .ok_or(RunFailure::Message("registration root missing"))?,
    )
    .map_err(RunFailure::Message)?;
    debug!("FAL2 registration reply boundary probe passed");
    debug!("FAL2 registration endpoint retirement probe starting");
    probe_registration_endpoint_retirement(
        root,
        first_fs
            .directory
            .as_ref()
            .ok_or(RunFailure::Message("service directory root missing"))?,
        first_fs
            .registration_root
            .as_ref()
            .ok_or(RunFailure::Message("registration root missing"))?,
    )
    .map_err(RunFailure::Message)?;
    debug!("FAL2 registration endpoint retirement probe passed");
    #[cfg(feature = "acceptance-stress")]
    stress_fal2_admission(
        root,
        &first_grant,
        first_fs
            .registration_root
            .as_ref()
            .ok_or(RunFailure::Message(
                "registration root missing during stress",
            ))?,
    )
    .map_err(RunFailure::Message)?;
    #[cfg(feature = "acceptance-stress")]
    cancel_stalled_delegate_while_provider_runs(root, first_fs.route, &first_grant)
        .map_err(RunFailure::Message)?;
    let stalled = stage_stalled_delegate(root, first_fs.route, &first_grant)?;
    drop(first_grant);
    let first_report =
        release_fs_provider(root, first_fs.bootstrap, first_fs.release, first_fs.route)?;
    finish_stalled_delegate(root, stalled)?;
    if first_report.committed == 0
        || second_report.committed == 0
        || first_report.abandoned != 1
        || second_report.abandoned != 1
        || first_report.downstream_abandoned != (CANCELLATION_PROBES + 1) as u64
        || second_report.downstream_abandoned != 0
    {
        return Err(RunFailure::Message("FAL2 provider shutdown report invalid"));
    }
    debug!(
        "independent FAL2 provider Delegate passed: roots={:#x}/{:#x}",
        first_fs
            .sender_identity
            .expect("primary sender identity missing"),
        second_fs
            .sender_identity
            .expect("discovered sender identity missing")
    );
    root.collect_process(first_fs.pid)?;
    root.collect_process(second_fs.pid)?;
    debug!("FAL2 provider supervision reclaimed both provider processes");
    let target_image = launched.target_image;
    let hammer_image = launched.hammer_image;

    // 运行时拓扑快照（调试参考）：此刻服务在域内运行，验收自测尚未
    // 展开；Drv之类短寿命服务可能已 REAPABLE 待收。
    debug!("topology: runtime snapshot after service launch");
    dump_topology(root_job, &names, 0);
    let pair = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("mailbox create failed");

    let event = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("notification create failed");
    let moves = [HandleMove {
        handle: event.peer,
        rights: Rights::SIGNAL,
    }];

    // —— 同步消息 + Handle move + Notification/WaitMany 快路径 ——
    match unsafe { send_raw(pair.peer, 114, &[5u8, 1u8, 4u8], &moves) } {
        Ok(()) => match receive(pair.owner) {
            Ok(message) => {
                debug!(
                    "message: kind={}, payload={:?}",
                    message.header.kind, message.payload
                );
                let moved = message
                    .handles
                    .get(0)
                    .expect("notification slot missing")
                    .as_handle();
                notification::signal(moved, 0x5).expect("notification signal failed");
                let result =
                    wait_many(&[WaitItem::new(event.owner, ObjectSignals::READABLE, 7)], 0)
                        .expect("notification wait failed");
                let bits =
                    notification::take(event.owner, u64::MAX).expect("notification take failed");
                debug!("notification: cookie={}, bits={:#x}", result.cookie, bits);
            }
            Err(e) => debug!("receive failed: {:?}", e),
        },
        Err(e) => debug!("send failed: {:?}", e),
    }
    let _ = unsafe { close(event.owner) };
    let _ = unsafe { close(pair.peer) };
    let _ = unsafe { close(pair.owner) };

    test_tunnel_geometry();
    test_capability_badges_and_affine_owners();
    #[cfg(feature = "acceptance-stress")]
    stress_control_plane();
    #[cfg(feature = "acceptance-stress")]
    test_tunnel_lifecycle();
    test_send_once();
    test_wait_set_retirement();
    test_rpc_reject_cleanup();
    test_writable_level();
    public_ipc::threaded_receive();
    time_checks::run();
    public_ipc::committed_kill(acceptance, target_image.expect("IPC target image missing"));

    // —— 数据面：建隧道 → Invitation 经消息面转移 → 阻塞读流 ——
    let (tunnel, invitation) =
        match blocking::Consumer::create(TUNNEL_BYTES, rinlib::mm::Placement::Anywhere) {
            Ok((consumer, invitation)) => (consumer, invitation),
            Err(blocking::CreateFailure::Tunnel(
                rinlib::ipc::invitation::CreateFailure::System(error),
            )) => {
                debug!("tunnel create failed: {:?}", error);
                return Err(RunFailure::Message("tunnel create failed"));
            }
            Err(blocking::CreateFailure::Tunnel(
                rinlib::ipc::invitation::CreateFailure::Published {
                    endpoint,
                    cleanup,
                    invitation,
                    error,
                },
            )) => {
                root.retain_failed_create(endpoint, cleanup, invitation);
                debug!("tunnel create output invalid: {:?}", error);
                return Err(RunFailure::Message("tunnel create output invalid"));
            }
            Err(blocking::CreateFailure::Protocol {
                endpoint,
                invitation,
                error,
            }) => {
                root.retain_transport(endpoint, invitation);
                debug!("tunnel protocol init failed: {:?}", error);
                return Err(RunFailure::Message("tunnel init failed"));
            }
        };
    root.start_read(tunnel, alloc::vec::Vec::new());
    debug!("tunnel created");
    // 生产侧以 typed Packet 转移 Invitation：失败完整返还 owner，
    // 不再以裸 HandleMove 表达（原失败路径承载直接丢失）。
    let mut packet =
        Packet::new(514, &[]).map_err(|_| "tunnel invitation packet allocation failed")?;
    if let Err(failure) = packet.push(invitation.into_capability(), Rights::MAP) {
        root.retain_capability(failure.capability);
        return Err(RunFailure::Message("tunnel invitation push failed"));
    }
    if let Err(mut failure) =
        packet.try_send(root.sender(pm_mailbox), rinlib::time::Deadline::INFINITE)
    {
        let (invitation, _) = failure
            .packet
            .pop()
            .expect("tunnel invitation transfer disappeared");
        root.retain_capability(invitation);
        debug!("send tunnel invitation failed: {:?}", failure.error);
        return Err(RunFailure::Message("send tunnel invitation failed"));
    }

    root.await_read()?;
    let read = root.read_world();
    let n = read.filled;
    if n != STREAM_LEN
        || !read.buffer[..n]
            .iter()
            .enumerate()
            .all(|(i, &byte)| byte == (i % 251 + 1) as u8)
    {
        return Err(RunFailure::Message("stream pattern or length mismatch"));
    }
    debug!("stream received {} bytes, pattern ok", n);
    debug!(
        "RNL2 multi-page stream passed: bytes={}, capacity={}",
        n,
        read.tunnel
            .as_ref()
            .expect("read duty owns its tunnel")
            .capacity()
    );

    // —— 流控唤醒面：pm 填满目标邮箱后在 WRITABLE 上阻塞，腾位唤醒 ——
    test_writable_wake(root, pm_mailbox)?;
    root.close_control(pm_mailbox)?;

    // —— live kill 正路径：Building 目标的确定性 kill/drain/终态验证 ——
    test_building_kill(acceptance);

    // —— Job 管理面验收（step 5）：封口与完成传播、派生兑底、递归
    // JobKill 组合与 seal 闸门可行子集 ——
    match target_image {
        Some(image) => test_job_management(root, acceptance, image)?,
        None => {
            return Err(RunFailure::Message(
                "job management acceptance image unavailable",
            ));
        }
    }

    // 短寿命服务的线程已退出并不释放 AddressSpace；在高峰矩阵前收束已进入
    // Terminating/Dead 的成员，仍 Running 的服务继续由末尾监督闭环持有。
    root.verify_failure_isolation(acceptance)?;
    let reclaimed = root.collect_terminated()?;
    debug!(
        "pre-race service supervision reclaimed {} process(es)",
        reclaimed
    );

    // 完整生命周期多核竞态矩阵属于 stress workload；core 仍覆盖确定性的
    // Building kill、Job 管理与服务监督收束。
    #[cfg(feature = "acceptance-stress")]
    match (target_image, hammer_image) {
        (Some(target), Some(hammer)) => race_matrix(root, acceptance, target, hammer)?,
        _ => {
            return Err(RunFailure::Message(
                "race matrix acceptance images unavailable",
            ));
        }
    }
    #[cfg(not(feature = "acceptance-stress"))]
    let _ = hammer_image;

    // acceptance 域用完即收：seal + 空即完成，不把一次性验收遗留带进
    // 稳态拓扑。
    root.collect_job(acceptance, 0x1E)?;
    debug!("acceptance domain collected");
    root.close_control(acceptance)?;

    // —— 监督闭环：等待全部服务 REAPABLE/CLOSED，Drain 至 Complete，
    // 查询稳定终态后释放 control。对象 close 回调（含 pm 隧道端点的
    // PEER_CLOSED 发布）发生在 Drain 期间——监督先于对端终态等待。 ——
    root.collect_services()?;
    debug!("all services supervised to completion");

    // —— 事件面：对端终态位（Drain 已置位，电平等待立即返回）——
    let peer_closed = wait_supervision_signal(
        root.read_world()
            .tunnel
            .as_mut()
            .expect("root owns the tunnel"),
        "peer-closed",
    )?;
    debug!(
        "peer closed observed: bits={:#x}",
        peer_closed.observed.raw()
    );
    #[cfg(not(feature = "acceptance-stress"))]
    let tunnel_pool_before_close =
        root_pool_allocated().map_err(|_| "tunnel Pool pre-close query failed")?;
    root.close_read()?;

    #[cfg(not(feature = "acceptance-stress"))]
    let tunnel_pool_after_close =
        root_pool_allocated().map_err(|_| "tunnel Pool post-close query failed")?;
    #[cfg(not(feature = "acceptance-stress"))]
    {
        let refunded = tunnel_pool_before_close.saturating_sub(tunnel_pool_after_close);
        if refunded == 0 {
            debug!(
                "tunnel Pool charge did not decrease after close: before={}, after={}",
                tunnel_pool_before_close, tunnel_pool_after_close
            );
            return Err(RunFailure::Message(
                "tunnel Pool charge did not refund after close",
            ));
        }
        debug!(
            "tunnel Pool conservation after close passed: before={}, after={}, refunded={}",
            tunnel_pool_before_close, tunnel_pool_after_close, refunded
        );
    }

    // —— 委托域终局：pm 的管理段应已把 pm_domain 收束到 Dead；降级时
    // init 以保留的直接收束权兜底（job_kill = seal + 枚举派生 kill +
    // drain + CLOSED 屏障）。 ——
    let domain_state = process::query_job(pm_domain);
    if !matches!(&domain_state, Ok(s) if s.state == JobState::Dead as u32) {
        debug!("pm delegated domain not collected by pm; init collecting");
        root.collect_job(pm_domain, 0x1F)?;
    }
    let domain_state = process::query_job(pm_domain).map_err(|error| {
        debug!("pm delegated domain final query failed: {:?}", error);
        "pm delegated domain final query failed"
    })?;
    if domain_state.state != JobState::Dead as u32 {
        return Err(RunFailure::Message(
            "pm delegated domain did not reach Dead",
        ));
    }
    debug!("pm delegated domain confirmed Dead");

    // 终态拓扑快照（调试参考）：预期 root 仅剩 init + services，services
    // 空（Dead 的 pm_domain/acceptance 已从成员表移除）——收束干净的不变量。
    let stack_cleanup = rinlib::thread::stack_cleanup_snapshot();
    if stack_cleanup.abandoned != 0 {
        debug!(
            "thread stack cleanup recorded {} abandoned mapping(s), last error {:?}",
            stack_cleanup.abandoned, stack_cleanup.last_error
        );
        return Err(RunFailure::Message(
            "thread stack cleanup abandoned mappings",
        ));
    }
    debug!("thread stack cleanup passed");
    debug!("topology: final snapshot before system reset");
    dump_topology(root_job, &names, 0);
    root.close_control(pm_domain)?;
    root.verify_idle_ownership()?;
    Ok(())
}

/// 递归打印 Job/Process 拓扑（调试参考）：直接成员经派生 control 查询
/// 生命周期快照后即关，child Job 派生 JobControl 下钻——纯观察，不消费
/// authority、不改状态。青色 topology 标记便于在灰度用户态日志中定位
/// （rinlib debug! 元许在消息内自拼 ANSI）。
fn dump_topology(job: Handle, names: &TopologyNames, depth: usize) {
    let indent = "  ".repeat(depth);
    let job_snapshot = process::query_job(job).ok();
    let members = enumerate_members(job, JobMemberKind::MemberProcesses).unwrap_or_default();
    let children = enumerate_members(job, JobMemberKind::ChildJobs).unwrap_or_default();
    match &job_snapshot {
        Some(snapshot) => debug!(
            "\x1b[36mtopology\x1b[0m: {indent}job {} (jid {}, {}, members {}, children {})",
            names.job_name(snapshot.jid),
            snapshot.jid,
            job_state_name(snapshot.state),
            members.len(),
            children.len()
        ),
        None => debug!(
            "\x1b[36mtopology\x1b[0m: {indent}job query failed (members {}, children {})",
            members.len(),
            children.len()
        ),
    }
    for pid in members {
        let process_snapshot = process::derive_job(
            job,
            JobMemberKind::MemberProcesses,
            pid,
            DERIVED_CONTROL_RIGHTS,
        )
        .ok()
        .and_then(|control| {
            let snapshot = process::query(control).ok();
            let _ = unsafe { close(control) };
            snapshot
        });
        match process_snapshot {
            Some(snapshot) => debug!(
                "\x1b[36mtopology\x1b[0m: {indent}  process {} (pid {}, {}, reason {}, code {})",
                names.process_name(pid),
                pid,
                process_state_name(snapshot.state),
                exit_reason_name(snapshot.reason),
                snapshot.code
            ),
            None => debug!(
                "\x1b[36mtopology\x1b[0m: {indent}  process pid {} (query failed)",
                pid
            ),
        }
    }
    for jid in children {
        match process::derive_job(job, JobMemberKind::ChildJobs, jid, DELEGATED_DOMAIN_RIGHTS) {
            Ok(child) => {
                dump_topology(child, names, depth + 1);
                let _ = unsafe { close(child) };
            }
            Err(error) => debug!(
                "\x1b[36mtopology\x1b[0m: {indent}  child jid {} derive failed: {:?}",
                jid, error
            ),
        }
    }
}

/// 只收束已经离开 Building/Running 的服务，避免已 reaped 线程的地址空间在
/// 后续高峰负载中继续占帧；状态仍活跃的成员原样留给最终监督闭环。
const KIND_STREAM_READ: SourceKind = 1;

/// 数据面段运行体的世界：流角色与缓冲在段内共享，段末交回脚本。
struct ReadWorld {
    tunnel: Option<blocking::Consumer>,
    buffer: alloc::vec::Vec<u8>,
    filled: usize,
    failed: bool,
}

/// 消费侧读取任务：arm/poll 三条件接入，按预算推进，EOF 即完成。
struct StreamReadTask {
    source: Option<SourceId>,
    arm_failed: Option<SystemCallError>,
    stopping: bool,
    removing: bool,
}

enum InitTask {
    StreamRead(StreamReadTask),
}

impl Task<ReadWorld> for StreamReadTask {
    type Family = InitTask;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut ReadWorld,
        requests: &mut Requests<InitTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if self.stopping {
            if let Some(source) = self.source {
                if !self.removing {
                    requests.remove(source)?;
                    self.removing = true;
                }
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            if let Some(tunnel) = world.tunnel.take() {
                match tunnel.close() {
                    Ok(()) => {}
                    Err((tunnel, error)) => {
                        world.tunnel = Some(tunnel);
                        debug!("stream read stop close failed: {:?}", error);
                        return Err(SystemCallError::InternalError);
                    }
                }
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        let tunnel = world
            .tunnel
            .as_mut()
            .expect("stream read world lost its tunnel");
        if let Some(error) = self.arm_failed.take() {
            debug!("stream read source registration failed: {:?}", error);
            world.failed = true;
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        let mut source = None;
        let mut replace_source = false;
        while let Some(event) = input.pull() {
            if event.kind != KIND_STREAM_READ {
                continue;
            }
            source = Some(event.source);
            if event.error != 0 {
                debug!("stream read source failed: error={}", event.error);
                world.failed = true;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            match tunnel.poll(event.observed) {
                Ok(Some(ConsumerReady::PeerAttached))
                | Ok(Some(ConsumerReady::Readable {
                    peer_attached: true,
                    ..
                })) => replace_source = true,
                Ok(Some(ConsumerReady::EofDrained)) => {
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
                Ok(_) => {}
                Err(error) => {
                    debug!("stream read poll failed: {:?}", error);
                    world.failed = true;
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
            }
        }
        let mut work = 0;
        while world.filled < world.buffer.len() && world.filled < STREAM_LEN && work < budget {
            match tunnel.read(&mut world.buffer[world.filled..]) {
                Ok(0) => break,
                Ok(count) => {
                    world.filled += count;
                    work += 1;
                }
                Err(error) => {
                    debug!("stream read failed: {:?}", error);
                    world.failed = true;
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
            }
        }
        if world.failed {
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        if world.filled >= STREAM_LEN {
            // 已读满目标量：EOF 判定后交付，否则等终末事件。
            match tunnel.eof_reached() {
                Ok(true) => {
                    return Ok(Advance {
                        work_done: work.max(1),
                        step: Step::Complete,
                    });
                }
                Ok(false) => {}
                Err(error) => {
                    debug!("stream eof check failed: {:?}", error);
                    world.failed = true;
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
            }
        }
        if replace_source {
            let source = source.expect("PeerAttached event lost its source");
            requests.remove(source)?;
            self.source = None;
        } else if let Some(source) = source {
            self.source = Some(source);
            requests.rearm(source)?;
        }
        if self.source.is_none() {
            let plan = match tunnel.wait_plan() {
                Ok(plan) => plan,
                Err(error) => {
                    debug!("stream wait plan failed: {:?}", error);
                    world.failed = true;
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    });
                }
            };
            requests
                .arm_source(plan, KIND_STREAM_READ)
                .map_err(|_| SystemCallError::ReachLimit)?;
        }
        Ok(Advance {
            work_done: work.max(1),
            step: Step::Parked,
        })
    }

    fn registered(&mut self, _world: &mut ReadWorld, kind: SourceKind, source: SourceId) {
        if kind == KIND_STREAM_READ {
            self.source = Some(source);
        }
    }
    fn unregistered(&mut self, _world: &mut ReadWorld, _kind: SourceKind, source: SourceId) {
        if self.source == Some(source) {
            self.source = None;
            self.removing = false;
        }
    }

    fn refused(&mut self, _world: &mut ReadWorld, failure: RequestFailure<InitTask>) {
        let error = match failure {
            RequestFailure::Spawn { error, .. }
            | RequestFailure::Source { error, .. }
            | RequestFailure::Wake { error, .. } => error,
        };
        self.arm_failed = Some(error);
    }

    fn stop(&mut self, _world: &mut ReadWorld) {
        self.stopping = true;
    }
}

impl Task<ReadWorld> for InitTask {
    type Family = InitTask;

    fn advance(
        &mut self,
        id: u64,
        world: &mut ReadWorld,
        requests: &mut Requests<InitTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        match self {
            Self::StreamRead(task) => task.advance(id, world, requests, input, budget),
        }
    }

    fn registered(&mut self, world: &mut ReadWorld, kind: SourceKind, source: SourceId) {
        match self {
            Self::StreamRead(task) => task.registered(world, kind, source),
        }
    }
    fn unregistered(&mut self, world: &mut ReadWorld, kind: SourceKind, source: SourceId) {
        match self {
            Self::StreamRead(task) => task.unregistered(world, kind, source),
        }
    }

    fn refused(&mut self, world: &mut ReadWorld, failure: RequestFailure<InitTask>) {
        match self {
            Self::StreamRead(task) => task.refused(world, failure),
        }
    }

    fn stop(&mut self, world: &mut ReadWorld) {
        match self {
            Self::StreamRead(task) => task.stop(world),
        }
    }

    fn deadline(&self) -> rinlib::shared::time::Deadline {
        rinlib::shared::time::Deadline::INFINITE
    }
}

fn wait_supervision_signal(
    tunnel: &mut blocking::Consumer,
    label: &'static str,
) -> Result<rinlib::shared::wait::WaitResult, &'static str> {
    for attempt in 1..=DEFAULT_SUPERVISION_POLICY.wait_attempts {
        match tunnel.wait_peer_closed(DEFAULT_SUPERVISION_POLICY.wait_timeout_ms) {
            Ok(result) if result.observed != ObjectSignals::NONE => return Ok(result),
            Ok(result) if WaitReason::from_u32(result.reason) == Some(WaitReason::Timeout) => {}
            Ok(result) => {
                debug!(
                    "{} supervision wait returned no signal at attempt {}: {:?}",
                    label, attempt, result
                );
                return Err("supervision wait returned no signal");
            }
            Err(error) => {
                debug!(
                    "{} supervision wait failed at attempt {}: {:?}",
                    label, attempt, error
                );
                return Err("supervision wait failed");
            }
        }
    }
    debug!(
        "{} supervision wait exhausted {} attempts",
        label, DEFAULT_SUPERVISION_POLICY.wait_attempts
    );
    Err("supervision wait timed out")
}

struct SupervisorWorld {
    results: alloc::vec::Vec<SuperviseResult>,
    failures: usize,
}

impl SuperviseSink for SupervisorWorld {
    fn supervised(&mut self, result: SuperviseResult) {
        self.failures += usize::from(result.outcome.is_err());
        self.results.push(result);
    }
}

/// Building 目标的确定性 kill：Create 后未 Start，kill 冻结终因
/// (Killed, code)，builder 关闭的 abandonment 竞争不覆盖；Drain 至
/// Complete 后 shell 快照应稳定报 Dead/Killed/code。
fn test_building_kill(job: Handle) {
    let created = match process::create(job, SUPERVISOR_RIGHTS) {
        Ok(created) => created,
        Err(error) => {
            debug!("building kill: create failed: {:?}", error);
            return;
        }
    };
    let before = process::query(created.control);
    if let Ok(snapshot) = before {
        debug!("building kill: initial state={}", snapshot.state);
    }
    if let Err(error) = process::kill(created.control, 0x123) {
        debug!("building kill: kill failed: {:?}", error);
        let _ = unsafe { close(created.builder) };
        let _ = unsafe { close(created.control) };
        return;
    }
    let _ = unsafe { close(created.builder) };
    let drained = process::drain_to_completion(created.control);
    let snapshot = process::query(created.control);
    match (drained, snapshot) {
        (Ok(_), Ok(snapshot))
            if snapshot.state == ProcessState::Dead as u32
                && snapshot.reason == ProcessExitReason::Killed as u32
                && snapshot.code == 0x123 =>
        {
            debug!(
                "building kill passed: pid {} Dead/Killed/{:#x}",
                created.pid, snapshot.code
            );
        }
        (work, snapshot) => {
            debug!(
                "building kill FAILED: drain={:?} snapshot={:?}",
                work, snapshot
            );
        }
    }
    let _ = unsafe { close(created.control) };
}

/// 验收线 1：枚举→派生→kill 通路。test_target 的 pid 经 acceptance Job
/// 枚举可见，JobDerive 派生 MANAGE control 后 kill 并监督收束；原保留
/// control 在派生接管后关闭（关闭 control 永不隐式终止）。
fn test_derive_kill(
    root: &mut RootSupervisor,
    job: Handle,
    pid: u64,
    retained: Handle,
) -> Result<(), &'static str> {
    let derived = (|| {
        let members = enumerate_members(job, JobMemberKind::MemberProcesses)?;
        if !members.contains(&pid) {
            debug!(
                "derive kill: pid {} missing among {} members",
                pid,
                members.len()
            );
            return Err(SystemCallError::ObjectNotFound);
        }
        debug!(
            "derive kill: pid {} visible among {} members",
            pid,
            members.len()
        );
        process::derive_job(
            job,
            JobMemberKind::MemberProcesses,
            pid,
            DERIVED_CONTROL_RIGHTS,
        )
    })();
    match derived {
        Ok(control) => {
            let _ = unsafe { close(retained) };
            process::kill(control, 0x77).expect("derived-control kill must be accepted");
            let mut exhausted = DEFAULT_SUPERVISION_POLICY;
            exhausted.drain_work = 1;
            exhausted.drain_attempts = 1;
            let failure = collect_process(SupervisionTarget::new(pid, control), exhausted)
                .expect_err("one work unit must not complete process drain");
            assert_eq!(failure.stage, SupervisionStage::Drain);
            assert_eq!(failure.cause, SupervisionCause::Timeout);
            assert_eq!(failure.progress.drain_attempts, 1);
            debug!(
                "supervision budget exhaustion retained authority: pid={}, work={}",
                pid, failure.progress.work_done
            );
            let mut machine = failure.collector;
            machine.replenish(DEFAULT_SUPERVISION_POLICY);
            let collected = root.collect_machine(machine)?;
            assert_eq!(collected.snapshot.reason, ProcessExitReason::Killed as u32);
            assert_eq!(collected.snapshot.code, 0x77);
            debug!(
                "pid {} supervised: work={}, state={}, reason={}, code={}",
                pid,
                collected.progress.work_done,
                collected.snapshot.state,
                collected.snapshot.reason,
                collected.snapshot.code
            );
        }
        Err(error) => {
            debug!("derive kill FAILED: enumerate/derive {:?}", error);
            process::kill(retained, 0x77).expect("live kill of a fresh process must be accepted");
            kill_and_supervise(
                root,
                job,
                alloc::vec::Vec::from([Supervised {
                    pid,
                    control: retained,
                }]),
            )?;
            return Err("derive kill enumeration or control derivation failed");
        }
    }
    Ok(())
}

/// Job 管理面验收入口（step 5）。真跨核竞态矩阵（含并发 Create/枚举
/// 乱序窗口）归 step 9 验证矩阵，此处只覆盖可确定性制造的场景。
fn test_job_management(
    root: &mut RootSupervisor,
    job: Handle,
    image: &[u8],
) -> Result<(), &'static str> {
    test_job_seal_completion(job);
    test_derive_fallback(job);
    test_job_kill_composition(root, job, image)?;
    seal_before_start(job);
    seal_before_create(job);
    enumerate_convergence(job);
    #[cfg(feature = "acceptance-stress")]
    {
        test_drain_minimum_budget(job, image)
    }
    #[cfg(not(feature = "acceptance-stress"))]
    {
        Ok(())
    }
}

/// 含 child Handle 的 REAPABLE 进程以 `max_work=1` 收束。More 必须由
/// rinlib 拒绝零进展；这里逐批推进至 Complete，覆盖 pending close 的
/// 扫描/关闭分离边界。
#[cfg(feature = "acceptance-stress")]
fn test_drain_minimum_budget(job: Handle, image: &[u8]) -> Result<(), &'static str> {
    let marker = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        Rights::SIGNAL | Rights::TRANSIT,
    )
    .map_err(|error| {
        debug!(
            "drain minimum-budget acceptance failed: notification create {:?}",
            error
        );
        "drain minimum-budget notification create failed"
    })?;
    let ready = notification::create(Rights::READ | Rights::WAIT, Rights::SIGNAL | Rights::GRANT)
        .map_err(|_| {
        let _ = unsafe { close(marker.owner) };
        let _ = unsafe { close(marker.peer) };
        "drain minimum-budget ready creation failed"
    })?;
    let grants = [
        HandleGrant {
            handle: marker.owner,
            rights: Rights::READ | Rights::WAIT,
        },
        HandleGrant {
            handle: ready.peer,
            rights: Rights::SIGNAL,
        },
    ];
    let result = (|| {
        let started = spawn(SpawnRequest {
            memory_pool: root_memory_pool(),
            job,
            image,
            payload: b"retirement",
            grants: &grants,
            control_rights: SUPERVISOR_RIGHTS,
        })
        .map_err(|error| {
            debug!("drain minimum-budget acceptance failed: spawn {:?}", error);
            let _ = unsafe { close(marker.owner) };
            let _ = unsafe { close(ready.peer) };
            "drain minimum-budget spawn failed"
        })?;
        wait_many(&[WaitItem::new(ready.owner, ObjectSignals::READABLE, 0)], 0).map_err(|_| {
            let _ = process::kill(started.control, 0x5D);
            let _ = unsafe { close(started.control) };
            "drain minimum-budget target readiness failed"
        })?;
        if let Err(error) = process::kill(started.control, 0x5D) {
            debug!("drain minimum-budget acceptance failed: kill {:?}", error);
            let _ = unsafe { close(started.control) };
            return Err("drain minimum-budget kill failed");
        }
        if let Err(error) = wait_many(
            &[WaitItem::new(
                started.control,
                ObjectSignals::REAPABLE | ObjectSignals::CLOSED,
                0,
            )],
            0,
        ) {
            debug!("drain minimum-budget acceptance failed: wait {:?}", error);
            let _ = unsafe { close(started.control) };
            return Err("drain minimum-budget wait failed");
        }
        let mut batches = 0usize;
        let drained = loop {
            batches += 1;
            if batches > 16_384 {
                break Err(SystemCallError::InternalError);
            }
            match process::drain(started.control, 1) {
                Ok(result) if result.work_done > 1 => break Err(SystemCallError::InternalError),
                Ok(result) if result.status == ProcessDrainStatus::Complete as u32 => break Ok(()),
                Ok(result) if result.work_done == 1 => continue,
                Ok(_) => break Err(SystemCallError::InternalError),
                Err(error) => break Err(error),
            }
        };
        debug!(
            "drain minimum-budget acceptance {}: {} batches, retired WaitSet with 256 registrations",
            if drained.is_ok() { "passed" } else { "failed" },
            batches
        );
        let _ = unsafe { close(started.control) };
        drained.map_err(|_| "drain minimum-budget did not complete")
    })();
    let _ = unsafe { close(ready.owner) };
    let _ = unsafe { close(marker.peer) };
    result
}

/// 验收线 2：封口与完成传播——空 child Job seal 后 CLOSED 电平可等待、
/// 快照转 Dead；重复 seal 幂管；完成后从 root 子表移除（枚举收敛）。
fn test_job_seal_completion(job: Handle) {
    let Ok(child) = process::create_job(job, JOB_FULL_RIGHTS) else {
        debug!("job seal completion FAILED: job create failed");
        return;
    };
    let sealed = process::seal_job(child);
    let waited = wait_many(&[WaitItem::new(child, ObjectSignals::CLOSED, 0)], 0);
    let snapshot = process::query_job(child);
    // 幂管：Dead 上重复 seal 成功且不改变状态。
    let resealed = process::seal_job(child);
    let resnapshot = process::query_job(child);
    let children = enumerate_members(job, JobMemberKind::ChildJobs);
    let passed = sealed.is_ok()
        && waited.is_ok()
        && matches!(&snapshot, Ok(s) if s.state == JobState::Dead as u32)
        && resealed.is_ok()
        && matches!(&resnapshot, Ok(s) if s.state == JobState::Dead as u32)
        && matches!(&children, Ok(list) if !list.contains(&snapshot.as_ref().unwrap().jid));
    match (&snapshot, &children) {
        (Ok(snapshot), Ok(children)) => debug!(
            "job seal completion {} (jid {}, state {}, root children left {})",
            if passed { "passed" } else { "FAILED" },
            snapshot.jid,
            snapshot.state,
            children.len()
        ),
        (snapshot, children) => debug!(
            "job seal completion FAILED: snapshot={:?} children={:?}",
            snapshot, children
        ),
    }
    let _ = unsafe { close(child) };
}

/// 验收线 3：派生兑底——control 全消散的 REAPABLE 进程经枚举+派生接管，
/// drain 至 Complete。铸造的新 shell 必须重放 REAPABLE，否则 drain
/// 入口直接拒绝——本场景即验证该重放。
fn test_derive_fallback(job: Handle) {
    let Ok(created) = process::create(job, SUPERVISOR_RIGHTS) else {
        debug!("derive fallback FAILED: create failed");
        return;
    };
    let pid = created.pid;
    if let Err(error) = process::kill(created.control, 0x1D) {
        debug!("derive fallback FAILED: kill {:?}", error);
        let _ = unsafe { close(created.builder) };
        let _ = unsafe { close(created.control) };
        return;
    }
    // 终因已冻结为 Killed；builder 关闭的 abandonment 竞争不覆盖。
    let _ = unsafe { close(created.builder) };
    // control 消散：无人收束，只能靠枚举+派生接管。
    let _ = unsafe { close(created.control) };
    let members = enumerate_members(job, JobMemberKind::MemberProcesses);
    let visible = matches!(&members, Ok(list) if list.contains(&pid));
    let minted = process::derive_job(
        job,
        JobMemberKind::MemberProcesses,
        pid,
        DERIVED_CONTROL_RIGHTS,
    );
    match minted {
        Ok(control) => {
            let drained = process::drain_to_completion(control);
            let snapshot = process::query(control);
            match (drained, snapshot) {
                (Ok(_), Ok(snapshot))
                    if snapshot.state == ProcessState::Dead as u32
                        && snapshot.reason == ProcessExitReason::Killed as u32
                        && snapshot.code == 0x1D =>
                {
                    debug!(
                        "derive fallback passed: pid {} minted control drained to Dead/Killed/{:#x}",
                        pid, snapshot.code
                    );
                }
                (drained, snapshot) => debug!(
                    "derive fallback FAILED: visible={} drain={:?} snapshot={:?}",
                    visible, drained, snapshot
                ),
            }
            let _ = unsafe { close(control) };
        }
        Err(error) => {
            debug!(
                "derive fallback FAILED: visible={} derive={:?}",
                visible, error
            );
        }
    }
}

/// 递归 JobKill 组合：child Job 内一个 Running 成员（Waiting 取消路径）
/// 以及一个 Building 成员，两者 control 均消散——libprocess::job_kill 一把
/// 收束（seal → 枚举 → 派生 kill → drain → 等 CLOSED 全链，派生走铸造
/// 路径）。
fn test_job_kill_composition(
    root: &mut RootSupervisor,
    job: Handle,
    image: &[u8],
) -> Result<(), &'static str> {
    let Ok(child) = root.create_job(job, JOB_FULL_RIGHTS) else {
        debug!("job kill composition FAILED: job create failed");
        return Err("job kill composition job create failed");
    };
    let running = spawn(SpawnRequest {
        memory_pool: root_memory_pool(),
        job: child,
        image,
        payload: &[],
        grants: &[],
        control_rights: SUPERVISOR_RIGHTS,
    });
    let building = process::create(child, SUPERVISOR_RIGHTS);
    match (running, building) {
        (Ok(running), Ok(building)) => {
            let running_pid = running.pid;
            let _ = unsafe { close(running.control) };
            let _ = unsafe { close(building.control) };
            match root.collect_job(child, 0x3C) {
                Ok(()) => {
                    let snapshot = process::query_job(child);
                    match snapshot {
                        Ok(snapshot) if snapshot.state == JobState::Dead as u32 => debug!(
                            "job kill composition passed (member pid {}, child jid {} Dead)",
                            running_pid, snapshot.jid
                        ),
                        snapshot => {
                            debug!("job kill composition FAILED: child snapshot={:?}", snapshot);
                            return Err("job kill composition child snapshot failed");
                        }
                    }
                }
                Err(error) => {
                    debug!(
                        "job kill composition FAILED: stage={:?}, cause={:?}, retained_collector={}",
                        "root-owned", error, true
                    );
                    return Err("job kill composition collection failed");
                }
            }
        }
        (running, building) => {
            debug!(
                "job kill composition FAILED: running={:?} building={:?}",
                running.err(),
                building.err()
            );
            return Err("job kill composition member creation failed");
        }
    }
    root.close_control(child)?;
    Ok(())
}

/// seal 先于 Start 的提交闸门：Building 成员在 seal 后 Start 返回
/// ObjectClosed（链锁内上行检查，两种线性化顺序的另一侧）；随后
/// kill/drain 收束，Job 因 sealed+空完成并发布 CLOSED。
fn seal_before_start(job: Handle) {
    let Ok(child) = process::create_job(job, JOB_FULL_RIGHTS) else {
        debug!("seal gate (start) FAILED: job create failed");
        return;
    };
    // 手工构建可启动的 Building：入口页（自旋）+ 栈顶页（失败自清理）。
    let Ok(created) = build_spin_building(child) else {
        debug!("seal gate (start) FAILED: building");
        let _ = unsafe { close(child) };
        return;
    };
    let sealed = process::seal_job(child);
    let started = process::start(created.builder, ExecutionProfile::Base64 as u32);
    let gated = matches!(started, Err(SystemCallError::ObjectClosed));
    // 收束：kill → drain → sealed+空完成 → CLOSED。
    let _ = process::kill(created.control, 0x3D);
    let _ = process::drain_to_completion(created.control);
    let _ = unsafe { close(created.control) };
    let _ = unsafe { close(created.builder) };
    let waited = wait_many(&[WaitItem::new(child, ObjectSignals::CLOSED, 0)], 0);
    let snapshot = process::query_job(child);
    let passed = sealed.is_ok()
        && gated
        && waited.is_ok()
        && matches!(&snapshot, Ok(s) if s.state == JobState::Dead as u32);
    debug!(
        "seal gate (start) {} (gated={}, state {:?})",
        if passed { "passed" } else { "FAILED" },
        gated,
        snapshot.as_ref().map(|s| s.state)
    );
    let _ = unsafe { close(child) };
}

/// seal 先于 Create：封口后成员/子 Job 创建口永久关闭（ObjectClosed）；
/// 空封口立即完成并发布 CLOSED。
fn seal_before_create(job: Handle) {
    let Ok(child) = process::create_job(job, JOB_FULL_RIGHTS) else {
        debug!("seal gate (create) FAILED: job create failed");
        return;
    };
    let sealed = process::seal_job(child);
    let waited = wait_many(&[WaitItem::new(child, ObjectSignals::CLOSED, 0)], 0);
    let member = process::create(child, SUPERVISOR_RIGHTS);
    let subjob = process::create_job(child, JOB_FULL_RIGHTS);
    let passed = sealed.is_ok()
        && waited.is_ok()
        && matches!(member, Err(SystemCallError::ObjectClosed))
        && matches!(subjob, Err(SystemCallError::ObjectClosed));
    debug!(
        "seal gate (create) {} (member={:?} subjob={:?})",
        if passed { "passed" } else { "FAILED" },
        member.err(),
        subjob.err()
    );
    if let Ok(handle) = subjob {
        let _ = unsafe { close(handle) };
    }
    let _ = unsafe { close(child) };
}

/// 枚举收敛（可制造子集）：创建/收束交错下，新 Pid 单调递增、Dead
/// 后从成员表消失；连续两轮全量枚举无残留。真跨核并发窗口归 step 9。
fn enumerate_convergence(job: Handle) {
    let mut ok = true;
    let mut previous = 0u64;
    for round in 0..4u64 {
        let Ok(created) = process::create(job, SUPERVISOR_RIGHTS) else {
            ok = false;
            debug!(
                "enumerate convergence FAILED: create failed at round {}",
                round
            );
            break;
        };
        if created.pid <= previous {
            ok = false;
            debug!(
                "enumerate convergence FAILED: pid {} not monotonic after {}",
                created.pid, previous
            );
        }
        previous = created.pid;
        if let Err(error) = process::kill(created.control, 0x3E + round as i64) {
            ok = false;
            debug!("enumerate convergence FAILED: kill {:?}", error);
            let _ = unsafe { close(created.builder) };
            let _ = unsafe { close(created.control) };
            continue;
        }
        let _ = unsafe { close(created.builder) };
        if let Err(error) = process::drain_to_completion(created.control) {
            ok = false;
            debug!("enumerate convergence FAILED: drain {:?}", error);
        }
        let _ = unsafe { close(created.control) };
        match enumerate_members(job, JobMemberKind::MemberProcesses) {
            Ok(members) => {
                if members.contains(&created.pid) {
                    ok = false;
                    debug!(
                        "enumerate convergence FAILED: dead pid {} still enumerable",
                        created.pid
                    );
                }
            }
            Err(error) => {
                ok = false;
                debug!("enumerate convergence FAILED: enumerate {:?}", error);
            }
        }
    }
    debug!(
        "enumerate convergence {}",
        if ok { "passed" } else { "FAILED" }
    );
}

/// 与 pm 约定的流控验证消息号：请求携带 [目标邮箱 sender、确认 signaler、
/// 虚假唤醒 signaler]；pm 填满目标邮箱后回样确认，末尾补发 WRITABLE_WAKE_TAIL。
const WRITABLE_WAKE_REQUEST: u64 = 640;
const WRITABLE_WAKE_FILL: u64 = 641;
const WRITABLE_WAKE_TAIL: u64 = 642;

/// badged sender 的来源盖章，以及 owner 的 GRANT/TRANSIT 运输边界。
fn test_capability_badges_and_affine_owners() {
    const BADGE: u64 = 0x51a7_0bad_f00d;
    assert!(matches!(
        create(
            Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::TRANSIT,
            Rights::WRITE,
        ),
        Err(SystemCallError::RightsDenied)
    ));
    let mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("badged mailbox create failed");
    let badged = mint_sender(
        mailbox.owner,
        BADGE,
        Rights::WRITE | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("badged sender mint failed");
    let lifetime = badged.lifetime;
    let badged = badged.sender;
    let copy = duplicate(badged, Rights::WRITE).expect("badged sender duplicate failed");
    assert!(matches!(
        mint_sender(mailbox.owner, BADGE + 1, Rights::SIGNAL),
        Err(SystemCallError::RightsDenied)
    ));
    let transport = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::TRANSIT,
    )
    .expect("capability transport mailbox create failed");

    unsafe { send_raw(mailbox.peer, 880, &[], &[]) }
        .expect("explicit zero-badge sender send failed");
    unsafe { send_raw(badged, 881, &[], &[]) }.expect("badged sender send failed");
    unsafe { send_raw(copy, 882, &[], &[]) }.expect("badged sender copy send failed");
    for (kind, badge) in [(880, 0), (881, BADGE), (882, BADGE)] {
        let message = receive(mailbox.owner).expect("badged message receive failed");
        assert_eq!(message.header.kind, kind);
        assert_eq!(message.header.sender_pid, env::pid() as u64);
        assert_eq!(message.header.sender_badge, badge);
    }
    let once = make_send_once(badged, Rights::WRITE).expect("badged send-once mint failed");
    unsafe { send_raw(once, 887, &[], &[]) }.expect("badged send-once send failed");
    let message = receive(mailbox.owner).expect("badged send-once receive failed");
    assert_eq!(message.header.sender_badge, BADGE);

    let transit =
        duplicate(badged, Rights::WRITE | Rights::TRANSIT).expect("badged transit copy failed");
    let moves = [HandleMove {
        handle: transit,
        rights: Rights::WRITE,
    }];
    unsafe { send_raw(transport.peer, 888, &[], &moves) }.expect("badged sender transit failed");
    let transferred = receive(transport.owner)
        .expect("badged sender transit receive failed")
        .handles
        .take(0)
        .expect("badged sender slot missing")
        .into_raw();
    unsafe { send_raw(transferred, 889, &[], &[]) }.expect("transferred badged sender send failed");
    let message = receive(mailbox.owner).expect("transferred badged message receive failed");
    assert_eq!(message.header.sender_badge, BADGE);
    unsafe { close(transferred) }.expect("transferred badged sender close failed");
    unsafe { close(copy) }.expect("badged sender copy close failed");
    unsafe { close(badged) }.expect("badged sender close failed");
    unsafe { close(lifetime) }.expect("badged sender observer close failed");

    assert!(matches!(
        duplicate(mailbox.owner, Rights::READ),
        Err(SystemCallError::RightsDenied)
    ));
    let owner_move = [HandleMove {
        handle: mailbox.owner,
        rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
    }];
    assert!(matches!(
        unsafe { send_raw(transport.peer, 883, &[], &owner_move) },
        Err(SystemCallError::RightsDenied)
    ));
    unsafe { send_raw(mailbox.peer, 884, &[], &[]) }.expect("owner must survive rejected transit");
    assert_eq!(
        receive(mailbox.owner)
            .expect("owner receive after rejected transit failed")
            .header
            .kind,
        884
    );

    let event = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        Rights::SIGNAL,
    )
    .expect("affine notification create failed");
    assert!(matches!(
        mint_sender(event.owner, BADGE, Rights::WRITE),
        Err(SystemCallError::WrongObjectType)
    ));
    let owner_move = [HandleMove {
        handle: event.owner,
        rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
    }];
    assert!(matches!(
        unsafe { send_raw(transport.peer, 886, &[], &owner_move) },
        Err(SystemCallError::RightsDenied)
    ));
    notification::signal(event.peer, 1).expect("notification owner must survive rejected transit");
    assert_eq!(
        notification::take(event.owner, 1)
            .expect("notification take after rejected transit failed"),
        1
    );

    unsafe { close(event.peer) }.expect("notification signaler close failed");
    unsafe { close(event.owner) }.expect("notification owner close failed");
    unsafe { close(mailbox.peer) }.expect("default sender close failed");
    unsafe { close(mailbox.owner) }.expect("mailbox owner close failed");
    unsafe { close(transport.peer) }.expect("owner transport sender close failed");
    unsafe { close(transport.owner) }.expect("owner transport owner close failed");
    debug!("capability badge and owner transport passed");
}

/// 持久观察的轮次、立即失效与内核独立退休。
fn test_wait_set_retirement() {
    use rinlib::{ipc::wait_set::WaitSet, time::Deadline};

    let event = notification::create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL)
        .expect("WaitSet retirement notification create failed");
    let set = WaitSet::create(128).expect("WaitSet retirement create failed");
    let mut tokens = alloc::vec::Vec::new();
    for cookie in 0..64 {
        tokens.push(
            set.register(WaitItem::new(event.owner, ObjectSignals::READABLE, cookie))
                .expect("WaitSet retirement registration failed"),
        );
    }
    set.remove(tokens[0]).expect("WaitSet removal failed");
    assert_eq!(set.rearm(tokens[0]), Err(SystemCallError::ObjectNotFound));
    assert_eq!(set.remove(tokens[0]), Err(SystemCallError::ObjectNotFound));
    notification::signal(event.peer, 1).expect("WaitSet notification signal failed");
    set.wait(Deadline::INFINITE)
        .expect("WaitSet readiness wait failed");
    let records = set.receive(64).expect("WaitSet ready receive failed");
    assert!(records.iter().all(|record| record.token != tokens[0]));
    set.close()
        .unwrap_or_else(|(_, error)| panic!("nonempty WaitSet close failed: {error:?}"));
    unsafe { close(event.owner) }.expect("WaitSet source owner close failed");
    unsafe { close(event.peer) }.expect("WaitSet source signaler close failed");

    let event = notification::create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL)
        .expect("WaitSet rearm notification create failed");
    let set = WaitSet::create(1).expect("WaitSet rearm create failed");
    let token = set
        .register(WaitItem::new(event.owner, ObjectSignals::READABLE, 4))
        .expect("WaitSet rearm registration failed");
    let mut generation = 0;
    for _ in 0..16 {
        notification::signal(event.peer, 1).expect("WaitSet rearm signal failed");
        set.wait(Deadline::INFINITE)
            .expect("WaitSet rearm wait failed");
        let records = set.receive(1).expect("WaitSet rearm receive failed");
        assert_eq!(records[0].token, token);
        assert!(records[0].arm_generation > generation);
        notification::take(event.owner, 1).expect("WaitSet rearm source reset failed");
        let next = set.rearm(token).expect("WaitSet next arm failed");
        assert!(next > records[0].arm_generation);
        assert_eq!(set.receive(1), Err(SystemCallError::ObjectNotAvailable));
        generation = records[0].arm_generation;
    }
    set.close()
        .unwrap_or_else(|(_, error)| panic!("rearmed WaitSet close failed: {error:?}"));
    unsafe { close(event.owner) }.expect("WaitSet rearm owner close failed");
    unsafe { close(event.peer) }.expect("WaitSet rearm signaler close failed");

    let set = WaitSet::create(4).expect("self-observing WaitSet create failed");
    set.register(WaitItem::new(set.handle(), ObjectSignals::READABLE, 1))
        .expect("WaitSet self-observation failed");
    set.close()
        .unwrap_or_else(|(_, error)| panic!("self-observing WaitSet close failed: {error:?}"));

    let first = WaitSet::create(4).expect("cross-observing first WaitSet create failed");
    let second = WaitSet::create(4).expect("cross-observing second WaitSet create failed");
    first
        .register(WaitItem::new(second.handle(), ObjectSignals::READABLE, 2))
        .expect("first WaitSet cross-observation failed");
    second
        .register(WaitItem::new(first.handle(), ObjectSignals::READABLE, 3))
        .expect("second WaitSet cross-observation failed");
    first
        .close()
        .unwrap_or_else(|(_, error)| panic!("first cross-observing close failed: {error:?}"));
    second
        .close()
        .unwrap_or_else(|(_, error)| panic!("second cross-observing close failed: {error:?}"));
    debug!("WaitSet nonempty, self and cross retirement passed");
}

/// 一次性投递权：本地、转移、失败保留与原 sender 独立性。
fn test_send_once() {
    let mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("send-once mailbox create failed");
    let once = make_send_once(mailbox.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        .expect("make send once failed");
    unsafe { send_raw(once, 900, &[1], &[]) }.expect("send once failed");
    assert!(matches!(
        unsafe { send_raw(once, 901, &[], &[]) },
        Err(SystemCallError::StaleHandle)
    ));

    let once = make_send_once(mailbox.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        .expect("transferred send-once mint failed");
    let moves = [HandleMove {
        handle: once,
        rights: Rights::WRITE,
    }];
    unsafe { send_raw(mailbox.peer, 902, &[], &moves) }.expect("send-once transit failed");
    let first = receive(mailbox.owner).expect("send-once receive failed");
    assert_eq!(first.header.kind, 900);
    let mut second = receive(mailbox.owner).expect("send-once transit receive failed");
    assert_eq!(second.header.kind, 902);
    let transferred_once = second
        .handles
        .take(0)
        .expect("send-once slot missing")
        .into_raw();
    unsafe { send_raw(transferred_once, 903, &[2], &[]) }.expect("transferred once send failed");
    assert!(matches!(
        unsafe { send_raw(transferred_once, 904, &[], &[]) },
        Err(SystemCallError::StaleHandle)
    ));

    // 原 sender 仍可长期使用，不受派生影响。
    unsafe { send_raw(mailbox.peer, 905, &[], &[]) }.expect("original sender still usable");
    for expected in [903u64, 905] {
        let message = receive(mailbox.owner).expect("tail receive failed");
        assert_eq!(message.header.kind, expected);
    }

    // 满箱失败不消费：撞 MailboxFull 后腾位，同一 once 仍可投递。
    let full = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("send-once full mailbox create failed");
    let once = make_send_once(full.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        .expect("send-once full mint failed");
    for _ in 0..MAILBOX_CAPACITY {
        unsafe { send_raw(full.peer, 0, &[], &[]) }.expect("send-once full fill failed");
    }
    assert!(matches!(
        unsafe { send_raw(once, 910, &[], &[]) },
        Err(SystemCallError::MailboxFull)
    ));
    discard(full.owner).expect("send-once full make-room failed");
    unsafe { send_raw(once, 911, &[], &[]) }.expect("failed send must not consume once");
    assert!(matches!(
        unsafe { send_raw(once, 912, &[], &[]) },
        Err(SystemCallError::StaleHandle)
    ));
    for _ in 0..MAILBOX_CAPACITY {
        discard(full.owner).expect("send-once full drain failed");
    }
    unsafe { close(full.peer) }.expect("send-once full sender close failed");
    unsafe { close(full.owner) }.expect("send-once full owner close failed");

    // once 同时作为发送目标与 transit move 会突破一次投递保证，必须在
    // 任何入队或摘除前整体拒绝；失败不消费 once。
    let both = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("send-once both mailbox create failed");
    let once = make_send_once(both.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        .expect("send-once both mint failed");
    assert!(matches!(
        make_send_once(once, Rights::WRITE),
        Err(SystemCallError::RightsDenied)
    ));
    let moves = [HandleMove {
        handle: once,
        rights: Rights::WRITE,
    }];
    assert!(matches!(
        unsafe { send_raw(once, 920, &[], &moves) },
        Err(SystemCallError::IllegalArgument)
    ));
    unsafe { send_raw(once, 921, &[], &[]) }.expect("rejected alias must not consume send-once");
    assert!(matches!(
        unsafe { send_raw(once, 922, &[], &[]) },
        Err(SystemCallError::StaleHandle)
    ));
    let message = receive(both.owner).expect("send-once alias recovery receive failed");
    assert_eq!(message.header.kind, 921);
    assert!(message.handles.is_empty());
    unsafe { close(both.peer) }.expect("send-once both sender close failed");
    unsafe { close(both.owner) }.expect("send-once both owner close failed");
    debug!("send-once passed");
}

/// WRITABLE 电平快路径：空箱即时就绪，填满后腾位重新就绪。
/// “满箱清零”的阻塞侧由 [`test_writable_wake`] 跨进程验证。
fn test_writable_level() {
    let mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT,
    )
    .expect("writable mailbox create failed");
    let result = wait_many(
        &[WaitItem::new(mailbox.peer, ObjectSignals::WRITABLE, 1)],
        0,
    )
    .expect("empty mailbox must be writable");
    assert!(result.observed.intersects(ObjectSignals::WRITABLE));

    for _ in 0..MAILBOX_CAPACITY {
        unsafe { send_raw(mailbox.peer, 0, &[], &[]) }.expect("writable fill failed");
    }
    discard(mailbox.owner).expect("writable make-room failed");
    let result = wait_many(
        &[WaitItem::new(mailbox.peer, ObjectSignals::WRITABLE, 2)],
        0,
    )
    .expect("mailbox below capacity must be writable");
    assert!(result.observed.intersects(ObjectSignals::WRITABLE));

    for _ in 0..MAILBOX_CAPACITY - 1 {
        discard(mailbox.owner).expect("writable drain failed");
    }
    unsafe { close(mailbox.peer) }.expect("writable sender close failed");
    unsafe { close(mailbox.owner) }.expect("writable owner close failed");
    debug!("writable level passed");
}

/// 跨进程流控唤醒：pm 填满目标邮箱后在 WRITABLE 上阻塞，本进程腾出一个
/// 位置唤醒它。pm 侧内联检测虚假唤醒（醒来后再撞满箱即置 spin 位），
/// 末尾校验 spin 为空——唤醒只能由腾位引起，证明等待路径真实走过。
fn test_writable_wake(root: &mut RootSupervisor, pm_mailbox: Handle) -> Result<(), &'static str> {
    let target = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        )
        .map_err(|_| "wake target mailbox create failed")?;
    let done = root
        .create_notification(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::SIGNAL | Rights::TRANSIT,
        )
        .map_err(|_| "wake done notification create failed")?;
    let spin = root
        .create_notification(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::SIGNAL | Rights::TRANSIT,
        )
        .map_err(|_| "wake spin notification create failed")?;
    let moves = [
        HandleMove {
            handle: target.peer,
            rights: Rights::WRITE | Rights::WAIT,
        },
        HandleMove {
            handle: done.peer,
            rights: Rights::SIGNAL,
        },
        HandleMove {
            handle: spin.peer,
            rights: Rights::SIGNAL,
        },
    ];
    unsafe { send_raw(pm_mailbox, WRITABLE_WAKE_REQUEST, &[], &moves) }
        .map_err(|_| "wake request send failed")?;
    root.transferred(target.peer);
    root.transferred(done.peer);
    root.transferred(spin.peer);

    // pm 确认已满后置位通知，此时它正阻塞在 WRITABLE 上。
    let deadline = rinlib::time::timeout_millis(5_000).map_err(|_| "wake deadline failed")?;
    let done_wait = rinlib::ipc::wait::wait_until(
        &[WaitItem::new(done.owner, ObjectSignals::READABLE, 0)],
        deadline,
    )
    .map_err(|_| "wake notification wait failed")?;
    if WaitReason::from_u32(done_wait.reason) == Some(WaitReason::Timeout) {
        return Err("pm writable wake confirmation timed out");
    }
    notification::take(done.owner, u64::MAX).map_err(|_| "wake notification take failed")?;
    discard(target.owner).map_err(|_| "wake make-room failed")?;

    // 队列：15 × FILL + TAIL。逐条校验，末尾必然是被唤醒后补发的 TAIL。
    for index in 0..MAILBOX_CAPACITY {
        let ready = rinlib::ipc::wait::wait_until(
            &[WaitItem::new(
                target.owner,
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                0,
            )],
            deadline,
        )
        .map_err(|_| "writable tail wait failed")?;
        if WaitReason::from_u32(ready.reason) == Some(WaitReason::Timeout) {
            return Err("pm writable tail timed out");
        }
        let message = receive(target.owner).map_err(|_| "writable tail receive failed")?;
        let expected = if index + 1 == MAILBOX_CAPACITY {
            WRITABLE_WAKE_TAIL
        } else {
            WRITABLE_WAKE_FILL
        };
        if message.header.kind != expected {
            return Err("unexpected writable wake message");
        }
    }
    // TAIL 已入队即 pm 已退出循环；此前的任何虚假唤醒都会留下 spin 位。
    if !matches!(
        notification::take(spin.owner, u64::MAX),
        Err(SystemCallError::ObjectNotAvailable)
    ) {
        return Err("writable wake reported a spurious completion");
    }
    root.close_control(done.owner)?;
    root.close_control(spin.owner)?;
    root.close_control(target.owner)?;
    debug!("writable wake passed");
    Ok(())
}

#[cfg(feature = "acceptance-stress")]
fn stress_control_plane() {
    for index in 0..CONTROL_STRESS {
        let mailbox = create(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
        )
        .expect("stress mailbox create failed");
        let event = notification::create(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::SIGNAL | Rights::TRANSIT,
        )
        .expect("stress notification create failed");
        let moves = [HandleMove {
            handle: event.peer,
            rights: Rights::SIGNAL,
        }];
        unsafe { send_raw(mailbox.peer, index as u64, &index.to_le_bytes(), &moves) }
            .expect("stress send failed");
        assert!(matches!(
            unsafe { close(event.peer) },
            Err(SystemCallError::StaleHandle)
        ));
        let message = receive(mailbox.owner).expect("stress receive failed");
        assert_eq!(message.header.kind, index as u64);
        assert_eq!(message.payload, index.to_le_bytes());
        notification::signal(
            message
                .handles
                .get(0)
                .expect("stress signaler slot missing")
                .as_handle(),
            1,
        )
        .expect("stress signal failed");
        assert_eq!(
            notification::take(event.owner, 1).expect("stress take failed"),
            1
        );
        drop(message);
        unsafe { close(event.owner) }.expect("stress notification owner close failed");
        unsafe { close(mailbox.peer) }.expect("stress mailbox sender close failed");
        unsafe { close(mailbox.owner) }.expect("stress mailbox owner close failed");
    }

    let mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("full mailbox create failed");
    for _ in 0..MAILBOX_CAPACITY {
        unsafe { send_raw(mailbox.peer, 0, &[], &[]) }.expect("mailbox fill failed");
    }
    let event = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT,
    )
    .expect("full mailbox notification create failed");
    let moves = [HandleMove {
        handle: event.peer,
        rights: Rights::SIGNAL,
    }];
    assert!(matches!(
        unsafe { send_raw(mailbox.peer, 0, &[], &moves) },
        Err(SystemCallError::MailboxFull)
    ));
    notification::signal(event.peer, 1).expect("failed Send must retain moved source");
    for _ in 0..MAILBOX_CAPACITY {
        discard(mailbox.owner).expect("mailbox discard failed");
    }
    unsafe { close(event.peer) }.expect("retained signaler close failed");
    unsafe { close(event.owner) }.expect("retained owner close failed");
    unsafe { close(mailbox.peer) }.expect("full mailbox sender close failed");
    unsafe { close(mailbox.owner) }.expect("full mailbox owner close failed");
    debug!(
        "control-plane stress passed: {} transactions",
        CONTROL_STRESS
    );
}

/// 实际 Running 调用链验证取整、双端不同 VA、完整共享范围与关闭后存活端。
fn test_tunnel_geometry() {
    use core::sync::atomic::Ordering;
    use rinlib::mm::Placement;
    for bytes in [1, 4096, 4097, 8192, 12288, 512 * 4096] {
        let (creator, invitation) =
            tunnel_sys::create(bytes, Placement::Anywhere).expect("Tunnel geometry Create failed");
        let geometry = creator.geometry();
        assert_eq!(geometry.bytes(), bytes.div_ceil(4096) * 4096);
        for page in 0..geometry.bytes() / 4096 {
            creator
                .memory()
                .store_u64(page * 4096, page as u64 + 1, Ordering::Release);
        }
        let peer = unsafe { tunnel_sys::attach(invitation, Placement::Anywhere) }
            .expect("Tunnel geometry Attach failed");
        assert_ne!(geometry.base(), peer.geometry().base());
        assert_eq!(geometry.bytes(), peer.geometry().bytes());
        for page in 0..geometry.bytes() / 4096 {
            assert_eq!(
                peer.memory().load_u64(page * 4096, Ordering::Acquire),
                page as u64 + 1
            );
        }
        let peer_base = peer.geometry().base();
        creator
            .close()
            .expect("Tunnel geometry creator close failed");
        assert_eq!(
            peer.memory()
                .load_u64(geometry.bytes() - 4096, Ordering::Acquire),
            (geometry.bytes() / 4096) as u64
        );
        peer.close().expect("Tunnel geometry peer close failed");
        for address in [geometry.base(), peer_base] {
            let region = rinlib::mm::MappedRegion::map_anonymous(
                geometry.bytes(),
                0,
                0,
                rinlib::shared::mem::MemoryProtection::ReadWrite,
                Placement::FixedEmpty {
                    usable_start: address,
                },
            )
            .expect("Tunnel geometry left a mapping behind");
            region.unmap().expect("Tunnel geometry reuse Unmap failed");
        }
    }
    debug!("Tunnel Running geometry checks passed: six lengths");
}

#[cfg(feature = "acceptance-stress")]
fn test_tunnel_lifecycle() {
    for _ in 0..TUNNEL_STRESS {
        let (abandoned, invitation) =
            tunnel_sys::create(TUNNEL_BYTES, rinlib::mm::Placement::Anywhere)
                .expect("lifecycle tunnel create failed");
        assert!(matches!(
            wait_many(&[WaitItem::new(invitation, ObjectSignals::CLOSED, 0)], 0,),
            Err(SystemCallError::RightsDenied)
        ));
        // SAFETY: 本轮独占、未运输的 Invitation。
        unsafe { close(invitation) }.expect("invitation close failed");
        let result = abandoned
            .events()
            .wait(ObjectSignals::PEER_CLOSED, 0)
            .expect("abandoned invitation wait failed");
        assert!(result.observed.intersects(ObjectSignals::PEER_CLOSED));
        abandoned.close().expect("lifecycle endpoint close failed");

        let (creator_closed, invitation) =
            tunnel_sys::create(TUNNEL_BYTES, rinlib::mm::Placement::Anywhere)
                .expect("closed-creator tunnel create failed");
        creator_closed
            .close()
            .expect("creator endpoint close failed");
        assert!(matches!(
            unsafe { tunnel_sys::attach(invitation, rinlib::mm::Placement::Anywhere) },
            Err(SystemCallError::ObjectClosed)
        ));
        // SAFETY: Attach 失败未消费本轮的 Invitation。
        unsafe { close(invitation) }.expect("closed invitation close failed");
    }
    debug!("tunnel lifecycle stress passed: {} rounds", TUNNEL_STRESS);
}
