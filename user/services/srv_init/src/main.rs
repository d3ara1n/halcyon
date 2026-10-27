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
//! 5. 监督闭环：全部服务等 DONE → 核验终态 → 关闭观察权
//!    快照 → close；pm_domain 由 pm 自行收束，未收束时 init 兜底；
//! 6. 等 pm 退出后的 PEER_CLOSED 终态位，关闭本端（帧归还）；
//! 7. 验证系统复位 capability 的负路径，由 init 显式提交 Shutdown；平台拒绝时
//!    终止 init，由 boot 的预付 DONE 观察进入不可恢复停止。
//!
//! 默认构建运行确定性的 core 验收；`acceptance-stress` feature 在同一用户态
//! 编排器中追加重复压力、自动回收与完整竞态矩阵。内核不感知该档位。

#![no_std]
#![feature(allocator_ext)]

extern crate alloc;
extern crate libruntime;

use erhino_shared::sync::spin::SimpleLock;
use erhino_shared::time::Deadline;
use libbudget::Budget;
use libexecution::{
    ExecutionResource,
    runtime::{
        Advance, Input, RequestFailure, Requests, Runtime, SourceId, SourceKind, Step, Task,
    },
};
use libfs::protocol;
use libkernel::ipc::wait_set::WaitSet;
use libkernel::ipc::{
    capability::Capability,
    message::{Mailbox, MailboxSender, SendOnce},
    packet::Packet,
};
use libkernel::memory_pool::MemoryPool;
use libkernel::shared::proc::Pid;
use libkernel::{
    ipc::{
        message::{create, discard, make_send_once, mint_sender, receive, send_raw, wait_message},
        notification,
        object::{close, duplicate},
        wait::wait_until,
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
        proc::{HandleGrant, JobMemberKind, JobState, ProcessState, TerminationCause},
        reset::{ResetAction, ResetReason},
        startup::initial,
        wait::WaitReason,
        wait_set::MemberRequest,
    },
    system,
};
use libprocess::{
    DEFAULT_SUPERVISION_POLICY, DERIVED_CONTROL_RIGHTS, JobCollector, RequiredLaunchSet,
    SpawnRequest, SuperviseResult, SuperviseSink, SuperviseTask, SupervisionCause,
    SupervisionStage, SupervisionTarget, collect_process,
    context::{CurrentProcessInfo, StartupPayload},
    enumerate_members, spawn,
};
use librpc::{
    CallCause, CallError, CallPhase, Caller, FrameRejection, Outbox, OutboxResult, Request,
    RequestContext, RpcMessageKind, RpcPrefix,
};
use librunnel::{ConsumerReady, blocking};
use libservice::{client as service_client, protocol as service_protocol};
use libthread::ThreadOutcome;
use lock_api::Mutex;

mod supervisor;
use supervisor::RootSupervisor;

mod building;
mod device;
#[cfg(feature = "acceptance-device")]
mod device_acceptance;
mod device_supervision;
mod dma_checks;
mod driver;
mod filesystem;
#[cfg(feature = "acceptance-device-function-session")]
mod function_checks;
mod irq_checks;
mod mmio_checks;
mod once_checks;
mod platform;
mod public_ipc;
#[cfg(feature = "acceptance-stress")]
mod race;
mod time_checks;
use building::build_spin_building;
use filesystem::*;
#[cfg(feature = "acceptance-stress")]
use race::race_matrix;

/// 受监督服务：init 保留的 control 与 pid。
struct Supervised {
    pid: u64,
    control: Handle,
}

struct LaunchedServices {
    first_fs: AcceptedFsProvider,
    registry_root_watch: service_client::Watch,
    pm_mailbox: Handle,
    fs_bootstrap_second: Handle,
    fs_release_second: Handle,
    fs_bootstrap_second_sender: Handle,
    fs_release_second_owner: Handle,
    fs_route_second_owner: Handle,
    fs_second_scope: MailboxSender,
    fs_second_image: StartupImage,
    test_fs_image: StartupImage,
    target_image: Option<StartupImage>,
    hammer_image: Option<StartupImage>,
}

struct InitStartup {
    process: CurrentProcessInfo,
    payload: StartupPayload,
    root_memory_pool: Handle,
    _platform: platform::PlatformDescription,
}

#[derive(Clone, Copy)]
struct StartupImage {
    ptr: *const u8,
    len: usize,
}

unsafe impl Send for StartupImage {}
unsafe impl Sync for StartupImage {}

impl StartupImage {
    fn from_slice(bytes: &[u8]) -> Self {
        Self {
            ptr: bytes.as_ptr(),
            len: bytes.len(),
        }
    }

    fn as_slice(&self) -> &[u8] {
        // SAFETY: the image points into the process-lifetime startup payload.
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }
}

static INIT_STARTUP: Mutex<SimpleLock, Option<InitStartup>> =
    Mutex::const_new(SimpleLock::new(), None);

fn init_pid() -> Pid {
    INIT_STARTUP
        .lock()
        .as_ref()
        .expect("init startup inputs are not initialized")
        .process
        .pid()
}

fn init_process_info() -> CurrentProcessInfo {
    INIT_STARTUP
        .lock()
        .as_ref()
        .expect("init startup inputs are not initialized")
        .process
}

fn with_init_payload<R>(f: impl FnOnce(&[u8]) -> R) -> R {
    let guard = INIT_STARTUP.lock();
    let startup = guard
        .as_ref()
        .expect("init startup inputs are not initialized");
    let payload = startup.payload;
    drop(guard);
    f(payload.as_bytes())
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
        1 => "Completed",
        2 => "Fault",
        3 => "Killed",
        4 => "Abandoned",
        _ => "unknown",
    }
}

/// 服务 ProcessControl 的监督基准权利：init 保留 control 用于查询、等待、
/// 终止与受保护收束，并可复制/直接转授。
const SUPERVISOR_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::DUPLICATE.raw()
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

#[cfg(feature = "acceptance-stress")]
const TUNNEL_STRESS: usize = 64;
#[cfg(feature = "acceptance-memory")]
const ACCEPTANCE_WORKLOAD: &str = "memory";
#[cfg(all(feature = "acceptance-stress", not(feature = "acceptance-memory")))]
const ACCEPTANCE_WORKLOAD: &str = "stress";
#[cfg(all(not(feature = "acceptance-stress"), not(feature = "acceptance-device")))]
const ACCEPTANCE_WORKLOAD: &str = "core";
#[cfg(feature = "acceptance-device")]
const ACCEPTANCE_WORKLOAD: &str = "device";

const REQUIRED_FS: u64 = 1 << 0;
const REQUIRED_PM: u64 = 1 << 1;
const REQUIRED_DRIVER: u64 = 1 << 2;
const REQUIRED_TARGET: u64 = 1 << 3;
const REQUIRED_PM_TARGET: u64 = 1 << 4;
const REQUIRED_FS_CONSUMER: u64 = 1 << 5;
const REQUIRED_DEVICE: u64 = 1 << 6;
const REQUIRED_IMAGES: u64 = REQUIRED_FS
    | REQUIRED_PM
    | REQUIRED_DRIVER
    | REQUIRED_TARGET
    | REQUIRED_FS_CONSUMER
    | REQUIRED_DEVICE;
const REQUIRED_STARTED: u64 = REQUIRED_FS
    | REQUIRED_PM
    | REQUIRED_DRIVER
    | REQUIRED_TARGET
    | REQUIRED_PM_TARGET
    | REQUIRED_DEVICE;

