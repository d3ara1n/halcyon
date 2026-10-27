//! HID的真实USB消费者；原seq跨Unknown保留，停止先取消/观察原传输再终结功能。
use super::*;
use libusb::protocol::{self as usb, Command, Progress, Transfer};
#[derive(Clone, Copy)]
enum Call {
    Resource,
    Usb(Command),
    Binding,
}
pub(super) struct Engine {
    instance: InstanceRef,
    pending: Option<(u64, Call)>,
    session: Option<Arc<MailboxSender>>,
    command: Command,
    next_poll: Deadline,
}
impl Engine {
    pub fn new(instance: InstanceRef) -> Self {
        Self {
            instance,
            pending: None,
            session: None,
            command: Command::Transfer {
                sequence: 1,
                request: Transfer::BootProtocol,
            },
            next_poll: Deadline::INFINITE,
        }
    }
    pub fn registered(&mut self, kind: u64, id: SourceId) {
        let mut state = self.instance.borrow_mut();
        state.slot.registered(kind, id);
        state.keys.registered(kind, id);
    }
    pub fn unregistered(&mut self, id: SourceId) {
        let mut state = self.instance.borrow_mut();
        state.slot.unregistered(id);
        state.keys.unregistered(id);
    }
    pub fn deadline(&self) -> Deadline {
        if self.pending.is_some() {
            Deadline::INFINITE
        } else {
            self.next_poll
        }
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
        let tx = world
            .rpc
            .begin_for(
                ServiceOwner::Shared(target),
                end,
                librpc::Request::new(kind, wire)?,
                Some(id),
                &world.funding,
            )
            .map_err(|_| Error::Unavailable)?;
        self.pending = Some((tx, call));
        world.rpc_dirty = true;
        Ok(())
    }
    fn terminal(&mut self, state: &mut Instance, error: Option<Error>) -> Result<(), Error> {
        let activation = state.slot.activation.as_mut().ok_or(Error::NotReady)?;
        activation.stop();
        if error.is_none() {
            // SAFETY: 仅由原seq明确Complete/Cancelled/Progress::Closed，或从未取得session的
            // 分支调用；HID无物理能力且不再提交USB访问。错误/Unknown不产生这个证明。
            unsafe {
                activation.consumer_stopped()?;
            }
        }
        activation.terminal(error);
        self.session = None;
        state.keys.state.disconnect()?;
        state.keys.changed()?;
        state.slot.changed().unwrap_or_else(|e| fatal(e));
        self.next_poll = Deadline::INFINITE;
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
        while let Some(event) = input.pull() {
            state.slot.event(event);
            state.keys.event(event);
        }
        input.take_timeout();
        let pending = state.slot.advance_lifetimes(requests)? | state.keys.advance(requests)?;
        if pending {
            return Ok(step(Step::Runnable));
        }
        if state.slot.activation.is_some() && state.slot.tail_done() && state.keys.is_empty() {
            world
                .instances
                .retain(|entry| !Rc::ptr_eq(entry, &instance));
            return Ok(step(Step::Complete));
        }
        let Some(activation) = state.slot.activation.as_ref() else {
            return Ok(step(Step::Parked));
        };
        let mut stopping = activation.snapshot().phase == protocol::Phase::Stopping;
        if let Some((tx, call)) = self.pending {
            let Some(completion) = world.rpc.take(tx) else {
                return Ok(step(Step::Parked));
            };
            self.pending = None;
            match completion.result {
                Err(_) => {
                    // 原USB seq未换轮；只读查询/运输Unknown仍回原会话，原父停止负责最终封口。
                    self.next_poll = libkernel::time::timeout_millis(2)?;
                    return Ok(step(Step::Parked));
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
                    Call::Binding => {
                        let (header, body) =
                            device::Header::decode(&reply.payload).unwrap_or_else(|e| fatal(e));
                        if header.op != 8
                            || header.error.is_some()
                            || reply.handles.remaining() != 0
                        {
                            fatal(header.error.unwrap_or(Error::Invalid));
                        }
                        let snapshot = device::decode_snapshot(body).unwrap_or_else(|e| fatal(e));
                        let activation = state
                            .slot
                            .activation
                            .as_mut()
                            .ok_or(SystemCallError::InternalError)?;
                        if snapshot.reference != activation.reference() {
                            fatal(Error::StaleEpoch);
                        }
                        if snapshot.phase == libdevice::binding::Phase::Released
                            && activation.snapshot().phase == protocol::Phase::Terminal
                        {
                            state.slot.release(snapshot).unwrap_or_else(|e| fatal(e));
                        } else if matches!(
                            snapshot.phase,
                            libdevice::binding::Phase::Stopping
                                | libdevice::binding::Phase::Released
                        ) {
                            activation.stop();
                            state.slot.changed().unwrap_or_else(|e| fatal(e));
                            stopping = true;
                        }
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
                                        state
                                            .slot
                                            .activation
                                            .as_mut()
                                            .ok_or(SystemCallError::InternalError)?
                                            .ready()
                                            .unwrap_or_else(|e| fatal(e));
                                        state.slot.changed().unwrap_or_else(|e| fatal(e));
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
            if !stopping {
                self.next_poll = libkernel::time::timeout_millis(2)?;
                return Ok(step(Step::Parked));
            }
        }
        if state
            .slot
            .activation
            .as_ref()
            .is_some_and(|a| a.snapshot().phase == protocol::Phase::Terminal)
        {
            if !state.slot.active() {
                self.next_poll = Deadline::INFINITE;
                return Ok(step(Step::Parked));
            }
            if self.next_poll == Deadline::INFINITE || libkernel::time::expired(self.next_poll)? {
                let usage = state
                    .slot
                    .activation
                    .as_ref()
                    .ok_or(SystemCallError::InternalError)?
                    .usage()
                    .unwrap_or_else(|e| fatal(e))
                    .clone();
                let mut wire = [0; device::HEADER_LEN];
                let used = device::Command::QueryBinding
                    .encode(operation_end()?, &mut wire)
                    .unwrap_or_else(|e| fatal(e));
                self.begin(id, world, usage, device::ID, &wire[..used], Call::Binding)
                    .unwrap_or_else(|e| fatal(e));
                self.next_poll = libkernel::time::timeout_millis(2)?;
            }
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
                .activation
                .as_ref()
                .ok_or(SystemCallError::InternalError)?
                .usage()
                .unwrap_or_else(|e| fatal(e))
                .clone();
            self.begin(id, world, usage, device::ID, &wire[..used], Call::Resource)
                .unwrap_or_else(|e| fatal(e));
        } else {
            let command = if stopping {
                Command::Cancel {
                    sequence: self.command.sequence(),
                }
            } else {
                self.command
            };
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
        self.next_poll = libkernel::time::timeout_millis(2)?;
        Ok(step(Step::Parked))
    }
}
