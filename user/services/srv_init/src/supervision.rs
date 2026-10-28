//! root独占部署实例的DONE观察、领域停止事实与唯一系统故障终局。
use super::supervisor::{Await, RootSupervisor};
use alloc::{sync::Arc, vec::Vec};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    proc::{ProcessSnapshot, ProcessState, TerminationCause, TerminationDetail},
    time::Deadline,
};
use libbudget::AccountView;
use libexecution::{
    ExecutionResource, Runtime, SourceRegistration, TaskSubmission,
    assembly::Assembly,
    runtime::{
        Advance, Input, RequestFailure, Requests, SourceId, SourceKind, SourcePlan, Step, Task,
    },
};
use libkernel::{
    ipc::{capability::Capability, message::MailboxSender, object, wait, wait_set::WaitSet},
    process,
};
use libprocess::{JobCollector, JobCollectorEvent, SupervisionPolicy};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Instance {
    Device,
    Driver,
    Filesystem,
    Secondary,
}
impl Instance {
    pub(super) fn is_filesystem(self) -> bool {
        matches!(self, Self::Filesystem | Self::Secondary)
    }

    pub(super) fn index(self) -> usize {
        match self {
            Self::Device => 0,
            Self::Driver => 1,
            Self::Filesystem => 2,
            Self::Secondary => 3,
        }
    }
}
struct SourceAdmin {
    entry: Arc<MailboxSender>,
    server: u64,
}
pub(super) struct Terminal {
    job: Capability,
    collector: JobCollector,
    platform: Vec<libpci::platform::Control>,
    reset: Capability,
}
fn owned_alias(handle: Handle, rights: Rights) -> Result<Capability, SystemCallError> {
    let raw = object::duplicate(handle, rights)?;
    // SAFETY: duplicate创建的新entry关闭责任唯一交给本owner。
    Ok(unsafe { Capability::from_raw(raw) })
}
impl Terminal {
    pub(super) fn prepare(job: Handle, reset: Handle) -> Result<Self, SystemCallError> {
        let job = owned_alias(job, Rights::READ | Rights::WAIT | Rights::MANAGE)?;
        let collector = JobCollector::new(
            job.as_handle(),
            SupervisionPolicy {
                wait_timeout_ms: 100,
                wait_attempts: 4,
                query_attempts: 4,
                enumerate_stalls: 8,
            },
        )?;
        let reset = owned_alias(reset, Rights::MANAGE)?;
        let facts = {
            let inputs = super::INIT_STARTUP.lock();
            let inputs = inputs.as_ref().ok_or(SystemCallError::ObjectNotAvailable)?;
            let mut facts = Vec::new();
            facts
                .try_reserve_exact(inputs._platform.platform().controls().len())
                .map_err(|_| SystemCallError::OutOfMemory)?;
            for root in inputs._platform.platform().controls() {
                let kind = match root {
                    libpci::platform::Control::Mmio { ecam, .. } => {
                        if *ecam {
                            0
                        } else {
                            1
                        }
                    }
                    libpci::platform::Control::Irq(_) => 2,
                    libpci::platform::Control::Dma(_) => 3,
                };
                facts.push((root.handle(), kind));
            }
            facts
        };
        let mut platform = Vec::new();
        platform
            .try_reserve_exact(facts.len())
            .map_err(|_| SystemCallError::OutOfMemory)?;
        for (handle, kind) in facts {
            let alias = owned_alias(handle, Rights::MANAGE)?.into_raw();
            // SAFETY: kind来自init已认证的实际平台Control，不来自payload；新entry唯一移交。
            platform.push(unsafe {
                match kind {
                    0 | 1 => libpci::platform::Control::Mmio {
                        control: libkernel::mmio::MmioControl::from_handle(alias),
                        ecam: kind == 0,
                    },
                    2 => libpci::platform::Control::Irq(libkernel::irq::IrqControl::from_handle(
                        alias,
                    )),
                    _ => libpci::platform::Control::Dma(
                        libkernel::dma::DmaDomainControl::from_handle(alias),
                    ),
                }
            });
        }
        Ok(Self {
            job,
            collector,
            platform,
            reset,
        })
    }
    fn run(mut self, admins: [Option<SourceAdmin>; 2], reason: &str) -> ! {
        libkernel::debug!(
            "system failure terminal entered: {}; hardware completion is unproven",
            reason
        );
        match process::seal_job(self.job.as_handle()) {
            Ok(()) => libkernel::debug!("system failure JobSeal committed"),
            Err(error) => libkernel::debug!("system failure JobSeal unconfirmed: {:?}", error),
        }
        let deadline = libkernel::time::timeout_millis(5_000).unwrap_or(Deadline::at(0));
        for admin in admins.iter().flatten() {
            let sealed = super::device::call_until(
                &mut librpc::Caller::new(),
                &admin.entry,
                libdevice::protocol::Command::SealSource,
                deadline,
            )
            .and_then(|reply| {
                let (header, body) = libdevice::protocol::Header::decode(&reply.payload)
                    .map_err(|_| "fault source seal reply invalid")?;
                if reply.sender_pid != admin.server
                    || header.op != 2
                    || header.error.is_some()
                    || !body.is_empty()
                    || reply.handles.remaining() != 0
                {
                    return Err("fault source seal unconfirmed");
                }
                Ok(())
            });
            if let Err(error) = sealed {
                libkernel::debug!("system failure source remains unconfirmed: {}", error);
            } else {
                libkernel::debug!(
                    "system failure source Seal confirmed: server={}",
                    admin.server
                );
            }
        }
        for root in &self.platform {
            let result = match root {
                libpci::platform::Control::Mmio { control, .. } => control.seal(),
                libpci::platform::Control::Irq(control) => control.seal(),
                libpci::platform::Control::Dma(control) => control.seal(),
            };
            if let Err(error) = result {
                libkernel::debug!("system failure resource Seal unconfirmed: {:?}", error);
            } else {
                libkernel::debug!("system failure platform root Seal committed");
            }
        }
        // 原JobCollector负责枚举、Kill与实际DONE；升级界只截断等待，不生成完成事实。
        while let Ok(now) = libkernel::time::snapshot() {
            if now.now_ns >= deadline.at_ns {
                libkernel::debug!(
                    "system failure execution stop deadline reached; isolation retained"
                );
                break;
            }
            let outcome = match self.collector.step(now.now_ns) {
                Ok(event) => event,
                Err(error) => {
                    libkernel::debug!(
                        "system failure execution collection incomplete: {:?}",
                        error
                    );
                    break;
                }
            };
            match outcome {
                JobCollectorEvent::Done => {
                    libkernel::debug!("system failure execution job collection complete");
                    break;
                }
                JobCollectorEvent::Progress => {}
                JobCollectorEvent::RetryAt(at) => {
                    if libkernel::time::sleep_until(Deadline::at(at.min(deadline.at_ns))).is_err() {
                        break;
                    }
                }
                JobCollectorEvent::Observe(request) => {
                    let bounded = libprocess::observation::Observation {
                        deadline: Deadline::at(request.deadline.at_ns.min(deadline.at_ns)),
                        ..request
                    };
                    match libprocess::observation::wait(bounded) {
                        Ok((now, result)) => {
                            if now >= deadline.at_ns {
                                // 总升级界已到，不向原观察伪造提前超时或完成回执。
                                break;
                            }
                            if let Err(error) = self.collector.observe(request, result, now) {
                                libkernel::debug!(
                                    "system failure execution observation incomplete: {:?}",
                                    error
                                );
                                break;
                            }
                        }
                        Err(error) => {
                            libkernel::debug!(
                                "system failure execution observation failed: {:?}",
                                error
                            );
                            break;
                        }
                    }
                }
            }
        }
        // 这些owner继续保留，既不声称全部Job/Pool结束，也不以Close触发活DMA退款。
        core::hint::black_box((&self, &admins));
        libkernel::debug!("system failure requesting cold reboot: SystemFailure");
        match libkernel::system::reset(
            self.reset.as_handle(),
            erhino_shared::reset::ResetAction::Reboot,
            erhino_shared::reset::ResetReason::SystemFailure,
        ) {
            Ok(never) => match never {},
            Err(error) => {
                libkernel::debug!(
                    "system failure cold reboot rejected: {:?}; terminating init",
                    error
                );
                // SAFETY: 唯一终局owner已完成有界升级；内核独立保留活DMA，boot观察整个init DONE。
                unsafe { libkernel::sys_process_abort() }
            }
        }
    }
}
enum ProcessOwner {
    Building(libprocess::PreparedProcess),
    Running(Capability),
}
impl ProcessOwner {
    fn control(&self) -> Handle {
        match self {
            Self::Building(prepared) => prepared.control(),
            Self::Running(control) => control.as_handle(),
        }
    }
}
struct Record {
    pid: u64,
    process: Option<ProcessOwner>,
    admin: Option<SourceAdmin>,
    stopping: Option<Deadline>,
    device_closed: bool,
    done: Option<ProcessSnapshot>,
    detach: bool,
    detached: bool,
    filesystem: Option<super::filesystem::hosting::Bootstrap>,
}
impl Record {
    fn new() -> Self {
        Self {
            pid: 0,
            process: None,
            admin: None,
            stopping: None,
            device_closed: false,
            done: None,
            detach: false,
            detached: false,
            filesystem: None,
        }
    }
    fn stopped(&self) -> bool {
        self.filesystem
            .as_ref()
            .map_or(self.device_closed, |fs| fs.stopped)
    }
    fn started(&self) -> bool {
        matches!(self.process, Some(ProcessOwner::Running(_)))
    }
}
struct World {
    records: Vec<Record>,
    failure: Option<&'static str>,
    admission_closed: bool,
}
struct InstanceTask {
    role: Instance,
    source: SourceRegistration,
    done_ready: bool,
    reports: SourceRegistration,
    report_ready: bool,
    deadline: Deadline,
}
impl Task<World> for InstanceTask {
    type Family = Self;
    fn advance(
        &mut self,
        _id: u64,
        world: &mut World,
        requests: &mut Requests<'_, Self>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Result<Advance, SystemCallError> {
        let record = &mut world.records[self.role.index()];
        while let Some(event) = input.pull() {
            if event.kind == 1 {
                self.done_ready |= event.observed.contains(ObjectSignals::DONE);
            }
            if event.kind == 2 {
                self.report_ready = true;
            }
        }
        input.take_timeout();
        self.deadline = record
            .stopping
            .or_else(|| {
                record
                    .filesystem
                    .as_ref()
                    .and_then(|fs| (!fs.ready).then_some(fs.start_deadline))
            })
            .unwrap_or(Deadline::INFINITE);
        if record.detach {
            self.deadline = Deadline::INFINITE;
            self.source.remove(requests)?;
            self.reports.remove(requests)?;
            record.detached = self.source.is_empty() && self.reports.is_empty();
            return Ok(Advance {
                work_done: 1,
                step: if record.detached {
                    Step::Complete
                } else {
                    Step::Runnable
                },
            });
        }
        let Some(owner) = &mut record.process else {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        };
        let control = owner.control();
        self.source
            .install(requests, SourcePlan::new(control, ObjectSignals::DONE), 1)?;
        if let Some(fs) = &record.filesystem {
            self.reports.install(
                requests,
                SourcePlan::new(fs.receiver, ObjectSignals::READABLE),
                2,
            )?;
        }
        if self.source.is_installing() || self.reports.is_installing() {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if let ProcessOwner::Building(prepared) = owner {
            if world.admission_closed || record.stopping.is_some() {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            if record
                .filesystem
                .as_ref()
                .is_some_and(|fs| input.now_ns() >= fs.start_deadline.at_ns)
            {
                world.failure = Some("filesystem Start deadline expired");
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            libkernel::debug!(
                "instance observer admitted before Start: role={:?}, pid={}, token={}",
                self.role,
                record.pid,
                self.source
                    .source()
                    .expect("instance source attached")
                    .token()
            );
            let spawned = prepared.try_start()?;
            // SAFETY: Start 成功，唯一 Control 从 PreparedProcess 移入本记录。
            *owner = ProcessOwner::Running(unsafe { Capability::from_raw(spawned.control) });
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        let expired = record
            .stopping
            .is_some_and(|end| input.now_ns() >= end.at_ns);
        if expired && record.done.is_none() {
            self.done_ready |=
                wait::probe(control, ObjectSignals::DONE)?.contains(ObjectSignals::DONE);
        }
        if self.done_ready && record.done.is_none() {
            let snapshot = process::query(control)?;
            libkernel::debug!(
                "instance observer DONE: pid={}, role={:?}, state={}, cause={}, detail={:?}",
                record.pid,
                self.role,
                snapshot.state,
                snapshot.cause,
                snapshot.detail
            );
            record.done = Some(snapshot);
            if record.stopping.is_none()
                || snapshot.pid != record.pid
                || snapshot.state != ProcessState::Dead as u32
                || snapshot.cause != TerminationCause::Completed as u32
                || snapshot.detail != TerminationDetail::NONE
            {
                world.failure = Some("required instance exited outside normal completion");
            }
        }
        if let Some(fs) = &mut record.filesystem {
            let stage_expired = self
                .deadline
                .instant()
                .map_err(|_| SystemCallError::IllegalArgument)?
                .is_some_and(|end| input.now_ns() >= end);
            if self.report_ready || stage_expired {
                let stopped = fs.stopped;
                self.report_ready = fs.receive_one(record.pid, record.stopping.is_some())?;
                if !stopped && fs.stopped {
                    libkernel::debug!(
                        "FS stop report authenticated: role={:?}, pid={}, DONE_seen={}, stage_expired={}",
                        self.role,
                        record.pid,
                        record.done.is_some(),
                        expired
                    );
                }
            }
            if record.stopping.is_none() && !fs.ready && stage_expired && !self.report_ready {
                world.failure = Some("filesystem startup deadline expired");
            }
        }
        if expired
            && (record.filesystem.is_none() || !self.report_ready)
            && !(record.done.is_some() && record.stopped())
        {
            world.failure = Some("required instance normal stop deadline expired");
        }
        if record.done.is_some() && record.stopped() {
            self.deadline = Deadline::INFINITE;
        }
        Ok(Advance {
            work_done: 1,
            step: if self.report_ready && record.filesystem.is_some() {
                Step::Runnable
            } else {
                Step::Parked
            },
        })
    }
    fn registered(&mut self, _world: &mut World, kind: SourceKind, source: SourceId) {
        if kind == 1 {
            self.source.registered(kind, source);
        } else {
            self.reports.registered(kind, source);
        }
    }
    fn unregistered(&mut self, _world: &mut World, _kind: SourceKind, source: SourceId) {
        if _kind == 1 {
            self.source.unregistered(source);
        } else {
            self.reports.unregistered(source);
        }
    }
    fn refused(&mut self, world: &mut World, failure: RequestFailure<Self>) {
        if let RequestFailure::Source { kind, .. } = failure {
            if kind == 1 {
                self.source.refused(kind);
            } else {
                self.reports.refused(kind);
            }
            world.failure = Some("manager observation registration failed");
        }
    }
    fn stop(&mut self, world: &mut World) {
        world.failure = Some("instance supervision stopped unexpectedly");
    }
    fn deadline(&self) -> Deadline {
        self.deadline
    }
}
pub(super) struct Supervisor {
    runtime: Runtime<InstanceTask, WaitSet>,
    world: World,
    tasks: Vec<u64>,
}
impl Supervisor {
    #[expect(
        clippy::result_large_err,
        reason = "关闭失败原样返还唯一owner，错误路径不分配"
    )]
    fn close(self) -> Result<(), (Self, SystemCallError)> {
        let Self {
            runtime,
            world,
            tasks,
        } = self;
        runtime.close().map_err(|(runtime, error)| {
            (
                Self {
                    runtime,
                    world,
                    tasks,
                },
                error,
            )
        })
    }
    pub(super) fn prepare(
        assembly: &Assembly<WaitSet>,
        funding: AccountView<ExecutionResource>,
    ) -> Result<Self, SystemCallError> {
        let roles = super::deployment::instances();
        let mut runtime = Runtime::new(
            assembly.bind()?,
            roles.len(),
            roles.len() + super::deployment::file_hosts(),
            &funding,
        )?;
        let mut records = Vec::new();
        records
            .try_reserve_exact(roles.len())
            .map_err(|_| SystemCallError::OutOfMemory)?;
        for _ in roles {
            records.push(Record::new());
        }
        let mut tasks = Vec::new();
        tasks
            .try_reserve_exact(roles.len())
            .map_err(|_| SystemCallError::OutOfMemory)?;
        for &role in roles {
            tasks.push(
                runtime
                    .spawn(TaskSubmission::new(
                        InstanceTask {
                            role,
                            source: SourceRegistration::new(),
                            done_ready: false,
                            reports: SourceRegistration::new(),
                            report_ready: true,
                            deadline: Deadline::INFINITE,
                        },
                        1 + usize::from(role.is_filesystem()),
                        funding.clone(),
                    ))
                    .map_err(|failure| failure.error)?,
            );
        }
        Ok(Self {
            runtime,
            world: World {
                records,
                failure: None,
                admission_closed: false,
            },
            tasks,
        })
    }
    pub(super) fn close_admission(&mut self, closed: bool) {
        self.world.admission_closed |= closed;
    }
    pub(super) fn feed(&mut self, record: erhino_shared::wait_set::ReadyRecord) {
        self.runtime.feed(record);
    }
    pub(super) fn step(&mut self) -> Result<(), SystemCallError> {
        if self.runtime.drive_state() != libexecution::runtime::DriveState::Drained {
            self.runtime
                .turn(&mut self.world, 2)
                .map_err(|failure| failure.error)?;
        }
        Ok(())
    }
    pub(super) fn state(&self) -> libexecution::runtime::DriveState {
        self.runtime.drive_state()
    }
    pub(super) fn failure(&self) -> Option<&'static str> {
        self.world.failure
    }
    pub(super) fn started(&self, role: Instance) -> bool {
        self.world.records[role.index()].started()
    }
    pub(super) fn detached(&self, role: Instance) -> bool {
        self.world.records[role.index()].detached
    }
    pub(super) fn done(&self, role: Instance) -> bool {
        self.world.records[role.index()].done.is_some()
    }
    fn wake(&mut self, role: Instance) -> Result<(), SystemCallError> {
        self.runtime.wake(self.tasks[role.index()])
    }
}

impl RootSupervisor {
    pub(super) fn finish_supervision(&mut self) -> Result<(), &'static str> {
        if let Some(supervisor) = &mut self.instances {
            for index in 0..supervisor.world.records.len() {
                if supervisor.world.records[index].pid == 0 {
                    supervisor.world.records[index].detach = true;
                    supervisor
                        .runtime
                        .wake(supervisor.tasks[index])
                        .map_err(|_| "unused instance retirement failed")?;
                }
            }
        }
        while self
            .instances
            .as_ref()
            .is_some_and(|s| s.world.records.iter().any(|r| !r.detached))
        {
            self.turn_and_wait(Some(Await::Turn))
                .map_err(|_| "instance retirement wait failed")?;
        }
        let supervisor = self
            .instances
            .take()
            .ok_or("instance supervision missing")?;
        if !supervisor
            .world
            .records
            .iter()
            .all(|record| record.detached && record.process.is_none() && record.admin.is_none())
        {
            self.instances = Some(supervisor);
            return Err("manager responsibilities remain owned");
        }
        if let Err((supervisor, _)) = supervisor.close() {
            self.instances = Some(supervisor);
            return Err("instance supervision retirement failed");
        }
        libkernel::debug!(
            "instance supervision retired: sources detached, processes collected, no observer thread"
        );
        Ok(())
    }
    pub(super) fn begin_supervision(&mut self) -> Result<(), &'static str> {
        let funding = super::supervisor::prepare_funding::<InstanceTask>(
            &mut None,
            super::deployment::instances().len(),
            super::deployment::instances().len() + super::deployment::file_hosts(),
        )
        .map_err(|_| "instance supervision funding failed")?;
        self.instances = Some(
            Supervisor::prepare(&self.assembly, funding)
                .map_err(|_| "instance supervision preparation failed")?,
        );
        Ok(())
    }
    pub(super) fn system_failure(&mut self, reason: &str) -> ! {
        if let Some(terminal) = self.terminal.take() {
            let admins = core::array::from_fn(|index| {
                self.instances
                    .as_mut()
                    .and_then(|s| s.world.records[index].admin.take())
            });
            terminal.run(admins, reason);
        }
        // SAFETY: 未发布基础宿主时准备失败；整个 init 退出交 boot 的原 DONE owner。
        unsafe { libkernel::sys_process_abort() }
    }
    pub(super) fn attach_manager_source(
        &mut self,
        role: Instance,
        entry: MailboxSender,
        server: u64,
    ) -> Result<(), &'static str> {
        let record = &mut self
            .instances
            .as_mut()
            .ok_or("instance supervision missing")?
            .world
            .records[role.index()];
        if record.admin.is_some() {
            return Err("manager source already installed");
        }
        record.admin = Some(SourceAdmin {
            entry: Arc::try_new(entry).map_err(|_| "manager source storage failed")?,
            server,
        });
        Ok(())
    }
    pub(super) fn manager_source_call(
        &mut self,
        role: Instance,
        command: libdevice::protocol::Command<'_>,
        end: Deadline,
    ) -> Result<librpc::Reply, &'static str> {
        let record = &self
            .instances
            .as_ref()
            .ok_or("instance supervision missing")?
            .world
            .records[role.index()];
        let admin = record.admin.as_ref().ok_or("manager source missing")?;
        let entry = admin.entry.clone();
        let server = admin.server;
        let reply = super::device::call_until(self, &entry, command, end)?;
        if reply.sender_pid != server {
            return Err("manager source response identity mismatch");
        }
        Ok(reply)
    }
    pub(super) fn start_instance(
        &mut self,
        role: Instance,
        prepared: libprocess::PreparedProcess,
    ) -> Result<u64, &'static str> {
        let failure = if self.stopping() {
            Some("system assembly admission closed")
        } else if let Some(supervisor) = &self.instances {
            supervisor
                .world
                .records
                .get(role.index())
                .filter(|record| record.process.is_none() && record.pid == 0)
                .is_none()
                .then_some("instance slot unavailable")
        } else {
            Some("instance supervision missing")
        };
        if let Some(error) = failure {
            self.cancel_prepared(prepared)?;
            return Err(error);
        }
        let supervisor = self.instances.as_mut().expect("instance slot validated");
        let record = &mut supervisor.world.records[role.index()];
        record.pid = prepared.pid();
        record.process = Some(ProcessOwner::Building(prepared));
        supervisor
            .wake(role)
            .map_err(|_| "manager preparation wake failed")?;
        loop {
            if self
                .turn_and_wait(Some(Await::ManagerStarted(role)))
                .is_err()
            {
                self.system_failure("manager start infrastructure failed");
            }
            let record = &self
                .instances
                .as_ref()
                .expect("supervisor remains owned")
                .world
                .records[role.index()];
            if record.started() {
                return Ok(record.pid);
            }
        }
    }
    pub(super) fn manager_stopping(
        &mut self,
        role: Instance,
        end: Deadline,
    ) -> Result<Deadline, &'static str> {
        let end = self.stop_deadline(end);
        let supervisor = self
            .instances
            .as_mut()
            .ok_or("instance supervision missing")?;
        let record = &mut supervisor.world.records[role.index()];
        let end = *record.stopping.get_or_insert(end);
        supervisor
            .wake(role)
            .map_err(|_| "manager stop wake failed")?;
        libkernel::debug!(
            "instance observer normal stop intent: role={:?}, deadline={}",
            role,
            end.at_ns
        );
        Ok(end)
    }
    pub(super) fn manager_device_closed(&mut self, role: Instance) -> Result<(), &'static str> {
        let supervisor = self
            .instances
            .as_mut()
            .ok_or("instance supervision missing")?;
        let record = &mut supervisor.world.records[role.index()];
        if record.stopping.is_none() {
            return Err("manager stop intent missing");
        }
        record.device_closed = true;
        record.admin = None;
        supervisor
            .wake(role)
            .map_err(|_| "manager source completion wake failed")
    }
    fn detach_instance(&mut self, role: Instance) -> Result<super::Supervised, &'static str> {
        loop {
            self.turn_and_wait(Some(Await::ManagerDone(role)))
                .map_err(|_| "manager completion wait failed")?;
            if self
                .instances
                .as_ref()
                .expect("supervisor remains owned")
                .done(role)
            {
                break;
            }
        }
        let supervisor = self.instances.as_mut().expect("supervisor remains owned");
        let record = &mut supervisor.world.records[role.index()];
        if !record.stopped() {
            return Err("manager source remains open");
        }
        record.detach = true;
        supervisor
            .wake(role)
            .map_err(|_| "manager detach wake failed")?;
        loop {
            self.turn_and_wait(Some(Await::ManagerDetached(role)))
                .map_err(|_| "manager detach wait failed")?;
            if self
                .instances
                .as_ref()
                .expect("supervisor remains owned")
                .detached(role)
            {
                break;
            }
        }
        let record = &mut self
            .instances
            .as_mut()
            .expect("supervisor remains owned")
            .world
            .records[role.index()];
        let Some(ProcessOwner::Running(control)) = record.process.take() else {
            return Err("manager collection owner missing");
        };
        Ok(super::Supervised {
            pid: record.pid,
            control: control.into_raw(),
        })
    }
    pub(super) fn collect_manager(&mut self, role: Instance) -> Result<(), &'static str> {
        let target = self.detach_instance(role)?;
        self.collect_machine(libprocess::Collector::new(
            libprocess::SupervisionTarget::new(target.pid, target.control),
            libprocess::DEFAULT_SUPERVISION_POLICY,
        ))
        .map(|_| ())
    }
    pub(super) fn instance_pid(&self, role: Instance) -> Result<u64, &'static str> {
        Ok(self
            .instances
            .as_ref()
            .ok_or("instance supervision missing")?
            .world
            .records[role.index()]
        .pid)
    }
    pub(super) fn instance_snapshot(
        &self,
        role: Instance,
    ) -> Result<ProcessSnapshot, &'static str> {
        let control = self
            .instances
            .as_ref()
            .and_then(|supervisor| supervisor.world.records[role.index()].process.as_ref())
            .ok_or("instance process owner missing")?
            .control();
        process::query(control).map_err(|_| "instance process state query failed")
    }
    pub(super) fn filesystem_bootstrap(
        &self,
        role: Instance,
    ) -> Result<&super::filesystem::hosting::Bootstrap, &'static str> {
        self.instances
            .as_ref()
            .and_then(|s| s.world.records.get(role.index()))
            .and_then(|record| record.filesystem.as_ref())
            .ok_or("filesystem bootstrap missing")
    }
    pub(super) fn attach_filesystem(
        &mut self,
        role: Instance,
        fs: super::filesystem::hosting::Bootstrap,
    ) -> Result<(), &'static str> {
        self.check_admission()?;
        let supervisor = self
            .instances
            .as_mut()
            .ok_or("instance supervision missing")?;
        let record = &mut supervisor.world.records[role.index()];
        if record.filesystem.is_some() || record.pid != 0 {
            return Err("filesystem instance already installed");
        }
        record.filesystem = Some(fs);
        Ok(())
    }
    pub(super) fn await_filesystem(
        &mut self,
        role: Instance,
    ) -> Result<super::filesystem::Host, &'static str> {
        loop {
            self.turn_and_wait(Some(Await::FilesystemReady(role)))
                .map_err(|_| "filesystem readiness wait failed")?;
            self.check_admission()?;
            let record = &mut self
                .instances
                .as_mut()
                .ok_or("instance supervision missing")?
                .world
                .records[role.index()];
            let running = record
                .process
                .as_ref()
                .ok_or("filesystem Control missing")?
                .control();
            let snapshot =
                process::query(running).map_err(|_| "filesystem delivery state query failed")?;
            if record.done.is_some()
                || record.stopping.is_some()
                || snapshot.state != ProcessState::Running as u32
            {
                libkernel::debug!(
                    "FS host delivery rejected: pid={}, state={}, cause={}, detail0={}, detail1={}",
                    snapshot.pid,
                    snapshot.state,
                    snapshot.cause,
                    snapshot.detail.word0,
                    snapshot.detail.word1
                );
                return Err("filesystem no longer running");
            }
            if let Some(host) = record.filesystem.as_mut().and_then(|fs| fs.take_host()) {
                return Ok(host);
            }
        }
    }
    pub(super) fn detach_filesystem(
        &mut self,
        role: Instance,
    ) -> Result<super::Supervised, &'static str> {
        // 正常 DONE 与已认证 Stopped 可能任意先后，原 receiver 保留到两事实汇齐。
        loop {
            self.turn_and_wait(Some(Await::FilesystemStopped(role)))
                .map_err(|_| "filesystem stop wait failed")?;
            let supervisor = self
                .instances
                .as_ref()
                .ok_or("instance supervision missing")?;
            let record = &supervisor.world.records[role.index()];
            if record.done.is_some() && record.stopped() {
                break;
            }
        }
        self.detach_instance(role)
    }
    pub(super) fn close_filesystem_report(&mut self, role: Instance) -> Result<(), &'static str> {
        let record = self
            .instances
            .as_ref()
            .and_then(|s| s.world.records.get(role.index()))
            .filter(|record| record.detached)
            .ok_or("filesystem report sources remain attached")?;
        let fs = record
            .filesystem
            .as_ref()
            .ok_or("filesystem bootstrap missing")?;
        let release = fs.release;
        let receiver = fs.receiver;
        self.close_control(release)?;
        self.close_control(receiver)
    }
    pub(super) fn begin_filesystem_stop(&mut self, role: Instance) -> Result<(), &'static str> {
        let fs = self.filesystem_bootstrap(role)?;
        let release = fs.release;
        super::filesystem::hosting::pool_sample(fs.pool, "Stopping");
        let end = libkernel::time::snapshot()
            .map_err(|_| "file stop clock failed")?
            .now_ns
            .checked_add(srv_fs::deployment::STOP_TIMEOUT_NS)
            .map(Deadline::at)
            .ok_or("file stop deadline overflow")?;
        self.manager_stopping(role, end)?;
        let record = &mut self
            .instances
            .as_mut()
            .expect("supervision remains owned")
            .world
            .records[role.index()];
        record
            .filesystem
            .as_mut()
            .expect("file record remains owned")
            .release_roots();
        if record.started() {
            self.notification_signaler(release)
                .signal(1)
                .map_err(|_| "file stop signal failed")?;
        }
        Ok(())
    }
    pub(super) fn filesystem_ready(&self, role: Instance) -> bool {
        self.instances.as_ref().is_some_and(|s| {
            s.world.records[role.index()]
                .filesystem
                .as_ref()
                .is_some_and(|fs| fs.ready)
        })
    }
    pub(super) fn filesystem_stopped(&self, role: Instance) -> bool {
        self.instances.as_ref().is_some_and(|s| {
            let r = &s.world.records[role.index()];
            r.stopped() && r.done.is_some()
        })
    }
}

