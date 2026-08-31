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

use libprocess::{DERIVED_CONTROL_RIGHTS, SpawnRequest, enumerate_members, job_kill, spawn};
use librunnel::blocking;
#[cfg(feature = "acceptance-stress")]
use rinlib::ipc::tunnel as tunnel_sys;
#[cfg(feature = "acceptance-stress")]
use rinlib::shared::proc::ProcessDrainStatus;
use rinlib::{
    env,
    ipc::{
        message::{create, discard, make_send_once, mint_sender, receive, send, wait_message},
        notification,
        object::{close, duplicate},
        wait::wait_many,
    },
    mm::{MappedRegion, Placement},
    preclude::*,
    process,
    shared::{
        call::SystemCallError,
        mem::MemoryProtection,
        message::{HandleMove, MAILBOX_CAPACITY},
        object::{Handle, ObjectSignals, Rights},
        proc::{
            ExecutionProfile, HandleGrant, JobMemberKind, JobState, ProcessExitReason, ProcessState,
        },
        reset::{ResetAction, ResetReason},
        startup::initial,
        wait::{WAIT_TIMEOUT_INFINITE, WaitItem},
    },
    system,
};

mod building;
#[cfg(feature = "acceptance-stress")]
mod race;
use building::build_spin_building;
#[cfg(feature = "acceptance-stress")]
use race::race_matrix;

/// 受监督服务：init 保留的 control 与 pid。
struct Supervised {
    pid: u64,
    control: Handle,
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

/// 隧道页在本进程的映射地址（VA 分配器落地前由调用方自报）。
const TUNNEL_VA: usize = 0x4000_0000;
#[cfg(feature = "acceptance-stress")]
const LIFECYCLE_VA: usize = TUNNEL_VA + 0x1000;
#[cfg(feature = "acceptance-stress")]
const FAILED_ATTACH_VA: usize = TUNNEL_VA + 0x2000;
/// 验证数据量：超过环形容量（3968），强制写端分批与回绕。
const STREAM_LEN: usize = 8192;
#[cfg(feature = "acceptance-stress")]
const CONTROL_STRESS: usize = 128;
#[cfg(feature = "acceptance-stress")]
const TUNNEL_STRESS: usize = 64;
#[cfg(feature = "acceptance-stress")]
const ACCEPTANCE_WORKLOAD: &str = "stress";
#[cfg(not(feature = "acceptance-stress"))]
const ACCEPTANCE_WORKLOAD: &str = "core";

/// 从 initfs 私有政策（ustar 字母序）启动服务拓扑：普通服务入 services
/// Job；srv_pm 额外经 StartupBlock grants 取得 pm_domain 委托域的
/// JobControl；test_target 起两个实例——acceptance 域的派生 kill 靶
/// （验收线 1）与 pm_domain 的委托靶（control 即弃，pm 派生走铸造路径）。
fn launch_test_services(
    services: Handle,
    pm_domain: Handle,
    acceptance: Handle,
    names: &mut TopologyNames,
) -> Result<
    (
        Handle,
        alloc::vec::Vec<Supervised>,
        Option<alloc::vec::Vec<u8>>,
        Option<alloc::vec::Vec<u8>>,
    ),
    &'static str,
> {
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
    let mut pm_started = false;
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
            job,
            image: entry.data,
            payload: &[],
            grants,
            control_rights,
        }) {
            Ok(process) => {
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
                        job: pm_domain,
                        image: entry.data,
                        payload: &[],
                        grants: &[],
                        control_rights,
                    }) {
                        Ok(second) => {
                            debug!("pm domain target started as pid {}", second.pid);
                            names.register_process(second.pid, "bin/test_target@pm_domain");
                            let _ = close(second.control);
                        }
                        Err(error) => {
                            debug!("pm domain target spawn failed: {:?}", error);
                        }
                    }
                } else {
                    supervised.push(Supervised {
                        pid: process.pid,
                        control: process.control,
                    });
                }
                if entry.name == "bin/srv_pm" {
                    pm_started = true;
                }
            }
            Err(error) => debug!("failed to start {}: {:?}", entry.name, error),
        }
    });
    if let Err(error) = result {
        debug!("initfs parse failed: {:?}", error);
    }
    if !pm_started {
        let _ = close(pm_mailbox.owner);
        let _ = close(pm_mailbox.peer);
        // 已启动的服务交回 main 的整树收束（job_kill(services)）兜底。
        return Err("pm service launch failed");
    }
    Ok((pm_mailbox.peer, supervised, target_image, hammer_image))
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
        // 整树收束：seal + 逐成员 kill + 子域递归 + CLOSED 屏障，一次性
        // 覆盖 services 全部成员与委托/验收子域。
        if let Err(error) = job_kill(services, 0x1F) {
            debug!("failure-path teardown degraded: {:?}", error);
        }
        panic!("init acceptance failed: {}", stage);
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
    close(attenuated).expect("attenuated SystemReset close must succeed");
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
fn kill_and_supervise(supervised: alloc::vec::Vec<Supervised>) {
    for target in &supervised {
        let _ = process::kill(target.control, 0x1F);
    }
    if let Err(error) = supervise_services(supervised) {
        debug!("failure-path supervision degraded: {:?}", error);
    }
}

