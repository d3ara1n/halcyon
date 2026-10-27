//! root独占两管理者的DONE观察、正常停止事实与唯一故障终局。
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
pub(super) enum Manager {
    Device,
    Driver,
}
impl Manager {
    fn index(self) -> usize {
        match self {
            Self::Device => 0,
            Self::Driver => 1,
        }
    }
}
struct SourceAdmin {
    entry: Arc<MailboxSender>,
    server: u64,
}
struct Terminal {
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
    fn prepare(job: Handle, reset: Handle) -> Result<Self, SystemCallError> {
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
            "device failure terminal entered: {}; hardware completion is unproven",
            reason
        );
        match process::seal_job(self.job.as_handle()) {
            Ok(()) => libkernel::debug!("device failure JobSeal committed"),
            Err(error) => libkernel::debug!("device failure JobSeal unconfirmed: {:?}", error),
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
                libkernel::debug!("device failure source remains unconfirmed: {}", error);
            } else {
                libkernel::debug!(
                    "device failure source Seal confirmed: server={}",
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
                libkernel::debug!("device failure resource Seal unconfirmed: {:?}", error);
            } else {
                libkernel::debug!("device failure platform root Seal committed");
            }
        }
        // 原JobCollector负责枚举、Kill与实际DONE；升级界只截断等待，不生成完成事实。
        while let Ok(now) = libkernel::time::snapshot() {
            if now.now_ns >= deadline.at_ns {
                libkernel::debug!(
                    "device failure execution stop deadline reached; isolation retained"
                );
                break;
            }
            let outcome = match self.collector.step(now.now_ns) {
                Ok(event) => event,
                Err(error) => {
                    libkernel::debug!(
                        "device failure execution collection incomplete: {:?}",
                        error
                    );
                    break;
                }
            };
            match outcome {
                JobCollectorEvent::Done => {
                    libkernel::debug!("device failure execution job collection complete");
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
                                    "device failure execution observation incomplete: {:?}",
                                    error
                                );
                                break;
                            }
                        }
                        Err(error) => {
                            libkernel::debug!(
                                "device failure execution observation failed: {:?}",
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
        libkernel::debug!("device failure requesting cold reboot: SystemFailure");
        match libkernel::system::reset(
            self.reset.as_handle(),
            erhino_shared::reset::ResetAction::Reboot,
            erhino_shared::reset::ResetReason::SystemFailure,
        ) {
            Ok(never) => match never {},
            Err(error) => {
                libkernel::debug!(
                    "device failure cold reboot rejected: {:?}; terminating init",
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
    source_closed: bool,
    done: Option<ProcessSnapshot>,
    detach: bool,
    detached: bool,
}
impl Record {
    fn new() -> Self {
        Self {
            pid: 0,
            process: None,
            admin: None,
            stopping: None,
            source_closed: false,
            done: None,
            detach: false,
            detached: false,
        }
    }
    fn started(&self) -> bool {
        matches!(self.process, Some(ProcessOwner::Running(_)))
    }
}
struct World {
    records: [Record; 2],
    failure: Option<&'static str>,
}
struct ManagerTask {
    role: Manager,
    source: SourceRegistration,
    ready: bool,
    deadline: Deadline,
}
impl Task<World> for ManagerTask {
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
            self.ready |= event.observed.contains(ObjectSignals::DONE);
        }
        input.take_timeout();
        self.deadline = record.stopping.unwrap_or(Deadline::INFINITE);
        let Some(owner) = &mut record.process else {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        };
        let control = owner.control();
        if record.detach {
            self.deadline = Deadline::INFINITE;
            self.source.remove(requests)?;
            record.detached = self.source.is_empty();
            return Ok(Advance {
                work_done: 1,
                step: if record.detached {
                    Step::Complete
                } else {
                    Step::Runnable
                },
            });
        }
        if self.source.is_empty() {
            self.source
                .install(requests, SourcePlan::new(control, ObjectSignals::DONE), 1)?;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if self.source.is_installing() {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if let ProcessOwner::Building(prepared) = owner {
            libkernel::debug!(
                "device observer admitted before Start: role={:?}, pid={}, token={}",
                self.role,
                record.pid,
                self.source
                    .source()
                    .expect("manager source attached")
                    .token()
            );
            let spawned = prepared.try_start()?;
            // SAFETY: Start成功消费builder，原Control关闭责任唯一从prepared移入记录。
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
            self.ready |= wait::probe(control, ObjectSignals::DONE)?.contains(ObjectSignals::DONE);
        }
        if self.ready && record.done.is_none() {
            let snapshot = process::query(control)?;
            libkernel::debug!(
                "device observer manager DONE: pid={}, role={:?}, state={}, cause={}, detail={:?}",
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
                world.failure = Some("device manager exited outside normal completion");
            }
        }
        if expired && !(record.done.is_some() && record.source_closed) {
            world.failure = Some("device manager normal stop deadline expired");
        }
        if record.done.is_some() && record.source_closed {
            self.deadline = Deadline::INFINITE;
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Parked,
        })
    }
    fn registered(&mut self, _world: &mut World, kind: SourceKind, source: SourceId) {
        self.source.registered(kind, source);
    }
    fn unregistered(&mut self, _world: &mut World, _kind: SourceKind, source: SourceId) {
        self.source.unregistered(source);
    }
    fn refused(&mut self, world: &mut World, failure: RequestFailure<Self>) {
        if let RequestFailure::Source { kind, .. } = failure {
            self.source.refused(kind);
            world.failure = Some("manager observation registration failed");
        }
    }
    fn stop(&mut self, world: &mut World) {
        world.failure = Some("manager supervision stopped unexpectedly");
    }
    fn deadline(&self) -> Deadline {
        self.deadline
    }
}
pub(super) struct Supervisor {
    runtime: Runtime<ManagerTask, WaitSet>,
    world: World,
    tasks: [u64; 2],
    terminal: Option<Terminal>,
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
            terminal,
        } = self;
        runtime.close().map_err(|(runtime, error)| {
            (
                Self {
                    runtime,
                    world,
                    tasks,
                    terminal,
                },
                error,
            )
        })
    }
    pub(super) fn prepare(
        assembly: &Assembly<WaitSet>,
        funding: AccountView<ExecutionResource>,
        job: Handle,
        reset: Handle,
    ) -> Result<Self, SystemCallError> {
        let terminal = Terminal::prepare(job, reset)?;
        let mut runtime = Runtime::new(assembly.bind()?, 2, 2, &funding)?;
        let mut tasks = [0; 2];
        for role in [Manager::Device, Manager::Driver] {
            tasks[role.index()] = runtime
                .spawn(TaskSubmission::new(
                    ManagerTask {
                        role,
                        source: SourceRegistration::new(),
                        ready: false,
                        deadline: Deadline::INFINITE,
                    },
                    1,
                    funding.clone(),
                ))
                .map_err(|failure| failure.error)?;
        }
        Ok(Self {
            runtime,
            world: World {
                records: core::array::from_fn(|_| Record::new()),
                failure: None,
            },
            tasks,
            terminal: Some(terminal),
        })
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
    pub(super) fn started(&self, role: Manager) -> bool {
        self.world.records[role.index()].started()
    }
    pub(super) fn detached(&self, role: Manager) -> bool {
        self.world.records[role.index()].detached
    }
    pub(super) fn done(&self, role: Manager) -> bool {
        self.world.records[role.index()].done.is_some()
    }
    fn wake(&mut self, role: Manager) -> Result<(), SystemCallError> {
        self.runtime.wake(self.tasks[role.index()])
    }
    fn terminal(&mut self, reason: &str) -> ! {
        let terminal = self.terminal.take().expect("root has one terminal owner");
        let admins = core::array::from_fn(|index| self.world.records[index].admin.take());
        terminal.run(admins, reason)
    }
}

impl RootSupervisor {
    pub(super) fn finish_device_supervision(&mut self) -> Result<(), &'static str> {
        let supervisor = self
            .device_supervisor
            .take()
            .ok_or("manager supervision missing")?;
        if !supervisor
            .world
            .records
            .iter()
            .all(|record| record.detached && record.process.is_none() && record.admin.is_none())
        {
            self.device_supervisor = Some(supervisor);
            return Err("manager responsibilities remain owned");
        }
        if let Err((supervisor, _)) = supervisor.close() {
            self.device_supervisor = Some(supervisor);
            return Err("manager supervision retirement failed");
        }
        libkernel::debug!(
            "device supervision retired: sources detached, managers collected, no observer thread"
        );
        Ok(())
    }
    pub(super) fn begin_device_supervision(
        &mut self,
        services: Handle,
    ) -> Result<(), &'static str> {
        let reset = self
            .reset_authority()
            .ok_or("device reset authority missing")?;
        let funding = super::supervisor::prepare_funding::<ManagerTask>(&mut None, 2, 2)
            .map_err(|_| "manager supervision funding failed")?;
        self.device_supervisor = Some(
            Supervisor::prepare(&self.assembly, funding, services, reset)
                .map_err(|_| "manager supervision preparation failed")?,
        );
        Ok(())
    }
    pub(super) fn device_failure(&mut self, reason: &str) -> ! {
        if let Some(supervisor) = &mut self.device_supervisor {
            supervisor.terminal(reason);
        }
        // SAFETY: 尚未发布管理者；init整体退出交boot监督，不在缺失终局owner时继续装配。
        unsafe { libkernel::sys_process_abort() }
    }
    pub(super) fn attach_manager_source(
        &mut self,
        role: Manager,
        entry: MailboxSender,
        server: u64,
    ) -> Result<(), &'static str> {
        let record = &mut self
            .device_supervisor
            .as_mut()
            .ok_or("manager supervision missing")?
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
        role: Manager,
        command: libdevice::protocol::Command<'_>,
        end: Deadline,
    ) -> Result<librpc::Reply, &'static str> {
        let record = &self
            .device_supervisor
            .as_ref()
            .ok_or("manager supervision missing")?
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
    pub(super) fn start_manager(
        &mut self,
        role: Manager,
        prepared: libprocess::PreparedProcess,
    ) -> Result<libprocess::Spawned, &'static str> {
        let supervisor = self
            .device_supervisor
            .as_mut()
            .ok_or("manager supervision missing")?;
        let record = &mut supervisor.world.records[role.index()];
        if record.process.is_some() || record.pid != 0 {
            return Err("manager already started");
        }
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
                self.device_failure("manager start infrastructure failed");
            }
            let record = &self
                .device_supervisor
                .as_ref()
                .expect("supervisor remains owned")
                .world
                .records[role.index()];
            if record.started() {
                return Ok(libprocess::Spawned {
                    pid: record.pid,
                    control: record
                        .process
                        .as_ref()
                        .expect("running owner present")
                        .control(),
                });
            }
        }
    }
    pub(super) fn manager_stopping(
        &mut self,
        role: Manager,
        end: Deadline,
    ) -> Result<Deadline, &'static str> {
        let supervisor = self
            .device_supervisor
            .as_mut()
            .ok_or("manager supervision missing")?;
        let record = &mut supervisor.world.records[role.index()];
        let end = *record.stopping.get_or_insert(end);
        supervisor
            .wake(role)
            .map_err(|_| "manager stop wake failed")?;
        libkernel::debug!(
            "device observer normal stop intent: role={:?}, deadline={}",
            role,
            end.at_ns
        );
        Ok(end)
    }
    pub(super) fn manager_source_closed(&mut self, role: Manager) -> Result<(), &'static str> {
        let supervisor = self
            .device_supervisor
            .as_mut()
            .ok_or("manager supervision missing")?;
        let record = &mut supervisor.world.records[role.index()];
        if record.stopping.is_none() {
            return Err("manager stop intent missing");
        }
        record.source_closed = true;
        record.admin = None;
        supervisor
            .wake(role)
            .map_err(|_| "manager source completion wake failed")
    }
    pub(super) fn collect_manager(&mut self, role: Manager) -> Result<(), &'static str> {
        loop {
            self.turn_and_wait(Some(Await::ManagerDone(role)))
                .map_err(|_| "manager completion wait failed")?;
            if self
                .device_supervisor
                .as_ref()
                .expect("supervisor remains owned")
                .done(role)
            {
                break;
            }
        }
        let supervisor = self
            .device_supervisor
            .as_mut()
            .expect("supervisor remains owned");
        let record = &mut supervisor.world.records[role.index()];
        if !record.source_closed {
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
                .device_supervisor
                .as_ref()
                .expect("supervisor remains owned")
                .detached(role)
            {
                break;
            }
        }
        let record = &mut self
            .device_supervisor
            .as_mut()
            .expect("supervisor remains owned")
            .world
            .records[role.index()];
        let Some(ProcessOwner::Running(control)) = record.process.take() else {
            return Err("manager collection owner missing");
        };
        let pid = record.pid;
        self.collect_machine(libprocess::Collector::new(
            libprocess::SupervisionTarget::new(pid, control.into_raw()),
            libprocess::DEFAULT_SUPERVISION_POLICY,
        ))?;
        Ok(())
    }
}