impl RootSupervisor {
    pub(super) fn active_filesystems(&self) -> u64 {
        self.instances.as_ref().map_or(0, |s| {
            s.world
                .records
                .iter()
                .filter(|r| r.filesystem.is_some() && !r.detached)
                .count() as u64
        })
    }
    pub(super) fn stop_filesystems(&mut self) -> Result<(), &'static str> {
        // 实际原记录集合决定责任；不依赖调用者提供一张可遗漏的 Host 清单。
        for role in [Instance::Secondary, Instance::Filesystem] {
            let Some(supervisor) = &self.instances else {
                return Ok(());
            };
            let Some(record) = supervisor.world.records.get(role.index()) else {
                continue;
            };
            if record.detached {
                continue;
            }
            let Some(fs) = &record.filesystem else {
                continue;
            };
            let pool = fs.pool;
            let started = record.started();
            self.begin_filesystem_stop(role)?;
            if started {
                let target = self.detach_filesystem(role)?;
                // detach 后立即把唯一 Control 交原 Collector，之间没有可失败操作。
                self.collect_machine(libprocess::Collector::new(
                    libprocess::SupervisionTarget::new(target.pid, target.control),
                    libprocess::DEFAULT_SUPERVISION_POLICY,
                ))?;
            } else {
                self.cancel_unstarted(role)?;
            }
            self.close_filesystem_report(role)?;
            let snapshot =
                libkernel::memory_pool::query(pool).map_err(|_| "file Pool query failed")?;
            libkernel::debug!(
                "FS host Pool after Collector: pid={}, pages={}, metadata={}",
                self.instance_pid(role)?,
                snapshot.allocated,
                snapshot.metadata.allocated
            );
            self.close_control(pool)?;
        }
        Ok(())
    }
    fn cancel_unstarted(&mut self, role: Instance) -> Result<(), &'static str> {
        let supervisor = self
            .instances
            .as_mut()
            .ok_or("instance supervision missing")?;
        supervisor.world.records[role.index()].detach = true;
        supervisor
            .wake(role)
            .map_err(|_| "unstarted instance detach failed")?;
        while !self
            .instances
            .as_ref()
            .expect("supervision remains owned")
            .detached(role)
        {
            self.turn_and_wait(Some(Await::ManagerDetached(role)))
                .map_err(|_| "unstarted instance retirement failed")?;
        }
        let record = &mut self
            .instances
            .as_mut()
            .expect("supervision remains owned")
            .world
            .records[role.index()];
        if let Some(ProcessOwner::Building(prepared)) = record.process.take() {
            self.cancel_prepared(prepared)?;
        }
        Ok(())
    }
}
