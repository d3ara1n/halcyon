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
    ActivationResult,
    WaitReleased,
    Detach,
    Retire,
}
pub(super) struct Managed {
    run: Rc<RefCell<Run>>,
    identity: Vec<u8>,
    next: Next,
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
    phase_end: Deadline,
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
            phase_end: Deadline::INFINITE,
        })
    }
    pub fn stop(&mut self) {
        self.run.borrow_mut().stop_requested = true;
    }
    pub fn deadline(&self) -> Deadline {
        self.phase_end
    }
    fn failed_call(&mut self, error: librpc::CallError, txid: Option<u64>) {
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
        // Dispatcher 完成已退休观察来源；未发送请求由 error 的 typed owner 关闭。
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
            Next::Activate | Next::ActivationResult => {
                let mailbox = self
                    .process(world)
                    .ok_or(Error::Unavailable)?
                    .borrow()
                    .mailbox;
                let receipt = libdriver::client::Receipt::decode(
                    reply,
                    reference,
                    mailbox,
                    if step == Next::Activate { 1 } else { 2 },
                )?;
                if let Some(observation) = receipt.observation {
                    if self.observation.as_ref().is_some_and(|old| {
                        old.description().is_ok_and(|d| {
                            observation
                                .description()
                                .is_ok_and(|new| d.object_id != new.object_id)
                        })
                    }) {
                        return Err(Error::Conflict);
                    }
                    self.observation =
                        Some(Arc::try_new(observation).map_err(|_| Error::Resource)?);
                }
                let mut run = self.run.borrow_mut();
                if run
                    .activation_status
                    .is_none_or(|old| receipt.snapshot.sequence >= old.sequence)
                {
                    run.activation_status = Some(receipt.snapshot);
                }
                if receipt.snapshot.phase == protocol::Phase::Ready && !run.stop_requested {
                    self.function = receipt
                        .function
                        .map(|entry| Arc::try_new(entry).map_err(|_| Error::Resource))
                        .transpose()?;
                    self.function_name = receipt.name;
                    self.function_description = receipt.description;
                }
                self.next = if step == Next::Activate {
                    Next::WatchActivation
                } else if self.function.is_some() && !run.stop_requested {
                    Next::InstallActivation
                } else {
                    Next::Running
                };
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
                self.next = Next::Running;
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
            _ => return Err(Error::Invalid),
        }
        Ok(())
    }
    /// 观察事实和单调意图是阶段的唯一输入；RPC 回复只推进对应操作。
    fn reconcile(&mut self, world: &World<'_>, now: u64) -> Result<(), SystemCallError> {
        let mut run = self.run.borrow_mut();
        run.stop_requested |= world.stopping;
        if let Some(snapshot) = run.binding_status.and_then(|info| info.binding) {
            match snapshot.phase {
                BindingPhase::Stopping => {
                    run.stop_requested = true;
                    self.stop_sent = true;
                }
                BindingPhase::Released if !run.released => {
                    libkernel::debug!("driver binding Released: device={}", run.device.serial());
                    run.released = true;
                    run.stop_requested = true;
                }
                BindingPhase::ResourcesReady if self.next == Next::WaitResources => {
                    self.next = Next::IssueUse;
                }
                _ => {}
            }
        }
        if !run.released
            && let Some(status) = run.activation_status
            && matches!(
                status.phase,
                protocol::Phase::Stopping | protocol::Phase::Terminal
            )
        {
            if let Some(error) = status.error {
                run.fail(error);
            }
            run.stop_requested = true;
        }
        if !run.released
            && self
                .phase_end
                .instant()
                .expect("driver phase deadline invalid")
                .is_some_and(|end| now >= end)
        {
            if run.phase == protocol::Phase::Stopping {
                fatal(Error::Unavailable);
            }
            libkernel::debug!(
                "driver stage deadline expired: device={}, stage={:?}, now_ns={}, end_ns={:?}",
                run.device.serial(),
                self.next,
                now,
                self.phase_end.instant()
            );
            run.fail(Error::NotReady);
        }
        if run.released {
            self.phase_end = Deadline::INFINITE;
            if self.call.is_none() && self.next != Next::Retire {
                self.next = Next::Detach;
            }
        } else if run.stop_requested {
            if run.phase != protocol::Phase::Stopping {
                run.phase = protocol::Phase::Stopping;
                self.phase_end = Deadline::at(
                    now.checked_add(30_000_000_000)
                        .ok_or(SystemCallError::ClockRange)?,
                );
            }
            // Claim 回包后必须先接通原轮观察；停止不能跳过唯一 Released 出口。
            if self.call.is_none() && self.control.is_some() && self.next != Next::WatchBinding {
                self.next = if self.stop_sent {
                    Next::WaitReleased
                } else {
                    Next::StopBinding
                };
            }
        } else if self.next == Next::Running {
            if self.function.is_some() && run.phase != protocol::Phase::Ready {
                run.phase = protocol::Phase::Ready;
                self.phase_end = Deadline::INFINITE;
                let reference = run.reference;
                drop(run);
                libkernel::debug!(
                    "driver activation ready: device={}, worker={}, now_ns={}",
                    reference.device().serial(),
                    self.process(world)
                        .and_then(|record| record.borrow().pid)
                        .expect("installed driver process missing"),
                    now
                );
                return Ok(());
            }
            if self.function.is_none()
                && run
                    .activation_status
                    .is_some_and(|status| status.phase == protocol::Phase::Ready)
            {
                self.next = Next::ActivationResult;
            }
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
        while input.pull().is_some() {}
        input.take_timeout();
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
        self.reconcile(world, input.now_ns())?;
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
                        "trusted physical worker DONE without Binding release: pid={}, device={}, epoch={}",
                        record.pid.unwrap_or(0),
                        self.run.borrow().device.serial(),
                        self.run.borrow().reference.epoch()
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
        self.reconcile(world, input.now_ns())?;
        if let Some((txid, step)) = self.call {
            let Some(completion) = world.rpc.take(txid) else {
                // 停止与期限已由 reconcile 统一处理，原调用继续兑现运输退休。
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
                        if self.control.is_none() || matches!(step, Next::StopBinding) {
                            fatal(error);
                        }
                        self.run.borrow_mut().fail(error);
                        self.next = Next::WaitReleased;
                    }
                }
                Err(error) => {
                    // 原调用及观察来源已由 Dispatcher 退休，不重签任何能力。
                    self.failed_call(error, Some(txid));
                }
            }
            self.reconcile(world, input.now_ns())?;
            return Ok(advance(Step::Runnable));
        }
        // 此后只按已协调的操作推进，不再从旧回复回写生命周期。
        if self.run.borrow().stop_requested && self.control.is_none() && self.next == Next::Claim {
            self.next = Next::Retire;
        }
        match self.next {
            Next::WaitResources | Next::Running => {
                return Ok(advance(Step::Parked));
            }
            Next::WaitReleased => return Ok(advance(Step::Parked)),
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
                        Next::StopBinding
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
            Next::Activate | Next::ActivationResult => {
                let command = if self.next == Next::Activate {
                    protocol::Command::Activate {
                        reference,
                        identity: &self.identity,
                        registrar: self.registrar.is_some(),
                    }
                } else {
                    protocol::Command::QueryActivationResult { reference }
                };
                let used = command
                    .encode(deadline, &mut bytes)
                    .unwrap_or_else(|error| fatal(error));
                let mut request = Request::new(protocol::ID, &bytes[..used])?;
                if self.next == Next::Activate {
                    request
                        .push(
                            alias(self.usage.as_ref().expect("driver BindingUse missing"))
                                .unwrap_or_else(|error| fatal(error))
                                .into_capability(),
                            TRANSPORT,
                        )
                        .map_err(|failure| failure.error)?;
                }
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
                self.activation_sent |= self.next == Next::Activate;
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
