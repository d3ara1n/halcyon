//! 根监督、服务装配与服务级验收编排。
#![no_std]
#![feature(allocator_ext)]

#[cfg(all(
    feature = "system",
    any(
        feature = "acceptance-stress",
        feature = "acceptance-device",
        feature = "acceptance-memory",
        feature = "acceptance-device-function-session",
        feature = "acceptance-mmio-publication",
        feature = "acceptance-mmio-forced",
        feature = "acceptance-dma-races",
        feature = "acceptance-dma-shared",
        feature = "acceptance-dma-quota",
        feature = "acceptance-dma-install-failure",
        feature = "acceptance-irq-retirement",
        feature = "acceptance-irq-exit"
    )
))]
compile_error!("system deployment and acceptance features are mutually exclusive");

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
use libkernel::ipc::wait_set::WaitSet;
use libkernel::ipc::{
    capability::Capability,
    message::{Mailbox, MailboxSender},
    packet::Packet,
};
use libkernel::shared::proc::Pid;
use libkernel::{
    ipc::{
        message::{create, send_raw},
        notification,
        object::{close, duplicate},
    },
    preclude::*,
    process,
    shared::{
        call::SystemCallError,
        message::HandleMove,
        object::{Handle, ObjectSignals, Rights},
        proc::{HandleGrant, JobMemberKind, ProcessState, TerminationCause},
        reset::{ResetAction, ResetReason},
        startup::initial,
        wait::WaitReason,
    },
    system,
};
use libprocess::{
    DEFAULT_SUPERVISION_POLICY, DERIVED_CONTROL_RIGHTS, JobCollector, SpawnRequest,
    SuperviseResult, SuperviseSink, SuperviseTask, SupervisionCause, SupervisionTarget,
    context::{CurrentProcessInfo, StartupPayload},
    enumerate_members,
};
use libservice::{client as service_client, protocol as service_protocol};
use lock_api::Mutex;

mod supervisor;
use supervisor::RootSupervisor;

mod deployment;
mod device;
#[cfg(feature = "acceptance-device")]
mod device_acceptance;
mod driver;
mod filesystem;
#[cfg(feature = "acceptance-device-function-session")]
mod function_checks;
mod platform;
mod supervision;
use filesystem::Host;
use filesystem::acceptance::*;

/// 受监督服务：init 保留的 control 与 pid。
struct Supervised {
    pid: u64,
    control: Handle,
}

/// 验收归档中的映像；在任何子进程出生前一次核完，缺失或重复即拒绝。
struct AcceptanceImages {
    fs: StartupImage,
    device: StartupImage,
    driver: StartupImage,
    test_fs: StartupImage,
    workers: [StartupImage; 3],
    pm: StartupImage,
    target: StartupImage,
    fp: StartupImage,
    kernel: StartupImage,
    /// 仅 stress/memory 部署需要。
    hammer: Option<StartupImage>,
}

impl AcceptanceImages {
    fn discover(stress: bool) -> Result<Self, &'static str> {
        const PATHS: [&str; 12] = [
            "bin/srv_fs",
            "bin/srv_device",
            "bin/srv_driver",
            "bin/test_fs",
            "bin/drv_pci_generic",
            "bin/drv_usb_xhci",
            "bin/drv_usb_hid",
            "bin/srv_pm",
            "bin/test_target",
            "bin/test_fp",
            "bin/test_kernel",
            "bin/test_hammer",
        ];
        let mut images = [None; PATHS.len()];
        let mut duplicate = false;
        with_init_payload(|payload| {
            tar::walk(payload, |entry| {
                if let Some(index) = PATHS.iter().position(|path| *path == entry.name) {
                    duplicate |= images[index]
                        .replace(StartupImage::from_slice(entry.data))
                        .is_some();
                }
            })
        })
        .map_err(|_| "acceptance boot archive invalid")?;
        if duplicate {
            return Err("acceptance image set duplicated");
        }
        let required = |index: usize| images[index].ok_or("acceptance image set incomplete");
        let hammer = images[11];
        if stress && hammer.is_none() {
            return Err("stress hammer image missing");
        }
        Ok(Self {
            fs: required(0)?,
            device: required(1)?,
            driver: required(2)?,
            test_fs: required(3)?,
            workers: [required(4)?, required(5)?, required(6)?],
            pm: required(7)?,
            target: required(8)?,
            fp: required(9)?,
            kernel: required(10)?,
            hammer,
        })
    }
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

#[cfg(feature = "acceptance-memory")]
const ACCEPTANCE_WORKLOAD: &str = "memory";
#[cfg(all(feature = "acceptance-stress", not(feature = "acceptance-memory")))]
const ACCEPTANCE_WORKLOAD: &str = "stress";
#[cfg(all(not(feature = "acceptance-stress"), not(feature = "acceptance-device")))]
const ACCEPTANCE_WORKLOAD: &str = "core";
#[cfg(feature = "acceptance-device")]
const ACCEPTANCE_WORKLOAD: &str = "device";

mod kernel_acceptance;
use kernel_acceptance::KernelAcceptance;

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
    primary: &Host,
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

fn root_memory_pool() -> Handle {
    INIT_STARTUP
        .lock()
        .as_ref()
        .expect("init startup inputs are not initialized")
        .root_memory_pool
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
            root.system_failure("services Job creation failed");
        }
    };
    if let Err(error) = root.bind_services(services) {
        debug!("root standby preparation failed: {:?}", error);
        root.system_failure("root standby preparation failed");
    }
    debug!("services job established");
    if cfg!(feature = "system") {
        if let Err(reason) = deployment::system(&mut root, services) {
            root.system_failure(reason);
        }
        root.system_failure("system deployment returned unexpectedly");
    }
    if let Err(stage) = run(&mut root, services, root_job) {
        debug!("init acceptance failed: {}", stage);
        root.system_failure("acceptance workload failed");
    }
    if let Err(reason) = root.finish_stop(services) {
        root.system_failure(reason);
    }
    let reset = root.take_reset();
    submit_shutdown(reset, root);
}

