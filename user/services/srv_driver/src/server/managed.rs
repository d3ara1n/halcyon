//! 原轮 Claim、真实 worker 出生、单一 Activate 与停止；控制/供应 Unknown 不换轮。
use super::*;
use alloc::string::String;
use libdevice::{
    binding::Phase as BindingPhase,
    protocol::{self as device, Command, Header},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Next {
    Claim,
    WatchBinding,
    QueryBinding,
    WaitResources,
    IssueUse,
    IssueRegistrar,
    Spawn,
    WaitBoot,
    Activate,
    WatchActivation,
    InstallActivation,
    InstallFunction,
    Running,
    StopBinding,
    StopActivation,
    ReportStopped,
    WaitReleased,
    Detach,
    Retire,
}
pub(super) struct Managed {
    run: Rc<RefCell<Run>>,
    identity: Vec<u8>,
    next: Next,
    resume: Next,
    call: Option<(u64, Next)>,
    control: Option<Arc<MailboxSender>>,
    usage: Option<Arc<MailboxSender>>,
    registrar: Option<Arc<MailboxSender>>,
    registrar_admin: Option<MailboxSender>,
    observation: Option<Arc<MailboxSender>>,
    function: Option<Arc<MailboxSender>>,
    function_name: String,
    function_description: Vec<u8>,
    stop_sent: bool,
    activation_sent: bool,
    activation_stop_sent: bool,
    reported: bool,
    phase_end: Deadline,
    cleanup: Option<librpc::CallError>,
    retry_at: Option<u64>,
}
impl Managed {
    fn process(&self, world: &World<'_>) -> Option<Rc<RefCell<worker::Record>>> {
        let process = self.run.borrow().process?;
        world.processes.get(process.serial()).cloned()
    }

    pub fn prepare(run: Rc<RefCell<Run>>, identity: &[u8]) -> Result<Self, SystemCallError> {
        let mut frozen = Vec::new();
        frozen
            .try_reserve_exact(identity.len())
            .map_err(|_| SystemCallError::OutOfMemory)?;
        frozen.extend_from_slice(identity);
        Ok(Self {
            run,
            identity: frozen,
            next: Next::Claim,
            resume: Next::WaitResources,
            call: None,
            control: None,
            usage: None,
            registrar: None,
            registrar_admin: None,
            observation: None,
            function: None,
            function_name: String::new(),
            function_description: Vec::new(),
            stop_sent: false,
            activation_sent: false,
            activation_stop_sent: false,
            reported: false,
            phase_end: Deadline::INFINITE,
            cleanup: None,
            retry_at: None,
        })
    }
    pub fn stop(&mut self) {
        self.run.borrow_mut().stop_requested = true;
    }
    pub fn deadline(&self) -> Deadline {
        self.retry_at.map_or(self.phase_end, |at| {
            self.phase_end
                .instant()
                .expect("driver phase deadline invalid")
                .map_or(Deadline::at(at), |end| Deadline::at(at.min(end)))
        })
    }
    fn failed_call(&mut self, mut error: librpc::CallError, txid: Option<u64>) {
        libkernel::debug!(
            "driver RPC failed: operation={:?}, device={}, epoch={}, txid={:?}, phase={:?}, cause={:?}",
            self.next,
            self.run.borrow().device.serial(),
            self.run.borrow().reference.epoch(),
            txid,
            error.phase,
            error.cause
        );
        if self.control.is_none() && error.phase != librpc::CallPhase::Unsent {
            // 没有取得控制的 Unknown 由 init 原 SourceAdmin 封整个旧来源收束，不猜 NotFound。
            fatal(Error::Unavailable);
        }
        let cause = match error.cause {
            librpc::CallCause::System(error) => Error::from(error),
            librpc::CallCause::Frame(_) => Error::Invalid,
            _ => Error::Unavailable,
        };
        drop(error.take_unsent_request());
        self.cleanup = error.has_unretired_owners().then_some(error);
        self.run.borrow_mut().fail(cause);
        self.next = if self.control.is_some() {
            Next::WaitReleased
        } else {
            Next::Retire
        };
    }
    fn entry(
        reply: &mut librpc::Reply,
        index: usize,
        registry: u64,
    ) -> Result<Arc<MailboxSender>, Error> {
        let (entry, description) = MailboxSender::from_capability(reply.handles.take(index)?)
            .map_err(|failure| Error::from(failure.error))?;
        if description.related_object_id != registry {
            return Err(Error::Permission);
        }
        Arc::try_new(entry).map_err(|_| Error::Resource)
    }
    fn device_result(reply: &librpc::Reply, op: u16) -> Result<&[u8], Error> {
        let (header, body) = Header::decode(&reply.payload)?;
        if header.op != op {
            return Err(Error::Invalid);
        }
        if let Some(error) = header.error {
            if !body.is_empty() || reply.handles.remaining() != 0 {
                return Err(Error::Invalid);
            }
            return Err(error);
        }
        Ok(body)
    }
    fn acknowledge_binding(
        &mut self,
        requests: &mut Requests<'_, Family>,
    ) -> Result<(), SystemCallError> {
        let mut run = self.run.borrow_mut();
        run.binding_changed = false;
        if run.binding_watch != 0 {
            requests.wake(run.binding_watch)?;
        }
        Ok(())
    }
    fn acknowledge_activation(
        &mut self,
        requests: &mut Requests<'_, Family>,
    ) -> Result<(), SystemCallError> {
        let mut run = self.run.borrow_mut();
        run.activation_changed = false;
        if run.activation_watch != 0 {
            requests.wake(run.activation_watch)?;
        }
        Ok(())
    }
    fn consume(
        &mut self,
        step: Next,
        mut reply: librpc::Reply,
        world: &mut World<'_>,
        requests: &mut Requests<'_, Family>,
    ) -> Result<(), Error> {
        let reference = self.run.borrow().reference;
        let registry = world.claim.description()?.related_object_id;
        match step {
            Next::Claim => {
                let snapshot = device::decode_snapshot(Self::device_result(
                    &reply,
                    Command::Claim { reference }.op(),
                )?)?;
                if snapshot.reference != reference || reply.handles.remaining() != 1 {
                    return Err(Error::Invalid);
                }
                self.control = Some(Self::entry(&mut reply, 0, registry)?);
                self.next = Next::WatchBinding;
            }
            Next::WatchBinding => {
                let watch = libdevice::client::Watch::from_reply(reply, registry, 16)?;
                let task = subscription::Subscription::binding(self.run.clone(), watch);
                requests
                    .spawn(TaskSubmission::new(
                        Family::Subscription(task),
                        1,
                        world.funding.clone(),
                    ))
                    .map_err(|_| Error::Quota)?;
                self.next = Next::WaitResources;
            }
            Next::QueryBinding => {
                let snapshot = device::decode_snapshot(Self::device_result(
                    &reply,
                    Command::QueryBinding.op(),
                )?)?;
                if snapshot.reference != reference || reply.handles.remaining() != 0 {
                    return Err(Error::Invalid);
                }
                self.acknowledge_binding(requests)?;
                match snapshot.phase {
                    BindingPhase::Preparing => {
                        self.next = if self.run.borrow().stop_requested {
                            Next::StopBinding
                        } else {
                            Next::WaitResources
                        }
                    }
                    BindingPhase::ResourcesReady => {
                        self.next = if self.run.borrow().stop_requested {
                            Next::StopBinding
                        } else if self.resume == Next::WaitResources {
                            Next::IssueUse
                        } else {
                            self.resume
                        }
                    }
                    BindingPhase::Stopping => {
                        self.run.borrow_mut().stop_requested = true;
                        self.next = Next::WaitReleased;
                        self.retry_at = libkernel::time::timeout_millis(1)?
                            .instant()
                            .map_err(|_| Error::Invalid)?;
                    }
                    BindingPhase::Released => {
                        libkernel::debug!(
                            "driver binding Released: device={}",
                            reference.device().serial()
                        );
                        self.run.borrow_mut().released = true;
                        self.phase_end = Deadline::INFINITE;
                        self.next = Next::Detach;
                    }
                }
            }
            Next::IssueUse => {
                if !Self::device_result(&reply, Command::IssueUse.op())?.is_empty()
                    || reply.handles.remaining() != 1
                {
                    return Err(Error::Invalid);
                }
                self.usage = Some(Self::entry(&mut reply, 0, registry)?);
                let declaration = world
                    .catalog
                    .records()
                    .nth(self.run.borrow().implementation)
                    .ok_or(Error::Invalid)??;
                self.next = if declaration.child_registrar {
                    Next::IssueRegistrar
                } else {
                    Next::Spawn
                };
            }
            Next::IssueRegistrar => {
                if !Self::device_result(&reply, Command::IssueRegistrar.op())?.is_empty()
                    || reply.handles.remaining() != 2
                {
                    return Err(Error::Invalid);
                }
                self.registrar = Some(Self::entry(&mut reply, 0, registry)?);
                let admin = Self::entry(&mut reply, 1, registry)?;
                let admin = Arc::try_unwrap(admin).map_err(|_| Error::Invalid)?;
                self.registrar_admin = Some(admin);
                self.next = Next::Spawn;
            }
            Next::Activate | Next::StopActivation => {
                let process = self.process(world).ok_or(Error::Unavailable)?;
                let mailbox = process.borrow().mailbox;
                let receipt = libdriver::client::Receipt::decode(
                    reply,
                    reference,
                    mailbox,
                    if step == Next::Activate { 1 } else { 2 },
                )?;
                if let Some(observation) = &self.observation {
                    if observation.description()?.object_id
                        != receipt.observation.description()?.object_id
                    {
                        return Err(Error::Conflict);
                    }
                } else {
                    self.observation =
                        Some(Arc::try_new(receipt.observation).map_err(|_| Error::Resource)?);
                }
                self.run.borrow_mut().activation_status = Some(receipt.snapshot);
                if receipt.snapshot.phase == protocol::Phase::Ready
                    && !self.run.borrow().stop_requested
                {
                    self.function = receipt
                        .function
                        .map(|entry| Arc::try_new(entry).map_err(|_| Error::Resource))
                        .transpose()?;
                    self.function_name = receipt.name;
                    self.function_description = receipt.description;
                    self.next = if self.run.borrow().activation_watch == 0 {
                        Next::WatchActivation
                    } else {
                        Next::InstallActivation
                    };
                } else {
                    if step == Next::StopActivation {
                        self.activation_stop_sent = true;
                    }
                    if receipt.snapshot.phase == protocol::Phase::Terminal {
                        if let Some(error) = receipt.snapshot.error {
                            self.run.borrow_mut().fail(error);
                        }
                        self.run.borrow_mut().stop_requested = true;
                    }
                    self.next = if self.run.borrow().activation_watch == 0 {
                        Next::WatchActivation
                    } else if self.run.borrow().stop_requested {
                        Next::WaitReleased
                    } else {
                        Next::Running
                    };
                }
                self.acknowledge_activation(requests)?;
            }
            Next::WatchActivation => {
                let observation = alias(self.observation.as_ref().ok_or(Error::NotReady)?)?;
                let watch = libdriver::client::Watch::from_reply(observation, reply, reference)?;
                requests
                    .spawn(TaskSubmission::new(
                        Family::Subscription(subscription::Subscription::activation(
                            self.run.clone(),
                            watch,
                        )),
                        1,
                        world.funding.clone(),
                    ))
                    .map_err(|_| Error::Quota)?;
                self.next = if self.function.is_some() && !self.run.borrow().stop_requested {
                    Next::InstallActivation
                } else if self.run.borrow().stop_requested {
                    Next::WaitReleased
                } else {
                    Next::Running
                };
            }
            Next::InstallActivation => {
                if !Self::device_result(
                    &reply,
                    Command::InstallActivation {
                        protocol: protocol::OBSERVATION_ID,
                    }
                    .op(),
                )?
                .is_empty()
                    || reply.handles.remaining() != 0
                {
                    return Err(Error::Invalid);
                }
                self.next = Next::InstallFunction;
            }
            Next::InstallFunction => {
                if !Self::device_result(
                    &reply,
                    Command::InstallFunction {
                        name: &self.function_name,
                        description: &self.function_description,
                    }
                    .op(),
                )?
                .is_empty()
                    || reply.handles.remaining() != 0
                {
                    return Err(Error::Invalid);
                }
                self.run.borrow_mut().phase = protocol::Phase::Ready;
                self.phase_end = Deadline::INFINITE;
                self.next = Next::Running;
                libkernel::debug!(
                    "driver activation ready: device={}, worker={}, now_ns={}",
                    reference.device().serial(),
                    self.process(world)
                        .ok_or(Error::Unavailable)?
                        .borrow()
                        .pid
                        .ok_or(Error::Unavailable)?,
                    libkernel::time::snapshot()?.now_ns
                );
            }
            Next::StopBinding => {
                if !Self::device_result(&reply, Command::StopBinding.op())?.is_empty()
                    || reply.handles.remaining() != 0
                {
                    return Err(Error::Invalid);
                }
                self.stop_sent = true;
                self.next = Next::WaitReleased;
            }
            Next::ReportStopped => {
                if !Self::device_result(&reply, Command::ReportStopped.op())?.is_empty()
                    || reply.handles.remaining() != 0
                {
                    return Err(Error::Invalid);
                }
                self.reported = true;
                self.next = Next::WaitReleased;
            }
            _ => return Err(Error::Invalid),
        }
        Ok(())
    }
    pub fn advance(
        &mut self,
        id: u64,
        world: &mut World<'_>,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        self.run.borrow_mut().task = id;
        let mut phase_expired = false;
        let was_stopping = self.run.borrow().phase == protocol::Phase::Stopping;
        while input.pull().is_some() {}
        if input.take_timeout() {
            phase_expired = self
                .phase_end
                .instant()
                .expect("driver phase deadline invalid")
                .is_some_and(|at| at <= input.now_ns());
            if phase_expired && !was_stopping {
                libkernel::debug!(
                    "driver stage deadline expired: device={}, stage={:?}, now_ns={}, end_ns={:?}",
                    self.run.borrow().device.serial(),
                    self.next,
                    input.now_ns(),
                    self.phase_end.instant()
                );
                self.run.borrow_mut().fail(Error::NotReady);
            }
        }
        if self.phase_end == Deadline::INFINITE && self.next == Next::Claim {
            // Claim到具名功能安装的共同准备窗口；实测debug出生/协议工作约10s，留运输/调度余量。
            self.phase_end = libkernel::time::timeout_millis(20_000)?;
            libkernel::debug!(
                "driver stage started: device={}, start_ns={}, end_ns={:?}",
                self.run.borrow().device.serial(),
                input.now_ns(),
                self.phase_end.instant()
            );
        }
        if world.stopping {
            self.run.borrow_mut().stop_requested = true;
        }
        if self.run.borrow().stop_requested
            && self.run.borrow().phase != protocol::Phase::Stopping
            && !self.run.borrow().released
        {
            self.run.borrow_mut().phase = protocol::Phase::Stopping;
            self.phase_end = libkernel::time::timeout_millis(30_000)?;
        }
        if let Some(process) = self.process(world) {
            let record = process.borrow();
            if (!record.accepting || record.dead || record.error.is_some())
                && !self.run.borrow().released
            {
                let trusted = world
                    .catalog
                    .records()
                    .nth(self.run.borrow().implementation)
                    .expect("driver implementation missing")
                    .unwrap_or_else(|error| fatal(error))
                    .trusted_physical;
                if record.dead && trusted && self.activation_sent {
                    libkernel::debug!(
                        "trusted physical worker DONE without Binding release: pid={}, device={}, epoch={}, reported={}",
                        record.pid.unwrap_or(0),
                        self.run.borrow().device.serial(),
                        self.run.borrow().reference.epoch(),
                        self.reported
                    );
                    fatal(Error::Unavailable);
                }
                if !world.stopping && (record.dead || record.error.is_some()) {
                    self.run
                        .borrow_mut()
                        .fail(record.error.unwrap_or(Error::Unavailable));
                } else {
                    self.run.borrow_mut().stop_requested = true;
                }
            }
        }
        if let Some(error) = self.cleanup.as_mut() {
            if self.retry_at.is_some_and(|at| input.now_ns() < at) {
                return Ok(advance(Step::Parked));
            }
            if error.retry_cleanup().is_err() {
                self.retry_at = Some(
                    input
                        .now_ns()
                        .checked_add(1_000_000)
                        .ok_or(SystemCallError::ClockRange)?,
                );
                return Ok(advance(Step::Parked));
            }
            self.cleanup = None;
            self.retry_at = None;
        }
        if let Some((txid, step)) = self.call {
            let Some(completion) = world.rpc.take(txid) else {
                if phase_expired && was_stopping {
                    fatal(Error::Unavailable);
                }
                return Ok(advance(Step::Parked));
            };
            self.call = None;
            match completion.result {
                Ok(reply) => {
                    if step == Next::Claim
                        && let Ok((header, body)) = Header::decode(&reply.payload)
                        && header.op
                            == (Command::Claim {
                                reference: self.run.borrow().reference,
                            })
                            .op()
                        && header.error.is_some()
                        && body.is_empty()
                        && reply.handles.remaining() == 0
                    {
                        self.run
                            .borrow_mut()
                            .fail(header.error.expect("Claim error checked"));
                        self.next = Next::Retire;
                        return Ok(advance(Step::Runnable));
                    }
                    if let Err(error) = self.consume(step, reply, world, requests) {
                        libkernel::debug!(
                            "driver operation rejected: device={}, operation={:?}, error={:?}",
                            self.run.borrow().device.serial(),
                            step,
                            error
                        );
                        if self.control.is_none()
                            || matches!(
                                step,
                                Next::QueryBinding | Next::ReportStopped | Next::StopBinding
                            )
                        {
                            fatal(error);
                        }
                        self.run.borrow_mut().fail(error);
                        self.next = Next::WaitReleased;
                    }
                }
                Err(error) => {
                    // 原调用已由 Dispatcher 退休；仍需兑现返还的 reply owner，不重签任何能力。
                    self.failed_call(error, Some(txid));
                }
            }
            return Ok(advance(Step::Runnable));
        }
        if phase_expired && was_stopping && !self.run.borrow().released {
            fatal(Error::Unavailable);
        }
        if self.control.is_some()
            && self.run.borrow().binding_changed
            && self.next != Next::QueryBinding
        {
            self.resume = self.next;
            self.next = Next::QueryBinding;
        } else if self.control.is_some()
            && self.run.borrow().stop_requested
            && !self.run.borrow().released
        {
            if !self.stop_sent {
                self.next = Next::StopBinding;
            } else if self.process(world).is_some_and(|process| {
                let record = process.borrow();
                record.entry.is_some() && !record.dead
            }) && !self.activation_stop_sent
            {
                self.next = Next::StopActivation;
            } else {
                let run = self.run.borrow();
                let trusted = world
                    .catalog
                    .records()
                    .nth(run.implementation)
                    .expect("driver implementation missing")
                    .unwrap_or_else(|error| fatal(error))
                    .trusted_physical;
                if trusted
                    && !self.reported
                    && run
                        .activation_status
                        .is_some_and(|status| status.consumer_stopped)
                {
                    self.next = Next::ReportStopped;
                } else if run.activation_changed
                    && self
                        .process(world)
                        .is_some_and(|process| !process.borrow().dead)
                {
                    self.next = Next::StopActivation;
                }
            }
        } else if !self.run.borrow().released
            && self.run.borrow().activation_changed
            && self.observation.is_some()
        {
            self.next = Next::Activate;
        }
        if self.run.borrow().stop_requested && self.control.is_none() && self.next == Next::Claim {
            self.next = Next::Retire;
        }
        match self.next {
            Next::WaitResources | Next::Running => {
                return Ok(advance(Step::Parked));
            }
            Next::WaitReleased => {
                // 来源封口可先终结 Binding Watch；原 Control 的完成观察仍由本任务推进，
                // 不能依赖已经退休的通知或外部再次 Stop 来发现 Released。
                if self.retry_at.is_some_and(|at| input.now_ns() < at) {
                    return Ok(advance(Step::Parked));
                }
                self.retry_at = None;
                self.resume = Next::WaitReleased;
                self.next = Next::QueryBinding;
            }
            Next::Spawn => {
                match worker::associate(world, requests, &self.run) {
                    Ok(_) => self.next = Next::WaitBoot,
                    Err(error) => {
                        libkernel::debug!("driver process admission failed: {:?}", error);
                        self.run.borrow_mut().fail(error);
                        self.next = Next::WaitReleased;
                    }
                }
                return Ok(advance(Step::Runnable));
            }
            Next::WaitBoot => {
                let ready = self
                    .process(world)
                    .is_some_and(|process| process.borrow().ready());
                if ready {
                    self.next = if self.run.borrow().stop_requested {
                        Next::StopActivation
                    } else {
                        Next::Activate
                    };
                    return Ok(advance(Step::Runnable));
                }
                return Ok(advance(Step::Parked));
            }
            Next::Detach => {
                {
                    let mut run = self.run.borrow_mut();
                    run.watchers_stopping = true;
                    if run.binding_watch != 0 {
                        requests.wake(run.binding_watch)?;
                    }
                    if run.activation_watch != 0 {
                        requests.wake(run.activation_watch)?;
                    }
                }
                if self.run.borrow().binding_watch != 0 || self.run.borrow().activation_watch != 0 {
                    return Ok(advance(Step::Parked));
                }
                self.usage = None;
                self.registrar = None;
                self.registrar_admin = None;
                self.observation = None;
                self.function = None;
                // 关联直到本轮安全Released且RPC/两类订阅真实脱离；不等待进程收集形成环。
                let process = self.run.borrow_mut().process.take();
                if let Some(process) = process {
                    let task = world
                        .processes
                        .get(process.serial())
                        .map(|record| record.borrow().task)
                        .unwrap_or(0);
                    if task != 0 {
                        requests.wake(task)?;
                    }
                }
                self.control = None;
                self.next = Next::Retire;
                return Ok(advance(Step::Runnable));
            }
            Next::Retire => {
                if self.control.is_some()
                    || self.call.is_some()
                    || self.cleanup.is_some()
                    || self.usage.is_some()
                    || self.observation.is_some()
                    || self.run.borrow().process.is_some()
                    || self.run.borrow().binding_watch != 0
                    || self.run.borrow().activation_watch != 0
                {
                    fatal(Error::Invalid);
                }
                let serial = self.run.borrow().device.serial();
                {
                    let run = self.run.borrow();
                    let key = world
                        .catalog
                        .records()
                        .nth(run.implementation)
                        .expect("driver implementation missing")
                        .unwrap_or_else(|e| fatal(e))
                        .key;
                    libkernel::debug!(
                        "driver attempt retired: device={}, epoch={}, key={}, released={}, failure={:?}",
                        run.device.serial(),
                        run.reference.epoch(),
                        key,
                        run.released,
                        run.failure
                    );
                }
                if world.stopping {
                    world.runs.remove(serial);
                } else {
                    // 原Run保留本实例已尝试候选；不是新的占用事实，撤出/退休发现后才删除。
                    let mut run = self.run.borrow_mut();
                    run.task = 0;
                    run.phase = protocol::Phase::Terminal;
                }
                world.discovery_dirty = true;
                requests.wake(world.manager_task)?;
                return Ok(advance(Step::Complete));
            }
            _ => {}
        }
        let reference = self.run.borrow().reference;
        let rpc_end = deadline()?;
        let deadline = if self.phase_end == Deadline::INFINITE {
            rpc_end
        } else {
            Deadline::at(self.phase_end.at_ns.min(rpc_end.at_ns))
        };
        let mut bytes = [0; device::HEADER_LEN + device::BODY_MAX];
        if self.next == Next::Claim || (self.next == Next::Activate && !self.activation_sent) {
            let run = self.run.borrow();
            let declaration = world
                .catalog
                .records()
                .nth(run.implementation)
                .expect("driver implementation missing")
                .unwrap_or_else(|e| fatal(e));
            libkernel::debug!(
                "driver binding input prepared: operation={:?}, device={}, epoch={}, key={}, registrar={}, trusted_physical={}",
                self.next,
                run.device.serial(),
                run.reference.epoch(),
                declaration.key,
                self.registrar.is_some(),
                declaration.trusted_physical
            );
        }
        let (target, mut request) = match self.next {
            Next::Activate | Next::StopActivation => {
                let command = if self.next == Next::Activate {
                    protocol::Command::Activate {
                        reference,
                        identity: &self.identity,
                        registrar: self.registrar.is_some(),
                    }
                } else {
                    protocol::Command::Stop { reference }
                };
                let used = command
                    .encode(deadline, &mut bytes)
                    .unwrap_or_else(|error| fatal(error));
                let mut request = Request::new(protocol::ID, &bytes[..used])?;
                request
                    .push(
                        alias(self.usage.as_ref().expect("driver BindingUse missing"))
                            .unwrap_or_else(|error| fatal(error))
                            .into_capability(),
                        TRANSPORT,
                    )
                    .map_err(|failure| failure.error)?;
                if self.next == Next::Activate
                    && let Some(registrar) = &self.registrar
                {
                    request
                        .push(
                            alias(registrar)
                                .unwrap_or_else(|error| fatal(error))
                                .into_capability(),
                            TRANSPORT,
                        )
                        .map_err(|failure| failure.error)?;
                }
                self.activation_sent = true;
                (
                    self.process(world)
                        .and_then(|process| process.borrow().entry.clone())
                        .expect("driver activation entry missing"),
                    request,
                )
            }
            Next::WatchActivation => {
                let used = protocol::Observe::Watch
                    .encode(deadline, &mut bytes)
                    .unwrap_or_else(|error| fatal(error));
                (
                    self.observation
                        .as_ref()
                        .expect("driver observation entry missing")
                        .clone(),
                    Request::new(protocol::OBSERVATION_ID, &bytes[..used])?,
                )
            }
            step => {
                let command = match step {
                    Next::Claim => Command::Claim { reference },
                    Next::WatchBinding => Command::WatchBinding,
                    Next::QueryBinding => Command::QueryBinding,
                    Next::IssueUse => Command::IssueUse,
                    Next::IssueRegistrar => Command::IssueRegistrar,
                    Next::InstallActivation => Command::InstallActivation {
                        protocol: protocol::OBSERVATION_ID,
                    },
                    Next::InstallFunction => Command::InstallFunction {
                        name: &self.function_name,
                        description: &self.function_description,
                    },
                    Next::StopBinding => Command::StopBinding,
                    Next::ReportStopped => Command::ReportStopped,
                    _ => return Err(SystemCallError::InternalError),
                };
                let used = command
                    .encode(deadline, &mut bytes)
                    .unwrap_or_else(|error| fatal(error));
                (
                    if step == Next::Claim {
                        world.claim.clone()
                    } else {
                        self.control
                            .as_ref()
                            .expect("driver BindingControl missing")
                            .clone()
                    },
                    Request::new(device::ID, &bytes[..used])?,
                )
            }
        };
        if self.next == Next::InstallActivation {
            request
                .push(
                    alias(
                        self.observation
                            .as_ref()
                            .expect("driver observation missing"),
                    )
                    .unwrap_or_else(|error| fatal(error))
                    .into_capability(),
                    TRANSPORT,
                )
                .map_err(|failure| failure.error)?;
        }
        if self.next == Next::InstallFunction {
            request
                .push(
                    alias(self.function.as_ref().expect("driver function missing"))
                        .unwrap_or_else(|error| fatal(error))
                        .into_capability(),
                    TRANSPORT,
                )
                .map_err(|failure| failure.error)?;
        }
        match world.call(id, target, request, deadline) {
            Ok(txid) => {
                self.call = Some((txid, self.next));
            }
            Err(error) => {
                self.failed_call(error, None);
                return Ok(advance(Step::Runnable));
            }
        }
        Ok(advance(Step::Parked))
    }
}