fn required_image(name: &str) -> Option<u64> {
    match name {
        "bin/srv_fs" => Some(REQUIRED_FS),
        "bin/srv_pm" => Some(REQUIRED_PM),
        "bin/srv_device" => Some(REQUIRED_DEVICE),
        "bin/srv_driver" => Some(REQUIRED_DRIVER),
        "bin/test_target" => Some(REQUIRED_TARGET),
        "bin/test_fs" => Some(REQUIRED_FS_CONSUMER),
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
    root.begin_device_supervision(services)?;
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
        .create_mailbox_owner(Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT)
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
        .create_mailbox_owner(Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT)
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
    let mut target_image: Option<StartupImage> = None;
    let mut hammer_image: Option<StartupImage> = None;
    let mut test_fs_image: Option<StartupImage> = None;
    let mut fs_second_image: Option<StartupImage> = None;
    let mut fs_second_scope = None;
    let mut primary_provider = None;
    let mut registry_root_watch = None;
    let mut driver_image = None;
    let mut pci_image = None;
    let mut xhci_image = None;
    let mut hid_image = None;
    let result = with_init_payload(|payload| {
        tar::walk(payload, |entry| {
            if !entry.name.starts_with("bin/") || entry.name.ends_with('/') {
                return;
            }
            if entry.name == "bin/test_hammer" {
                // 竞态锤不是常驻服务：只留存映像，由剧本按需 spawn。
                hammer_image = Some(StartupImage::from_slice(entry.data));
                return;
            }
            if entry.name == "bin/drv_pci_generic" {
                pci_image = Some(StartupImage::from_slice(entry.data));
                return;
            }
            if entry.name == "bin/drv_usb_xhci" {
                xhci_image = Some(StartupImage::from_slice(entry.data));
                return;
            }
            if entry.name == "bin/drv_usb_hid" {
                hid_image = Some(StartupImage::from_slice(entry.data));
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
            if entry.name == "bin/srv_driver" {
                driver_image = Some(StartupImage::from_slice(entry.data));
                return;
            }
            if entry.name == "bin/srv_device" {
                if root.devices.is_some() {
                    stage_failure = Some("duplicate device manager image");
                    return;
                }
                match device::Manager::launch(root, services, names, entry.data) {
                    Ok(manager) => {
                        root.devices = Some(manager);
                        manifest.mark_started(REQUIRED_DEVICE);
                    }
                    Err(error) => stage_failure = Some(error),
                }
                return;
            }
            if entry.name == "bin/test_fs" {
                test_fs_image = Some(StartupImage::from_slice(entry.data));
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
            let fs_pool_grant = if entry.name == "bin/srv_fs" {
                match root.duplicate(root_memory_pool(), Rights::CREATE | Rights::GRANT) {
                    Ok(pool) => pool,
                    Err(_) => {
                        stage_failure = Some("provider Pool grant failed");
                        return;
                    }
                }
            } else {
                Handle::INVALID
            };
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
                    handle: fs_route,
                    rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
                },
                HandleGrant {
                    handle: fs_registration,
                    rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
                },
                HandleGrant {
                    handle: fs_pool_grant,
                    rights: Rights::CREATE,
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
            let spawned = root.spawn_process(SpawnRequest {
                memory_pool: root_memory_pool(),
                parent: init_process_info(),
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
                root.transferred(fs_route);
                root.transferred(fs_registration);
                root.transferred(fs_pool_grant);
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
                        target_image = Some(StartupImage::from_slice(entry.data));
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
                        match root.spawn_process(SpawnRequest {
                            memory_pool: root_memory_pool(),
                            parent: init_process_info(),
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
                                root,
                                fs_bootstrap.owner,
                                fs_release.peer,
                                true,
                            )?;
                            if primary.pid != process.pid {
                                return Err("primary provider bootstrap sender mismatch");
                            }
                            let directory = primary
                                .directory
                                .as_ref()
                                .ok_or("primary provider omitted service directory")?;
                            let deadline = libkernel::time::timeout_millis(10_000)
                                .map_err(|_| "registration bootstrap deadline invalid")?;
                            let watch = name_watch(root, directory)?;
                            let authority = primary
                                .registration_root
                                .as_ref()
                                .ok_or("primary provider omitted registration authority")?;

                            let scoped = service_client::Publisher {
                                sender: authority,
                                caller: root,
                            }
                            .delegate_name(
                                "fs.secondary",
                                service_protocol::PublishRights::CREATE
                                    | service_protocol::PublishRights::REPLACE,
                                deadline,
                            )
                            .map_err(|_| "fs name authority delegation failed")?;
                            Ok((primary, watch, scoped))
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
                        fs_second_image = Some(StartupImage::from_slice(entry.data));
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
        })
    });
    if let Err(error) = result {
        debug!("required initfs parse failed: {:?}", error);
        stage_failure = Some("initfs parse failed");
    }
    if stage_failure.is_none() {
        match (driver_image, pci_image, xhci_image, hid_image) {
            (Some(manager), Some(pci), Some(xhci), Some(hid)) => {
                match driver::Manager::launch(
                    root,
                    services,
                    names,
                    manager.as_slice(),
                    [pci.as_slice(), xhci.as_slice(), hid.as_slice()],
                ) {
                    Ok(manager) => {
                        root.drivers = Some(manager);
                        manifest.mark_started(REQUIRED_DRIVER);
                    }
                    Err(error) => stage_failure = Some(error),
                }
            }
            _ => stage_failure = Some("driver boot image pairing incomplete"),
        }
    }
    if let Some(failure) = stage_failure {
        debug!("required service preparation failed: {}", failure);
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
        fs_bootstrap_second_sender: fs_bootstrap_second.peer,
        fs_release_second_owner: fs_release_second.owner,
        fs_route_second_owner: fs_route_second,
        fs_second_scope: fs_second_scope.ok_or("second fs name scope missing")?,
        fs_second_image: fs_second_image.ok_or("second fs image missing")?,
        test_fs_image: test_fs_image.ok_or("FS consumer image missing")?,
        target_image,
        hammer_image,
    })
}

fn launch_secondary_fs(
    root: &mut RootSupervisor,
    services: Handle,
    names: &mut TopologyNames,
    image: &[u8],
    grants: [Handle; 3],
    authority: MailboxSender,
) -> Result<(), &'static str> {
    let [bootstrap, release, route] = grants;
    let inbox = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        )
        .map_err(|_| "secondary publication inbox creation failed")?;
    let fs_pool_grant = root
        .duplicate(root_memory_pool(), Rights::CREATE | Rights::GRANT)
        .map_err(|_| "second provider Pool grant failed")?;
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
            handle: inbox.owner,
            rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
        },
        HandleGrant {
            handle: fs_pool_grant,
            rights: Rights::CREATE,
        },
    ];
    root.reserve_process()
        .map_err(|_| "second fs supervision admission failed")?;
    let spawned = root.spawn_process(SpawnRequest {
        memory_pool: root_memory_pool(),
        parent: init_process_info(),
        job: services,
        image,
        payload: b"secondary",
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
    let rights = authority
        .description()
        .map_err(|_| "secondary publication authority query failed")?
        .rights;
    let mut packet = Packet::new(service_protocol::AUTHORITY_GRANT_KIND, &[])
        .map_err(|_| "secondary authority packet failed")?;
    packet
        .push(authority.into_capability(), rights)
        .map_err(|_| "secondary authority packet layout failed")?;
    // 唯一 init sender 向新建空 inbox 投递一次；出生安装不需要此 authority 有 GRANT。
    packet
        .try_send(
            root.sender(inbox.peer),
            libkernel::time::timeout_millis(10_000)
                .map_err(|_| "secondary authority delivery deadline failed")?,
        )
        .map_err(|_| "secondary authority delivery failed")?;
    root.close_control(inbox.peer)?;
    Ok(())
}

struct FsConsumerCoordinator {
    pid: u64,
    control: Handle,
    report_owner: Handle,
    command_signaler: Handle,
    role_sender: Handle,
}

impl FsConsumerCoordinator {
    fn deliver_management(
        &self,
        root: &RootSupervisor,
        management: &MailboxSender,
    ) -> Result<(), &'static str> {
        let rights = management
            .description()
            .map_err(|_| "management rights query failed")?
            .rights
            & !Rights::GRANT;
        let handle = duplicate(management.as_handle(), rights)
            .map_err(|_| "secondary management duplicate failed")?;
        // SAFETY: duplicate 返回本次独占的新、未映射 handle；Packet 接管完整关闭责任。
        let capability = unsafe { Capability::from_raw(handle) };
        let mut packet = Packet::new(libfs::route::MANAGEMENT_GRANT_KIND, &[])
            .map_err(|_| "management handoff packet failed")?;
        packet
            .push(capability, rights)
            .map_err(|_| "management handoff layout failed")?;
        packet
            .try_send(root.sender(self.role_sender), filesystem::deadline()?)
            .map_err(|_| "management handoff delivery failed")?;
        Ok(())
    }
    fn await_report(&self, root: &mut RootSupervisor, expected: u64) -> Result<(), &'static str> {
        let deadline = libkernel::time::timeout_millis(test_fs::command::PHASE_TIMEOUT_MILLIS)
            .map_err(|_| "FS consumer phase deadline invalid")?;
        debug!(
            "FS consumer phase wait started: pid={}, report={}, now_ns={}, end_ns={}",
            self.pid,
            expected,
            libkernel::time::snapshot()
                .map_err(|_| "FS phase clock failed")?
                .now_ns,
            deadline.at_ns
        );
        let mut terminal = None;
        loop {
            let bits = match root.notification_owner(self.report_owner).take(u64::MAX) {
                Ok(bits) => bits,
                Err(SystemCallError::ObjectNotAvailable) => 0,
                Err(_) => return Err("FS consumer phase report take failed"),
            };
            if bits != 0 {
                debug!(
                    "FS consumer phase report received: pid={}, report={}, now_ns={}",
                    self.pid,
                    bits,
                    libkernel::time::snapshot()
                        .map_err(|_| "FS phase clock failed")?
                        .now_ns
                );
                return if bits == expected {
                    Ok(())
                } else {
                    Err("FS consumer phase report out of order")
                };
            }
            if let Some(reason) = terminal {
                debug!(
                    "FS consumer phase ended without expected report: pid={}, report={}, now_ns={}, reason={}",
                    self.pid,
                    expected,
                    libkernel::time::snapshot()
                        .map_err(|_| "FS phase clock failed")?
                        .now_ns,
                    reason
                );
                return Err(reason);
            }
            let observed = root
                .wait_sources(
                    &[
                        (
                            self.report_owner,
                            ObjectSignals::READABLE | ObjectSignals::CLOSED,
                        ),
                        (self.control, ObjectSignals::DONE),
                    ],
                    deadline,
                )
                .map_err(|_| "FS consumer phase wait failed")?;
            if observed[1].contains(ObjectSignals::DONE) {
                terminal = Some("FS consumer exited before phase report");
            } else if observed[0].contains(ObjectSignals::CLOSED) {
                terminal = Some("FS consumer phase channel closed");
            } else if observed.iter().all(|signals| signals.raw() == 0) {
                terminal = Some("FS consumer phase timed out");
            }
        }
    }

    fn continue_business(&self, root: &RootSupervisor) -> Result<(), &'static str> {
        root.notification_signaler(self.command_signaler)
            .signal(test_fs::command::CONTINUE)
            .map_err(|_| "FS consumer continuation signal failed")
    }

    fn observe_shutdown(&self, root: &RootSupervisor) -> Result<(), &'static str> {
        root.notification_signaler(self.command_signaler)
            .signal(test_fs::command::OBSERVE_SHUTDOWN)
            .map_err(|_| "FS consumer shutdown signal failed")
    }

    fn finish(self, root: &mut RootSupervisor) -> Result<(), &'static str> {
        let deadline = libkernel::time::timeout_millis(test_fs::command::PHASE_TIMEOUT_MILLIS)
            .map_err(|_| "FS consumer exit deadline invalid")?;
        let result = root
            .wait_until(self.control, ObjectSignals::DONE, deadline)
            .map_err(|_| "FS consumer exit wait failed")?;
        if WaitReason::from_u32(result.reason) == Some(WaitReason::Timeout) {
            return Err("FS consumer exit timed out");
        }
        let exit = process::query(self.control).map_err(|_| "FS consumer exit query failed")?;
        if exit.cause != TerminationCause::Completed as u32 || exit.detail.word0 != 0 {
            return Err("FS consumer exited without successful result");
        }
        root.collect_process(self.pid)?;
        root.close_control(self.report_owner)?;
        root.close_control(self.command_signaler)?;
        root.close_control(self.role_sender)?;
        Ok(())
    }
}

fn launch_fs_consumer(
    root: &mut RootSupervisor,
    services: Handle,
    names: &mut TopologyNames,
    image: &[u8],
    primary: &AcceptedFsProvider,
) -> Result<FsConsumerCoordinator, &'static str> {
    let root_grant = primary
        .grant
        .as_ref()
        .ok_or("FS consumer primary grant missing")?;
    let directory = primary
        .directory
        .as_ref()
        .ok_or("FS consumer service directory missing")?;

    let authority = libservice::client::Publisher {
        sender: primary
            .registration_root
            .as_ref()
            .ok_or("publication root missing")?,
        caller: root,
    }
    .delegate_name(
        "init.registry-probe",
        libservice::protocol::PublishRights::CREATE | libservice::protocol::PublishRights::REPLACE,
        filesystem::deadline()?,
    )
    .map_err(|_| "consumer registry authority delegation failed")?;
    let authority_b = libservice::client::Publisher {
        sender: primary
            .registration_root_b
            .as_ref()
            .ok_or("consumer B publication root missing")?,
        caller: root,
    }
    .delegate_name(
        "init.registry-probe",
        libservice::protocol::PublishRights::CREATE | libservice::protocol::PublishRights::REPLACE,
        filesystem::deadline()?,
    )
    .map_err(|_| "consumer B registry authority delegation failed")?;
    let inbox = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        )
        .map_err(|_| "consumer authority inbox create failed")?;
    let root_rights = root_grant
        .description()
        .map_err(|_| "FS consumer primary rights query failed")?
        .rights;
    let directory_rights = directory
        .description()
        .map_err(|_| "FS consumer directory rights query failed")?
        .rights;
    let primary_child_rights = root_rights & !Rights::GRANT;
    let directory_child_rights = directory_rights & !Rights::GRANT;
    root.reserve_process()
        .map_err(|_| "FS consumer supervision admission failed")?;
    let primary_copy = root
        .duplicate(root_grant.as_handle(), root_rights)
        .map_err(|_| "FS consumer primary grant duplicate failed")?;
    let directory_copy = root
        .duplicate(directory.as_handle(), directory_rights)
        .map_err(|_| "FS consumer directory grant duplicate failed")?;
    let root_b = primary
        .grant_b
        .as_ref()
        .ok_or("consumer B file root missing")?;
    let directory_b = primary
        .directory_b
        .as_ref()
        .ok_or("consumer B directory missing")?;
    let root_b_rights = root_b
        .description()
        .map_err(|_| "consumer B file rights query failed")?
        .rights;
    let directory_b_rights = directory_b
        .description()
        .map_err(|_| "consumer B directory rights query failed")?
        .rights;
    let root_b_copy = root
        .duplicate(root_b.as_handle(), root_b_rights)
        .map_err(|_| "consumer B file root duplicate failed")?;
    let directory_b_copy = root
        .duplicate(directory_b.as_handle(), directory_b_rights)
        .map_err(|_| "consumer B directory duplicate failed")?;
    let command = root
        .create_notification(
            Rights::READ | Rights::WAIT | Rights::GRANT,
            Rights::SIGNAL | Rights::GRANT,
        )
        .map_err(|_| "FS consumer command channel create failed")?;
    let report = root
        .create_notification(
            Rights::READ | Rights::WAIT | Rights::GRANT,
            Rights::SIGNAL | Rights::GRANT,
        )
        .map_err(|_| "FS consumer report channel create failed")?;
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
        HandleGrant {
            handle: inbox.owner,
            rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
        },
        HandleGrant {
            handle: root_b_copy,
            rights: root_b_rights & !Rights::GRANT,
        },
        HandleGrant {
            handle: directory_b_copy,
            rights: directory_b_rights & !Rights::GRANT,
        },
    ];
    let spawned = root.spawn_process(SpawnRequest {
        memory_pool: root_memory_pool(),
        parent: init_process_info(),
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
        debug!("required FS consumer spawn failed: {:?}", error);
        "required FS consumer spawn failed"
    })?;
    names.register_process(consumer.pid, "bin/test_fs");
    root.track_process(Supervised {
        pid: consumer.pid,
        control: consumer.control,
    });
    let rights = authority
        .description()
        .map_err(|_| "consumer authority rights query failed")?
        .rights;
    let mut packet = Packet::new(libservice::protocol::AUTHORITY_GRANT_KIND, &[])
        .map_err(|_| "consumer authority packet allocation failed")?;
    packet
        .push(authority.into_capability(), rights)
        .map_err(|_| "consumer authority packet failed")?;
    let rights_b = authority_b
        .description()
        .map_err(|_| "consumer B authority rights query failed")?
        .rights;
    packet
        .push(authority_b.into_capability(), rights_b)
        .map_err(|_| "consumer B authority packet failed")?;
    for management in [&primary.management, &primary.management_b] {
        let management = management
            .as_ref()
            .ok_or("consumer management root missing")?;
        let rights = management
            .description()
            .map_err(|_| "consumer management query failed")?
            .rights
            & !Rights::GRANT;
        let handle = duplicate(management.as_handle(), rights)
            .map_err(|_| "consumer management duplicate failed")?;
        // SAFETY: 本次 duplicate 交付独占未映射 owner，由 Packet 消费或失败收束。
        packet
            .push(unsafe { Capability::from_raw(handle) }, rights)
            .map_err(|_| "consumer management packet failed")?;
    }
    packet
        .try_send(
            root.sender(inbox.peer),
            libkernel::time::timeout_millis(10_000)
                .map_err(|_| "consumer authority deadline invalid")?,
        )
        .map_err(|_| "consumer authority delivery failed")?;
    Ok(FsConsumerCoordinator {
        role_sender: inbox.peer,
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
    grant_b: Option<MailboxSender>,
    directory_b: Option<MailboxSender>,
    registration_root_b: Option<MailboxSender>,
    management: Option<MailboxSender>,
    management_b: Option<MailboxSender>,
    bootstrap: Handle,
    release: Handle,
}

fn exercise_cancellable_rpc(root: &mut RootSupervisor) -> Result<(), &'static str> {
    let deadline = libkernel::time::timeout_millis(5_000)
        .map_err(|_| "FS1 cancellable RPC deadline failed")?;
    let event = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT,
    )
    .map_err(|_| "FS1 cancellable RPC Notification failed")?;
    let cancel = event.owner;
    event
        .peer
        .signal(1)
        .map_err(|_| "FS1 cancellable RPC signal failed")?;
    let blocked = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .map_err(|_| "FS1 cancellable RPC blocked mailbox failed")?;
    for index in 0..MAILBOX_CAPACITY {
        root.sender(blocked.peer)
            .send(0x4341_4e43_454c_0000 + index as u64, &[])
            .map_err(|_| "FS1 cancellable RPC mailbox fill failed")?;
    }
    let mut caller = librpc::Caller::new();
    let request = librpc::Request::new(protocol::ID, b"cancel")
        .map_err(|_| "FS1 cancellable RPC first request failed")?;
    let first = caller.call_cancelable(
        root.sender(blocked.peer),
        deadline,
        request,
        Some(cancel.as_handle()),
    );
    let Err(mut first) = first else {
        return Err("FS1 cancellable RPC did not return unsent owner");
    };
    if first.phase != librpc::CallPhase::Unsent
        || first.cause != librpc::CallCause::Cancelled
        || !first.has_unretired_owners()
    {
        return Err("FS1 cancellable RPC unsent phase invalid");
    }
    drop(
        first
            .take_unsent_request()
            .ok_or("FS1 cancellable RPC request missing")?,
    );
    if first.has_unretired_owners() {
        return Err("FS1 cancellable RPC request owner not released");
    }
    root.close_control(blocked.owner)?;
    root.close_control(blocked.peer)?;

    let sent = root
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .map_err(|_| "FS1 cancellable RPC sent mailbox failed")?;
    let request = librpc::Request::new(protocol::ID, b"cancel")
        .map_err(|_| "FS1 cancellable RPC second request failed")?;
    cancel
        .take(u64::MAX)
        .map_err(|_| "FS1 cancellation reset failed")?;
    let mut operation = librpc::CallOperation::new(
        &mut caller,
        root.sender(sent.peer),
        deadline,
        request,
        Some(cancel.as_handle()),
    )
    .map_err(|_| "FS1 cancellable RPC preparation failed")?;
    if operation
        .advance()
        .map_err(|_| "FS1 cancellable RPC send failed")?
        .is_some()
        || operation.phase() != Some(librpc::CallPhase::Sent)
    {
        return Err("FS1 cancellable RPC did not enter Sent");
    }
    // 取消在真实运输提交后发布；不再以预先有信号的空 mailbox 偶然进入 Sent。
    event
        .peer
        .signal(1)
        .map_err(|_| "FS1 sent cancellation signal failed")?;
    let cause = operation
        .poll_ready()
        .expect_err("sent cancellation was ignored");
    let mut second = operation.abort(cause).expect("Sent operation lost owner");
    drop(operation);
    if second.phase != librpc::CallPhase::Sent
        || second.cause != librpc::CallCause::Cancelled
        || !second.has_unretired_owners()
    {
        return Err("FS1 cancellable RPC sent phase invalid");
    }
    second
        .retry_cleanup()
        .map_err(|_| "FS1 cancellable RPC reply owner close failed")?;
    if second.has_unretired_owners() {
        return Err("FS1 cancellable RPC reply owner not released");
    }
    root.close_control(sent.owner)?;
    root.close_control(sent.peer)?;
    cancel
        .close()
        .map_err(|_| "FS1 cancellable RPC owner close failed")?;
    // SAFETY: 本函数仍独占 Notification 的信号端点。
    event
        .peer
        .close()
        .map_err(|(_, error)| error)
        .map_err(|_| "FS1 cancellable RPC peer close failed")?;
    debug!("FS1 cancellable RPC phases passed");
    Ok(())
}

