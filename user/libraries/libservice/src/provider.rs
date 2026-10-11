//! 独立 registry provider 的实际实例和 Runtime 请求责任；不建立 OAL 投影树。
use crate::{
    authority::{AccessTable, AuthorityTable, DirectoryAccess, NameAuthority, PreparedRole},
    protocol::{
        self, AccessRights, Header, PublishRights, Reason, Request as Command, Response, Status,
    },
    registry::{PreparedRegistration, Registry},
    resource::ServiceResource,
    watch::PreparedWatch,
};
use erhino_shared::{
    call::SystemCallError,
    message::PAYLOAD_MAX,
    object::{ObjectSignals, Rights},
    time::Deadline,
};
use libbudget::{AccountView, Charge};
use libexecution::{
    SourceRegistration,
    runtime::{Advance, Input, RequestFailure, Requests, SourceId, SourcePlan, Step},
};
use libkernel::ipc::{
    capability::Capability,
    message::{Mailbox, MailboxSender, MintedSender, ReceivedMessage},
};
use librpc::{
    exchange::{RejectedRequest, RequestContext, ResponseFailure},
    outbox::Outbox,
};
pub const REPLY: u64 = 1;
pub const CONTROL: u64 = 2;
pub const ENDPOINT: u64 = 3;
pub struct State {
    pub mailbox: Mailbox,
    pub registry: Registry,
    pub accesses: AccessTable,
    pub authorities: AuthorityTable,
    pub account: AccountView<ServiceResource>,
    pub execution: AccountView<libexecution::ExecutionResource>,
    pub maintenance: u64,
    pub dirty: bool,
}
impl State {
    pub fn changed<F>(&mut self, requests: &mut Requests<'_, F>) {
        self.dirty = true;
        if self.maintenance != 0 {
            let _ = requests.wake(self.maintenance);
        }
    }
    pub fn seal(&mut self) {
        self.accesses.seal();
        self.authorities.seal();
        self.registry.seal();
        self.dirty = true;
    }
}
pub enum Rejected {
    Unknown(ReceivedMessage),
    Rpc(RejectedRequest),
    Header(RequestContext),
    Resource(ResponseFailure),
}
#[derive(Clone, Copy)]
enum Owner {
    Authority(u64),
    Registration(u64),
    Watch(u64),
}
enum Pending {
    Authority(PreparedRole<PublishRights>),
    Registration(PreparedRegistration),
    Watch(PreparedWatch),
}
pub struct Request {
    funding: crate::Funding,
    outbox: Option<Outbox>,
    header: Header,
    oal: bool,
    access: Option<DirectoryAccess>,
    authority: Option<NameAuthority>,
    pending: Option<Pending>,
    sender: Option<MailboxSender>,
    lifetime: Option<libkernel::ipc::message::LifetimeObserver>,
    waiter: Option<libkernel::ipc::notification::NotificationObserver>,
    control_source: SourceRegistration,
    endpoint_source: SourceRegistration,
    owner: Option<Owner>,
    prepared: bool,
    encoded: bool,
    abort: Option<Status>,
    control_closed: bool,
    endpoint_closed: bool,
    stopping: bool,
    fees: Option<crate::funding::RequestFees>,
    _task_bytes: Charge,
}
fn system(e: SystemCallError) -> Status {
    match e {
        SystemCallError::QuotaExceeded => Status::Quota,
        SystemCallError::RightsDenied => Status::Permission,
        SystemCallError::ObjectClosed => Status::Unavailable,
        _ => Status::Resource,
    }
}
fn step(step: Step) -> Advance {
    Advance { work_done: 1, step }
}
impl Request {
    #[expect(
        clippy::result_large_err,
        reason = "拒收返还真实原消息/回复/Delivery owner"
    )]
    pub fn accept(message: ReceivedMessage, state: &State) -> Result<Self, Rejected> {
        let oal = message.header.kind == liboal::protocol::ID;
        if !oal && message.header.kind != protocol::ID {
            return Err(Rejected::Unknown(message));
        }
        let context = RequestContext::decode(
            message,
            if oal {
                liboal::protocol::ID
            } else {
                protocol::ID
            },
        )
        .map_err(Rejected::Rpc)?;
        let header = if oal {
            liboal::protocol::Header::decode(&context.payload)
                .map(|(h, _)| Header {
                    version: h.version,
                    op: h.op,
                    status: h.status,
                    body_len: h.body_len,
                    deadline: h.deadline,
                })
                .ok()
        } else {
            Header::decode(&context.payload).map(|(h, _)| h).ok()
        };
        let Some(header) = header.filter(|h| h.status == 0) else {
            return Err(Rejected::Header(context));
        };
        let id = context.envelope.sender_context_id;
        let access = state.accesses.binding(id);
        let authority = state.authorities.binding(id);
        let funding = access
            .as_ref()
            .map(DirectoryAccess::funding)
            .or_else(|| authority.as_ref().map(NameAuthority::funding))
            .or_else(|| state.registry.control_funding(id))
            .or_else(|| state.registry.watches.control_funding(id));
        let funding = match funding {
            Some(funding) => funding.clone(),
            None => match crate::Funding::new(&state.account, &state.execution) {
                Ok(funding) => funding,
                Err(error) => return Err(Rejected::Resource(ResponseFailure { error, context })),
            },
        };
        let (fees, task_bytes) = match funding.prepare_request::<Self>() {
            Ok(v) => v,
            Err(error) => return Err(Rejected::Resource(ResponseFailure { error, context })),
        };
        let outbox = Outbox::prepare(
            context,
            PAYLOAD_MAX - librpc::PREFIX_LEN,
            header.deadline,
            REPLY,
            &libexecution::SourceFunding::Task,
        )
        .map_err(Rejected::Resource)?;
        Ok(Self {
            funding,
            outbox: Some(outbox),
            header,
            oal,
            access,
            authority,
            pending: None,
            sender: None,
            lifetime: None,
            waiter: None,
            control_source: SourceRegistration::new(),
            endpoint_source: SourceRegistration::new(),
            owner: None,
            prepared: false,
            encoded: false,
            abort: None,
            control_closed: false,
            endpoint_closed: false,
            stopping: false,
            fees: Some(fees),
            _task_bytes: task_bytes,
        })
    }
    pub fn execution_funding(&self) -> &AccountView<libexecution::ExecutionResource> {
        self.funding.execution()
    }
    pub fn deadline(&self) -> Deadline {
        self.outbox
            .as_ref()
            .map_or(Deadline::INFINITE, Outbox::deadline)
    }
    pub fn observe(&mut self, event: libexecution::runtime::SourceEvent) {
        if let Some(outbox) = self.outbox.as_mut() {
            outbox.observe(event);
        }
        if event.observed.intersects(ObjectSignals::CLOSED) {
            match event.kind {
                CONTROL => self.control_closed = true,
                ENDPOINT => self.endpoint_closed = true,
                _ => {}
            }
        }
    }
    pub fn timed_out(&mut self) {
        if let Some(outbox) = self.outbox.as_mut() {
            outbox.timed_out();
        }
    }
    pub fn registered(&mut self, kind: u64, source: SourceId) {
        match kind {
            CONTROL => self.control_source.registered(kind, source),
            ENDPOINT => self.endpoint_source.registered(kind, source),
            _ => {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.registered(&mut (), kind, source);
                }
            }
        }
    }
    pub fn unregistered(&mut self, state: &mut State, kind: u64, source: SourceId) {
        match kind {
            CONTROL => {
                if self.control_source.unregistered(source)
                    && let Some(Owner::Registration(context)) = self.owner
                {
                    state.registry.control_source_removed(context);
                }
            }
            ENDPOINT => {
                if self.endpoint_source.unregistered(source)
                    && let Some(Owner::Registration(context)) = self.owner
                {
                    state.registry.endpoint_source_removed(context);
                }
            }
            _ => {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.unregistered(&mut (), kind, source);
                }
            }
        }
    }
    pub fn refused<F>(&mut self, failure: RequestFailure<F>) {
        match failure {
            RequestFailure::Source { kind, error } if matches!(kind, CONTROL | ENDPOINT) => {
                if kind == CONTROL {
                    self.control_source.refused(kind);
                } else {
                    self.endpoint_source.refused(kind);
                }
                self.abort = Some(system(error));
            }
            failure => {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.refused(&mut (), failure);
                }
            }
        }
    }
    pub fn stop(&mut self) {
        self.stopping = true;
        if let Some(outbox) = self.outbox.as_mut() {
            outbox.stop(&mut ());
        }
    }
    pub fn reject(&mut self, status: Status) {
        self.abort = Some(status);
        self.prepared = true;
    }
    fn endpoint_handle(&self, state: &State) -> Option<erhino_shared::object::Handle> {
        match self.pending.as_ref() {
            Some(Pending::Registration(record)) => Some(record.endpoint_handle()),
            _ => match self.owner {
                Some(Owner::Registration(context)) => state.registry.endpoint_handle(context),
                _ => None,
            },
        }
    }
    pub fn advance<F>(
        &mut self,
        task: u64,
        state: &mut State,
        requests: &mut Requests<'_, F>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if budget == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        while let Some(event) = input.pull() {
            if let Some(outbox) = self.outbox.as_mut() {
                outbox.observe(event);
            }
            if event.observed.intersects(ObjectSignals::CLOSED) {
                match event.kind {
                    CONTROL => self.control_closed = true,
                    ENDPOINT => self.endpoint_closed = true,
                    _ => {}
                }
            }
        }
        if input.take_timeout()
            && let Some(outbox) = self.outbox.as_mut()
        {
            outbox.timed_out();
        }
        if self.stopping {
            self.control_closed = true;
            self.endpoint_closed = true;
        }
        if self.owner.is_none() && self.outbox.as_ref().is_some_and(|o| o.result().is_some()) {
            self.abort.get_or_insert(Status::Cancelled);
        }
        if !self.prepared && self.abort.is_none() {
            let outbox = self.outbox.as_mut().expect("request reply owner missing");
            if !outbox.is_admitted() {
                return Ok(outbox.admit(requests));
            }
            let result = {
                let (response_context, _) = outbox.response_mut()?.parts()?;
                prepare(
                    state,
                    self.header,
                    self.oal,
                    self.access.as_ref(),
                    self.authority.as_ref(),
                    response_context,
                )
            };
            self.prepared = true;
            match result {
                Ok(Some((pending, minted, waiter))) => {
                    self.pending = Some(pending);
                    self.sender = Some(minted.sender);
                    self.lifetime = Some(minted.lifetime);
                    self.waiter = waiter;
                }
                Ok(None) => {}
                Err(status) => self.abort = Some(status),
            }
        }
        if self.owner.is_none() && self.abort.is_none() && self.pending.is_some() {
            if self.control_closed || self.endpoint_closed {
                self.abort = Some(Status::Unavailable);
            } else {
                if self.control_source.is_empty() {
                    let handle = self
                        .lifetime
                        .as_ref()
                        .expect("prepared control Lifetime missing")
                        .as_handle();
                    if self
                        .control_source
                        .install(
                            requests,
                            SourcePlan::new(handle, ObjectSignals::CLOSED),
                            CONTROL,
                        )
                        .is_err()
                    {
                        return Ok(step(Step::Runnable));
                    }
                }
                if let Some(handle) = self.endpoint_handle(state)
                    && self.endpoint_source.is_empty()
                    && self
                        .endpoint_source
                        .install(
                            requests,
                            SourcePlan::new(handle, ObjectSignals::CLOSED),
                            ENDPOINT,
                        )
                        .is_err()
                {
                    return Ok(step(Step::Runnable));
                }
                if self.control_source.source().is_none()
                    || (matches!(self.pending, Some(Pending::Registration(_)))
                        && self.endpoint_source.source().is_none())
                {
                    return Ok(step(Step::Runnable));
                }
                let expired = libkernel::time::expired(self.header.deadline)?;
                let closed = libkernel::ipc::wait::probe(
                    self.lifetime
                        .as_ref()
                        .expect("prepared control Lifetime missing")
                        .as_handle(),
                    ObjectSignals::CLOSED,
                )?
                .intersects(ObjectSignals::CLOSED)
                    || self
                        .endpoint_handle(state)
                        .map(|handle| libkernel::ipc::wait::probe(handle, ObjectSignals::CLOSED))
                        .transpose()?
                        .is_some_and(|signals| signals.intersects(ObjectSignals::CLOSED));
                if expired || closed {
                    self.abort = Some(if expired {
                        Status::Expired
                    } else {
                        Status::Unavailable
                    });
                } else {
                    self.commit(task, state, requests)?;
                }
            }
        }
        if !self.encoded && self.pending.is_none() && self.owner.is_none() {
            self.encode_simple(state)?;
            state.changed(requests);
        }
        if let Some(owner) = self.owner {
            match owner {
                Owner::Authority(context) => {
                    if self.control_closed {
                        if self.stopping {
                            state.authorities.revoke(context);
                        }
                        state.changed(requests);
                    }
                }
                Owner::Registration(context) => {
                    if self.stopping {
                        let _ = state.registry.withdraw(context, Reason::ProviderStopping);
                    }
                    if self.control_closed {
                        let _ = state.registry.mark_control_closed(context);
                        state.changed(requests);
                    } else if self.endpoint_closed {
                        let _ = state.registry.withdraw(context, Reason::EndpointClosed);
                        state.changed(requests);
                    }
                }
                Owner::Watch(context) => {
                    if self.stopping {
                        let _ = state.registry.watches.cancel(context);
                    }
                }
            }
            let removing_endpoint = matches!(owner,Owner::Registration(context)if state.registry.needs_endpoint_remove(context));
            if removing_endpoint && !self.endpoint_source.is_empty() {
                let _ = self.endpoint_source.remove(requests);
                return Ok(step(Step::Runnable));
            }
            if let Owner::Registration(context) = owner {
                state.registry.retire_one(context);
                if state
                    .registry
                    .query(context)
                    .is_ok_and(|info| info.state == protocol::State::Withdrawn)
                    && state.registry.endpoint_handle(context).is_some()
                {
                    return Ok(step(Step::Runnable));
                }
            }
            if self.control_closed && !self.control_source.is_empty() {
                let _ = self.control_source.remove(requests);
                return Ok(step(Step::Runnable));
            }
            if self.control_closed
                && self.control_source.is_empty()
                && self.endpoint_source.is_empty()
            {
                if let Some(lifetime) = self.lifetime.take()
                    && let Err((lifetime, _)) = lifetime.close()
                {
                    self.lifetime = Some(lifetime);
                    return Ok(step(Step::Runnable));
                }
                match owner {
                    Owner::Authority(context) => {
                        state.authorities.remove(context);
                    }
                    Owner::Registration(context) => {
                        if !state.registry.retire_one(context) {
                            return Ok(step(Step::Runnable));
                        }
                    }
                    Owner::Watch(context) => {
                        state.registry.watches.remove(context);
                    }
                }
                self.owner = None;
            }
        }
        if self.abort.is_some() && self.owner.is_none() {
            if !self.control_source.is_empty() {
                let _ = self.control_source.remove(requests);
                return Ok(step(Step::Runnable));
            }
            if !self.endpoint_source.is_empty() {
                let _ = self.endpoint_source.remove(requests);
                return Ok(step(Step::Runnable));
            }
            self.pending = None;
            self.sender = None;
            self.waiter = None;
            if let Some(lifetime) = self.lifetime.take()
                && let Err((lifetime, _)) = lifetime.close()
            {
                self.lifetime = Some(lifetime);
                return Ok(step(Step::Runnable));
            }
            if !self.encoded && self.outbox.as_ref().is_some_and(|o| o.result().is_none()) {
                self.encode_simple(state)?;
            }
        }
        if let Some(outbox) = self.outbox.as_mut() {
            let advance = outbox.drive(requests, budget)?;
            if !outbox.is_complete() {
                return Ok(advance);
            }
            self.outbox = None;
            self.fees = None;
            self.access = None;
            self.authority = None;
        }
        Ok(step(if self.owner.is_some() {
            Step::Parked
        } else {
            Step::Complete
        }))
    }
    fn commit<F>(
        &mut self,
        task: u64,
        state: &mut State,
        requests: &mut Requests<'_, F>,
    ) -> Result<(), SystemCallError> {
        let outbox = self
            .outbox
            .as_mut()
            .expect("prepared request reply missing");
        let response = outbox.response_mut()?;
        let (_, out) = response.parts()?;
        let pending = self
            .pending
            .take()
            .expect("prepared business owner missing");
        let context = self
            .sender
            .as_ref()
            .expect("prepared sender missing")
            .description()?
            .object_id;
        let (owner, used) = match pending {
            Pending::Authority(role) => {
                if !self.authority.as_ref().is_some_and(|a| a.active()) {
                    self.pending = Some(Pending::Authority(role));
                    self.abort = Some(Status::Revoked);
                    return Ok(());
                }
                let used = protocol::encode_response(
                    self.header.op,
                    &Response::Empty,
                    self.header.deadline,
                    out,
                )
                .map_err(|_| SystemCallError::IllegalArgument)?;
                if let Err(f) = state.authorities.install(role, context) {
                    self.pending = Some(Pending::Authority(f.prepared));
                    self.abort = Some(f.status);
                    return Ok(());
                }
                (Owner::Authority(context), used)
            }
            Pending::Registration(mut record) => {
                record.set_task(task);
                let used = protocol::encode_response(
                    self.header.op,
                    &Response::Registration(record.info()),
                    self.header.deadline,
                    out,
                )
                .map_err(|_| SystemCallError::IllegalArgument)?;
                match state.registry.commit(record) {
                    Ok(_) => (Owner::Registration(context), used),
                    Err(f) => {
                        self.pending = Some(Pending::Registration(f.prepared));
                        self.abort = Some(f.status);
                        return Ok(());
                    }
                }
            }
            Pending::Watch(watch) => {
                let used = protocol::encode_response(
                    self.header.op,
                    &Response::Watch(watch.info()),
                    self.header.deadline,
                    out,
                )
                .map_err(|_| SystemCallError::IllegalArgument)?;
                match state.registry.watches.install(watch) {
                    Ok(_) => (Owner::Watch(context), used),
                    Err(watch) => {
                        self.pending = Some(Pending::Watch(watch));
                        self.abort = Some(Status::Revoked);
                        return Ok(());
                    }
                }
            }
        };
        self.owner = Some(owner);
        let sender = self.sender.take().expect("committed reply sender missing");
        let rights = sender.description()?.rights;
        response
            .push(sender.into_capability(), rights)
            .map_err(|f| f.error)?;
        if let Some(waiter) = self.waiter.take() {
            let rights = Rights::WAIT
                | Rights::TRANSIT
                | (self
                    .access
                    .as_ref()
                    .expect("committed watch access missing")
                    .output()
                    & Rights::DUPLICATE);
            response
                .push(waiter.into_capability(), rights)
                .map_err(|f| f.error)?;
        }
        response.finish_body(used)?;
        self.encoded = true;
        state.changed(requests);
        Ok(())
    }
    fn encode_simple(&mut self, state: &mut State) -> Result<(), SystemCallError> {
        let outbox = self.outbox.as_mut().expect("request reply missing");
        let response = outbox.response_mut()?;
        let (context, out) = response.parts()?;
        let (used, export) = if self.oal {
            (
                serve_oal(
                    state,
                    self.header,
                    self.access.as_ref(),
                    context,
                    out,
                    self.abort,
                )
                .map_err(|_| SystemCallError::IllegalArgument)?,
                None,
            )
        } else {
            serve_service(
                state,
                self.header,
                self.access.as_ref(),
                context,
                out,
                self.abort,
            )
            .map_err(|_| SystemCallError::IllegalArgument)?
        };
        if let Some((cap, rights)) = export {
            response.push(cap, rights).map_err(|f| f.error)?;
        }
        response.finish_body(used)?;
        self.encoded = true;
        Ok(())
    }
}
fn slots(context: &RequestContext) -> u64 {
    // Outbox 准备时已验证并接管必需回复槽 0；剩余 HandleSet 只持业务槽。
    // 领域帧的槽位合同仍包含原回复槽，不能据已取走的 owner 判断请求缺槽。
    let mut mask = 1;
    for index in 0..context.handles.len() {
        if context.handles.get(index).is_ok() {
            mask |= 1 << index;
        }
    }
    mask
}
fn prepare(
    state: &mut State,
    header: Header,
    oal: bool,
    access: Option<&DirectoryAccess>,
    authority: Option<&NameAuthority>,
    context: &mut RequestContext,
) -> Result<
    Option<(
        Pending,
        MintedSender,
        Option<libkernel::ipc::notification::NotificationObserver>,
    )>,
    Status,
