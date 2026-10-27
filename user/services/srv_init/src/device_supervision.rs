//! init内固定两管理者的持续观察；原RootSupervisor账本不跨线程。
use alloc::{sync::Arc, vec::Vec};
use core::mem;
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    proc::{ProcessSnapshot, ProcessState, TerminationCause, TerminationDetail},
    time::Deadline,
    wait_set::{MemberRequest, ReadyRecord},
};
use libkernel::{
    ipc::{
        capability::Capability,
        message::MailboxSender,
        notification::{self, NotificationOwner, NotificationSignaler},
        object, wait,
        wait_set::WaitSet,
    },
    process,
};
use libprocess::{JobCollector, JobCollectorEvent, SupervisionPolicy};
use lock_api::Mutex;
type Lock = erhino_shared::sync::spin::SimpleLock;
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
#[derive(Clone, Copy, PartialEq, Eq)]
enum Gate {
    Running,
    Stopping,
    FailureTerminal,
}
struct Stopping {
    deadline: Deadline,
    source_closed: bool,
}
struct SourceAdmin {
    entry: MailboxSender,
    server: u64,
}
enum Admin {
    Absent,
    Held(SourceAdmin),
    Closing,
    Released,
}
struct Record {
    pid: u64,
    control: Option<Capability>,
    token: Option<u64>,
    stopping: Option<Stopping>,
    done: Option<ProcessSnapshot>,
    admin: Admin,
    disarm: bool,
    retired: bool,
}
impl Record {
    fn finished(&self) -> bool {
        self.retired
    }
}
struct Shared {
    gate: Gate,
    records: [Option<Record>; 2],
    terminal: Option<Terminal>,
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
fn admin_alias(sender: &MailboxSender, server: u64) -> Result<SourceAdmin, SystemCallError> {
    let cap = owned_alias(sender.as_handle(), Rights::WRITE | Rights::WAIT)?;
    let (entry, _) = MailboxSender::from_capability(cap).map_err(|e| e.error)?;
    Ok(SourceAdmin { entry, server })
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
fn terminal(shared: &Arc<Mutex<Lock, Shared>>, reason: &str) -> ! {
    let (owner, admins) = {
        let mut state = shared.lock();
        state.gate = Gate::FailureTerminal;
        let owner = state.terminal.take();
        let admins = core::array::from_fn(|i| {
            state.records[i].as_mut().and_then(|record| {
                match mem::replace(&mut record.admin, Admin::Closing) {
                    Admin::Held(admin) => Some(admin),
                    other => {
                        record.admin = other;
                        None
                    }
                }
            })
        });
        (owner, admins)
    };
    if let Some(owner) = owner {
        owner.run(admins, reason)
    }
    // SAFETY: 终局authority已由另一线程接管；不发送第二次reset，也不提前杀死其整个进程。
    // 此失败线程不再返回用户业务，其未析构cap由进程终局统一关闭。
    unsafe { libkernel::thread::exit() }
}
pub(super) struct Observer {
    shared: Arc<Mutex<Lock, Shared>>,
    set: Option<Arc<WaitSet>>,
    changed: Option<NotificationSignaler>,
    acknowledged: Option<NotificationOwner>,
    thread: Option<libthread::ResultReceiver<(), libthread::NoEnvironment>>,
}
impl Observer {
    pub fn prepare(job: Handle, reset: Handle) -> Result<Self, SystemCallError> {
        let authority = Terminal::prepare(job, reset)?;
        let shared = Arc::try_new(Mutex::new(Shared {
            gate: Gate::Running,
            records: [None, None],
            terminal: Some(authority),
        }))
        .map_err(|_| SystemCallError::OutOfMemory)?;
        let set = Arc::try_new(WaitSet::create(3)?).map_err(|_| SystemCallError::OutOfMemory)?;
        let changed =
            notification::create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL)?;
        let ack =
            notification::create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL)?;
        let token = set.register(MemberRequest::new(
            changed.owner.as_handle(),
            ObjectSignals::READABLE,
            0,
        ))?;
        let state = shared.clone();
        let set_thread = set.clone();
        // spawn为最后一个可失败准备动作；Err时没有观察用户代码执行，不遗留隐式监督线程。
        let thread = libruntime::thread::Builder::new()
            .spawn(move || observe(state, set_thread, changed.owner, token, ack.peer))?;
        Ok(Self {
            shared,
            set: Some(set),
            changed: Some(changed.peer),
            acknowledged: Some(ack.owner),
            thread: Some(thread),
        })
    }
    pub fn register(
        &self,
        manager: Manager,
        prepared: &libprocess::PreparedProcess,
        admin: Option<(&MailboxSender, u64)>,
    ) -> Result<(), &'static str> {
        let control = owned_alias(prepared.control(), Rights::READ | Rights::WAIT)
            .map_err(|_| "manager DONE authority preparation failed")?;
        let handle = control.as_handle();
        let admin = admin
            .map(|(sender, server)| admin_alias(sender, server))
            .transpose()
            .map_err(|_| "manager source administration preparation failed")?
            .map_or(Admin::Absent, Admin::Held);
        {
            let mut state = self.shared.lock();
            if state.gate != Gate::Running || state.records[manager.index()].is_some() {
                return Err("device assembly admission closed");
            }
            state.records[manager.index()] = Some(Record {
                pid: prepared.pid(),
                control: Some(control),
                token: None,
                stopping: None,
                done: None,
                admin,
                disarm: false,
                retired: false,
            });
        }
        // 记录先接管cap，注册不持gate锁；Building立即终止也只会留下持续DONE电平。
        let token = self
            .set
            .as_ref()
            .ok_or("device observation set unavailable")?
            .register(MemberRequest::new(
                handle,
                ObjectSignals::DONE,
                manager.index() as u64 + 1,
            ))
            .map_err(|_| "manager DONE registration failed")?;
        let mut state = self.shared.lock();
        let record = state.records[manager.index()]
            .as_mut()
            .ok_or("manager observation record missing")?;
        record.token = Some(token);
        if state.gate == Gate::FailureTerminal {
            return Err("device assembly failed during registration");
        }
        drop(state);
        libkernel::debug!(
            "device observer admitted before Start: role={:?}, pid={}, token={}",
            manager,
            prepared.pid(),
            token
        );
        self.signal()
    }
    pub fn attach_admin(
        &self,
        manager: Manager,
        sender: &MailboxSender,
        server: u64,
    ) -> Result<(), &'static str> {
        let admin = admin_alias(sender, server)
            .map_err(|_| "manager source observation authority failed")?;
        let mut state = self.shared.lock();
        if state.gate == Gate::FailureTerminal {
            return Err("device assembly already failed");
        }
        let record = state.records[manager.index()]
            .as_mut()
            .ok_or("manager observation record missing")?;
        if !matches!(record.admin, Admin::Absent) {
            return Err("manager source administration already installed");
        }
        record.admin = Admin::Held(admin);
        drop(state);
        self.signal()
    }
    fn signal(&self) -> Result<(), &'static str> {
        match self.changed.as_ref() {
            Some(changed) => changed
                .signal(1)
                .map_err(|_| "device observation notification failed"),
            None => Ok(()),
        }
    }
    pub fn stopping(
        &mut self,
        manager: Manager,
        deadline: Deadline,
    ) -> Result<Deadline, &'static str> {
        let mut state = self.shared.lock();
        if state.gate == Gate::FailureTerminal {
            return Err("device assembly already failed");
        }
        state.gate = Gate::Stopping;
        let record = state.records[manager.index()]
            .as_mut()
            .ok_or("manager observation record missing")?;
        let end = record
            .stopping
            .get_or_insert(Stopping {
                deadline,
                source_closed: false,
            })
            .deadline;
        // 全部manager已发布后封正常新装配；观察线程最终唯一关闭WaitSet。
        self.set = None;
        drop(state);
        libkernel::debug!(
            "device observer normal stop intent: role={:?}, deadline={}",
            manager,
            end.at_ns
        );
        self.signal()?;
        Ok(end)
    }
    pub fn source_closed(&self, manager: Manager) -> Result<(), &'static str> {
        let mut state = self.shared.lock();
        let record = state.records[manager.index()]
            .as_mut()
            .ok_or("manager observation record missing")?;
        record
            .stopping
            .as_mut()
            .ok_or("manager stop intent missing")?
            .source_closed = true;
        drop(state);
        self.signal()
    }
    fn await_state(
        &self,
        manager: Manager,
        deadline: Deadline,
        predicate: fn(&Record) -> bool,
    ) -> Result<(), &'static str> {
        let acknowledged = self
            .acknowledged
            .as_ref()
            .ok_or("device observer already joined")?;
        loop {
            match acknowledged.take(u64::MAX) {
                Ok(_) | Err(SystemCallError::ObjectNotAvailable) => {}
                Err(_) => return Err("device observation acknowledgement failed"),
            }
            {
                let state = self.shared.lock();
                if state.gate == Gate::FailureTerminal {
                    return Err("device assembly already failed");
                }
                if state.records[manager.index()]
                    .as_ref()
                    .is_some_and(predicate)
                {
                    return Ok(());
                }
            }
            let result =
                wait::wait_until(acknowledged.as_handle(), ObjectSignals::READABLE, deadline)
                    .map_err(|_| "device observation acknowledgement wait failed")?;
            if !result.observed.contains(ObjectSignals::READABLE) {
                return Err("device observation handover timed out");
            }
        }
    }
    pub fn release_admin(&self, manager: Manager, deadline: Deadline) -> Result<(), &'static str> {
        self.signal()?;
        self.await_state(manager, deadline, |record| {
            matches!(record.admin, Admin::Released)
        })
    }
    pub fn disarm(&self, manager: Manager, deadline: Deadline) -> Result<(), &'static str> {
        let mut state = self.shared.lock();
        let record = state.records[manager.index()]
            .as_mut()
            .ok_or("manager observation record missing")?;
        record.disarm = true;
        drop(state);
        self.signal()?;
        self.await_state(manager, deadline, Record::finished)
    }
    pub fn join(&mut self) -> Result<(), &'static str> {
        let thread = self.thread.take().ok_or("device observer already joined")?;
        match thread.join() {
            libthread::ThreadOutcome::Returned {
                value: (),
                leave: Ok(()),
            } => {}
            _ => return Err("device observer did not finish normally"),
        }
        self.changed = None;
        self.acknowledged = None;
        // 本程序此点的其他派生线程均已join；main自身的原entry记录仍Live。
        // 不能仅用join的一次reap把Busy栈或Control尾部写成已经返还。
        let deadline = libkernel::time::timeout_millis(5_000)
            .map_err(|_| "device observer retirement clock failed")?;
        while libruntime::thread::retained_records() != 1 {
            let count = libruntime::thread::retained_records();
            if matches!(
                libruntime::thread::reap(count),
                libthread::ReapProgress::Failed(_)
            ) {
                return Err("device observer thread registry failed");
            }
            if libkernel::time::expired(deadline)
                .map_err(|_| "device observer retirement clock failed")?
            {
                return Err("device observer thread records remain retained");
            }
            libkernel::thread::yield_now()
                .map_err(|_| "device observer retirement yield failed")?;
        }
        libkernel::debug!(
            "device observer normal retirement: manager sources/capabilities closed; thread registry retains main only"
        );
        Ok(())
    }
    pub fn fail(&self, reason: &str) -> ! {
        let _ = self.signal();
        terminal(&self.shared, reason)
    }
}
fn observe(
    shared: Arc<Mutex<Lock, Shared>>,
    set: Arc<WaitSet>,
    changed: NotificationOwner,
    change_token: u64,
    ack: NotificationSignaler,
) {
    let result = observe_inner(&shared, &set, &changed, &ack, change_token);
    if let Err(reason) = result {
        terminal(&shared, reason);
    }
    if set.remove(change_token).is_err() {
        terminal(&shared, "device observer control removal failed");
    }
    drop(changed);
    match Arc::try_unwrap(set) {
        Ok(set) => {
            if let Err((_set, _)) = set.close() {
                terminal(&shared, "device observer WaitSet close failed");
            }
        }
        Err(_) => terminal(
            &shared,
            "device observer WaitSet remains shared after disarm",
        ),
    }
}
fn observe_inner(
    shared: &Arc<Mutex<Lock, Shared>>,
    set: &WaitSet,
    changed: &NotificationOwner,
    ack: &NotificationSignaler,
    change_token: u64,
) -> Result<(), &'static str> {
    let mut records = [ReadyRecord {
        token: 0,
        cookie: 0,
        observed: ObjectSignals::NONE,
        reason: 0,
        reserved: 0,
    }; 3];
    loop {
        if shared.lock().gate == Gate::FailureTerminal {
            terminal(shared, "concurrent device failure");
        }
        let count = set
            .receive_into(&mut records)
            .map_err(|_| "device observer Receive failed")?;
        for event in &records[..count] {
            if event.cookie == 0 {
                if event.token != change_token {
                    return Err("device observer control identity invalid");
                }
                match changed.take(u64::MAX) {
                    Ok(_) | Err(SystemCallError::ObjectNotAvailable) => {}
                    Err(_) => return Err("device observer control consume failed"),
                }
                continue;
            }
            let index =
                usize::try_from(event.cookie - 1).map_err(|_| "device observer cookie invalid")?;
            if index >= 2 {
                return Err("device observer cookie invalid");
            }
            let action = {
                let mut state = shared.lock();
                let record = state.records[index]
                    .as_mut()
                    .ok_or("device observer event has no owner")?;
                if record.token.is_none() {
                    continue;
                }
                if record.token != Some(event.token)
                    || !event.observed.contains(ObjectSignals::DONE)
                {
                    return Err("device observer DONE identity invalid");
                }
                (
                    record
                        .control
                        .take()
                        .ok_or("manager observation authority missing")?,
                    record.stopping.is_some(),
                    record.pid,
                )
            };
            let (control, expected, pid) = action;
            let snapshot =
                process::query(control.as_handle()).map_err(|_| "manager DONE query failed")?;
            libkernel::debug!(
                "device observer manager DONE: pid={}, cause={}, detail={:?}",
                pid,
                snapshot.cause,
                snapshot.detail
            );
            if snapshot.state != ProcessState::Dead as u32
                || !expected
                || snapshot.cause != TerminationCause::Completed as u32
                || snapshot.detail != TerminationDetail::NONE
            {
                return Err("device manager terminated unexpectedly");
            }
            // DONE已认证且不可能复活，先移除持续电平；保留Control与快照等待同次SourceClosed证明。
            set.remove(event.token)
                .map_err(|_| "manager DONE source removal failed")?;
            let mut state = shared.lock();
            let record = state.records[index]
                .as_mut()
                .ok_or("manager DONE owner missing")?;
            record.control = Some(control);
            record.token = None;
            record.done = Some(snapshot);
        }
        for index in 0..2 {
            let admin = {
                let mut state = shared.lock();
                state.records[index].as_mut().and_then(|record| {
                    if record.stopping.as_ref().is_some_and(|s| s.source_closed) {
                        match mem::replace(&mut record.admin, Admin::Closing) {
                            Admin::Held(admin) => Some(admin),
                            other => {
                                record.admin = other;
                                None
                            }
                        }
                    } else {
                        None
                    }
                })
            };
            if let Some(admin) = admin {
                admin
                    .entry
                    .into_capability()
                    .close()
                    .map_err(|_| "manager source administration close failed")?;
                shared.lock().records[index]
                    .as_mut()
                    .ok_or("manager source owner missing")?
                    .admin = Admin::Released;
                libkernel::debug!(
                    "device observer source administration closed: role={}",
                    index
                );
                ack.signal(1)
                    .map_err(|_| "device observer acknowledgement signal failed")?;
            }
            let control = {
                let mut state = shared.lock();
                state.records[index].as_mut().and_then(|record| {
                    if record.disarm
                        && record.done.is_some()
                        && record.stopping.as_ref().is_some_and(|s| s.source_closed)
                        && matches!(record.admin, Admin::Released)
                    {
                        record.control.take()
                    } else {
                        None
                    }
                })
            };
            if let Some(control) = control {
                control
                    .close()
                    .map_err(|_| "manager observation control close failed")?;
                shared.lock().records[index]
                    .as_mut()
                    .ok_or("manager close owner missing")?
                    .retired = true;
                libkernel::debug!("device observer normal disarm complete: role={}", index);
                ack.signal(1)
                    .map_err(|_| "device observer acknowledgement signal failed")?;
            }
        }
        let now = libkernel::time::snapshot()
            .map_err(|_| "device observer clock failed")?
            .now_ns;
        let deadline = {
            let state = shared.lock();
            if state
                .records
                .iter()
                .all(|record| record.as_ref().is_some_and(Record::finished))
            {
                return Ok(());
            }
            let mut end = None;
            for record in state.records.iter().flatten() {
                if !record.finished()
                    && let Some(stopping) = &record.stopping
                {
                    if now >= stopping.deadline.at_ns {
                        return Err("device manager normal stop evidence timed out");
                    }
                    end = Some(end.map_or(stopping.deadline.at_ns, |old: u64| {
                        old.min(stopping.deadline.at_ns)
                    }));
                }
            }
            end.map_or(Deadline::INFINITE, Deadline::at)
        };
        set.wait(deadline)
            .map_err(|_| "device observer wait failed")?;
    }
}

impl super::RootSupervisor {
    pub(super) fn begin_device_supervision(
        &mut self,
        services: Handle,
    ) -> Result<(), &'static str> {
        if self.device_observer.is_some() {
            return Err("device observation already prepared");
        }
        let reset = self
            .reset_authority()
            .ok_or("device reset authority missing")?;
        match Observer::prepare(services, reset) {
            Ok(observer) => {
                self.device_observer = Some(observer);
                Ok(())
            }
            Err(error) => {
                // prepare的线程spawn是最后一个可失败动作；此处尚无管理者Start或并行终局owner。
                libkernel::debug!(
                    "device observation preparation failed: {:?}; cold reboot required",
                    error
                );
                let _ = process::seal_job(services);
                match libkernel::system::reset(
                    reset,
                    erhino_shared::reset::ResetAction::Reboot,
                    erhino_shared::reset::ResetReason::SystemFailure,
                ) {
                    Ok(never) => match never {},
                    Err(error) => {
                        libkernel::debug!("device observation early reboot rejected: {:?}", error);
                        unsafe { libkernel::sys_process_abort() }
                    }
                }
            }
        }
    }
}