fn test_memory_mapping() -> Result<(), &'static str> {
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
    debug!("public memory mapping acceptance passed");
    Ok(())
}

/// 全部测试剧本。失败只短路后续阶段，交回 main 以 services 整树收束兜底。
fn run(services: Handle) -> Result<(), &'static str> {
    debug!("acceptance workload: {}", ACCEPTANCE_WORKLOAD);
    let root_job = env::startup_handle(initial::ROOT_JOB).expect("init must hold root JobControl");
    test_memory_mapping()?;
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
    let (pm_mailbox, mut supervised, target_image, hammer_image) =
        launch_test_services(services, pm_domain, acceptance, &mut names)?;

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
    match send(pair.peer, 114, &[5u8, 1u8, 4u8], &moves) {
        Ok(()) => match receive(pair.owner) {
            Ok(message) => {
                debug!(
                    "message: kind={}, payload={:?}",
                    message.header.kind, message.payload
                );
                let moved = message.handles[0];
                notification::signal(moved, 0x5).expect("notification signal failed");
                let result = wait_many(
                    &[WaitItem::new(event.owner, ObjectSignals::READABLE, 7)],
                    WAIT_TIMEOUT_INFINITE,
                )
                .expect("notification wait failed");
                let bits =
                    notification::take(event.owner, u64::MAX).expect("notification take failed");
                debug!("notification: cookie={}, bits={:#x}", result.cookie, bits);
                let _ = close(moved);
            }
            Err(e) => debug!("receive failed: {:?}", e),
        },
        Err(e) => debug!("send failed: {:?}", e),
    }
    let _ = close(event.owner);
    let _ = close(pair.peer);
    let _ = close(pair.owner);

    test_capability_badges_and_affine_owners();
    #[cfg(feature = "acceptance-stress")]
    stress_control_plane();
    #[cfg(feature = "acceptance-stress")]
    test_tunnel_lifecycle();
    test_send_once();
    test_writable_level();

    // —— 数据面：建隧道 → Invitation 经消息面转移 → 阻塞读流 ——
    let (mut tunnel, invitation) = match blocking::create_consumer(TUNNEL_VA) {
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
    if let Err(e) = send(pm_mailbox, 514, &[], &invitation_move) {
        debug!("send tunnel invitation failed: {:?}", e);
        return Err("send tunnel invitation failed");
    }

    let mut buf = [0u8; STREAM_LEN];
    match tunnel.read_exact_or_eof(&mut buf) {
        Ok(n) => {
            let ok = buf
                .iter()
                .enumerate()
                .all(|(i, &b)| b == (i % 251 + 1) as u8);
            debug!(
                "stream received {} bytes, pattern {}",
                n,
                if ok { "ok" } else { "MISMATCH" }
            );
        }
        Err(e) => debug!("stream read failed: {:?}", e),
    }

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
    let _ = close(acceptance);

    // —— 监督闭环：等待全部服务 REAPABLE/CLOSED，Drain 至 Complete，
    // 查询稳定终态后释放 control。对象 close 回调（含 pm 隧道端点的
    // PEER_CLOSED 发布）发生在 Drain 期间——监督先于对端终态等待。 ——
    if let Err(error) = supervise_services(supervised) {
        debug!("service supervision failed: {:?}", error);
        return Err("service supervision failed");
    }
    debug!("all services supervised to completion");

    // —— 事件面：对端终态位（Drain 已置位，电平等待立即返回）——
    let items = [WaitItem::new(
        tunnel.handle(),
        ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED,
        0,
    )];
    let peer_closed = wait_many(&items, WAIT_TIMEOUT_INFINITE).map_err(|error| {
        debug!("peer-closed wait failed: {:?}", error);
        "peer-closed wait failed"
    })?;
    debug!(
        "peer closed observed: bits={:#x}",
        peer_closed.observed.raw()
    );
    let _ = tunnel.close();

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
    debug!("topology: final snapshot before system reset");
    dump_topology(root_job, &names, 0);
    let _ = close(pm_domain);
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
            let _ = close(control);
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
                let _ = close(child);
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
) -> Result<usize, SystemCallError> {
    let mut reclaimed = 0;
    let mut index = 0;
    while index < supervised.len() {
        let state = process::query(supervised[index].control)?.state;
        if state == ProcessState::Building as u32 || state == ProcessState::Running as u32 {
            index += 1;
            continue;
        }
        let target = supervised.swap_remove(index);
        let mut ready = alloc::vec::Vec::with_capacity(1);
        ready.push(target);
        supervise_services(ready)?;
        reclaimed += 1;
    }
    Ok(reclaimed)
}

