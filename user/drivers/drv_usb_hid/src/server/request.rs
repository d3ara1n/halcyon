//! 有界入站 RPC；统一激活/观察由 libdriver 持有，HID 仅解释本领域供应。
use super::*;
use alloc::vec::Vec;
use libkernel::ipc::message::ReceivedMessage;
use librpc::{exchange::RequestContext, outbox::Outbox};
pub(super) struct Request {
    kind: u64,
    context: u64,
    wire: Vec<u8>,
    header: protocol::Header,
    handles: usize,
    instance: Option<InstanceRef>,
    outbox: Outbox,
    done: bool,
    control_request: bool,
}
impl Request {
    pub fn accept(message: ReceivedMessage, world: &World) -> Result<Self, ()> {
        let kind = message.header.kind;
        if ![
            protocol::ID,
            protocol::OBSERVATION_ID,
            liboal::protocol::ID,
            libhid::protocol::ID,
        ]
        .contains(&kind)
        {
            return Err(());
        }
        let context = RequestContext::decode(message, kind).map_err(|_| ())?;
        let (header, _) = protocol::Header::decode(&context.payload).map_err(|_| ())?;
        if header.error.is_some() {
            return Err(());
        }
        let mut wire = Vec::new();
        wire.try_reserve_exact(context.payload.len())
            .map_err(|_| ())?;
        wire.extend_from_slice(&context.payload);
        let who = context.envelope.sender_context_id;
        let handles = context.handles.remaining();
        let control_request = kind == protocol::ID
            && world.control.is_manager_context(who)
            && protocol::Command::decode(&wire, handles).is_ok_and(|(_, command)| {
                matches!(command, protocol::Command::QueryActivationResult { .. })
            });
        let outbox = Outbox::prepare(
            context,
            protocol::HEADER_LEN + protocol::BODY_MAX,
            header.deadline,
            2,
            &libexecution::SourceFunding::Task,
        )
        .map_err(|_| ())?;
        Ok(Self {
            kind,
            context: who,
            wire,
            header,
            handles,
            instance: world.context(who),
            outbox,
            done: false,
            control_request,
        })
    }
    pub fn notification_target(&self, task: u64) -> bool {
        task != 0
            && self
                .instance
                .as_ref()
                .is_some_and(|instance| instance.borrow().task == task)
    }
    pub fn control_request(&self) -> bool {
        self.control_request
    }
    pub fn registered(&mut self, kind: u64, id: SourceId) {
        self.outbox.registered(&mut (), kind, id);
    }
    pub fn unregistered(&mut self, kind: u64, id: SourceId) {
        self.outbox.unregistered(&mut (), kind, id);
    }
    pub fn deadline(&self) -> Deadline {
        self.outbox.deadline()
    }
    fn response(&mut self, error: Option<Error>, body: &[u8]) -> Result<(), Error> {
        let response = self.outbox.response_mut()?;
        let used = protocol::Header {
            op: self.header.op,
            error,
            deadline: Deadline::INFINITE,
        }
        .encode(body, response.body_mut()?)?;
        response.finish_body(used)?;
        self.done = true;
        Ok(())
    }
    fn activate(
        &mut self,
        _id: u64,
        world: &mut World,
        requests: &mut Requests<'_, Family>,
    ) -> Result<(), Error> {
        if !world.control.is_manager_context(self.context) {
            return Err(Error::Permission);
        }
        let command = world
            .control
            .command(self.context, &self.wire, self.handles)?;
        if self.instance.is_none() {
            self.instance = match command {
                protocol::Command::Activate { .. } => {
                    Some(world.associate(command.reference(), requests)?)
                }
                protocol::Command::QueryActivationResult { reference } => world
                    .instances
                    .iter()
                    .find(|instance| instance.borrow().slot.reference() == reference)
                    .cloned(),
            };
        }
        let instance = self.instance.as_ref().ok_or(Error::NotFound)?.clone();
        let mut state = instance.borrow_mut();
        match command {
            protocol::Command::Activate { .. } => {
                if state.slot.accept_activation(
                    &world.control,
                    self.context,
                    &self.wire,
                    self.handles,
                    &mut self.outbox,
                )? {
                    if state.task != 0 {
                        requests.wake(state.task)?;
                    }
                    if let Some(task) = state.slot.lifecycle_task() {
                        requests.wake(task)?;
                    }
                }
            }
            protocol::Command::QueryActivationResult { reference } => {
                state.slot.query_result(reference)?
            }
        }
        state.slot.receipt(
            &mut self.outbox,
            self.header.op,
            "keyboard",
            b"hid:boot-keyboard",
        )?;
        self.done = true;
        Ok(())
    }