/// 消费已完成停止意图和原 reset authority，提交唯一正常终局动作。
fn submit_shutdown(reset: Handle, root: RootSupervisor) -> ! {
    // finish_stop 已完成唯一正常停止意图；最终动作消费同一记录，不另选政策。
    let action = root
        .stop
        .as_ref()
        .expect("normal stop completed before reset")
        .action;
    match action {
        ResetAction::Shutdown => debug!("init: submitting explicit system shutdown"),
        ResetAction::Reboot => debug!("init: submitting explicit system cold reboot"),
    }
    match system::reset(reset, action, ResetReason::Requested) {
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

/// 全部测试剧本。失败只短路后续阶段，交回 main 以 services 整树收束兜底。
fn run(root: &mut RootSupervisor, services: Handle, root_job: Handle) -> Result<(), &'static str> {
    debug!("acceptance workload: {}", ACCEPTANCE_WORKLOAD);
    root.verify_startup_capture()?;
    let mut names = TopologyNames::new();
    names.register_job(root_job, "root");
    names.register_job(services, "services");
    names.register_process(init_pid() as u64, "init");
    let acceptance = root
        .create_job(services, root_memory_pool(), JOB_FULL_RIGHTS)
        .map_err(|_| "acceptance Job create failed")?;
    let images = AcceptanceImages::discover(cfg!(feature = "acceptance-stress"))?;
    let kernel = KernelAcceptance::run(root, services, &mut names, &images)?;
    dump_topology(root_job, &names, 0);
    kernel.finish(root, acceptance)?;
    if cfg!(feature = "acceptance-memory") {
        root.collect_job(acceptance)?;
        root.close_control(acceptance)?;
        root.verify_idle_ownership()?;
        return Ok(());
    }
    dump_topology(root_job, &names, 0);
    root.begin_supervision()?;
    root.devices = Some(device::Manager::launch(
        root,
        services,
        &mut names,
        images.device.as_slice(),
    )?);
    root.drivers = Some(driver::Manager::launch(
        root,
        services,
        &mut names,
        images.driver.as_slice(),
        [
            images.workers[0].as_slice(),
            images.workers[1].as_slice(),
            images.workers[2].as_slice(),
        ],
    )?);
    let mut first_fs = Host::launch(
        root,
        services,
        &mut names,
        images.fs.as_slice(),
        srv_fs::startup::Profile::AcceptancePrimary,
        None,
    )?;
    let registry_watch = name_watch(
        root,
        first_fs
            .directory
            .as_ref()
            .ok_or("primary directory missing")?,
    )?;
    let fs_second_scope = service_client::Publisher {
        sender: first_fs
            .registration_root
            .as_ref()
            .ok_or("primary authority missing")?,
        caller: root,
    }
    .delegate_name(
        "fs.secondary",
        service_protocol::PublishRights::CREATE | service_protocol::PublishRights::REPLACE,
        filesystem::deadline()?,
    )
    .map_err(|_| "secondary publication authority failed")?;
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
        images.test_fs.as_slice(),
        &first_fs,
    )?;
    consumer.await_report(root, test_fs::report::DISCOVERY_ARMED)?;
    let mut second_fs = Host::launch(
        root,
        services,
        &mut names,
        images.fs.as_slice(),
        srv_fs::startup::Profile::AcceptanceSecondary,
        Some(fs_second_scope),
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
    filesystem::copy::start_shutdown(
        root,
        first_fs.grant.as_ref().ok_or("primary file root missing")?,
        second_fs
            .grant
            .as_ref()
            .ok_or("secondary file root missing")?,
    )?;
    root.begin_filesystem_stop(second_fs.instance)?;
    filesystem::copy::finish_shutdown(
        root,
        first_fs.grant.as_ref().ok_or("primary file root missing")?,
    )?;
    consumer.observe_shutdown(root)?;
    consumer.await_report(root, test_fs::report::PROVIDER_CLOSED)?;
    consumer.finish(root)?;
    filesystem::acceptance::track_stopped_host(root, second_fs.instance)?;
    observe_withdrawal(root, directory, &registry_watch)?;
    drop(registry_watch);
    drop(second_fs.grant.take());
    drop(second_fs.management.take());
    drop(second_fs.management_b.take());
    filesystem::acceptance::collect_stopped_pool(root, second_fs.instance)?;
    // 消费者与需要 Create 的自检已完成，提交全局 gate 前释放 primary 的验收根。
    first_fs.release_roots();
    root.request_stop(services, ResetAction::Shutdown)?;
    debug!("FS1 provider supervision reclaimed both provider processes");

    // 共同停止已完成文件与设备领域责任，释放验收保留的未读 Observer 回包。
    #[cfg(feature = "acceptance-device")]
    {
        debug!("DEVICE Drivers stopped while Observer reply backlog remained unread");
        drop(driver_reply_backlog);
    }

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

    root.verify_idle_ownership()?;
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
        match process::derive_job(job, JobMemberKind::ChildJobs, jid, DERIVED_CONTROL_RIGHTS) {
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
/// 数据面段运行体的世界：流角色与缓冲在段内共享，段末交回脚本。
#[derive(Default)]
struct ReadWorld {
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

enum InitTask {
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
            Self::Rpc { deadline } => *deadline,
            Self::FsWatch(task) => task.deadline(),
            Self::ServiceWatch(task) => task.deadline(),
            Self::Copy { task, .. } => task.deadline(),
        }
    }
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