/// 监督循环：对保留的每个 control 等待 REAPABLE|CLOSED，Drain 至
/// Complete，再以固定宽快照确认终态。逐项推进，全部完成后返回。
fn supervise_services(mut supervised: alloc::vec::Vec<Supervised>) -> Result<(), SystemCallError> {
    while !supervised.is_empty() {
        let items: alloc::vec::Vec<WaitItem> = supervised
            .iter()
            .enumerate()
            .map(|(index, s)| {
                WaitItem::new(
                    s.control,
                    ObjectSignals::REAPABLE | ObjectSignals::CLOSED,
                    index as u64,
                )
            })
            .collect();
        let result = wait_many(&items, WAIT_TIMEOUT_INFINITE)?;
        let index = result.cookie as usize;
        let Some(target) = supervised.get(index) else {
            break;
        };
        let pid = target.pid;
        let control = target.control;
        let drained = process::drain_to_completion(control);
        let snapshot = process::query(control);
        match (drained, snapshot) {
            (Ok(work), Ok(snapshot)) => {
                debug!(
                    "pid {} supervised: work={}, state={}, reason={}, code={}",
                    pid, work, snapshot.state, snapshot.reason, snapshot.code
                );
            }
            (work, query) => {
                debug!(
                    "pid {} supervision degraded: drain={:?} query={:?}",
                    pid, work, query
                );
            }
        }
        let _ = close(control);
        supervised.swap_remove(index);
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
        let _ = close(created.builder);
        let _ = close(created.control);
        return;
    }
    let _ = close(created.builder);
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
    let _ = close(created.control);
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
            let _ = close(retained);
            process::kill(control, 0x77).expect("derived-control kill must be accepted");
            kill_and_supervise(alloc::vec::Vec::from([Supervised { pid, control }]));
        }
        Err(error) => {
            debug!("derive kill degraded ({:?}); using retained control", error);
            process::kill(retained, 0x77).expect("live kill of a fresh process must be accepted");
            kill_and_supervise(alloc::vec::Vec::from([Supervised {
                pid,
                control: retained,
            }]));
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
    let grants = [HandleGrant {
        handle: marker.owner,
        rights: Rights::READ | Rights::WAIT,
    }];
    let result = (|| {
        let started = spawn(SpawnRequest {
            job,
            image,
            payload: &[],
            grants: &grants,
            control_rights: SUPERVISOR_RIGHTS,
        })
        .map_err(|error| {
            debug!("drain minimum-budget acceptance failed: spawn {:?}", error);
            let _ = close(marker.owner);
            "drain minimum-budget spawn failed"
        })?;
        if let Err(error) = process::kill(started.control, 0x5D) {
            debug!("drain minimum-budget acceptance failed: kill {:?}", error);
            let _ = close(started.control);
            return Err("drain minimum-budget kill failed");
        }
        if let Err(error) = wait_many(
            &[WaitItem::new(
                started.control,
                ObjectSignals::REAPABLE | ObjectSignals::CLOSED,
                0,
            )],
            WAIT_TIMEOUT_INFINITE,
        ) {
            debug!("drain minimum-budget acceptance failed: wait {:?}", error);
            let _ = close(started.control);
            return Err("drain minimum-budget wait failed");
        }
        let mut batches = 0usize;
        let drained = loop {
            batches += 1;
            if batches > 16_384 {
                break Err(SystemCallError::InternalError);
            }
            match process::drain(started.control, 1) {
                Ok(result) if result.status == ProcessDrainStatus::Complete as u32 => break Ok(()),
                Ok(_) => continue,
                Err(error) => break Err(error),
            }
        };
        debug!(
            "drain minimum-budget acceptance {}: {} batches",
            if drained.is_ok() { "passed" } else { "failed" },
            batches
        );
        let _ = close(started.control);
        drained.map_err(|_| "drain minimum-budget did not complete")
    })();
    let _ = close(marker.peer);
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
    let waited = wait_many(
        &[WaitItem::new(child, ObjectSignals::CLOSED, 0)],
        WAIT_TIMEOUT_INFINITE,
    );
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
    let _ = close(child);
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
        let _ = close(created.builder);
        let _ = close(created.control);
        return;
    }
    // 终因已冻结为 Killed；builder 关闭的 abandonment 竞争不覆盖。
    let _ = close(created.builder);
    // control 消散：无人收束，只能靠枚举+派生接管。
    let _ = close(created.control);
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
            let _ = close(control);
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
/// + 一个 Building 成员，两者 control 均消散——libprocess::job_kill 一把
/// 收束（seal → 枚举 → 派生 kill → drain → 等 CLOSED 全链，派生走铸造
/// 路径）。
fn test_job_kill_composition(job: Handle, image: &[u8]) {
    let Ok(child) = process::create_job(job, JOB_FULL_RIGHTS) else {
        debug!("job kill composition FAILED: job create failed");
        return;
    };
    let running = spawn(SpawnRequest {
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
            let _ = close(running.control);
            let _ = close(building.control);
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
    let _ = close(child);
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
        let _ = close(child);
        return;
    };
    let sealed = process::seal_job(child);
    let started = process::start(created.builder, ExecutionProfile::Base64 as u32);
    let gated = matches!(started, Err(SystemCallError::ObjectClosed));
    // 收束：kill → drain → sealed+空完成 → CLOSED。
    let _ = process::kill(created.control, 0x3D);
    let _ = process::drain_to_completion(created.control);
    let _ = close(created.control);
    let _ = close(created.builder);
    let waited = wait_many(
        &[WaitItem::new(child, ObjectSignals::CLOSED, 0)],
        WAIT_TIMEOUT_INFINITE,
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
    let _ = close(child);
}

/// seal 先于 Create：封口后成员/子 Job 创建口永久关闭（ObjectClosed）；
/// 空封口立即完成并发布 CLOSED。
fn seal_before_create(job: Handle) {
    let Ok(child) = process::create_job(job, JOB_FULL_RIGHTS) else {
        debug!("seal gate (create) FAILED: job create failed");
        return;
    };
    let sealed = process::seal_job(child);
    let waited = wait_many(
        &[WaitItem::new(child, ObjectSignals::CLOSED, 0)],
        WAIT_TIMEOUT_INFINITE,
    );
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
        let _ = close(handle);
    }
    let _ = close(child);
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
            let _ = close(created.builder);
            let _ = close(created.control);
            continue;
        }
        let _ = close(created.builder);
        if let Err(error) = process::drain_to_completion(created.control) {
            ok = false;
            debug!("enumerate convergence FAILED: drain {:?}", error);
        }
        let _ = close(created.control);
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

    send(mailbox.peer, 880, &[], &[]).expect("default sender send failed");
    send(badged, 881, &[], &[]).expect("badged sender send failed");
    send(copy, 882, &[], &[]).expect("badged sender copy send failed");
    for (kind, badge) in [(880, 0), (881, BADGE), (882, BADGE)] {
        let message = receive(mailbox.owner).expect("badged message receive failed");
        assert_eq!(message.header.kind, kind);
        assert_eq!(message.header.sender_pid, env::pid() as u64);
        assert_eq!(message.header.sender_badge, badge);
    }
    let once = make_send_once(badged, Rights::WRITE).expect("badged send-once mint failed");
    send(once, 887, &[], &[]).expect("badged send-once send failed");
    let message = receive(mailbox.owner).expect("badged send-once receive failed");
    assert_eq!(message.header.sender_badge, BADGE);

    let transit =
        duplicate(badged, Rights::WRITE | Rights::TRANSIT).expect("badged transit copy failed");
    let moves = [HandleMove {
        handle: transit,
        rights: Rights::WRITE,
    }];
    send(transport.peer, 888, &[], &moves).expect("badged sender transit failed");
    let transferred = receive(transport.owner)
        .expect("badged sender transit receive failed")
        .handles[0];
    send(transferred, 889, &[], &[]).expect("transferred badged sender send failed");
    let message = receive(mailbox.owner).expect("transferred badged message receive failed");
    assert_eq!(message.header.sender_badge, BADGE);
    close(transferred).expect("transferred badged sender close failed");
    close(copy).expect("badged sender copy close failed");
    close(badged).expect("badged sender close failed");

    assert!(matches!(
        duplicate(mailbox.owner, Rights::READ),
        Err(SystemCallError::RightsDenied)
    ));
    let owner_move = [HandleMove {
        handle: mailbox.owner,
        rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
    }];
    assert!(matches!(
        send(transport.peer, 883, &[], &owner_move),
        Err(SystemCallError::RightsDenied)
    ));
    send(mailbox.peer, 884, &[], &[]).expect("owner must survive rejected transit");
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
        send(transport.peer, 886, &[], &owner_move),
        Err(SystemCallError::RightsDenied)
    ));
    notification::signal(event.peer, 1).expect("notification owner must survive rejected transit");
    assert_eq!(
        notification::take(event.owner, 1)
            .expect("notification take after rejected transit failed"),
        1
    );

    close(event.peer).expect("notification signaler close failed");
    close(event.owner).expect("notification owner close failed");
    close(mailbox.peer).expect("default sender close failed");
    close(mailbox.owner).expect("mailbox owner close failed");
    close(transport.peer).expect("owner transport sender close failed");
    close(transport.owner).expect("owner transport owner close failed");
    debug!("capability badge and owner transport passed");
}