    fn discover(&mut self, _world: &World) -> Result<(), Error> {
        use liboal::protocol::{self as oal, Response, Status};
        let instance = self.instance.as_ref().ok_or(Error::Permission)?.clone();
        let state = instance.borrow();
        let result = (|| {
            if self.handles != 0 || !state.slot.is_function_context(self.context) {
                return Err(Status::Permission);
            }
            if !state
                .slot
                .activation()
                .is_some_and(|a| a.snapshot().phase == protocol::Phase::Ready)
            {
                return Err(Status::Revoked);
            }
            oal::decode_request(&self.wire).map(|(_, request)| request)
        })();
        let mut info = [0; 32];
        let mut page = [0; 12];
        let response = match result {
            Ok(oal::Request::Lookup { path: "" }) => {
                let used = liboal::info::encode(&[libhid::KEYBOARD], &[], &mut info)
                    .map_err(|_| Error::Invalid)?;
                Response::Found(
                    liboal::ObjectInfo::decode(&info[..used]).map_err(|_| Error::Invalid)?,
                )
            }
            Ok(oal::Request::Enumerate {
                path: "",
                cursor: [],
                ..
            }) => {
                let used =
                    oal::encode_page(&[], true, &[], &mut page).map_err(|_| Error::Invalid)?;
                Response::Page(oal::Page::decode(&page[..used]).map_err(|_| Error::Invalid)?)
            }
            Ok(_) => Response::Error(Status::NotFound),
            Err(status) => Response::Error(status),
        };
        let output = self.outbox.response_mut()?;
        let used = oal::encode_response(
            self.header.op,
            &response,
            self.header.deadline,
            output.body_mut()?,
        )
        .map_err(|_| Error::Invalid)?;
        output.finish_body(used)?;
        self.done = true;
        Ok(())
    }
    fn keyboard(&mut self, world: &mut World) -> Result<(), Error> {
        let (_, command) = libhid::protocol::Command::decode(&self.wire, self.handles)?;
        let instance = self.instance.as_ref().ok_or(Error::Permission)?.clone();
        let mut state = instance.borrow_mut();
        let function = state.slot.is_function_context(self.context);
        if function
            && !state
                .slot
                .activation()
                .is_some_and(|a| a.snapshot().phase == protocol::Phase::Ready)
        {
            return Err(Error::Closed);
        }
        state.keys.request(
            self.context,
            command,
            function,
            &world.control.mailbox,
            &mut self.outbox,
        )?;
        self.done = true;
        Ok(())
    }
    pub fn advance(
        &mut self,
        id: u64,
        world: &mut World,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        let changes_instance = self.kind != protocol::ID || self.header.op == 1;
        while let Some(event) = input.pull() {
            self.outbox.observe(event);
        }
        if input.take_timeout() {
            self.outbox.timed_out();
        }
        if !self.done {
            if self.outbox.result().is_some() {
                if changes_instance && let Some(instance) = &self.instance {
                    let mut state = instance.borrow_mut();
                    if self.kind == protocol::ID {
                        state.slot.reject_uncommitted();
                    }
                    if state.task != 0 {
                        requests.wake(state.task)?;
                    }
                }
                self.done = true;
            } else if !self.outbox.is_admitted() {
                return Ok(self.outbox.admit(requests));
            } else {
                let result = match self.kind {
                    protocol::ID => self.activate(id, world, requests),
                    protocol::OBSERVATION_ID => self
                        .instance
                        .as_ref()
                        .ok_or(Error::Permission)
                        .and_then(|instance| {
                            instance.borrow_mut().slot.observe(
                                &world.control,
                                self.context,
                                &self.wire,
                                self.handles,
                                &mut self.outbox,
                            )
                        })
                        .map(|()| {
                            self.done = true;
                        }),
                    liboal::protocol::ID => self.discover(world),
                    libhid::protocol::ID => self.keyboard(world),
                    _ => Err(Error::Unsupported),
                };
                if changes_instance && let Some(instance) = &self.instance {
                    let mut state = instance.borrow_mut();
                    if result.is_err() && self.kind == protocol::ID {
                        state.slot.reject_uncommitted();
                    }
                    if state.task != 0 {
                        requests.wake(state.task)?;
                    }
                }
                if let Err(e) = result {
                    self.response(Some(e), &[]).unwrap_or_else(|e| fatal(e));
                }
                if !self.done {
                    return Ok(step(Step::Parked));
                }
            }
        }
        let result = self.outbox.drive(requests, 1)?;
        if self.outbox.is_complete() {
            return Ok(step(Step::Complete));
        }
        Ok(result)
    }
}
