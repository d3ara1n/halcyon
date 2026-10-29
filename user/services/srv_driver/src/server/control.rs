//! 独立Control/Observer收件入口；原请求与Outbox承担所有回复责任。
use super::*;
use libexecution::{
    SourceFunding,
    runtime::{SourceEvent, SourcePlan},
};
use libkernel::ipc::message::{MessageStorage, ReceiveBuffer, ReceivedMessage};
use librpc::{exchange::RequestContext, outbox::Outbox};

/// 原Request的一次只读RPC与待核记录；不持另一份选择/占用账本。
struct Retry {
    txid: u64,
    run: Rc<RefCell<Run>>,
    reference: BindingRef,
    implementation: usize,
}

pub(super) struct Request {
    outbox: Outbox,
    header: protocol::Header,
    authorized: bool,
    control: bool,
    oal: bool,
    wire: Vec<u8>,
    started: bool,
    stopping: bool,
    retry: Option<Retry>,
}
impl Request {
    fn accept(message: ReceivedMessage, world: &World<'_>) -> Result<Self, ()> {
        let oal = message.header.kind == liboal::protocol::ID;
        let mut context = RequestContext::decode(
            message,
            if oal {
                liboal::protocol::ID
            } else {
                protocol::MANAGER_ID
            },
        )
        .map_err(|_| ())?;
        let header = if oal {
            let (header, _) = liboal::protocol::decode_request(&context.payload).map_err(|_| ())?;
            protocol::Header {
                op: header.op,
                error: None,
                deadline: header.deadline,
            }
        } else {
            let (header, body) = protocol::Header::decode(&context.payload).map_err(|_| ())?;
            let body_valid = if header.op == protocol::MANAGER_RETRY {
                let mut reader = librpc::bytes::Reader::new(body);
                DeviceId::decode(&mut reader).is_ok() && reader.finish().is_ok()
            } else {
                body.is_empty()
            };
            if header.error.is_some() || !body_valid {
                return Err(());
            }
            header
        };
        if context.handles.remaining() != 0 {
            return Err(());
        }
        let control = context.envelope.sender_context_id == world.control_context;
        let authorized =
            control || (oal && context.envelope.sender_context_id == world.observer_context);
        let reply_deadline = if control {
            header.deadline
        } else {
            let end = deadline().map_err(|_| ())?.at_ns;
            Deadline::at(
                header
                    .deadline
                    .instant()
                    .map_err(|_| ())?
                    .map_or(end, |at| at.min(end)),
            )
        };
        let wire = core::mem::take(&mut context.payload);
        let outbox = Outbox::prepare(
            context,
            if oal {
                erhino_shared::message::PAYLOAD_MAX - librpc::PREFIX_LEN
            } else {
                protocol::HEADER_LEN + 16
            },
            reply_deadline,
            2,
            &SourceFunding::Task,
        )
        .map_err(|_| ())?;
        Ok(Self {
            outbox,
            header,
            authorized,
            control,
            oal,
            wire,
            started: false,
            stopping: false,
            retry: None,
        })
    }
    fn manager_response(
        &mut self,
        world: &World<'_>,
        error: Option<Error>,
    ) -> Result<(), SystemCallError> {
        let mut body = [0; 16];
        body[..4].copy_from_slice(&u32::from(world.stopping).to_le_bytes());
        body[8..16].copy_from_slice(&(world.runs.len() as u64).to_le_bytes());
        let response = self.outbox.response_mut()?;
        let used = protocol::Header {
            op: self.header.op,
            error,
            deadline: Deadline::INFINITE,
        }
        .encode(
            if error.is_none() && self.header.op != protocol::MANAGER_RETRY {
                &body
            } else {
                &[]
            },
            response.body_mut()?,
        )
        .unwrap_or_else(|error| fatal(error));
        response.finish_body(used)
    }
    fn begin_retry(&mut self, id: u64, world: &mut World<'_>) -> Result<(), Error> {
        if world.stopping {
            return Err(Error::Closed);
        }
        let (_, body) = protocol::Header::decode(&self.wire)?;
        let mut reader = librpc::bytes::Reader::new(body);
        let device = DeviceId::decode(&mut reader)?;
        reader.finish()?;
        let run = world
            .runs
            .get(device.serial())
            .ok_or(Error::NotFound)?
            .clone();
        let (reference, implementation) = {
            let record = run.borrow();
            if record.device != device {
                return Err(Error::StaleInstance);
            }
            if !record.settled() {
                return Err(Error::Busy);
            }
            (record.reference, record.implementation)
        };
        let deadline =
            if let Some(at) = self.header.deadline.instant().map_err(|_| Error::Invalid)? {
                Deadline::at(at.min(super::deadline()?.at_ns))
            } else {
                super::deadline()?
            };
        let mut wire = [0; libdevice::protocol::HEADER_LEN + DeviceId::ENCODED_LEN];
        let used =
            libdevice::protocol::Command::QueryDevice { id: device }.encode(deadline, &mut wire)?;
        let request = librpc::Request::new(libdevice::protocol::ID, &wire[..used])?;
        let txid = match world.call(id, world.claim.clone(), request, deadline) {
            Ok(txid) => txid,
            Err(_) => {
                return Err(Error::Unavailable);
            }
        };
        self.retry = Some(Retry {
            txid,
            run,
            reference,
            implementation,
        });
        Ok(())
    }
    fn commit_retry(
        retry: &Retry,
        reply: &librpc::Reply,
        world: &mut World<'_>,
    ) -> Result<(), Error> {
        if world.stopping {
            return Err(Error::Closed);
        }
        let device = retry.reference.device();
        let info =
            libdevice::protocol::decode_device(&reply.payload, device, reply.handles.remaining())?;
        if !info.present || info.epoch.is_none() {
            return Err(Error::Closed);
        }
        if info.binding.is_some() {
            return Err(Error::Busy);
        }
        let current = world.runs.get(device.serial()).ok_or(Error::NotFound)?;
        if !Rc::ptr_eq(current, &retry.run) {
            return Err(Error::Conflict);
        }
        {
            let record = current.borrow();
            if !record.settled() {
                return Err(Error::Busy);
            }
            if record.reference != retry.reference || record.implementation != retry.implementation
            {
                return Err(Error::Conflict);
            }
        }
        // Query只是取样；成功只重置本地纯选择。远端Seal即使提示尚未到达，
        // 后续Claim仍在srv_device提交点核来源/设备门，不能由此授予新使用。
        world.runs.remove(device.serial());
        world.discovery_dirty = true;
        libkernel::debug!(
            "driver terminal selection reset committed: device={}, epoch={}, implementation={}",
            device.serial(),
            retry.reference.epoch(),
            retry.implementation
        );
        Ok(())
    }
    fn observe(&mut self, event: SourceEvent) {
        self.outbox.observe(event);
    }
    pub fn registered(&mut self, kind: u64, source: SourceId) {
        self.outbox.registered(&mut (), kind, source);
    }
    pub fn unregistered(&mut self, kind: u64, source: SourceId) {
        self.outbox.unregistered(&mut (), kind, source);
    }
    pub fn deadline(&self) -> Deadline {
        if self.retry.is_some() && self.outbox.result().is_some() {
            // 原Dispatcher拥有RPC期限及注销，Outbox退出后不反复调度过期任务期限。
            Deadline::INFINITE
        } else {
            self.outbox.deadline()
        }
    }
    pub fn stop(&mut self) {
        self.stopping = true;
        self.outbox.stop(&mut ());
    }
    pub fn advance(
        &mut self,
        id: u64,
        world: &mut World<'_>,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            self.observe(event);
        }
        if input.take_timeout() {
            self.outbox.timed_out();
        }
        if self.stopping {
            self.outbox.stop(&mut ());
        }
        if let Some(retry) = &self.retry {
            if self.outbox.result().is_some() && world.rpc.cancel(retry.txid) {
                world.rpc_dirty = true;
                requests.wake(world.transport_task)?;
            }
            if let Some(completion) = world.rpc.take(retry.txid) {
                let retry = self.retry.take().expect("Retry RPC owner missing");
                let result = match completion.result {
                    Ok(reply) => {
                        if self.outbox.result().is_none() {
                            Self::commit_retry(&retry, &reply, world)
                        } else {
                            Ok(())
                        }
                    }
                    Err(_) => Err(Error::Unavailable),
                };
                if self.outbox.result().is_none() {
                    if result.is_ok() {
                        requests.wake(world.manager_task)?;
                    }
                    self.manager_response(world, result.err())?;
                }
            } else if self.outbox.result().is_none() {
                return Ok(advance(Step::Parked));
            }
        }
        if !self.started {
            if self.outbox.result().is_some() {
                self.started = true;
            } else if !self.outbox.is_admitted() {
                return Ok(self.outbox.admit(requests));
            } else {
                self.started = true;
                if self.oal {
                    let response = self.outbox.response_mut()?;
                    let result = if self.authorized {
                        world.project(&self.wire, response.body_mut()?)
                    } else {
                        Err(liboal::protocol::Status::Permission)
                    };
                    let used = match result {
                        Ok(used) => used,
                        Err(status) => liboal::protocol::encode_response(
                            self.header.op,
                            &liboal::protocol::Response::Error(status),
                            self.header.deadline,
                            response.body_mut()?,
                        )
                        .map_err(|_| SystemCallError::IllegalArgument)?,
                    };
                    response.finish_body(used)?;
                } else {
                    let error = if !self.authorized {
                        Some(Error::Permission)
                    } else if self.header.op == 2 {
                        None
                    } else if self.header.op == protocol::MANAGER_RETRY {
                        match self.begin_retry(id, world) {
                            Ok(()) => return Ok(advance(Step::Parked)),
                            Err(error) => Some(error),
                        }
                    } else {
                        Some(Error::Unsupported)
                    };
                    self.manager_response(world, error)?;
                }
            }
        }
        let result = self.outbox.drive(requests, 1)?;
        if self.outbox.is_complete() && self.retry.is_none() {
            world.outstanding_requests = world
                .outstanding_requests
                .checked_sub(1)
                .expect("driver control request accounting missing");
            requests.wake(world.manager_task)?;
            return Ok(advance(Step::Complete));
        }
        if self.retry.is_some() && self.outbox.is_complete() {
            return Ok(advance(Step::Parked));
        }
        Ok(result)
    }
}

