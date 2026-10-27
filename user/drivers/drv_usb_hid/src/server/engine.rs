//! HID的真实USB消费者；原seq跨Unknown保留，停止先取消/观察原传输再终结功能。
use super::*;
use libusb::protocol::{self as usb, Command, Progress, Transfer};
#[derive(Clone, Copy)]
enum Call {
    Resource,
    Usb(Command),
}
pub(super) struct Engine {
    instance: InstanceRef,
    pending: Option<(u64, Call)>,
    session: Option<Arc<MailboxSender>>,
    command: Command,
    lifecycle: Option<Box<libdriver::binding::BindingTask>>,
    observed_sequence: u64,
    completion: Option<libkernel::ipc::notification::NotificationPair>,
    completion_source: SourceRegistration,
    completion_pending: bool,
    cancel_sent: bool,
}
impl Engine {
    pub fn new(instance: InstanceRef) -> Result<Self, Error> {
        let completion = libkernel::ipc::notification::create(
            erhino_shared::object::Rights::READ | erhino_shared::object::Rights::WAIT,
            erhino_shared::object::Rights::SIGNAL
                | erhino_shared::object::Rights::DUPLICATE
                | erhino_shared::object::Rights::TRANSIT,
        )?;
        Ok(Self {
            instance,
            pending: None,
            session: None,
            command: Command::Transfer {
                sequence: 1,
                request: Transfer::BootProtocol,
            },
            lifecycle: Some(
                Box::try_new(libdriver::binding::BindingTask::new())
                    .map_err(|_| Error::Resource)?,
            ),
            observed_sequence: 0,
            completion: Some(completion),
            completion_source: SourceRegistration::new(),
            completion_pending: false,
            cancel_sent: false,
        })
    }
    pub fn registered(&mut self, kind: u64, id: SourceId) {
        if kind == 9 {
            self.completion_source.registered(kind, id);
        }
        let mut state = self.instance.borrow_mut();
        state.slot.registered(kind, id);
        state.keys.registered(kind, id);
    }
    pub fn unregistered(&mut self, id: SourceId) {
        self.completion_source.unregistered(id);
        let mut state = self.instance.borrow_mut();
        state.slot.unregistered(id);
        state.keys.unregistered(id);
    }
    pub fn deadline(&self) -> Deadline {
        Deadline::INFINITE
    }
    fn begin(
        &mut self,
        id: u64,
        world: &mut World,
        target: Arc<MailboxSender>,
        kind: u64,
        wire: &[u8],
        call: Call,
    ) -> Result<(), Error> {
        let end = libkernel::time::timeout_millis(5_000)?;
        let mut request = librpc::Request::new(kind, wire)?;
        if let Call::Usb(command) = call
            && command.handles() != 0
        {
            let rights = erhino_shared::object::Rights::SIGNAL;
            let handle = object::duplicate(
                self.completion
                    .as_ref()
                    .ok_or(Error::Closed)?
                    .peer
                    .as_handle(),
                rights | erhino_shared::object::Rights::TRANSIT,
            )?;
            // SAFETY: 新 entry 的唯一关闭责任移入这次原 seq 请求。
            request
                .push(unsafe { Capability::from_raw(handle) }, rights)
                .map_err(|failure| failure.error)?;
        }
        let tx = world
            .rpc
            .begin_for(
                ServiceOwner::Shared(target),
                end,
                request,
                Some(id),
                &world.funding,
            )
            .map_err(|_| Error::Unavailable)?;
        self.pending = Some((tx, call));
        world.rpc_dirty = true;
        Ok(())
    }
    fn terminal(&mut self, state: &mut Instance, error: Option<Error>) -> Result<(), Error> {
        state.slot.stop()?;
        if error.is_none() {
            // SAFETY: 仅由原seq明确Complete/Cancelled/Progress::Closed，或从未取得session的
            // 分支调用；HID无物理能力且不再提交USB访问。错误/Unknown不产生这个证明。
            unsafe {
                state.slot.consumer_stopped()?;
            }
        }
        state.slot.terminal(error)?;
        self.session = None;
        state.keys.state.disconnect()?;
        state.keys.changed()?;
        if let Some(error) = error {
            libkernel::debug!(
                "HID interface failed: {:?}; original provider retains unsettled transfer responsibility",
                error
            );
        } else {
            libkernel::debug!(
                "HID interface stopped: original USB transfer settled; state disconnected"
            );
        }
        Ok(())
    }
    pub fn advance(
        &mut self,
        id: u64,
        world: &mut World,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        let instance = self.instance.clone();
        let mut state = instance.borrow_mut();
        state.task = id;
        if let Some(lifecycle) = self.lifecycle.take() {
            requests
                .spawn(TaskSubmission::new(
                    Family::Binding(lifecycle, instance.clone()),
                    1,
                    world.instance_funding.clone(),
                ))
                .map_err(|_| SystemCallError::OutOfMemory)?;
        }
        let sequence = state
            .slot
            .activation()
            .map_or(0, |activation| activation.snapshot().sequence);
        if sequence != self.observed_sequence
            && let Some(task) = state.slot.lifecycle_task()
        {
            self.observed_sequence = sequence;
            requests.wake(task)?;
        }
        while let Some(event) = input.pull() {
            self.completion_pending |= event.kind == 9;
            state.slot.event(event);
            state.keys.event(event);
        }
        input.take_timeout();
        let pending = state.slot.advance_lifetimes(requests)? | state.keys.advance(requests)?;
        if pending {
            return Ok(step(Step::Runnable));
        }
        if state
            .slot
            .activation()
            .is_some_and(|activation| activation.snapshot().phase == protocol::Phase::Terminal)
            || !state.slot.active()
        {
            self.completion_source.remove(requests)?;
            if self.completion_source.is_empty() {
                self.completion = None;
            }
        }
        if state.slot.tail_done() && state.keys.is_empty() && self.completion_source.is_empty() {
            world
                .instances
                .retain(|entry| !Rc::ptr_eq(entry, &instance));
            return Ok(step(Step::Complete));
        }
        if !state.slot.io_ready() {
            return Ok(step(Step::Parked));
        }
        let Some(activation) = state.slot.activation() else {
            return Ok(step(Step::Parked));
        };
        let stopping = activation.snapshot().phase == protocol::Phase::Stopping;
        if let Some((tx, call)) = self.pending {
            let Some(completion) = world.rpc.take(tx) else {
                return Ok(step(Step::Parked));
            };
            self.pending = None;
            match completion.result {
                Err(_) => {
                    // Unknown 不合成安全完成；本地封口并交原 provider 停止 TD。
                    self.terminal(&mut state, Some(Error::Unavailable))
                        .unwrap_or_else(|error| fatal(error));
                    return Ok(step(Step::Runnable));
                }
                Ok(mut reply) => match call {
                    Call::Resource => {
                        let (header, body) =
                            device::Header::decode(&reply.payload).unwrap_or_else(|e| fatal(e));
                        if header.op != 10 {
                            fatal(Error::Invalid);
                        }
                        if let Some(error) = header.error {
                            if error == Error::Closed
                                && body.is_empty()
                                && reply.handles.remaining() == 0
                            {
                                // Stop先到：未取得会话，此实例不再提交访问；其余实例继续运行。
                                self.terminal(&mut state, None).unwrap_or_else(|e| fatal(e));
                                return Ok(step(Step::Runnable));
                            }
                            fatal(error);
                        }
                        let entry = libdevice::resource::Entry::decode(
                            body,
                            device::HEADER_LEN,
                            reply.handles.remaining(),
                        )
                        .unwrap_or_else(|e| fatal(e));
                        if entry.name != "usb-interface"
                            || entry.descriptor.kind
                                != erhino_shared::object::ObjectKind::MailboxSender
                            || entry.descriptor.protocol != usb::ID
                            || entry.descriptor.rights != TRANSPORT
                        {
                            fatal(Error::Permission);
                        }
                        libusb::descriptor::Keyboard::decode(entry.metadata)
                            .unwrap_or_else(|e| fatal(e));
                        let cap = reply.handles.take(0)?;
                        entry
                            .descriptor
                            .check_handle(&cap.description()?)
                            .unwrap_or_else(|e| fatal(e));
                        self.session = Some(
                            Arc::try_new(
                                MailboxSender::from_capability(cap).map_err(|f| f.error)?.0,
                            )
                            .map_err(|_| SystemCallError::OutOfMemory)?,
                        );
                        libkernel::debug!("HID acquired restricted USB interface session");
                    }
                    Call::Usb(command) => {
                        let (header, body) =
                            usb::Header::decode(&reply.payload).unwrap_or_else(|e| fatal(e));
                        if header.op != command.op() || reply.handles.remaining() != 0 {
                            fatal(Error::Invalid);
                        }
                        if let Some(error) = header.error {
                            if !body.is_empty() {
                                fatal(Error::Invalid);
                            }
                            // 认证/旧轮/拒绝错误只封本地提交；原TD仍由provider的Provision Stop收束。
                            self.terminal(&mut state, Some(error))
                                .unwrap_or_else(|e| fatal(e));
                            return Ok(step(Step::Runnable));
                        }
                        match Progress::decode(body, command.sequence())
                            .unwrap_or_else(|e| fatal(e))
                        {
                            Progress::Pending => {
                                self.command = Command::Query {
                                    sequence: command.sequence(),
                                }
                            }
                            Progress::Closed | Progress::Cancelled => {
                                self.terminal(&mut state, None).unwrap_or_else(|e| fatal(e));
                                return Ok(step(Step::Runnable));
                            }
                            Progress::Complete { bytes, count } => {
                                if command.sequence() == 1 {
                                    if count != 0 {
                                        fatal(Error::Invalid);
                                    }
                                    if !stopping {
                                        state.slot.ready().unwrap_or_else(|e| fatal(e));
                                        libkernel::debug!(
                                            "HID activation ready: boot protocol confirmed"
                                        );
                                    }
                                } else {
                                    if count != 8 {
                                        fatal(Error::Invalid);
                                    }
                                    state.keys.state.report(bytes).unwrap_or_else(|e| fatal(e));
                                    state.keys.changed().unwrap_or_else(|e| fatal(e));
                                    libkernel::debug!(
                                        "HID keyboard report: sequence={}, modifiers={:#x}, keys={:?}, valid={}",
                                        state.keys.state.snapshot().sequence,
                                        state.keys.state.snapshot().modifiers,
                                        state.keys.state.snapshot().keys,
                                        state.keys.state.snapshot().valid
                                    );
                                }
                                if stopping {
                                    self.terminal(&mut state, None).unwrap_or_else(|e| fatal(e));
                                    return Ok(step(Step::Runnable));
                                }
                                self.cancel_sent = false;
                                self.command = Command::Transfer {
                                    sequence: command
                                        .sequence()
                                        .checked_add(1)
                                        .unwrap_or_else(|| fatal(Error::Exhausted)),
                                    request: Transfer::InterruptIn,
                                };
                            }
                        }
                    }
                },
            }
            return Ok(step(Step::Runnable));
        }
        if state
            .slot
            .activation()
            .is_some_and(|a| a.snapshot().phase == protocol::Phase::Terminal)
        {
            return Ok(step(Step::Parked));
        }
        if stopping && self.session.is_none() {
            self.terminal(&mut state, None).unwrap_or_else(|e| fatal(e));
            return Ok(step(Step::Runnable));
        }
        if self.session.is_none() {
            let mut wire = [0; device::HEADER_LEN + 32];
            let used = device::Command::GetResource {
                name: "usb-interface",
            }
            .encode(operation_end()?, &mut wire)
            .unwrap_or_else(|e| fatal(e));
            let usage = state
                .slot
                .activation()
                .ok_or(SystemCallError::InternalError)?
                .usage()
                .unwrap_or_else(|e| fatal(e))
                .clone();
            self.begin(id, world, usage, device::ID, &wire[..used], Call::Resource)
                .unwrap_or_else(|e| fatal(e));
        } else {
            if self.completion_source.is_empty() {
                self.completion_source.install(
                    requests,
                    SourcePlan::new(
                        self.completion
                            .as_ref()
                            .ok_or(SystemCallError::ObjectClosed)?
                            .owner
                            .as_handle(),
                        ObjectSignals::READABLE,
                    ),
                    9,
                )?;
                return Ok(step(Step::Runnable));
            }
            if self.completion_source.source().is_none() {
                return Ok(step(Step::Runnable));
            }
            let command = if stopping && !self.cancel_sent {
                self.cancel_sent = true;
                Command::Cancel {
                    sequence: self.command.sequence(),
                }
            } else {
                self.command
            };
            if matches!(command, Command::Query { .. }) {
                if !self.completion_pending {
                    return Ok(step(Step::Parked));
                }
                match self
                    .completion
                    .as_ref()
                    .ok_or(SystemCallError::ObjectClosed)?
                    .owner
                    .take(u64::MAX)
                {
                    Ok(_) | Err(SystemCallError::ObjectNotAvailable) => {}
                    Err(error) => return Err(error),
                }
                self.completion_pending = false;
            }
            let mut wire = [0; usb::HEADER_LEN + 12];
            // Interrupt IN可持续NAK，RPC回复期限与该已提交TD的寿命分别建模。
            let end = if command.sequence() == 1 || stopping {
                operation_end()?
            } else {
                Deadline::INFINITE
            };
            let used = command.encode(end, &mut wire).unwrap_or_else(|e| fatal(e));
            self.begin(
                id,
                world,
                self.session
                    .as_ref()
                    .ok_or(SystemCallError::InternalError)?
                    .clone(),
                usb::ID,
                &wire[..used],
                Call::Usb(command),
            )
            .unwrap_or_else(|e| fatal(e));
        }
        Ok(step(Step::Parked))
    }
}
