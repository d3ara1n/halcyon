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

use libprocess::{
    DEFAULT_SUPERVISION_POLICY, DERIVED_CONTROL_RIGHTS, RequiredLaunchSet, SpawnRequest,
    SupervisionCause, SupervisionProgress, SupervisionStage, SupervisionTarget, collect_process,
    enumerate_members, job_kill, spawn,
};
use librpc::{
    CallCause, CallError, CallPhase, Caller, FrameRejection, Request, RpcMessageKind, RpcPrefix,
};
use librunnel::blocking;
use rinlib::ipc::tunnel as tunnel_sys;
use rinlib::ipc::{capability::Capability, packet::Packet};
use rinlib::memory_pool::MemoryPool;
#[cfg(feature = "acceptance-stress")]
use rinlib::shared::proc::ProcessDrainStatus;
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
    pm_mailbox: Handle,
    supervised: alloc::vec::Vec<Supervised>,
    target_image: Option<alloc::vec::Vec<u8>>,
    hammer_image: Option<alloc::vec::Vec<u8>>,
}

#[derive(Debug, Clone, Copy)]
struct SupervisionBatchFailure {
    pid: u64,
    stage: SupervisionStage,
    cause: SupervisionCause,
    progress: SupervisionProgress,
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
const REQUIRED_IMAGES: u64 = REQUIRED_FS | REQUIRED_PM | REQUIRED_DRIVER | REQUIRED_TARGET;
const REQUIRED_STARTED: u64 = REQUIRED_IMAGES | REQUIRED_PM_TARGET;

fn required_image(name: &str) -> Option<u64> {
    match name {
        "bin/srv_fs" => Some(REQUIRED_FS),
        "bin/srv_pm" => Some(REQUIRED_PM),
        "bin/drv_spi_sifive" => Some(REQUIRED_DRIVER),
        "bin/test_target" => Some(REQUIRED_TARGET),
        _ => None,
    }
}

/// 从声明式 initfs 政策启动服务拓扑。必选映像缺失或任一必选 spawn 失败使
/// 整个 stage 失败；test_fp 是按 admitted execution domain 决定的可选服务。
fn launch_test_services(
    services: Handle,
    pm_domain: Handle,
    acceptance: Handle,
    names: &mut TopologyNames,
) -> Result<LaunchedServices, &'static str> {
    let pm_mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::GRANT | Rights::DUPLICATE,
    )
    .map_err(|_| "pm mailbox create failed")?;
    // GRANT 是直接跨表安装：授出的源 handle 被消费，先复制保留 init 对
    // 委托域的直接收束权（兜底 job_kill 的 authority 源）。
    let delegated_domain =
        duplicate(pm_domain, JOB_FULL_RIGHTS).map_err(|_| "pm domain duplicate failed")?;
    let control_rights = SUPERVISOR_RIGHTS;
    let mut supervised = alloc::vec::Vec::new();
    let mut manifest = RequiredLaunchSet::new(REQUIRED_IMAGES, REQUIRED_STARTED);
    let mut stage_failure = None;
    // test_target/test_hammer 映像留存：竞态矩阵与验收线的靶/锤复用。
    let mut target_image: Option<alloc::vec::Vec<u8>> = None;
    let mut hammer_image: Option<alloc::vec::Vec<u8>> = None;
    let result = tar::walk(env::startup_payload(), |entry| {
        if !entry.name.starts_with("bin/") || entry.name.ends_with('/') {
            return;
        }
        if entry.name == "bin/test_hammer" {
            // 竞态锤不是常驻服务：只留存映像，由剧本按需 spawn。
            hammer_image = Some(alloc::vec::Vec::from(entry.data));
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
        // test_target 首实例入 acceptance 域（枚举+派生验收线的靶域）。
        let (job, grants): (Handle, &[HandleGrant]) = if entry.name == "bin/test_target" {
            (acceptance, &[])
        } else if entry.name == "bin/srv_pm" {
            (services, pm_grants.as_slice())
        } else {
            (services, &[])
        };
        match spawn(SpawnRequest {
            memory_pool: root_memory_pool(),
            job,
            image: entry.data,
            payload: &[],
            grants,
            control_rights,
        }) {
            Ok(process) => {
                if let Some(bit) = required {
                    manifest.mark_started(bit);
                }
                debug!("started {} as pid {}", entry.name, process.pid);
                names.register_process(process.pid, entry.name);
                // 持久 init 保留 control：监督、等待与收束的 authority 源。
                if entry.name == "bin/test_target" {
                    target_image = Some(alloc::vec::Vec::from(entry.data));
                    // live kill 正路径改经枚举+派生（Job 管理面验收线 1）：
                    // acceptance Job 枚举可见 test_target 的 pid，派生 MANAGE
                    // control 后 kill——保留 control 在派生接管后关闭。
                    test_derive_kill(acceptance, process.pid, process.control);
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
                } else {
                    supervised.push(Supervised {
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
        let _ = unsafe { close(delegated_domain) };
        let _ = unsafe { close(pm_mailbox.owner) };
        let _ = unsafe { close(pm_mailbox.peer) };
        return Err(failure);
    }
    debug!(
        "required service topology complete: images={:#x}, started={:#x}",
        manifest.present(),
        manifest.started()
    );
    Ok(LaunchedServices {
        pm_mailbox: pm_mailbox.peer,
        supervised,
        target_image,
        hammer_image,
    })
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
    let services = match process::create_job(root_job, JOB_FULL_RIGHTS) {
        Ok(job) => job,
        Err(error) => {
            debug!("services job create failed: {:?}", error);
            steady_state();
        }
    };
    debug!("services job established");
    if let Err(stage) = run(services) {
        debug!("init acceptance failed: {}", stage);
        // root 仍持 services JobControl；有限收束失败时保留该 authority 并进入
        // 稳态接管，成功才提交 SystemFailure reset。
        match job_kill(services, 0x1F) {
            Ok(()) => {
                debug!("failure-path services collection completed");
                debug!("init: submitting failure shutdown");
                match system::reset(reset, ResetAction::Shutdown, ResetReason::SystemFailure) {
                    Err(error) => {
                        debug!("failure shutdown rejected: {:?}", error);
                        steady_state()
                    }
                    Ok(never) => match never {},
                }
            }
            Err(error) => {
                debug!(
                    "root supervision retained authority after failed collection: {:?}",
                    error
                );
                steady_state()
            }
        }
    }
    submit_shutdown(root_job, reset);
}

/// 先验证 capability 负路径，再提交唯一的成功路径。
fn submit_shutdown(root_job: Handle, reset: Handle) -> ! {
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
            steady_state()
        }
        Ok(never) => match never {},
    }
}

/// 平台拒绝系统复位后的稳定形态：init 保持 root supervisor 存活并等待。
fn steady_state() -> ! {
    let endpoint = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::GRANT,
    )
    .expect("steady-state endpoint create failed");
    debug!("init: steady-state supervision (persistent root supervisor)");
    loop {
        match wait_message(endpoint.owner) {
            Ok(message) => {
                debug!(
                    "BUG: steady-state endpoint received kind {}",
                    message.header.kind
                )
            }
            Err(error) => debug!("steady-state wait ended: {:?}", error),
        }
    }
}

/// 小集合收束辅助：对保留 control 的服务 kill → 等待收束 → Drain →
/// close（验收线 1 的派生接管路径复用）。
fn kill_and_supervise(job: Handle, mut supervised: alloc::vec::Vec<Supervised>) {
    for target in &supervised {
        let _ = process::kill(target.control, 0x1F);
    }
    if let Err(error) = supervise_services(&mut supervised) {
        debug!(
            "process supervision handoff: pid={}, stage={:?}, cause={:?}, progress={:?}",
            error.pid, error.stage, error.cause, error.progress
        );
        if let Err(escalation) = job_kill(job, 0x1F) {
            debug!("job supervision escalation failed: {:?}", escalation);
        }
    }
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
                let prefix =
                    RpcPrefix::decode(&request.payload).expect("RPC test request prefix invalid");
                assert_eq!(prefix.kind, RpcMessageKind::Request);
                let mut reply_once = request
                    .handles
                    .take(0)
                    .expect("RPC test reply slot missing");
                let mut response = [0u8; librpc::PREFIX_LEN + 1];
                RpcPrefix::new(RpcMessageKind::Response, prefix.txid).encode(&mut response);
                response[librpc::PREFIX_LEN] = attempt;
                let protocol = if attempt == 0 { PROTOCOL + 1 } else { PROTOCOL };
                let mut packet = Packet::new(protocol, &response)
                    .expect("RPC response packet allocation failed");
                if attempt == 0 {
                    // SAFETY: 新创建的原始 signaler 尚未被其他 owner 接管，alias 仅用于 stale 检查。
                    let capability = unsafe { Capability::from_raw(rejected.peer) };
                    packet
                        .push(capability, Rights::SIGNAL)
                        .expect("RPC response capability preparation failed");
                }
                packet
                    .try_reply(&mut reply_once, rinlib::time::Deadline::INFINITE)
                    .expect("RPC response publication failed");
            }
            unsafe { close(service.owner) }.expect("RPC test service owner close failed");
        })
        .expect("RPC test worker spawn failed");

    // SAFETY: 此 sender 由原始队列工厂创建，本测试在此唯一接管关闭责任。
    let service_sender = unsafe { Capability::from_raw(service.peer) };
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

/// 全部测试剧本。失败只短路后续阶段，交回 main 以 services 整树收束兜底。
fn run(services: Handle) -> Result<(), &'static str> {
    debug!("acceptance workload: {}", ACCEPTANCE_WORKLOAD);
    let root_job = env::startup_handle(initial::ROOT_JOB).expect("init must hold root JobControl");
    test_memory_mapping()?;
    #[cfg(not(feature = "acceptance-stress"))]
    test_process_memory_binding(services)?;
    let pm_domain = process::create_job(services, JOB_FULL_RIGHTS)
        .map_err(|_| "pm domain job create failed")?;
    let acceptance = process::create_job(services, JOB_FULL_RIGHTS)
        .map_err(|_| "acceptance job create failed")?;
    let mut names = TopologyNames::new();
    names.register_job(root_job, "root");
    names.register_job(services, "services");
    names.register_job(pm_domain, "pm_domain");
    names.register_job(acceptance, "acceptance");
    names.register_process(env::pid() as u64, "init");
    let launched = launch_test_services(services, pm_domain, acceptance, &mut names)?;
    let pm_mailbox = launched.pm_mailbox;
    let mut supervised = launched.supervised;
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
    public_ipc::committed_kill(
        acceptance,
        target_image.as_deref().expect("IPC target image missing"),
    );

    // —— 数据面：建隧道 → Invitation 经消息面转移 → 阻塞读流 ——
    let (mut tunnel, invitation) =
        match blocking::create_consumer(TUNNEL_BYTES, rinlib::mm::Placement::Anywhere) {
            Ok(t) => t,
            Err(e) => {
                debug!("tunnel create failed: {:?}", e);
                return Err("tunnel create failed");
            }
        };
    debug!("tunnel created");
    let invitation_move = [HandleMove {
        handle: invitation,
        rights: Rights::MAP,
    }];
    if let Err(e) = unsafe { send_raw(pm_mailbox, 514, &[], &invitation_move) } {
        debug!("send tunnel invitation failed: {:?}", e);
        return Err("send tunnel invitation failed");
    }

    let mut buf = rinlib::alloc::vec![0u8; STREAM_LEN + 1];
    let n = tunnel.read_exact_or_eof(&mut buf).map_err(|error| {
        debug!("stream read failed: {:?}", error);
        "stream read failed"
    })?;
    if n != STREAM_LEN
        || !buf[..n]
            .iter()
            .enumerate()
            .all(|(i, &byte)| byte == (i % 251 + 1) as u8)
    {
        return Err("stream pattern or length mismatch");
    }
    debug!("stream received {} bytes, pattern ok", n);
    debug!(
        "RNL2 multi-page stream passed: bytes={}, capacity={}",
        n,
        tunnel.capacity()
    );

    // —— 流控唤醒面：pm 填满目标邮箱后在 WRITABLE 上阻塞，腾位唤醒 ——
    test_writable_wake(pm_mailbox);

    // —— live kill 正路径：Building 目标的确定性 kill/drain/终态验证 ——
    test_building_kill(acceptance);

    // —— Job 管理面验收（step 5）：封口与完成传播、派生兑底、递归
    // JobKill 组合与 seal 闸门可行子集 ——
    match target_image.as_deref() {
        Some(image) => test_job_management(acceptance, image)?,
        None => return Err("job management acceptance image unavailable"),
    }

    // 短寿命服务的线程已退出并不释放 AddressSpace；在高峰矩阵前收束已进入
    // Terminating/Dead 的成员，仍 Running 的服务继续由末尾监督闭环持有。
    let reclaimed = supervise_terminated_services(&mut supervised).map_err(|error| {
        debug!("pre-race service supervision failed: {:?}", error);
        "pre-race service supervision failed"
    })?;
    debug!(
        "pre-race service supervision reclaimed {} process(es)",
        reclaimed
    );

    // 完整生命周期多核竞态矩阵属于 stress workload；core 仍覆盖确定性的
    // Building kill、Job 管理与服务监督收束。
    #[cfg(feature = "acceptance-stress")]
    match (target_image.as_deref(), hammer_image.as_deref()) {
        (Some(target), Some(hammer)) => race_matrix(acceptance, target, hammer)?,
        _ => return Err("race matrix acceptance images unavailable"),
    }
    #[cfg(not(feature = "acceptance-stress"))]
    let _ = hammer_image;

    // acceptance 域用完即收：seal + 空即完成，不把一次性验收遗留带进
    // 稳态拓扑。
    if let Err(error) = job_kill(acceptance, 0x1E) {
        debug!("acceptance domain collection failed: {:?}", error);
        return Err("acceptance domain collection failed");
    }
    debug!("acceptance domain collected");
    let _ = unsafe { close(acceptance) };

    // —— 监督闭环：等待全部服务 REAPABLE/CLOSED，Drain 至 Complete，
    // 查询稳定终态后释放 control。对象 close 回调（含 pm 隧道端点的
    // PEER_CLOSED 发布）发生在 Drain 期间——监督先于对端终态等待。 ——
    if let Err(error) = supervise_services(&mut supervised) {
        debug!(
            "service supervision requires root handoff: pid={}, stage={:?}, cause={:?}, progress={:?}",
            error.pid, error.stage, error.cause, error.progress
        );
        return Err("service supervision failed");
    }
    debug!("all services supervised to completion");

    // —— 事件面：对端终态位（Drain 已置位，电平等待立即返回）——
    let peer_closed = wait_supervision_signal(&mut tunnel, "peer-closed")?;
    debug!(
        "peer closed observed: bits={:#x}",
        peer_closed.observed.raw()
    );
    #[cfg(not(feature = "acceptance-stress"))]
    let tunnel_pool_before_close =
        root_pool_allocated().map_err(|_| "tunnel Pool pre-close query failed")?;
    tunnel.close().map_err(|_| "tunnel close failed")?;

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
            return Err("tunnel Pool charge did not refund after close");
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
        job_kill(pm_domain, 0x1F).map_err(|error| {
            debug!("pm delegated domain collection failed: {:?}", error);
            "pm delegated domain collection failed"
        })?;
    }
    let domain_state = process::query_job(pm_domain).map_err(|error| {
        debug!("pm delegated domain final query failed: {:?}", error);
        "pm delegated domain final query failed"
    })?;
    if domain_state.state != JobState::Dead as u32 {
        return Err("pm delegated domain did not reach Dead");
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
        return Err("thread stack cleanup abandoned mappings");
    }
    debug!("thread stack cleanup passed");
    debug!("topology: final snapshot before system reset");
    dump_topology(root_job, &names, 0);
    let _ = unsafe { close(pm_domain) };
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
fn supervise_terminated_services(
    supervised: &mut alloc::vec::Vec<Supervised>,
) -> Result<usize, SupervisionBatchFailure> {
    let mut reclaimed = 0;
    let mut index = 0;
    while index < supervised.len() {
        let state =
            process::query(supervised[index].control).map_err(|error| SupervisionBatchFailure {
                pid: supervised[index].pid,
                stage: SupervisionStage::VerifyDead,
                cause: SupervisionCause::System(error),
                progress: SupervisionProgress::default(),
            })?;
        if state.state == ProcessState::Building as u32
            || state.state == ProcessState::Running as u32
        {
            index += 1;
            continue;
        }
        let target = supervised.swap_remove(index);
        let mut ready = alloc::vec::Vec::from([target]);
        if let Err(failure) = supervise_services(&mut ready) {
            supervised.append(&mut ready);
            return Err(failure);
        }
        reclaimed += 1;
    }
    Ok(reclaimed)
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

/// 有限 policy 下逐项等待、Drain、Query；只有 VerifiedDead 后才由 libprocess
/// 关闭 control 并移除。失败 target 重新放回集合，供 root Job 接管或后续重试。
fn supervise_services(
    supervised: &mut alloc::vec::Vec<Supervised>,
) -> Result<(), SupervisionBatchFailure> {
    while let Some(target) = supervised.pop() {
        match collect_process(
            SupervisionTarget::new(target.pid, target.control),
            DEFAULT_SUPERVISION_POLICY,
        ) {
            Ok(collected) => {
                debug!(
                    "pid {} supervised: work={}, state={}, reason={}, code={}",
                    collected.pid,
                    collected.progress.work_done,
                    collected.snapshot.state,
                    collected.snapshot.reason,
                    collected.snapshot.code
                );
            }
            Err(failure) => {
                let reported = SupervisionBatchFailure {
                    pid: failure.target.pid(),
                    stage: failure.stage,
                    cause: failure.cause,
                    progress: failure.progress,
                };
                supervised.push(Supervised {
                    pid: failure.target.pid(),
                    control: failure.target.into_control(),
                });
                return Err(reported);
            }
        }
    }
    Ok(())
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
/// control 在派生接管后关闭（关闭 control 永不隐式终止）。任一步失败
/// 降级回保留 control 路径，不影响既有验收面。
fn test_derive_kill(job: Handle, pid: u64, retained: Handle) {
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
            let collected = collect_process(failure.target, DEFAULT_SUPERVISION_POLICY)
                .expect("retained supervision authority must resume collection");
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
            debug!("derive kill degraded ({:?}); using retained control", error);
            process::kill(retained, 0x77).expect("live kill of a fresh process must be accepted");
            kill_and_supervise(
                job,
                alloc::vec::Vec::from([Supervised {
                    pid,
                    control: retained,
                }]),
            );
        }
    }
}