pub(super) struct Ingress {
    mailbox: erhino_shared::object::Handle,
    public: bool,
    source: SourceRegistration,
    buffer: ReceiveBuffer,
    storage: Option<MessageStorage>,
    stopping: bool,
}
impl Ingress {
    pub fn new(
        mailbox: erhino_shared::object::Handle,
        public: bool,
    ) -> Result<Self, SystemCallError> {
        Ok(Self {
            mailbox,
            public,
            source: SourceRegistration::new(),
            buffer: ReceiveBuffer::new()?,
            storage: Some(MessageStorage::new()?),
            stopping: false,
        })
    }
    pub fn registered(&mut self, kind: u64, source: SourceId) {
        if kind == 1 {
            self.source.registered(kind, source);
        }
    }
    pub fn unregistered(&mut self, _: u64, source: SourceId) {
        self.source.unregistered(source);
    }
    pub fn stop(&mut self) {
        self.stopping = true;
    }
    pub fn deadline(&self) -> Deadline {
        Deadline::INFINITE
    }
    pub fn advance(
        &mut self,
        world: &mut World<'_>,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        let mut readable = false;
        while let Some(event) = input.pull() {
            if event.kind == 1 {
                readable = true;
            }
        }
        input.take_timeout();
        if self.public && world.stopping {
            self.stopping = true;
        }
        if self.stopping {
            self.source.remove(requests)?;
            if self.public
                && self.source.is_empty()
                && let Some(mailbox) = world.observer_mailbox.take()
                && let Err((mailbox, error)) = mailbox.close()
            {
                world.observer_mailbox = Some(mailbox);
                return Err(error);
            }
            return Ok(advance(if self.source.is_empty() {
                Step::Complete
            } else {
                Step::Runnable
            }));
        }
        if self.source.is_empty() {
            self.source.install(
                requests,
                SourcePlan::new(
                    self.mailbox,
                    ObjectSignals::READABLE | ObjectSignals::CLOSED,
                ),
                1,
            )?;
            return Ok(advance(Step::Runnable));
        }
        if !readable {
            return Ok(advance(Step::Parked));
        }
        if self.storage.is_none() {
            self.storage = Some(MessageStorage::new()?);
        }
        match self.buffer.receive(self.mailbox) {
            Ok(()) => {}
            Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => {
                return Ok(advance(Step::Parked));
            }
            Err(error) => return Err(error),
        }
        let message = self
            .storage
            .take()
            .expect("driver ingress storage missing")
            .take(&mut self.buffer)?;
        if let Ok(request) = Request::accept(message, world) {
            let limit = if request.control {
                REQUESTS
            } else {
                REQUESTS - CONTROL_RESERVE
            };
            if world.outstanding_requests >= limit || (world.stopping && !request.control) {
                return Ok(advance(Step::Runnable));
            }
            let request = Box::try_new(request).map_err(|_| SystemCallError::OutOfMemory)?;
            world.outstanding_requests += 1;
            if let Err(submission) = requests.spawn(TaskSubmission::new(
                Family::Control(request),
                1,
                world.funding.clone(),
            )) {
                drop(submission);
                world.outstanding_requests -= 1;
            }
        }
        Ok(advance(Step::Runnable))
    }
}