/// 一次性投递权（send-once）：本进程内验证 mint、用后即摘、
/// 经消息转移后由接收方一次性使用，以及原 sender 不受影响。
fn test_send_once() {
    let mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("send-once mailbox create failed");
    let once = make_send_once(mailbox.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        .expect("make send once failed");
    send(once, 900, &[1], &[]).expect("send once failed");
    assert!(matches!(
        send(once, 901, &[], &[]),
        Err(SystemCallError::StaleHandle)
    ));

    let once = make_send_once(mailbox.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        .expect("transferred send-once mint failed");
    let moves = [HandleMove {
        handle: once,
        rights: Rights::WRITE,
    }];
    send(mailbox.peer, 902, &[], &moves).expect("send-once transit failed");
    let first = receive(mailbox.owner).expect("send-once receive failed");
    assert_eq!(first.header.kind, 900);
    let second = receive(mailbox.owner).expect("send-once transit receive failed");
    assert_eq!(second.header.kind, 902);
    send(second.handles[0], 903, &[2], &[]).expect("transferred once send failed");
    assert!(matches!(
        send(second.handles[0], 904, &[], &[]),
        Err(SystemCallError::StaleHandle)
    ));

    // 原 sender 仍可长期使用，不受派生影响。
    send(mailbox.peer, 905, &[], &[]).expect("original sender still usable");
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
        send(full.peer, 0, &[], &[]).expect("send-once full fill failed");
    }
    assert!(matches!(
        send(once, 910, &[], &[]),
        Err(SystemCallError::MailboxFull)
    ));
    discard(full.owner).expect("send-once full make-room failed");
    send(once, 911, &[], &[]).expect("failed send must not consume once");
    assert!(matches!(
        send(once, 912, &[], &[]),
        Err(SystemCallError::StaleHandle)
    ));
    for _ in 0..MAILBOX_CAPACITY {
        discard(full.owner).expect("send-once full drain failed");
    }
    close(full.peer).expect("send-once full sender close failed");
    close(full.owner).expect("send-once full owner close failed");

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
        send(once, 920, &[], &moves),
        Err(SystemCallError::IllegalArgument)
    ));
    send(once, 921, &[], &[]).expect("rejected alias must not consume send-once");
    assert!(matches!(
        send(once, 922, &[], &[]),
        Err(SystemCallError::StaleHandle)
    ));
    let message = receive(both.owner).expect("send-once alias recovery receive failed");
    assert_eq!(message.header.kind, 921);
    assert!(message.handles.is_empty());
    close(both.peer).expect("send-once both sender close failed");
    close(both.owner).expect("send-once both owner close failed");
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
        WAIT_TIMEOUT_INFINITE,
    )
    .expect("empty mailbox must be writable");
    assert!(result.observed.intersects(ObjectSignals::WRITABLE));

    for _ in 0..MAILBOX_CAPACITY {
        send(mailbox.peer, 0, &[], &[]).expect("writable fill failed");
    }
    discard(mailbox.owner).expect("writable make-room failed");
    let result = wait_many(
        &[WaitItem::new(mailbox.peer, ObjectSignals::WRITABLE, 2)],
        WAIT_TIMEOUT_INFINITE,
    )
    .expect("mailbox below capacity must be writable");
    assert!(result.observed.intersects(ObjectSignals::WRITABLE));

    for _ in 0..MAILBOX_CAPACITY - 1 {
        discard(mailbox.owner).expect("writable drain failed");
    }
    close(mailbox.peer).expect("writable sender close failed");
    close(mailbox.owner).expect("writable owner close failed");
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
    send(pm_mailbox, WRITABLE_WAKE_REQUEST, &[], &moves).expect("wake request send failed");

    // pm 确认已满后置位通知，此时它正阻塞在 WRITABLE 上。
    wait_many(
        &[WaitItem::new(done.owner, ObjectSignals::READABLE, 0)],
        WAIT_TIMEOUT_INFINITE,
    )
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
    close(done.owner).expect("wake done owner close failed");
    close(spin.owner).expect("wake spin owner close failed");
    close(target.owner).expect("wake target owner close failed");
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
        send(mailbox.peer, index as u64, &index.to_le_bytes(), &moves).expect("stress send failed");
        assert!(matches!(
            close(event.peer),
            Err(SystemCallError::StaleHandle)
        ));
        let message = receive(mailbox.owner).expect("stress receive failed");
        assert_eq!(message.header.kind, index as u64);
        assert_eq!(message.payload, index.to_le_bytes());
        notification::signal(message.handles[0], 1).expect("stress signal failed");
        assert_eq!(
            notification::take(event.owner, 1).expect("stress take failed"),
            1
        );
        close(message.handles[0]).expect("stress moved handle close failed");
        close(event.owner).expect("stress notification owner close failed");
        close(mailbox.peer).expect("stress mailbox sender close failed");
        close(mailbox.owner).expect("stress mailbox owner close failed");
    }

    let mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("full mailbox create failed");
    for _ in 0..MAILBOX_CAPACITY {
        send(mailbox.peer, 0, &[], &[]).expect("mailbox fill failed");
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
        send(mailbox.peer, 0, &[], &moves),
        Err(SystemCallError::MailboxFull)
    ));
    notification::signal(event.peer, 1).expect("failed Send must retain moved source");
    for _ in 0..MAILBOX_CAPACITY {
        discard(mailbox.owner).expect("mailbox discard failed");
    }
    close(event.peer).expect("retained signaler close failed");
    close(event.owner).expect("retained owner close failed");
    close(mailbox.peer).expect("full mailbox sender close failed");
    close(mailbox.owner).expect("full mailbox owner close failed");
    debug!(
        "control-plane stress passed: {} transactions",
        CONTROL_STRESS
    );
}