> {
    if oal {
        return Ok(None);
    }
    let (_, request) = protocol::decode_request(&context.payload, slots(context))?;
    match request {
        Command::DelegateName {
            name,
            publish_rights,
        } => {
            let authority = authority.ok_or(Status::Permission)?;
            let transport = authority.control_transport()?;
            let role = state
                .authorities
                .prepare_exact(authority, name, publish_rights)?;
            let minted = state.mailbox.mint(0, transport).map_err(system)?;
            Ok(Some((Pending::Authority(role), minted, None)))
        }
        Command::Register { .. } => {
            let authority = authority.ok_or(Status::Permission)?;
            let transport = authority.control_transport()?;
            let minted = state.mailbox.mint(0, transport).map_err(system)?;
            let endpoint = context.handles.take(1).map_err(system)?;
            let (endpoint, _) =
                MailboxSender::from_capability(endpoint).map_err(|f| system(f.error))?;
            let record = state.registry.prepare_register(
                authority,
                minted.sender.description().map_err(system)?.object_id,
                &request,
                endpoint,
            )?;
            Ok(Some((Pending::Registration(record), minted, None)))
        }
        Command::WatchDirectory { directory_path } | Command::WatchName { directory_path, .. } => {
            let access = access.ok_or(Status::Permission)?;
            if !directory_path.is_empty() {
                state
                    .registry
                    .lookup(access, directory_path, &mut [0; protocol::BODY_MAX])?;
                return Err(Status::NotDirectory);
            }
            let name = if let Command::WatchName { name, .. } = request {
                Some(name)
            } else {
                None
            };
            access.check(name, AccessRights::WATCH)?;
            let pair = libkernel::ipc::notification::create(
                Rights::READ | Rights::WAIT | Rights::MANAGE,
                Rights::SIGNAL,
            )
            .map_err(system)?;
            if !access.output().contains(Rights::WAIT | Rights::TRANSIT) {
                return Err(Status::Permission);
            }
            let notification = pair.owner;
            let signaler = pair.peer;
            let waiter = notification
                .mint_observer(
                    Rights::WAIT | Rights::TRANSIT | (access.output() & Rights::DUPLICATE),
                )
                .map_err(system)?;
            let minted = state
                .mailbox
                .mint(0, access.control_transport()?)
                .map_err(system)?;
            let watch = state.registry.watches.prepare(
                access,
                minted.sender.description().map_err(system)?.object_id,
                name,
                signaler,
                notification,
            )?;
            Ok(Some((Pending::Watch(watch), minted, Some(waiter))))
        }
        _ => {
            let _ = header;
            Ok(None)
        }
    }
}
fn serve_oal(
    state: &mut State,
    header: Header,
    access: Option<&DirectoryAccess>,
    context: &RequestContext,
    out: &mut [u8],
    forced: Option<Status>,
) -> Result<usize, liboal::protocol::Status> {
    let parsed = if slots(context) == 1 {
        liboal::protocol::decode_request(&context.payload)
    } else {
        Err(liboal::protocol::Status::Invalid)
    };
    let mut scratch = [0; protocol::BODY_MAX];
    let result = match (forced, access, parsed) {
        (Some(status), _, _) => Err(status),
        (_, None, _) => Err(Status::Permission),
        (_, Some(access), Ok((_, request))) => match request {
            liboal::protocol::Request::Lookup { path } => state
                .registry
                .lookup(access, path, &mut scratch)
                .map(liboal::protocol::Response::Found),
            liboal::protocol::Request::Enumerate {
                path,
                cursor,
                max_entries,
            } => state
                .registry
                .enumerate(access, path, cursor, max_entries, &mut scratch)
                .map(liboal::protocol::Response::Page),
        },
        (_, _, Err(status)) => Err(Status::from_raw(status as u32).unwrap_or(Status::Invalid)),
    };
    let response = result.unwrap_or_else(|s| {
        liboal::protocol::Response::Error(
            liboal::protocol::Status::from_raw(s as u32)
                .unwrap_or(liboal::protocol::Status::Invalid),
        )
    });
    liboal::protocol::encode_response(header.op, &response, header.deadline, out)
}
fn serve_service(
    state: &mut State,
    header: Header,
    access: Option<&DirectoryAccess>,
    context: &mut RequestContext,
    out: &mut [u8],
    forced: Option<Status>,
) -> Result<(usize, Option<(Capability, Rights)>), Status> {
    let result = (|| {
        if let Some(status) = forced {
            return Err(status);
        }
        let (_, request) = protocol::decode_request(&context.payload, slots(context))?;
        let id = context.envelope.sender_context_id;
        match request {
            Command::GetEntry { path } => {
                let entry = state
                    .registry
                    .get_entry(access.ok_or(Status::Permission)?, path)?;
                let descriptors = protocol::Descriptors::decode(&entry.descriptors)?;
                let used = protocol::encode_response(
                    header.op,
                    &Response::Entry {
                        id: entry.id,
                        descriptors,
                    },
                    header.deadline,
                    out,
                )?;
                Ok((
                    used,
                    Some((entry.endpoint.into_capability(), entry.transport)),
                ))
            }
            Command::Withdraw => {
                let info = state.registry.withdraw(id, Reason::Explicit)?;
                Ok((
                    protocol::encode_response(
                        header.op,
                        &Response::Registration(info),
                        header.deadline,
                        out,
                    )?,
                    None,
                ))
            }
            Command::QueryRegistration => Ok((
                protocol::encode_response(
                    header.op,
                    &Response::Registration(state.registry.query(id)?),
                    header.deadline,
                    out,
                )?,
                None,
            )),
            Command::QueryWatch => Ok((
                protocol::encode_response(
                    header.op,
                    &Response::Watch(state.registry.watches.query(id)?),
                    header.deadline,
                    out,
                )?,
                None,
            )),
            Command::AckWatch { sequence } => Ok((
                protocol::encode_response(
                    header.op,
                    &Response::Watch(state.registry.watches.ack(id, sequence)?),
                    header.deadline,
                    out,
                )?,
                None,
            )),
            Command::CancelWatch => Ok((
                protocol::encode_response(
                    header.op,
                    &Response::Watch(state.registry.watches.cancel(id)?),
                    header.deadline,
                    out,
                )?,
                None,
            )),
            _ => Err(Status::Unsupported),
        }
    })();
    match result {
        Ok(value) => Ok(value),
        Err(status) => Ok((
            protocol::encode_response(header.op, &Response::Error(status), header.deadline, out)?,
            None,
        )),
    }
}
