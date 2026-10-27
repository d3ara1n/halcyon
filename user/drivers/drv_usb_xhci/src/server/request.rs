//! 有界入站 RPC；统一激活/观察由 libdriver 持有，xHCI 仅解释本领域供应。
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
            device::PROVISION_ID,
            libusb::protocol::ID,
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
        let control_request = (kind == protocol::ID
            && world.control.is_manager_context(who)
            && protocol::Command::decode(&wire, handles).is_ok_and(|(_, command)| {
                matches!(command, protocol::Command::QueryActivationResult { .. })
            }))
            || (kind == device::PROVISION_ID
                && world.host.is_provider_context(who)
                && device::ProvisionCommand::decode(&wire, handles).is_ok_and(|(_, command)| {
                    matches!(command, device::ProvisionCommand::Stop { .. })
                }));
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
            outbox,
            done: false,
            control_request,
        })
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
        if world.slot.is_none() && matches!(command, protocol::Command::Activate { .. }) {
            world.slot = Some(ActivationSlot::new(&world.control, command.reference())?);
        }
        let slot = world.slot.as_mut().ok_or(Error::NotFound)?;
        match command {
            protocol::Command::Activate { .. } => {
                if slot.accept_activation(
                    &world.control,
                    self.context,
                    &self.wire,
                    self.handles,
                    &mut self.outbox,
                )? {
                    requests.wake(world.engine)?;
                    requests.wake(world.lifecycle)?;
                }
            }
            protocol::Command::QueryActivationResult { reference } => {
                slot.query_result(reference)?
            }
        }
        slot.receipt(&mut self.outbox, self.header.op, "usb", &[])?;
        self.done = true;
        Ok(())
    }

    fn discover(&mut self, world: &World) -> Result<(), Error> {
        use liboal::protocol::{self as oal, Response, Status};
        let result = (|| {
            if self.handles != 0
                || !world
                    .slot
                    .as_ref()
                    .is_some_and(|slot| slot.is_function_context(self.context))
            {
                return Err(Status::Permission);
            }
            if !world
                .slot
                .as_ref()
                .and_then(|slot| slot.activation())
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
                let used = liboal::info::encode(&[liboal::DIRECTORY], &[], &mut info)
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
    fn supply(
        &mut self,
        world: &mut World,
        requests: &mut Requests<'_, Family>,
    ) -> Result<(), Error> {
        let (_, command) = device::ProvisionCommand::decode(&self.wire, self.handles)?;
        if let device::ProvisionCommand::Get { epoch, index } = command {
            let resource = world.host.resource(self.context, epoch, index)?;
            let mut body = [0; device::BODY_MAX];
            body[..8].copy_from_slice(&epoch.to_le_bytes());
            let used = libdevice::resource::Entry {
                index,
                name: resource.name,
                descriptor: resource.descriptor,
                metadata: resource.metadata,
            }
            .encode(device::RESOURCE_HEADER_LEN, &mut body[8..])?;
            let rights = resource.descriptor.rights;
            let handle = object::duplicate(resource.capability.as_handle(), rights)?;
            // SAFETY: duplicate的新entry独占关闭责任移入Outbox运输。
            self.outbox
                .response_mut()?
                .push(unsafe { Capability::from_raw(handle) }, rights)
                .map_err(|f| f.error)?;
            return self.response(None, &body[..8 + used]);
        }
        let open = world
            .slot
            .as_ref()
            .and_then(|slot| slot.activation())
            .is_some_and(|a| a.snapshot().phase == protocol::Phase::Ready);
        let signal = if matches!(command, device::ProvisionCommand::Prepare(_)) {
            Some(self.outbox.response_mut()?.parts()?.0.handles.take(1)?)
        } else {
            None
        };
        let progress = world.host.command(
            self.context,
            command,
            signal,
            self.header.deadline,
            &world.control.mailbox,
            open,
        )?;
        requests.wake(world.engine)?;
        let mut bytes = [0; device::HEADER_LEN + 16];
        let used = progress.encode(command, &mut bytes)?;
        self.response(None, &bytes[device::HEADER_LEN..used])
    }
    fn usb(&mut self, world: &mut World, requests: &mut Requests<'_, Family>) -> Result<(), Error> {
        let (header, command) = libusb::protocol::Command::decode(&self.wire, self.handles)?;
        let open = world
            .slot
            .as_ref()
            .and_then(|slot| slot.activation())
            .is_some_and(|a| a.snapshot().phase == protocol::Phase::Ready);
        let completion = if command.handles() != 0 {
            Some(self.outbox.response_mut()?.parts()?.0.handles.take(1)?)
        } else {
            None
        };
        let progress =
            world
                .host
                .transfer(self.context, command, completion, header.deadline, open)?;
        if !matches!(command, libusb::protocol::Command::Query { .. }) {
            requests.wake(world.engine)?;
        }
        let mut bytes = [0; libusb::protocol::Progress::BYTES];
        let used = progress.encode(command.sequence(), &mut bytes)?;
        self.response(None, &bytes[..used])
    }
    pub fn advance(
        &mut self,
        id: u64,
        world: &mut World,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            self.outbox.observe(event);
        }
        if input.take_timeout() {
            self.outbox.timed_out();
        }
        if !self.done {
            if self.outbox.result().is_some() {
                self.done = true;
            } else if !self.outbox.is_admitted() {
                return Ok(self.outbox.admit(requests));
            } else {
                let result = match self.kind {
                    protocol::ID => self.activate(id, world, requests),
                    protocol::OBSERVATION_ID => {
                        let result = world.slot.as_mut().ok_or(Error::NotReady).and_then(|slot| {
                            slot.observe(
                                &world.control,
                                self.context,
                                &self.wire,
                                self.handles,
                                &mut self.outbox,
                            )
                        });
                        requests.wake(world.engine)?;
                        result.map(|()| {
                            self.done = true;
                        })
                    }
                    liboal::protocol::ID => self.discover(world),
                    device::PROVISION_ID => self.supply(world, requests),
                    libusb::protocol::ID => self.usb(world, requests),
                    _ => Err(Error::Unsupported),
                };
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