fn root_memory_pool() -> Handle {
    INIT_STARTUP
        .lock()
        .as_ref()
        .expect("init startup inputs are not initialized")
        .root_memory_pool
}

fn root_pool_allocated() -> Result<u64, SystemCallError> {
    let handle = duplicate(root_memory_pool(), Rights::READ)?;
    // SAFETY: duplicate 新建唯一 raw owner，本作用域不保留 alias，Drop 负责关闭。
    let pool = unsafe { MemoryPool::from_handle(handle) };
    Ok(pool.query()?.allocated)
}

#[cfg(not(feature = "acceptance-stress"))]
fn test_process_funding(job: Handle) -> Result<(), &'static str> {
    // 独立付款域只包含本次 Building 与一页 Map；全局 Root 还可能推进其他对象的异步退款。
    let pool = libkernel::memory_pool::derive(
        root_memory_pool(),
        libkernel::shared::memory_pool::MemoryPoolBudget {
            pages: 16,
            // 空间的区域账本、View 与预付退休记录需要 metadata；页预算不代替这些费用。
            metadata: 1024 * 1024 / libkernel::shared::memory_pool::METADATA_SLOT_BYTES,
        },
        Rights::READ | Rights::CREATE | Rights::DUPLICATE,
    )
    .map_err(|_| "process funding Pool admission failed")?;
    let baseline = pool
        .query()
        .map_err(|_| "process funding Pool query failed")?
        .allocated;
    if baseline != 0 {
        return Err("process funding Pool is not empty");
    }
    let wrong_kind = duplicate(job, Rights::CREATE).map_err(|_| "Job duplicate failed")?;
    let rejected = process::create(job, wrong_kind, SUPERVISOR_RIGHTS);
    let _ = unsafe { close(wrong_kind) };
    if !matches!(rejected, Err(SystemCallError::WrongObjectType)) {
        if let Ok(created) = rejected {
            let _ = unsafe { process::abandon_to_completion(created) };
        }
        return Err("ProcessCreate accepted non-Pool funding authority");
    }
    let no_create =
        duplicate(pool.as_handle(), Rights::READ).map_err(|_| "Pool duplicate failed")?;
    let denied = process::create(job, no_create, SUPERVISOR_RIGHTS);
    let _ = unsafe { close(no_create) };
    if !matches!(denied, Err(SystemCallError::RightsDenied)) {
        if let Ok(created) = denied {
            let _ = unsafe { process::abandon_to_completion(created) };
        }
        return Err("ProcessCreate accepted Pool without CREATE");
    }
    let created = process::create(job, pool.as_handle(), SUPERVISOR_RIGHTS).map_err(|error| {
        debug!("Funded process creation failed: {:?}", error);
        "funded process create failed"
    })?;
    let result = process::map(
        created.builder,
        0x1000,
        libkernel::shared::proc::PROCESS_PAGE_SIZE,
        libkernel::shared::proc::ProcessMapFlags::READ,
    );
    // 创建不消费付款 authority；创建后的 Building 可直接映射。
    let pool_alive = pool
        .query()
        .is_ok_and(|snapshot| snapshot.allocated > baseline);
    let cleanup = unsafe { process::abandon_to_completion(created) };
    if result.is_err() || !pool_alive || cleanup.is_err() {
        return Err("ProcessCreate funding or cleanup failed");
    }
    let after = pool
        .query()
        .map_err(|_| "process funding Pool final query failed")?
        .allocated;
    if after != baseline {
        debug!(
            "ProcessCreate funding mismatch: baseline={}, after={}",
            baseline, after
        );
        return Err("ProcessCreate charge did not refund");
    }
    debug!("process creation funding acceptance passed");
    Ok(())
}

fn take_thread_result<T>(outcome: ThreadOutcome<T, libthread::NoEnvironment>) -> T {
    match outcome {
        ThreadOutcome::Returned {
            value,
            leave: Ok(()),
        } => value,
        ThreadOutcome::Returned { leave: Err(()), .. }
        | ThreadOutcome::EnterFailed(())
        | ThreadOutcome::Incomplete => panic!("runtime worker did not complete"),
    }
}

fn main() {
    debug!("Hello, init!");
    let startup = libruntime::startup::take().expect("init startup context is missing");
    let (process, payload, mut grants) = startup.into_parts();
    let platform = platform::PlatformDescription::adopt(
        grants
            .take(initial::PLATFORM_DESCRIPTION)
            .expect("init must hold a platform description"),
        &mut grants,
    )
    .expect("initial platform description is invalid");
    let description = platform.description();
    debug!(
        "platform description admitted: DTB {} bytes, {} resource roots",
        description.header().dtb_bytes,
        description.header().resource_count
    );
    let root_job = grants
        .take(initial::ROOT_JOB)
        .map(Capability::into_raw)
        .expect("init must hold root JobControl");
    let reset_handle = grants
        .take(initial::SYSTEM_RESET)
        .map(Capability::into_raw)
        .expect("init must hold SystemReset authority");
    let root_memory_pool = grants
        .take(initial::ROOT_MEMORY_POOL)
        .map(Capability::into_raw)
        .expect("init must hold root MemoryPool authority");
    *INIT_STARTUP.lock() = Some(InitStartup {
        process,
        payload,
        root_memory_pool,
        _platform: platform,
    });
    let mut reset = Some(reset_handle);
    // 尚无服务责任时准备根承载；失败不会遗忘已启动服务。
    let mut root = loop {
        match RootSupervisor::prepare(&mut reset) {
            Ok(root) => break root,
            Err(error) => {
                debug!("root supervisor preparation failed: {:?}", error);
                if let Ok(deadline) = libkernel::time::timeout_millis(100) {
                    let _ = libkernel::time::sleep_until(deadline);
                }
            }
        }
    };
    let services = match root.create_job(root_job, root_memory_pool, JOB_FULL_RIGHTS) {
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
    if let Err(failure) = run(&mut root, services, root_job) {
        match failure {
            RunFailure::Message(stage) => debug!("init acceptance failed: {}", stage),
        }
        root.serve_forever();
    }
    let reset = root.take_reset();
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
            core::hint::black_box(&root);
            // SAFETY: reset 失败是不可恢复终局；内核稳定持 DMA 责任，
            // 用户析构不承担硬件停止。boot 的预付 DONE 观察接管最后停驻。
            unsafe { libkernel::sys_process_terminate() }
        }
        Ok(never) => match never {},
    }
}

/// 小集合收束辅助：对保留 control 的服务 kill → 等 DONE → 核验终态 → close。
fn kill_and_supervise(
    root: &mut RootSupervisor,
    _job: Handle,
    supervised: alloc::vec::Vec<Supervised>,
) -> Result<(), &'static str> {
    for target in &supervised {
        let _ = process::kill(target.control);
    }
    root.collect_targets(supervised)
}