#[cfg(feature = "acceptance-stress")]
fn test_tunnel_lifecycle() {
    for _ in 0..TUNNEL_STRESS {
        let abandoned = tunnel_sys::create(LIFECYCLE_VA).expect("lifecycle tunnel create failed");
        assert!(matches!(
            wait_many(
                &[WaitItem::new(abandoned.peer, ObjectSignals::CLOSED, 0)],
                WAIT_TIMEOUT_INFINITE,
            ),
            Err(SystemCallError::RightsDenied)
        ));
        close(abandoned.peer).expect("invitation close failed");
        let result = wait_many(
            &[WaitItem::new(
                abandoned.owner,
                ObjectSignals::PEER_CLOSED,
                0,
            )],
            WAIT_TIMEOUT_INFINITE,
        )
        .expect("abandoned invitation wait failed");
        assert!(result.observed.intersects(ObjectSignals::PEER_CLOSED));
        close(abandoned.owner).expect("lifecycle endpoint close failed");

        let creator_closed =
            tunnel_sys::create(LIFECYCLE_VA).expect("closed-creator tunnel create failed");
        close(creator_closed.owner).expect("creator endpoint close failed");
        assert!(matches!(
            tunnel_sys::attach(creator_closed.peer, FAILED_ATTACH_VA),
            Err(SystemCallError::ObjectClosed)
        ));
        close(creator_closed.peer).expect("closed invitation close failed");
    }
    debug!("tunnel lifecycle stress passed: {} rounds", TUNNEL_STRESS);
}