/// Job 管理面验收入口（step 5）。真跨核竞态矩阵（含并发 Create/枚举
/// 乱序窗口）归 step 9 验证矩阵，此处只覆盖可确定性制造的场景。
fn test_job_management(job: Handle, image: &[u8]) -> Result<(), &'static str> {
    test_job_seal_completion(job);
    test_derive_fallback(job);
    test_job_kill_composition(job, image);
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
fn test_job_kill_composition(job: Handle, image: &[u8]) {
    let Ok(child) = process::create_job(job, JOB_FULL_RIGHTS) else {
        debug!("job kill composition FAILED: job create failed");
        return;
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
            match job_kill(child, 0x3C) {
                Ok(()) => {
                    let snapshot = process::query_job(child);
                    match snapshot {
                        Ok(snapshot) if snapshot.state == JobState::Dead as u32 => debug!(
                            "job kill composition passed (member pid {}, child jid {} Dead)",
                            running_pid, snapshot.jid
                        ),
                        snapshot => {
                            debug!("job kill composition FAILED: child snapshot={:?}", snapshot)
                        }
                    }
                }
                Err(error) => {
                    debug!("job kill composition FAILED: {:?}", error);
                }
            }
        }
        (running, building) => {
            debug!(
                "job kill composition FAILED: running={:?} building={:?}",
                running.err(),
                building.err()
            );
        }
    }
    let _ = unsafe { close(child) };
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
fn test_writable_wake(pm_mailbox: Handle) {
    let target = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
    )
    .expect("wake target mailbox create failed");
    let done = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT,
    )
    .expect("wake done notification create failed");
    let spin = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT,
    )
    .expect("wake spin notification create failed");
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
        .expect("wake request send failed");

    // pm 确认已满后置位通知，此时它正阻塞在 WRITABLE 上。
    wait_many(&[WaitItem::new(done.owner, ObjectSignals::READABLE, 0)], 0)
        .expect("wake notification wait failed");
    notification::take(done.owner, u64::MAX).expect("wake notification take failed");
    discard(target.owner).expect("wake make-room failed");

    // 队列：15 × FILL + TAIL。逐条校验，末尾必然是被唤醒后补发的 TAIL。
    for index in 0..MAILBOX_CAPACITY {
        let message = wait_message(target.owner).expect("wake drain failed");
        let expected = if index + 1 == MAILBOX_CAPACITY {
            WRITABLE_WAKE_TAIL
        } else {
            WRITABLE_WAKE_FILL
        };
        assert_eq!(message.header.kind, expected);
    }
    // TAIL 已入队即 pm 已退出循环；此前的任何虚假唤醒都会留下 spin 位。
    assert!(matches!(
        notification::take(spin.owner, u64::MAX),
        Err(SystemCallError::ObjectNotAvailable)
    ));
    unsafe { close(done.owner) }.expect("wake done owner close failed");
    unsafe { close(spin.owner) }.expect("wake spin owner close failed");
    unsafe { close(target.owner) }.expect("wake target owner close failed");
    debug!("writable wake passed");
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