fn test_memory_mapping() -> Result<(), &'static str> {
    #[cfg(not(feature = "acceptance-stress"))]
    let pool_baseline = root_pool_allocated().map_err(|_| "memory Pool baseline query failed")?;
    let page = libkernel::shared::proc::PROCESS_PAGE_SIZE;
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
        || snapshot.write_permits != 0
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
        .write_permits
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
    if snapshot.state() != Some(MemoryObjectState::Sealing) || snapshot.write_permits != 2 {
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
    if snapshot.state() != Some(MemoryObjectState::Executable) || snapshot.write_permits != 0 {
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
    let no_execute_handle = duplicate(executable.handle(), Rights::READ)
        .map_err(|_| "non-execute object capability derive failed")?;
    let rx_handle = duplicate(executable.handle(), Rights::READ | Rights::EXECUTE)
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
    let rejected = { let mut pair = libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */ unsafe { libkernel::sys_notification_create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT, &mut pair) }.map(|()| pair) }
    .expect("RPC rejected Handle notification create failed");
    let rejected_alias = rejected.peer;
    let worker = libruntime::thread::Builder::new()
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
                        .try_reply(reply_once, libkernel::time::Deadline::INFINITE)
                        .expect("RPC rejection response publication failed");
                    continue;
                }

                let context = RequestContext::decode(request, PROTOCOL)
                    .map_err(|rejected| rejected.reason)
                    .expect("RPC test request context invalid");
                let mut outbox = Outbox::prepare(
                    context,
                    1,
                    libkernel::time::Deadline::INFINITE,
                    1,
                    &libexecution::SourceFunding::Task,
                )
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
            libkernel::time::Deadline::INFINITE,
            Request::new(PROTOCOL, b"reject").expect("RPC request preparation failed")
        ),
        Err(CallError {
            phase: CallPhase::Sent,
            cause: CallCause::Frame(FrameRejection::ProtocolMismatch),
            ..
        })
    ));
    assert!(matches!(
        /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */
        unsafe { libkernel::sys_notification_signal(rejected_alias, 1) },
        Err(SystemCallError::StaleHandle)
    ));
    let reply = caller
        .call(
            &service_sender,
            libkernel::time::Deadline::INFINITE,
            Request::new(PROTOCOL, b"accept").expect("RPC request preparation failed"),
        )
        .expect("RPC call after rejected reply failed");
    assert_eq!(reply.payload, [1]);
    assert!(reply.handles.is_empty());

    take_thread_result(worker.join());
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
        requests: &mut Requests<'_, Self>,
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

    fn deadline(&self) -> libkernel::time::Deadline {
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
    let mut assembly = libexecution::assembly::Assembly::new(set, &account)
        .map_err(|(_, error)| error)
        .expect("RPC Outbox Assembly creation failed");
    let mut runtime =
        Runtime::<RpcOutboxTask, WaitSet>::new(assembly.bind().unwrap(), 1, 1, &account)
            .expect("RPC Outbox Runtime creation failed");
    runtime
        .spawn(libexecution::TaskSubmission::new(
            RpcOutboxTask { outbox },
            1,
            account.clone(),
        ))
        .map_err(|failure| failure.error)
        .expect("RPC Outbox task admission failed");
    let mut world = RpcOutboxWorld::default();
    assembly
        .run(&mut runtime, &mut world, 1)
        .expect("RPC Outbox Runtime failed");
    let result = world.result.expect("RPC Outbox lost terminal result");
    runtime
        .close()
        .map_err(|(_, error)| error)
        .expect("RPC Outbox Runtime close failed");
    assembly
        .close()
        .map_err(|(_, error)| error)
        .expect("RPC Outbox Assembly close failed");
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
fn run(root: &mut RootSupervisor, services: Handle, root_job: Handle) -> Result<(), RunFailure> {
    debug!("acceptance workload: {}", ACCEPTANCE_WORKLOAD);
    #[cfg(feature = "acceptance-memory")]
    if ACCEPTANCE_WORKLOAD == "memory" {
        return run_memory(root, services).map_err(RunFailure::from);
    }
    test_memory_mapping()?;
    let ecam = INIT_STARTUP
        .lock()
        .as_ref()
        .expect("init startup inputs missing")
        ._platform
        .ecam_handle();
    if let Some(ecam) = ecam {
        mmio_checks::run(ecam).map_err(RunFailure::from)?;
    }
    let irq = INIT_STARTUP
        .lock()
        .as_ref()
        .expect("init startup inputs missing")
        ._platform
        .irq_handle();
    if let Some(irq) = irq {
        irq_checks::run(irq).map_err(RunFailure::from)?;
    }
    let dma = INIT_STARTUP
        .lock()
        .as_ref()
        .expect("init startup inputs missing")
        ._platform
        .dma_handle();
    if let Some(dma) = dma {
        dma_checks::run(dma).map_err(RunFailure::from)?;
    }
    #[cfg(not(feature = "acceptance-stress"))]
    test_process_funding(services)?;
    let pm_domain = root
        .create_job(services, root_memory_pool(), JOB_FULL_RIGHTS)
        .map_err(|_| "pm domain job create failed")?;
    let acceptance = root
        .create_job(services, root_memory_pool(), JOB_FULL_RIGHTS)
        .map_err(|_| "acceptance job create failed")?;
    root.verify_startup_capture()?;
    let mut names = TopologyNames::new();
    names.register_job(root_job, "root");
    names.register_job(services, "services");
    names.register_job(pm_domain, "pm_domain");
    names.register_job(acceptance, "acceptance");
    names.register_process(init_pid() as u64, "init");
    public_ipc::shared_sender();
    public_ipc::notification_roles();
    test_pool_budget();
    let launched = launch_test_services(root, services, pm_domain, acceptance, &mut names)?;
    once_checks::run(
        root,
        acceptance,
        launched
            .target_image
            .ok_or("Once target image missing")?
            .as_slice(),
    )?;
    #[cfg(feature = "acceptance-device-function-session")]
    {
        let manager = root.devices.take().ok_or("device manager unavailable")?;
        let result = function_checks::run(root, &manager);
        root.devices = Some(manager);
        result?;
    }
    #[cfg(feature = "acceptance-device")]
    let driver_reply_backlog = {
        let manager = root
            .devices
            .take()
            .ok_or("device manager bootstrap incomplete")?;
        let sources = manager.issue_function_source(root);
        root.devices = Some(manager);
        let sources = sources?;
        let drivers = root
            .drivers
            .as_ref()
            .ok_or("driver assembly record missing")?
            .observer_handle()?;
        device_acceptance::exercise(root, acceptance, sources, drivers)?
    };
    let pm_mailbox = launched.pm_mailbox;
    let mut first_fs = launched.first_fs;
    let registry_watch = launched.registry_root_watch;
    confirm_primary_fs_ready(root, &first_fs)?;
    root.verify_dispatcher_funding([
        first_fs
            .directory
            .as_ref()
            .ok_or("primary registry A missing")?,
        first_fs
            .directory_b
            .as_ref()
            .ok_or("primary registry B missing")?,
    ])?;
    // primary 探针只借已就绪文件根，不依赖独立消费者或 secondary。
    exercise_fs_provider(
        root,
        first_fs.grant.as_ref().ok_or("primary file root missing")?,
    )?;
    exercise_async_name_watch(
        root,
        first_fs
            .directory
            .as_ref()
            .ok_or("primary service directory missing")?,
        first_fs
            .registration_root
            .as_ref()
            .ok_or("publication root missing")?,
        first_fs.grant.as_ref().ok_or("primary file root missing")?,
    )?;
    filesystem::registry::queued_control(
        root,
        first_fs
            .directory
            .as_ref()
            .ok_or("primary service directory missing")?,
        first_fs
            .registration_root
            .as_ref()
            .ok_or("publication root missing")?,
        first_fs.grant.as_ref().ok_or("primary file root missing")?,
    )?;
    let consumer = launch_fs_consumer(
        root,
        services,
        &mut names,
        launched.test_fs_image.as_slice(),
        &first_fs,
    )?;
    consumer.await_report(root, test_fs::report::DISCOVERY_ARMED)?;
    launch_secondary_fs(
        root,
        services,
        &mut names,
        launched.fs_second_image.as_slice(),
        [
            launched.fs_bootstrap_second_sender,
            launched.fs_release_second_owner,
            launched.fs_route_second_owner,
        ],
        launched.fs_second_scope,
    )?;
    let mut second_fs = accept_fs_provider(
        root,
        launched.fs_bootstrap_second,
        launched.fs_release_second,
        false,
    )?;
    let directory = first_fs
        .directory
        .as_ref()
        .ok_or("primary service directory missing")?;
    let primary_mailbox = first_fs
        .mailbox_identity
        .ok_or("primary file mailbox identity missing")?;
    let (entry, sender_identity, mailbox_identity) =
        discover_registered_fs(root, directory, primary_mailbox, &registry_watch)?;
    second_fs.grant = Some(entry);
    second_fs.sender_identity = Some(sender_identity);
    second_fs.mailbox_identity = Some(mailbox_identity);
    consumer.await_report(root, test_fs::report::SECONDARY_DISCOVERED)?;
    exercise_fs_basic(
        root,
        second_fs
            .grant
            .as_ref()
            .ok_or("secondary file root missing")?,
    )?;
    consumer.deliver_management(
        root,
        second_fs
            .management
            .as_ref()
            .ok_or("secondary management root missing")?,
    )?;
    consumer.continue_business(root)?;
    consumer.await_report(root, test_fs::report::COMPLETE)?;
    filesystem::copy::exercise(
        root,
        first_fs.grant.as_ref().ok_or("primary file root missing")?,
        second_fs
            .grant
            .as_ref()
            .ok_or("secondary file root missing")?,
    )?;
    exercise_cancellable_rpc(root)?;
    filesystem::copy::start_shutdown(
        root,
        first_fs.grant.as_ref().ok_or("primary file root missing")?,
        second_fs
            .grant
            .as_ref()
            .ok_or("secondary file root missing")?,
    )?;
    root.notification_signaler(second_fs.release)
        .signal(1)
        .map_err(|_| "secondary release failed")?;
    filesystem::copy::finish_shutdown(
        root,
        first_fs.grant.as_ref().ok_or("primary file root missing")?,
    )?;
    consumer.observe_shutdown(root)?;
    consumer.await_report(root, test_fs::report::PROVIDER_CLOSED)?;
    consumer.finish(root)?;
    collect_fs_provider(root, &second_fs)?;
    observe_withdrawal(root, directory, &registry_watch)?;
    drop(registry_watch);
    drop(second_fs.grant.take());
    drop(second_fs.management.take());
    drop(second_fs.management_b.take());
    drop(first_fs.grant.take());
    drop(first_fs.directory.take());
    drop(first_fs.registration_root.take());
    drop(first_fs.grant_b.take());
    drop(first_fs.directory_b.take());
    drop(first_fs.registration_root_b.take());
    drop(first_fs.management.take());
    drop(first_fs.management_b.take());
    release_fs_provider(root, &first_fs)?;
    // 两个真实provider业务已经关闭；把原control一起交原批次，覆盖部分准入/失败隔离。
    root.verify_failure_isolation(acceptance, [first_fs.pid, second_fs.pid])?;
    debug!("FS1 provider supervision reclaimed both provider processes");
    let target_image = launched.target_image;
    let hammer_image = launched.hammer_image;

    // 设备和文件消费者已完成，先履行两管理者的正常停止责任；独立内核ABI自检随后执行。
    root.stop_drivers()?;
    #[cfg(feature = "acceptance-device")]
    {
        debug!("DEVICE Drivers stopped while Observer reply backlog remained unread");
        drop(driver_reply_backlog);
    }
    root.stop_devices()?;

    // 运行时拓扑快照（调试参考）：此刻服务在域内运行，验收自测尚未
    // 展开；短寿命服务可能已自动完成退休。
    debug!("topology: runtime snapshot after service launch");
    dump_topology(root_job, &names, 0);
    let pair = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("mailbox create failed");

    let event = { let mut pair = libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */ unsafe { libkernel::sys_notification_create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT | Rights::DUPLICATE, &mut pair) }.map(|()| pair) }
    .expect("notification create failed");
    let moves = [HandleMove {
        handle: event.peer,
        rights: Rights::SIGNAL,
    }];

    // —— 同步消息 + Handle move + Notification 单来源快路径 ——
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
                /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */
                unsafe { libkernel::sys_notification_signal(moved, 0x5) }
                    .expect("notification signal failed");
                let _result = wait_until(
                    event.owner,
                    ObjectSignals::READABLE,
                    libkernel::time::timeout_millis(0).unwrap(),
                )
                .expect("notification wait failed");
                let bits =
                    { let mut bits = 0; /* SAFETY: ABI 夹具借用有效 handle，输出在同步调用期间私有。 */ unsafe { libkernel::sys_notification_take(event.owner, u64::MAX, &mut bits) }.map(|()| bits) }.expect("notification take failed");
                debug!("notification: bits={:#x}", bits);
            }
            Err(e) => debug!("receive failed: {:?}", e),
        },
        Err(e) => debug!("send failed: {:?}", e),
    }
    let _ = unsafe { close(event.owner) };
    let _ = unsafe { close(pair.peer) };
    let _ = unsafe { close(pair.owner) };

    test_tunnel_failure_owners();
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
    public_ipc::committed_kill(
        acceptance,
        target_image.expect("IPC target image missing").as_slice(),
    );

    // —— 数据面：建隧道 → Invitation 经消息面转移 → 阻塞读流 ——
    let (tunnel, invitation) = match blocking::Consumer::create(
        root_memory_pool(),
        TUNNEL_BYTES,
        libkernel::mm::Placement::Anywhere,
    ) {
        Ok((consumer, invitation)) => (consumer, invitation),
        Err(blocking::CreateFailure::Tunnel(
            libkernel::ipc::invitation::CreateFailure::System(error),
        )) => {
            debug!("tunnel create failed: {:?}", error);
            return Err(RunFailure::Message("tunnel create failed"));
        }
        Err(blocking::CreateFailure::Tunnel(
            libkernel::ipc::invitation::CreateFailure::Published {
                endpoint,
                invitation,
                error,
            },
        )) => {
            root.retain_failed_create(endpoint, invitation);
            debug!("tunnel create output invalid: {:?}", error);
            return Err(RunFailure::Message("tunnel create output invalid"));
        }
        Err(blocking::CreateFailure::Map {
            failure,
            invitation,
        }) => {
            root.retain_failed_create(Some(failure.endpoint), Some(invitation));
            return Err(RunFailure::Message("tunnel mapping failed"));
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
    if let Err(failure) = packet.push(
        invitation.into_capability(),
        Rights::READ | Rights::WRITE | Rights::WAIT | Rights::SIGNAL | Rights::MANAGE,
    ) {
        root.retain_capability(failure.capability);
        return Err(RunFailure::Message("tunnel invitation push failed"));
    }
    if let Err(mut failure) =
        packet.try_send(root.sender(pm_mailbox), libkernel::time::Deadline::INFINITE)
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

    // —— live kill 正路径：Building 目标的确定性 Kill/自动退休/终态验证 ——
    test_building_kill(acceptance);

    // JobKill 组合与 seal 闸门可行子集 ——
    match target_image {
        Some(image) => test_job_management(root, acceptance, image.as_slice())?,
        None => {
            return Err(RunFailure::Message(
                "job management acceptance image unavailable",
            ));
        }
    }

    // 短寿命服务的线程已退出并不释放 AddressSpace；在高峰矩阵前收束已进入
    // Terminating/Dead 的成员，仍 Running 的服务继续由末尾监督闭环持有。
    let reclaimed = root.collect_terminated()?;
    debug!(
        "pre-race service supervision reclaimed {} process(es)",
        reclaimed
    );

    // 完整生命周期多核竞态矩阵属于 stress workload；core 仍覆盖确定性的
    // Building kill、Job 管理与服务监督收束。
    #[cfg(feature = "acceptance-stress")]
    match (target_image, hammer_image) {
        (Some(target), Some(hammer)) => {
            race_matrix(root, acceptance, target.as_slice(), hammer.as_slice())?
        }
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
    root.collect_job(acceptance)?;
    debug!("acceptance domain collected");
    root.close_control(acceptance)?;

    // —— 监督闭环：等待全部服务 DONE 后，核验终态并关闭观察权，
    // 查询稳定终态后释放 control。pm 隧道端点在进程自动退休时关闭并
    // 发布 PEER_CLOSED；先完成服务监督，再观察对端终态。 ——
    root.collect_services()?;
    debug!("all services supervised to completion");

    // —— 事件面：自动退休已发布对端终态，电平等待立即返回 ——
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
        root.collect_job(pm_domain)?;
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
    debug!("topology: final snapshot before system reset");
    dump_topology(root_job, &names, 0);
    root.close_control(pm_domain)?;
    root.verify_idle_ownership()?;
    Ok(())
}

/// 复用完整 stress 的正式线程/映射消费者及根监督器，供发布窗口旁观路线使用。
#[cfg(feature = "acceptance-memory")]
fn run_memory(root: &mut RootSupervisor, services: Handle) -> Result<(), &'static str> {
    let mut image = None;
    with_init_payload(|payload| {
        tar::walk(payload, |entry| {
            if entry.name == "bin/test_hammer" {
                image = Some(StartupImage::from_slice(entry.data));
            }
        })
    })
    .map_err(|_| "memory workload initfs parse failed")?;
    let image = image.ok_or("memory workload hammer image missing")?;
    let job = root
        .create_job(services, root_memory_pool(), JOB_FULL_RIGHTS)
        .map_err(|_| "memory workload job create failed")?;
    if !race::thread_memory_suite(job, image.as_slice()) {
        return Err("memory workload thread suite failed");
    }
    root.collect_job(job)?;
    root.close_control(job)?;
    root.verify_idle_ownership()?;
    debug!("memory workload collected");
    Ok(())
}

/// 递归打印 Job/Process 拓扑（调试参考）：直接成员经派生 control 查询
/// 生命周期快照后即关，child Job 派生 JobControl 下钻——纯观察，不消费
/// authority、不改状态。青色 topology 标记便于在灰度用户态日志中定位
/// （libkernel debug! 元许在消息内自拼 ANSI）。
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
                exit_reason_name(snapshot.cause),
                snapshot.detail.word0
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
#[derive(Default)]
struct ReadWorld {
    tunnel: Option<blocking::Consumer>,
    buffer: alloc::vec::Vec<u8>,
    filled: usize,
    failed: bool,
    rpc: Option<librpc::dispatcher::Dispatcher>,
    rpc_task: Option<u64>,
    client_quiescent: bool,
    rpc_complete: bool,
    client_armed: bool,
    client_cycles: u64,
    source_detached: bool,
    fs_snapshot: Option<libfs::protocol::WatchInfo>,
    service_snapshot: Option<libservice::protocol::WatchInfo>,
    client_tasks: alloc::vec::Vec<u64>,
    copies: alloc::vec::Vec<CopyObservation>,
    rpc_sealed: bool,
    rpc_parked: bool,
    ignored_rpc_replies: u64,
}

/// 单个 Copy 客户的观测与编排状态；真实会话和清理责任留在 CopyTask。
struct CopyObservation {
    outcome: Option<libfs::client::copy::Outcome>,
    progress: libfs::client::copy::Progress,
    done: bool,
    hold: bool,
}

impl ReadWorld {
    /// 客户保留在任务表，直到共享运输来源实际退出并唤醒全部客户。
    fn await_transport(&mut self, requests: &mut Requests<'_, InitTask>) -> Step {
        if self.client_quiescent {
            self.rpc
                .as_mut()
                .expect("client transport remains owned")
                .stop(&mut ());
        }
        if self.rpc_complete {
            Step::Complete
        } else if requests
            .wake(self.rpc_task.expect("transport remains admitted"))
            .is_err()
        {
            Step::Runnable
        } else {
            Step::Parked
        }
    }
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
    Rpc {
        deadline: Deadline,
    },
    FsWatch(alloc::boxed::Box<librpc::subscription::ConsumerTask<libfs::client::Watch>>),
    ServiceWatch(alloc::boxed::Box<librpc::subscription::ConsumerTask<libservice::client::Watch>>),
    Copy {
        task: alloc::boxed::Box<libfs::client::copy::CopyTask>,
        slot: usize,
    },
}

impl Task<ReadWorld> for StreamReadTask {
    type Family = InitTask;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut ReadWorld,
        requests: &mut Requests<'_, InitTask>,
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

            match tunnel.poll(event.observed) {
                Ok(Some(ConsumerReady::Started))
                | Ok(Some(ConsumerReady::Readable {
                    peer_started: true, ..
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
            let source = source.expect("Started event lost its source");
            requests.remove(source)?;
            self.source = None;
        } else if let Some(source) = source {
            self.source = Some(source);
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
                .register(plan, KIND_STREAM_READ)
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
        requests: &mut Requests<'_, InitTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        match self {
            Self::StreamRead(task) => task.advance(id, world, requests, input, budget),
            Self::Rpc { deadline } => {
                let rpc = world.rpc.as_mut().expect("client transport remains owned");
                let mut advance = rpc.advance(requests, input, budget)?;
                world.ignored_rpc_replies = rpc.ignored_reply_count();
                *deadline = rpc.deadline();
                world.rpc_parked = advance.step == Step::Parked;
                if advance.step == Step::Complete {
                    if world.client_quiescent {
                        if world
                            .client_tasks
                            .iter()
                            .any(|&id| requests.wake(id).is_err())
                        {
                            advance.step = Step::Runnable;
                        } else {
                            world.rpc_complete = true;
                        }
                    } else {
                        advance.step = Step::Parked;
                    }
                }
                Ok(advance)
            }
            Self::FsWatch(task) => {
                let mut advance = task.advance(
                    id,
                    world.rpc_task.expect("transport task remains admitted"),
                    world.rpc.as_mut().expect("client transport remains owned"),
                    requests,
                    input,
                    budget,
                );
                world.fs_snapshot = task.snapshot();
                if !world.failed
                    && let Some(failure) = task.failure()
                {
                    debug!("file watch consumer failed: {:?}", failure);
                }
                world.failed |= task.failure().is_some();
                world.client_armed = task.armed();
                world.client_cycles = task.cycles();
                world.source_detached = task.source_is_detached();
                if advance.step == Step::Complete {
                    world.client_quiescent = true;
                    advance.step = world.await_transport(requests);
                }
                Ok(advance)
            }
            Self::ServiceWatch(task) => {
                let mut advance = task.advance(
                    id,
                    world.rpc_task.expect("transport task remains admitted"),
                    world.rpc.as_mut().expect("client transport remains owned"),
                    requests,
                    input,
                    budget,
                );
                world.service_snapshot = task.snapshot();
                if !world.failed
                    && let Some(failure) = task.failure()
                {
                    debug!("service watch consumer failed: {:?}", failure);
                }
                world.failed |= task.failure().is_some();
                world.client_armed = task.armed();
                world.client_cycles = task.cycles();
                world.source_detached = task.source_is_detached();
                if advance.step == Step::Complete {
                    world.client_quiescent = true;
                    advance.step = world.await_transport(requests);
                }
                Ok(advance)
            }
            Self::Copy { task, slot } => {
                // init 的短期编排窗口保留真实 waiter；期限仍由原任务约束。
                if world.copies[*slot].hold
                    && task.in_flight()
                    && !libkernel::time::expired(task.deadline())?
                {
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Parked,
                    });
                }
                let mut advance = task.advance(
                    id,
                    world.rpc_task.expect("transport task remains admitted"),
                    world.rpc.as_mut().expect("Copy transport remains owned"),
                    requests,
                    input,
                    budget,
                );
                if !world.copies[*slot].done && advance.step == Step::Complete
                    || world.copies[*slot].outcome.is_none()
                        && matches!(
                            task.outcome(),
                            Some(libfs::client::copy::Outcome::Failed { .. })
                        )
                {
                    debug!("Copy task {} transition: {:?}", slot, task);
                }
                world.copies[*slot].outcome = task.outcome();
                world.copies[*slot].progress = task.progress();
                if advance.step == Step::Complete {
                    world.copies[*slot].done = true;
                    world.client_quiescent = world.copies.iter().all(|copy| copy.done);
                    advance.step = world.await_transport(requests);
                }
                Ok(advance)
            }
        }
    }

    fn registered(&mut self, world: &mut ReadWorld, kind: SourceKind, source: SourceId) {
        match self {
            Self::StreamRead(task) => task.registered(world, kind, source),
            Self::Rpc { .. } => world
                .rpc
                .as_mut()
                .expect("client transport remains owned")
                .registered(&mut (), kind, source),
            Self::FsWatch(task) => task.registered(kind, source),
            Self::ServiceWatch(task) => task.registered(kind, source),
            Self::Copy { task, .. } => task.registered(kind, source),
        }
    }
    fn unregistered(&mut self, world: &mut ReadWorld, kind: SourceKind, source: SourceId) {
        match self {
            Self::StreamRead(task) => task.unregistered(world, kind, source),
            Self::Rpc { .. } => world
                .rpc
                .as_mut()
                .expect("client transport remains owned")
                .unregistered(&mut (), kind, source),
            Self::FsWatch(task) => task.unregistered(source),
            Self::ServiceWatch(task) => task.unregistered(source),
            Self::Copy { task, .. } => task.unregistered(source),
        }
    }

    fn refused(&mut self, world: &mut ReadWorld, failure: RequestFailure<InitTask>) {
        match self {
            Self::StreamRead(task) => task.refused(world, failure),
            Self::Rpc { .. } => world
                .rpc
                .as_mut()
                .expect("client transport remains owned")
                .refused(&mut (), failure),
            Self::FsWatch(task) => task.refused(failure),
            Self::ServiceWatch(task) => task.refused(failure),
            Self::Copy { task, .. } => task.refused(failure),
        }
    }

    fn stop(&mut self, world: &mut ReadWorld) {
        match self {
            Self::StreamRead(task) => task.stop(world),
            Self::Rpc { .. } => world
                .rpc
                .as_mut()
                .expect("client transport remains owned")
                .stop(&mut ()),
            Self::FsWatch(task) => task.stop(),
            Self::ServiceWatch(task) => task.stop(),
            Self::Copy { task, slot } => {
                // 停止撤销编排暂停；Stopping 的任务不再靠业务期限重新武装。
                world.copies[*slot].hold = false;
                task.stop();
            }
        }
    }

    fn deadline(&self) -> libkernel::shared::time::Deadline {
        match self {
            Self::StreamRead(_) => Deadline::INFINITE,
            Self::Rpc { deadline } => *deadline,
            Self::FsWatch(task) => task.deadline(),
            Self::ServiceWatch(task) => task.deadline(),
            Self::Copy { task, .. } => task.deadline(),
        }
    }
}

fn wait_supervision_signal(
    tunnel: &mut blocking::Consumer,
    label: &'static str,
) -> Result<libkernel::shared::wait::WaitResult, &'static str> {
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
/// (Killed, code)，builder 关闭的 abandonment 竞争不覆盖；自动退休完成后
/// shell 快照应稳定报 Dead/Killed/code。
fn test_building_kill(job: Handle) {
    let created = match process::create(job, root_memory_pool(), SUPERVISOR_RIGHTS) {
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
    if let Err(error) = process::kill(created.control) {
        debug!("building kill: kill failed: {:?}", error);
        let _ = unsafe { close(created.builder) };
        let _ = unsafe { close(created.control) };
        return;
    }
    let _ = unsafe { close(created.builder) };
    let drained = wait_until(
        created.control,
        ObjectSignals::DONE,
        libkernel::time::Deadline::INFINITE,
    );
    let snapshot = process::query(created.control);
    match (drained, snapshot) {
        (Ok(_), Ok(snapshot))
            if snapshot.state == ProcessState::Dead as u32
                && snapshot.cause == TerminationCause::Killed as u32
                && snapshot.detail
                    == libkernel::shared::proc::TerminationDetail::PROCESS_CONTROL_KILL =>
        {
            debug!(
                "building kill passed: pid {} Dead/Killed/{:#x}",
                created.pid, snapshot.detail.word0
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
            let mut exhausted = DEFAULT_SUPERVISION_POLICY;
            exhausted.query_attempts = 0;
            let failure = collect_process(SupervisionTarget::new(pid, control), exhausted)
                .expect_err("invalid query policy must retain supervision authority");
            assert_eq!(failure.stage, SupervisionStage::WaitDone);
            assert_eq!(failure.cause, SupervisionCause::InvalidPolicy);
            assert_eq!(failure.progress.query_attempts, 0);
            debug!(
                "supervision policy rejection retained authority: pid={}, queries={}",
                pid, failure.progress.query_attempts
            );
            let mut machine = failure.collector;
            let mut finite = DEFAULT_SUPERVISION_POLICY;
            finite.wait_timeout_ms = 1;
            finite.wait_attempts = 1;
            machine.replenish(finite);
            let failure = loop {
                let now = libkernel::time::snapshot()
                    .expect("finite collection clock failed")
                    .now_ns;
                match machine.step(now) {
                    libprocess::StepOutcome::Continue(next) => machine = next,
                    libprocess::StepOutcome::RetryAt(at, next) => {
                        root.sleep_until(libkernel::shared::time::Deadline::at(at))
                            .expect("finite collection retry wait failed");
                        machine = next;
                    }
                    libprocess::StepOutcome::Observe(request, mut next) => {
                        let observed = root
                            .wait_until(request.control, request.signals, request.deadline)
                            .expect("finite collection observation failed");
                        let now = libkernel::time::snapshot()
                            .expect("finite collection clock failed")
                            .now_ns;
                        let result = if observed.observed.raw() == 0 {
                            libprocess::observation::ObservationResult::TimedOut
                        } else {
                            libprocess::observation::ObservationResult::Ready(observed.observed)
                        };
                        next.observe(request, result, now)
                            .expect("finite collection observation rejected");
                        machine = next;
                    }
                    libprocess::StepOutcome::Escalate(failure) => break failure,
                    libprocess::StepOutcome::Done(_) => {
                        panic!("live target unexpectedly completed")
                    }
                }
            };
            assert_eq!(failure.stage, SupervisionStage::WaitDone);
            assert_eq!(failure.cause, SupervisionCause::Timeout);
            assert_eq!(failure.progress.wait_attempts, 1);
            assert_eq!(failure.progress.query_attempts, 0);
            assert_eq!(
                process::query(failure.collector.wait_control())
                    .unwrap()
                    .pid,
                pid
            );
            debug!(
                "supervision budget exhaustion retained authority: pid={}, waits={}, queries={}",
                pid, failure.progress.wait_attempts, failure.progress.query_attempts
            );
            // 超时交回同一机器和 authority；Kill 后续作不重建完成责任。
            process::kill(failure.collector.wait_control())
                .expect("derived-control kill must be accepted");
            let mut machine = failure.collector;
            machine.replenish(DEFAULT_SUPERVISION_POLICY);
            let collected = root.collect_machine(machine)?;
            assert_eq!(collected.snapshot.cause, TerminationCause::Killed as u32);
            assert_eq!(
                collected.snapshot.detail,
                libkernel::shared::proc::TerminationDetail::PROCESS_CONTROL_KILL
            );
            debug!(
                "pid {} supervised: queries={}, state={}, cause={}, detail={}",
                pid,
                collected.progress.query_attempts,
                collected.snapshot.state,
                collected.snapshot.cause,
                collected.snapshot.detail.word0
            );
        }
        Err(error) => {
            debug!("derive kill FAILED: enumerate/derive {:?}", error);
            process::kill(retained).expect("live kill of a fresh process must be accepted");
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
    let ecam = INIT_STARTUP
        .lock()
        .as_ref()
        .expect("init startup inputs missing")
        ._platform
        .ecam_handle();
    if let Some(ecam) = ecam {
        mmio_checks::process_retirement(ecam, job, image)?;
    }
    #[cfg(feature = "acceptance-irq-exit")]
    {
        let irq = INIT_STARTUP
            .lock()
            .as_ref()
            .expect("init startup inputs missing")
            ._platform
            .irq_handle();
        if let Some(irq) = irq {
            irq_checks::process_retirement(irq, job, image)?;
        }
    }
    let dma = INIT_STARTUP
        .lock()
        .as_ref()
        .expect("init startup inputs missing")
        ._platform
        .dma_handle();
    if let Some(dma) = dma {
        dma_checks::process_retirement(dma, job, image)?;
    }
    #[cfg(feature = "acceptance-stress")]
    {
        test_automatic_retirement(job, image)
    }
    #[cfg(not(feature = "acceptance-stress"))]
    {
        Ok(())
    }
}

/// 含大量 WaitSet 成员的进程退出后，观察 DONE 核验独立退休完成。
#[cfg(feature = "acceptance-stress")]
fn test_automatic_retirement(job: Handle, image: &[u8]) -> Result<(), &'static str> {
    let marker = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        Rights::SIGNAL | Rights::WAIT,
    )
    .map_err(|_| "automatic retirement source creation failed")?;
    let observer = marker
        .owner
        .mint_observer(Rights::WAIT | Rights::DUPLICATE | Rights::GRANT)
        .map_err(|_| "automatic retirement observer mint failed")?;
    let child_observer = observer
        .duplicate(Rights::WAIT | Rights::GRANT)
        .map_err(|_| "automatic retirement observer duplicate failed")?;
    let ready = notification::create(Rights::READ | Rights::WAIT, Rights::SIGNAL | Rights::GRANT)
        .map_err(|_| "automatic retirement ready creation failed")?;
    let grants = [
        HandleGrant {
            handle: marker.owner.as_handle(),
            rights: Rights::READ | Rights::WAIT,
        },
        HandleGrant {
            handle: ready.peer.as_handle(),
            rights: Rights::SIGNAL,
        },
        HandleGrant {
            handle: child_observer.as_handle(),
            rights: Rights::WAIT,
        },
    ];
    let launched = spawn(SpawnRequest {
        memory_pool: root_memory_pool(),
        parent: init_process_info(),
        job,
        image,
        payload: b"retirement",
        grants: &grants,
        control_rights: SUPERVISOR_RIGHTS,
    });
    let started = match launched {
        Ok(started) => {
            marker.owner.into_raw();
            ready.peer.into_raw();
            child_observer.into_raw();
            started
        }
        Err(failure) => {
            if failure.grants == libprocess::GrantOutcome::Consumed {
                marker.owner.into_raw();
                ready.peer.into_raw();
                child_observer.into_raw();
            }
            return Err("automatic retirement spawn failed");
        }
    };
    let deadline =
        libkernel::time::timeout_millis(30_000).map_err(|_| "retirement deadline failed")?;
    let result = (|| {
        let ready_state = wait_until(ready.owner.as_handle(), ObjectSignals::READABLE, deadline)
            .map_err(|_| "automatic retirement target readiness failed")?;
        if !ready_state.observed.intersects(ObjectSignals::READABLE) {
            return Err("automatic retirement target readiness timed out");
        }
        process::kill(started.control).map_err(|_| "automatic retirement kill failed")?;
        let completion = wait_until(started.control, ObjectSignals::DONE, deadline)
            .map_err(|_| "automatic retirement wait failed")?;
        if !completion.observed.intersects(ObjectSignals::DONE) {
            return Err("automatic retirement completion timed out");
        }
        let snapshot = process::query(started.control).map_err(|_| "retirement snapshot failed")?;
        if snapshot.state != ProcessState::Dead as u32
            || snapshot.cause != TerminationCause::Killed as u32
        {
            return Err("automatic retirement terminal snapshot incorrect");
        }
        let observed = wait_until(observer.as_handle(), ObjectSignals::CLOSED, deadline)
            .map_err(|_| "retired Notification observer wait failed")?;
        if !observed.observed.intersects(ObjectSignals::CLOSED) {
            return Err("retired Notification owner remained alive");
        }
        let signaler = wait_until(marker.peer.as_handle(), ObjectSignals::CLOSED, deadline)
            .map_err(|_| "retired Notification signaler wait failed")?;
        if !signaler.observed.intersects(ObjectSignals::CLOSED)
            || marker.peer.signal(1) != Err(SystemCallError::ObjectClosed)
        {
            return Err("retired Notification signaler state incorrect");
        }
        debug!("automatic retirement acceptance passed: retired WaitSet with 256 registrations");
        debug!(
            "Notification observer process exit passed: owner closed with observer and signaler alive"
        );
        Ok(())
    })();
    if result.is_err() {
        let _ = process::kill(started.control);
        let _ = wait_until(started.control, ObjectSignals::DONE, deadline);
    }
    // SAFETY: 唯一父方 Control；退出/失败收束已等待同一完成来源。
    unsafe { close(started.control) }.map_err(|_| "automatic retirement control close failed")?;
    result
}

/// 验收线 2：封口与异步完成传播——空 child Job seal 后等待 CLOSED、
/// 快照转Dead；最后child capability关闭后父域仍可派生；只Seal父即收束空后代。
fn test_job_seal_completion(job: Handle) {
    let Ok(child) = process::create_job(job, root_memory_pool(), JOB_FULL_RIGHTS) else {
        debug!("job seal completion FAILED: job create failed");
        return;
    };
    let mut grandchild = None;
    let mut leaf = None;
    let nested = (|| {
        let grand = process::create_job(child, root_memory_pool(), JOB_FULL_RIGHTS)?;
        grandchild = Some(grand);
        leaf = Some(process::create_job(
            grand,
            root_memory_pool(),
            JOB_FULL_RIGHTS,
        )?);
        let jid = process::query_job(grand)?.jid;
        // 原最后cap归零：重新派生依据父MANAGE，而非子旧Holders资格。
        unsafe { close(grand)? };
        grandchild = None;
        let reminted = process::derive_job(child, JobMemberKind::ChildJobs, jid, JOB_FULL_RIGHTS)?;
        grandchild = Some(reminted);
        unsafe { close(reminted)? };
        grandchild = None;
        Ok::<_, SystemCallError>(())
    })();
    let sealed = process::seal_job(child);
    let waited = wait_until(
        child,
        ObjectSignals::CLOSED,
        libkernel::time::timeout_millis(5_000).unwrap(),
    );
    let snapshot = process::query_job(child);
    let leaf_snapshot = leaf.and_then(|handle| process::query_job(handle).ok());
    // 幂管：Dead 上重复 seal 成功且不改变状态。
    let resealed = process::seal_job(child);
    let resnapshot = process::query_job(child);
    let children = enumerate_members(job, JobMemberKind::ChildJobs);
    let passed = nested.is_ok()
        && leaf_snapshot.is_some_and(|snapshot| snapshot.state == JobState::Dead as u32)
        && sealed.is_ok()
        && matches!(waited, Ok(result) if result.observed.intersects(ObjectSignals::CLOSED))
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
    for handle in [leaf, grandchild].into_iter().flatten() {
        let _ = unsafe { close(handle) };
    }
    let _ = unsafe { close(child) };
}

/// Control 全消散不取消仍由 Builder 持有的组装；Job 枚举和派生接管终止。
fn test_derive_fallback(job: Handle) {
    let Ok(created) = process::create(job, root_memory_pool(), SUPERVISOR_RIGHTS) else {
        debug!("derive fallback FAILED: create failed");
        return;
    };
    let pid = created.pid;
    // SAFETY: 原 Control 在本函数独占，Builder 仍稳定持有组装责任。
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
            process::kill(control).expect("derived Control failed to stop Building target");
            // SAFETY: 冻结后退还唯一 Builder，自动退休不依赖其继续调用。
            let _ = unsafe { close(created.builder) };
            let drained = wait_until(
                control,
                ObjectSignals::DONE,
                libkernel::time::Deadline::INFINITE,
            );
            let snapshot = process::query(control);
            match (drained, snapshot) {
                (Ok(_), Ok(snapshot))
                    if snapshot.state == ProcessState::Dead as u32
                        && snapshot.cause == TerminationCause::Killed as u32
                        && snapshot.detail
                            == libkernel::shared::proc::TerminationDetail::PROCESS_CONTROL_KILL =>
                {
                    debug!(
                        "derive fallback passed: pid {} minted control completed at Dead/Killed/{:#x}",
                        pid, snapshot.detail.word0
                    );
                }
                (drained, snapshot) => debug!(
                    "derive fallback FAILED: visible={} completion={:?} snapshot={:?}",
                    visible, drained, snapshot
                ),
            }
            let _ = unsafe { close(control) };
        }
        Err(error) => {
            // SAFETY: 派生失败仍须放弃本函数拥有的 Builder。
            let _ = unsafe { close(created.builder) };
            debug!(
                "derive fallback FAILED: visible={} derive={:?}",
                visible, error
            );
        }
    }
}

/// 递归 JobKill 组合：child Job 内一个 Running 成员（Waiting 取消路径）
/// 以及一个 Building 成员，两者 control 均消散——libprocess::job_kill 一把
/// 收束（seal → 枚举 → 派生 kill → 等 DONE → 等 CLOSED 全链，派生走铸造
/// 路径）。
fn test_job_kill_composition(
    root: &mut RootSupervisor,
    job: Handle,
    image: &[u8],
) -> Result<(), &'static str> {
    let Ok(child) = root.create_job(job, root_memory_pool(), JOB_FULL_RIGHTS) else {
        debug!("job kill composition FAILED: job create failed");
        return Err("job kill composition job create failed");
    };
    let running = spawn(SpawnRequest {
        memory_pool: root_memory_pool(),
        parent: init_process_info(),
        job: child,
        image,
        payload: &[],
        grants: &[],
        control_rights: SUPERVISOR_RIGHTS,
    });
    let building = process::create(child, root_memory_pool(), SUPERVISOR_RIGHTS);
    match (running, building) {
        (Ok(running), Ok(building)) => {
            let running_pid = running.pid;
            let _ = unsafe { close(running.control) };
            let _ = unsafe { close(building.control) };
            match root.collect_job(child) {
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
    let Ok(child) = process::create_job(job, root_memory_pool(), JOB_FULL_RIGHTS) else {
        debug!("seal gate (start) FAILED: job create failed");
        return;
    };
    // 手工构建可启动的 Building：入口页（自旋）+ 栈顶页（失败自清理）。
    let Ok(created) = build_spin_building(child, SUPERVISOR_RIGHTS) else {
        debug!("seal gate (start) FAILED: building");
        let _ = unsafe { close(child) };
        return;
    };
    let sealed = process::seal_job(child);
    let started = process::start(created.builder);
    let gated = matches!(started, Err(SystemCallError::ObjectClosed));
    // 收束：kill → drain → sealed+空完成 → CLOSED。
    let _ = process::kill(created.control);
    let _ = wait_until(
        created.control,
        ObjectSignals::DONE,
        libkernel::time::Deadline::INFINITE,
    );
    let _ = unsafe { close(created.control) };
    let _ = unsafe { close(created.builder) };
    let waited = wait_until(
        child,
        ObjectSignals::CLOSED,
        libkernel::time::timeout_millis(0).unwrap(),
    );
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
    let Ok(child) = process::create_job(job, root_memory_pool(), JOB_FULL_RIGHTS) else {
        debug!("seal gate (create) FAILED: job create failed");
        return;
    };
    let sealed = process::seal_job(child);
    let waited = wait_until(
        child,
        ObjectSignals::CLOSED,
        libkernel::time::timeout_millis(0).unwrap(),
    );
    let member = process::create(child, root_memory_pool(), SUPERVISOR_RIGHTS);
    let subjob = process::create_job(child, root_memory_pool(), JOB_FULL_RIGHTS);
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
        let Ok(created) = process::create(job, root_memory_pool(), SUPERVISOR_RIGHTS) else {
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
        if let Err(error) = process::kill(created.control) {
            ok = false;
            debug!("enumerate convergence FAILED: kill {:?}", error);
            let _ = unsafe { close(created.builder) };
            let _ = unsafe { close(created.control) };
            continue;
        }
        let _ = unsafe { close(created.builder) };
        if let Err(error) = wait_until(
            created.control,
            ObjectSignals::DONE,
            libkernel::time::Deadline::INFINITE,
        ) {
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
        assert_eq!(message.header.sender_pid, init_pid() as u64);
        assert_eq!(message.header.sender_badge, badge);
    }
    let once_pair = make_send_once(badged, Rights::WRITE).expect("badged send-once mint failed");
    let once = once_pair.owner;
    // SAFETY: 本夹具不观察回复终态，关闭本次派生的独立观察表项。
    unsafe { libkernel::ipc::object::close(once_pair.peer) }.expect("reply observer close failed");
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

    let event = { let mut pair = libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */ unsafe { libkernel::sys_notification_create(
        Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        Rights::SIGNAL, &mut pair) }.map(|()| pair) }
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
    /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */
    unsafe { libkernel::sys_notification_signal(event.peer, 1) }
        .expect("notification owner must survive rejected transit");
    assert_eq!(
        {
            let mut bits = 0; /* SAFETY: ABI 夹具借用有效 handle，输出在同步调用期间私有。 */
            unsafe { libkernel::sys_notification_take(event.owner, 1, &mut bits) }.map(|()| bits)
        }
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
    use libkernel::{ipc::wait_set::WaitSet, time::Deadline};

    let event = { let mut pair = libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */ unsafe { libkernel::sys_notification_create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL, &mut pair) }.map(|()| pair) }
        .expect("WaitSet retirement notification create failed");
    let set = WaitSet::create(128).expect("WaitSet retirement create failed");
    let mut tokens = alloc::vec::Vec::new();
    for cookie in 0..64 {
        tokens.push(
            set.register(MemberRequest::new(
                event.owner,
                ObjectSignals::READABLE,
                cookie,
            ))
            .expect("WaitSet retirement registration failed"),
        );
    }
    set.remove(tokens[0]).expect("WaitSet removal failed");
    assert_eq!(set.remove(tokens[0]), Err(SystemCallError::ObjectNotFound));
    /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */
    unsafe { libkernel::sys_notification_signal(event.peer, 1) }
        .expect("WaitSet notification signal failed");
    set.wait(Deadline::INFINITE)
        .expect("WaitSet readiness wait failed");
    let records = set.receive(64).expect("WaitSet ready receive failed");
    assert!(records.iter().all(|record| record.token != tokens[0]));
    set.close()
        .unwrap_or_else(|(_, error)| panic!("nonempty WaitSet close failed: {error:?}"));
    unsafe { close(event.owner) }.expect("WaitSet source owner close failed");
    unsafe { close(event.peer) }.expect("WaitSet source signaler close failed");

    let event = { let mut pair = libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */ unsafe { libkernel::sys_notification_create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL, &mut pair) }.map(|()| pair) }
        .expect("persistent WaitSet source create failed");
    let set = WaitSet::create(1).expect("persistent WaitSet create failed");
    let token = set
        .register(MemberRequest::new(event.owner, ObjectSignals::READABLE, 4))
        .unwrap();
    for _ in 0..16 {
        /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */
        unsafe { libkernel::sys_notification_signal(event.peer, 1) }.unwrap();
        set.wait(Deadline::INFINITE).unwrap();
        let records = set.receive(1).unwrap();
        assert_eq!((records[0].token, records[0].cookie), (token, 4));
        // 未消费来源时持续返回同一电平；消费后旧提示必须重查并淘汰。
        assert_eq!(set.receive(1).unwrap()[0].token, token);
        {
            let mut bits = 0; /* SAFETY: ABI 夹具借用有效 handle，输出在同步调用期间私有。 */
            unsafe { libkernel::sys_notification_take(event.owner, 1, &mut bits) }.map(|()| bits)
        }
        .unwrap();
        assert!(set.receive(1).unwrap().is_empty());
    }
    set.remove(token).unwrap();
    let replacement = set
        .register(MemberRequest::new(event.owner, ObjectSignals::READABLE, 5))
        .unwrap();
    assert_ne!(replacement, token);
    /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */
    unsafe { libkernel::sys_notification_signal(event.peer, 1) }.unwrap();
    set.wait(Deadline::INFINITE).unwrap();
    assert_eq!(set.receive(1).unwrap()[0].token, replacement);
    drop(set);
    // SAFETY: 本地集合 owner 已提交退休，两个剩余来源表项均属于本函数。
    unsafe { close(event.owner) }.unwrap();
    unsafe { close(event.peer) }.unwrap();

    let first = WaitSet::create(4).unwrap();
    let second = WaitSet::create(4).unwrap();
    assert_eq!(
        first.register(MemberRequest::new(
            first.handle(),
            ObjectSignals::READABLE,
            1
        )),
        Err(SystemCallError::WrongObjectType)
    );
    assert_eq!(
        first.register(MemberRequest::new(
            second.handle(),
            ObjectSignals::READABLE,
            2
        )),
        Err(SystemCallError::WrongObjectType)
    );
    assert_eq!(
        second.register(MemberRequest::new(
            first.handle(),
            ObjectSignals::READABLE,
            3
        )),
        Err(SystemCallError::WrongObjectType)
    );
    first
        .close()
        .unwrap_or_else(|(_, error)| panic!("first flat WaitSet close failed: {error:?}"));
    second
        .close()
        .unwrap_or_else(|(_, error)| panic!("second flat WaitSet close failed: {error:?}"));
    debug!(
        "WaitSet persistent levels, stale removal, nonempty retirement and nested rejection passed"
    );
}

/// 一次性投递权：本地、转移、失败保留与原 sender 独立性。
fn test_send_once() {
    let mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("send-once mailbox create failed");
    let once_pair = make_send_once(mailbox.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        .expect("make send once failed");
    let once = once_pair.owner;
    // SAFETY: 本夹具不观察回复终态，关闭本次派生的独立观察表项。
    unsafe { libkernel::ipc::object::close(once_pair.peer) }.expect("reply observer close failed");
    unsafe { send_raw(once, 900, &[1], &[]) }.expect("send once failed");
    assert!(matches!(
        unsafe { send_raw(once, 901, &[], &[]) },
        Err(SystemCallError::StaleHandle)
    ));

    let once_pair = make_send_once(mailbox.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        .expect("transferred send-once mint failed");

    let once = once_pair.owner;

    // SAFETY: 本夹具不观察回复终态，关闭本次派生的独立观察表项。

    unsafe { libkernel::ipc::object::close(once_pair.peer) }.expect("reply observer close failed");
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
    let once_pair = make_send_once(full.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        .expect("send-once full mint failed");
    let once = once_pair.owner;
    // SAFETY: 本夹具不观察回复终态，关闭本次派生的独立观察表项。
    unsafe { libkernel::ipc::object::close(once_pair.peer) }.expect("reply observer close failed");
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
    let once_pair = make_send_once(both.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        .expect("send-once both mint failed");
    let once = once_pair.owner;
    // SAFETY: 本夹具不观察回复终态，关闭本次派生的独立观察表项。
    unsafe { libkernel::ipc::object::close(once_pair.peer) }.expect("reply observer close failed");
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
    let result = wait_until(
        mailbox.peer,
        ObjectSignals::WRITABLE,
        libkernel::time::timeout_millis(0).unwrap(),
    )
    .expect("empty mailbox must be writable");
    assert!(result.observed.intersects(ObjectSignals::WRITABLE));

    for _ in 0..MAILBOX_CAPACITY {
        unsafe { send_raw(mailbox.peer, 0, &[], &[]) }.expect("writable fill failed");
    }
    discard(mailbox.owner).expect("writable make-room failed");
    let result = wait_until(
        mailbox.peer,
        ObjectSignals::WRITABLE,
        libkernel::time::timeout_millis(0).unwrap(),
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
    let deadline = libkernel::time::timeout_millis(5_000).map_err(|_| "wake deadline failed")?;
    let done_wait = libkernel::ipc::wait::wait_until(done.owner, ObjectSignals::READABLE, deadline)
        .map_err(|_| "wake notification wait failed")?;
    if WaitReason::from_u32(done_wait.reason) == Some(WaitReason::Timeout) {
        return Err("pm writable wake confirmation timed out");
    }
    root.notification_owner(done.owner)
        .take(u64::MAX)
        .map_err(|_| "wake notification take failed")?;
    discard(target.owner).map_err(|_| "wake make-room failed")?;

    // 队列：15 × FILL + TAIL。逐条校验，末尾必然是被唤醒后补发的 TAIL。
    for index in 0..MAILBOX_CAPACITY {
        let ready = libkernel::ipc::wait::wait_until(
            target.owner,
            ObjectSignals::READABLE | ObjectSignals::CLOSED,
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
        root.notification_owner(spin.owner).take(u64::MAX),
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
        let event = { let mut pair = libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */ unsafe { libkernel::sys_notification_create(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::SIGNAL | Rights::TRANSIT, &mut pair) }.map(|()| pair) }
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
        /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */
        unsafe {
            libkernel::sys_notification_signal(
                message
                    .handles
                    .get(0)
                    .expect("stress signaler slot missing")
                    .as_handle(),
                1,
            )
        }
        .expect("stress signal failed");
        assert_eq!(
            {
                let mut bits = 0; /* SAFETY: ABI 夹具借用有效 handle，输出在同步调用期间私有。 */
                unsafe { libkernel::sys_notification_take(event.owner, 1, &mut bits) }
                    .map(|()| bits)
            }
            .expect("stress take failed"),
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
    let event = { let mut pair = libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */ unsafe { libkernel::sys_notification_create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT, &mut pair) }.map(|()| pair) }
    .expect("full mailbox notification create failed");
    let moves = [HandleMove {
        handle: event.peer,
        rights: Rights::SIGNAL,
    }];
    assert!(matches!(
        unsafe { send_raw(mailbox.peer, 0, &[], &moves) },
        Err(SystemCallError::MailboxFull)
    ));
    /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */
    unsafe { libkernel::sys_notification_signal(event.peer, 1) }
        .expect("failed Send must retain moved source");
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

/// 双维预算源自父账户；关闭外部授权不丢失子树与已付存储的退款责任。
fn test_pool_budget() {
    use erhino_shared::memory_pool::MemoryPoolBudget;
    use libkernel::memory_pool::MemoryPool;
    let rights = Rights::CREATE | Rights::READ | Rights::DUPLICATE;
    let root = duplicate(root_memory_pool(), rights).unwrap();
    // SAFETY: duplicate 返回一份唯一 raw owner，交 typed 管理。
    let funding = unsafe { MemoryPool::from_handle(root) };
    let supply = funding.query().unwrap();
    // 测试域独立记账；调用者表增长等合法成本不属于本域，不以全局 Root 当退款基线。
    let root = funding
        .derive(
            MemoryPoolBudget {
                pages: 1,
                metadata: supply.metadata.available / 4,
            },
            rights,
        )
        .unwrap();
    let before = root.query().unwrap();
    assert!(matches!(
        root.derive(MemoryPoolBudget::default(), rights),
        Err(SystemCallError::IllegalArgument)
    ));
    let pages = root
        .derive(
            MemoryPoolBudget {
                pages: 1,
                metadata: 0,
            },
            rights,
        )
        .unwrap();
    let snapshot = pages.query().unwrap();
    assert!(matches!(
        pages.derive(
            MemoryPoolBudget {
                pages: 1,
                metadata: 1
            },
            rights
        ),
        Err(SystemCallError::QuotaExceeded)
    ));
    assert_eq!(
        pages.query().unwrap(),
        snapshot,
        "failed second dimension retained page reservation"
    );
    // 非空对象必须支付出生存储；页供给充足也不能隐式借用调用者 metadata。
    let page_owner = pages.into_handle();
    assert!(matches!(
        libkernel::ipc::invitation::Invitation::create(page_owner, 4096),
        Err(libkernel::ipc::invitation::CreateFailure::System(
            SystemCallError::QuotaExceeded
        ))
    ));
    // SAFETY: Create 只接受 Pool 授权，不消费该表项；raw 使用已结束，恢复唯一 owner。
    let pages = unsafe { MemoryPool::from_handle(page_owner) };
    assert_eq!(
        pages.query().unwrap(),
        snapshot,
        "metadata-denied birth retained page or metadata credit"
    );
    pages.close();
    assert_eq!(
        root.query().unwrap(),
        before,
        "page-only Pool retained parent metadata charge"
    );
    // 额度从实际供给取一部分，只检查守恒，不复制内核结构大小或旧对象个数限额。
    let budget = before.metadata.available / 4;
    assert!(budget > 0);
    let child = root
        .derive(
            MemoryPoolBudget {
                pages: 0,
                metadata: budget,
            },
            rights,
        )
        .unwrap();
    let child_snapshot = child.query().unwrap();
    assert_eq!(child_snapshot.total, 0);
    assert_eq!(child_snapshot.metadata.available, budget);
    let grand = child
        .derive(
            MemoryPoolBudget {
                pages: 0,
                metadata: 1,
            },
            rights,
        )
        .unwrap();
    assert!(
        child.query().unwrap().metadata.allocated > 0,
        "grandchild stored no parent-paid metadata"
    );
    child.close();
    assert_eq!(
        root.query().unwrap().metadata.delegated,
        before.metadata.delegated + budget
    );
    assert_eq!(
        grand.query().unwrap().metadata.available,
        1,
        "parent Handle close revoked child budget"
    );
    grand.close();
    assert_eq!(
        root.query().unwrap(),
        before,
        "Pool tree lost page or metadata credit"
    );
    // 最大资金父链通过真实 API 释放；不复制内核 header 的私有布局。
    let payer = root
        .derive(
            MemoryPoolBudget {
                pages: 1,
                metadata: budget,
            },
            rights,
        )
        .unwrap();
    let baseline = payer.query().unwrap();
    let mut last: Option<MemoryPool> = None;
    for _ in baseline.depth..erhino_shared::memory_pool::MEMORY_POOL_MAX_DEPTH {
        let parent = last.as_ref().unwrap_or(&payer);
        let snapshot = parent.query().unwrap();
        let mut metadata = snapshot.metadata.available;
        let child = loop {
            match parent.derive(MemoryPoolBudget { pages: 1, metadata }, rights) {
                Ok(child) => break child,
                Err(SystemCallError::QuotaExceeded) => {
                    assert_eq!(
                        parent.query().unwrap(),
                        snapshot,
                        "rejected Pool birth retained credit"
                    );
                    metadata = metadata
                        .checked_sub(1)
                        .expect("Pool birth could not reserve storage");
                }
                Err(error) => panic!("maximum Pool chain preparation failed: {error:?}"),
            }
        };
        if let Some(old) = last.replace(child) {
            old.close();
        }
    }
    let leaf = last.unwrap();
    assert_eq!(
        leaf.query().unwrap().depth,
        erhino_shared::memory_pool::MEMORY_POOL_MAX_DEPTH
    );
    let depth_before = leaf.query().unwrap();
    assert!(matches!(
        leaf.derive(
            MemoryPoolBudget {
                pages: 0,
                metadata: 1
            },
            rights
        ),
        Err(SystemCallError::ReachLimit)
    ));
    assert_eq!(leaf.query().unwrap(), depth_before);
    let raw = leaf.into_handle();
    let (endpoint, invitation, _) =
        libkernel::ipc::invitation::Invitation::create(raw, 4096).unwrap();
    // 普通 View 是独立的真实 use；两端和Pool cap都先关，再由Unmap最后解脱。
    let mapping = libkernel::mm::MappedRegion::map_endpoint(
        &endpoint,
        4096,
        libkernel::shared::mem::MemoryProtection::ReadWrite,
        libkernel::mm::Placement::Anywhere,
    )
    .unwrap();
    // SAFETY: Create 未消费 Pool 表项，raw 调用结束后恢复唯一 owner。
    let leaf = unsafe { MemoryPool::from_handle(raw) };
    leaf.close();
    invitation.close().unwrap();
    endpoint.close().unwrap();
    assert_ne!(
        payer.query().unwrap(),
        baseline,
        "live View lost its funding chain before retirement"
    );
    mapping.unmap().unwrap();
    // 真实 Backing 退休记录是最后 payer，最终父链在 M 段解脱。
    let deadline = libkernel::time::timeout_millis(5_000).unwrap();
    while payer.query().unwrap() != baseline {
        assert!(
            !libkernel::time::expired(deadline).unwrap(),
            "maximum Pool chain retained credit after Close"
        );
        libkernel::time::sleep_until(libkernel::time::timeout_millis(1).unwrap()).unwrap();
    }
    payer.close();
    debug!(
        "Pool metadata budget checks passed: dual rollback, no implicit quota, parent close, complete refund, maximum chain"
    );
}

/// 真实失败保存 owner；最后事件授权关闭不撤销已接受的普通 View。
fn test_tunnel_failure_owners() {
    use libkernel::{
        ipc::invitation::{CreateFailure, Invitation},
        memory_pool::MemoryPool,
        mm::{MappedRegion, Placement},
        shared::mem::MemoryProtection,
    };
    let parent = duplicate(
        root_memory_pool(),
        Rights::CREATE | Rights::READ | Rights::DUPLICATE,
    )
    .unwrap();
    // SAFETY: duplicate 新建唯一 raw owner，交给 typed Pool 管理。
    let parent = unsafe { MemoryPool::from_handle(parent) };
    // 子域承担 Tunnel 存储；其余供给留给调用者域的 View/操作增长，不复制内核布局成本。
    let metadata = parent.query().unwrap().metadata.available / 2;
    let child = parent
        .derive(
            erhino_shared::memory_pool::MemoryPoolBudget { pages: 4, metadata },
            Rights::CREATE | Rights::READ | Rights::DUPLICATE,
        )
        .unwrap()
        .into_handle();
    let observation = duplicate(child, Rights::READ).unwrap();
    // SAFETY: 独立只读查询 owner；付款授权 child 由本函数在末尾关闭。
    let pool = unsafe { MemoryPool::from_handle(observation) };
    assert!(matches!(
        Invitation::create(child, 0),
        Err(CreateFailure::System(SystemCallError::IllegalArgument))
    ));
    assert!(matches!(
        Invitation::create(child, 5 * 4096),
        Err(CreateFailure::System(SystemCallError::QuotaExceeded))
    ));
    assert_eq!(
        pool.query().unwrap().allocated,
        0,
        "failed Tunnel retained page charge"
    );
    let (endpoint, invitation, bytes) = Invitation::create(child, 4096).unwrap();
    let occupied = MappedRegion::map_anonymous(
        bytes,
        0,
        0,
        MemoryProtection::ReadWrite,
        Placement::Anywhere,
    )
    .unwrap();
    let address = occupied.usable().unwrap().start;
    let failure = librunnel::MappedEndpoint::map(
        endpoint,
        bytes,
        Placement::FixedEmpty {
            usable_start: address,
        },
    )
    .expect_err("Tunnel Map replaced an occupied range");
    assert_eq!(failure.error, SystemCallError::AddressConflict);
    occupied.unmap().unwrap();
    let endpoint = failure.endpoint;
    let mapping = MappedRegion::map_endpoint(
        &endpoint,
        bytes,
        MemoryProtection::ReadWrite,
        Placement::FixedEmpty {
            usable_start: address,
        },
    )
    .unwrap();
    {
        // SAFETY: 唯一 MappedRegion 持有已发布的 RW 页；本作用域不解除映射。
        let memory = unsafe { libkernel::shared_memory::SharedMemory::new(address, bytes) };
        memory.store_u64(0, 0x5120_fade, core::sync::atomic::Ordering::Release);
    }
    endpoint.close().unwrap();
    let failure = invitation
        .attach()
        .expect_err("Attach accepted a closed creator");
    assert_eq!(failure.error, SystemCallError::ObjectClosed);
    failure.invitation.close().unwrap();
    assert_eq!(
        pool.query().unwrap().allocated,
        1,
        "Endpoint Close revoked a live View"
    );
    {
        // SAFETY: 两端授权已关，但 mapping 独立持 use；它在本次访问后才消费。
        let memory = unsafe { libkernel::shared_memory::SharedMemory::new(address, bytes) };
        assert_eq!(
            memory.load_u64(0, core::sync::atomic::Ordering::Acquire),
            0x5120_fade
        );
    }
    mapping.unmap().unwrap();
    let deadline = libkernel::time::timeout_millis(5_000).unwrap();
    // root 放弃由出生记录自主推进；页退款须同时越过 root 与 View 两份退休责任。
    while {
        let snapshot = pool.query().unwrap();
        snapshot.allocated != 0 || snapshot.metadata.allocated != 0
    } {
        assert!(
            !libkernel::time::expired(deadline).unwrap(),
            "Tunnel View retirement did not refund"
        );
        libkernel::time::sleep_until(libkernel::time::timeout_millis(1).unwrap()).unwrap();
    }
    unsafe { close(child) }.unwrap();
    pool.close();
    debug!(
        "Tunnel failure owner checks passed: quota rollback, Map conflict, Attach rejection, independent View refund"
    );
}

/// 实际 Running 调用链验证取整、双端不同 VA、完整共享范围与关闭后存活端。
fn test_tunnel_geometry() {
    use core::sync::atomic::Ordering;
    use libkernel::mm::Placement;
    for bytes in [1usize, 4096, 4097, 8192, 12288, 512 * 4096] {
        let (endpoint, invitation, mapped_bytes) =
            libkernel::ipc::invitation::Invitation::create(root_memory_pool(), bytes)
                .expect("Tunnel geometry Create failed");
        let creator = librunnel::MappedEndpoint::map(endpoint, mapped_bytes, Placement::Anywhere)
            .expect("Tunnel geometry Map failed");
        let geometry = creator.geometry();
        assert_eq!(geometry.bytes(), bytes.div_ceil(4096) * 4096);
        for page in 0..geometry.bytes() / 4096 {
            creator
                .memory()
                .store_u64(page * 4096, page as u64 + 1, Ordering::Release);
        }
        let endpoint = invitation.attach().expect("Tunnel geometry Attach failed");
        let peer = librunnel::MappedEndpoint::map(endpoint, mapped_bytes, Placement::Anywhere)
            .expect("Tunnel geometry peer Map failed");
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
            let region = libkernel::mm::MappedRegion::map_anonymous(
                geometry.bytes(),
                0,
                0,
                libkernel::shared::mem::MemoryProtection::ReadWrite,
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
    use libkernel::ipc::invitation::Invitation;
    for _ in 0..TUNNEL_STRESS {
        let (abandoned, invitation, _) = Invitation::create(root_memory_pool(), TUNNEL_BYTES)
            .expect("lifecycle Tunnel create failed");
        // SAFETY: 只借未消费的Invitation验证其不提供等待来源，不取得第二owner；
        // WAIT模板位约束Attach后Endpoint，不能把它当成Invitation自己的观察来源。
        assert_eq!(
            wait_until(
                unsafe { invitation.raw_handle() },
                ObjectSignals::CLOSED,
                libkernel::time::Deadline::at(0)
            ),
            Err(SystemCallError::WrongObjectType)
        );
        invitation.close().expect("Invitation close failed");
        assert!(
            abandoned
                .events()
                .wait(ObjectSignals::PEER_CLOSED, 0)
                .unwrap()
                .observed
                .intersects(ObjectSignals::PEER_CLOSED)
        );
        abandoned.close().expect("lifecycle Endpoint close failed");
        let (creator, invitation, _) =
            Invitation::create(root_memory_pool(), TUNNEL_BYTES).unwrap();
        creator.close().unwrap();
        let failure = invitation
            .attach()
            .expect_err("Attach accepted a closed creator");
        assert_eq!(failure.error, SystemCallError::ObjectClosed);
        failure
            .invitation
            .close()
            .expect("failed Attach lost Invitation owner");
    }
    debug!("Tunnel abandonment and closed-creator Attach checks passed");
}
