//! 文件 mailbox 的实际 RPC 接收/执行支持；OAL 与 fs 共用传入的同一 backing。
//! 宿主把 Request 放进已有 Runtime 任务族，所有来源登记仍由 Outbox 推进。

use crate::{
    FsRights,
    authority::{AccessSnapshot, Issuance},
    backend::{Discovery, Enumeration, MemoryBackend, PreparedMutation, Resolved},
    grant::{GrantError, GrantPublication, GrantTable},
    management::{ManagementSnapshot, ManagementTable},
    protocol::{
        self, Header, IoEnd, IoResult, Request as FsRequest, Response as FsResponse, Status,
    },
    resource::FsResource,
};
use erhino_shared::{call::SystemCallError, message::PAYLOAD_MAX, object::Rights, time::Deadline};
use libbudget::{AccountView, Charge};
use libexecution::runtime::{Advance, Input, RequestFailure, Requests, SourceId};
use libkernel::ipc::{capability::Capability, message::ReceivedMessage};
use librpc::{
    exchange::{RejectedRequest, RequestContext, ResponseFailure},
    outbox::Outbox,
};
mod outbound;
mod streams;
use outbound::PendingOutbound;

/// 宿主借用真实流表与启动 Pool；不从请求正文选择资源来源。
pub struct StreamHost<'a> {
    pub table: &'a mut crate::stream::StreamTable,
    pub pool: erhino_shared::object::Handle,
    pub config: crate::stream::Config,
    pub driver: u64,
}

/// 借用宿主唯一运输任务；仅正在 Runtime 中推进的业务任务可以提交下游调用。
pub struct Transport<'a> {
    pub dispatcher: &'a mut librpc::dispatcher::Dispatcher,
    pub driver: u64,
    pub waiter: u64,
}
enum Export {
    Capability(Capability, Rights),
    Derive(Resolved),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Domain {
    Oal,
    Fs,
    Management,
}
impl Domain {
    pub fn from_kind(kind: u64) -> Option<Self> {
        match kind {
            liboal::protocol::ID => Some(Self::Oal),
            protocol::ID => Some(Self::Fs),
            crate::route::ID => Some(Self::Management),
            _ => None,
        }
    }
}
/// 拒收值始终持有收到的原 owner；宿主沿既有 RPC 路径释放，不猜陌生协议。
pub enum Rejected {
    UnknownKind(ReceivedMessage),
    Rpc(RejectedRequest),
    Header(RequestContext),
    Role(RequestContext),
    Resource(ResponseFailure),
}

struct PreparedSubscription {
    watch: crate::watch::PreparedWatch,
    selected: crate::store::NodeRef,
    sender: libkernel::ipc::message::MailboxSender,
    observer: libkernel::ipc::notification::NotificationObserver,
    sender_rights: Rights,
    observer_rights: Rights,
}
struct PreparedDirectory {
    publication: GrantPublication,
    policy: Issuance,
    sender: libkernel::ipc::message::MailboxSender,
}
#[expect(
    clippy::large_enum_variant,
    reason = "原 Request 预付内联准备/失败 owner 的存储，Runnel 出生后不补清理分配"
)]
enum PreparedControl {
    Watch(PreparedSubscription),
    Directory(PreparedDirectory),
    Stream(PreparedFileStream),
}
struct PreparedFileStream {
    stream: crate::stream::PreparedStream,
    creation: Option<PreparedMutation>,
}
#[derive(Clone, Copy)]
enum ControlKind {
    Watch,
    Directory,
    Stream,
}
struct SubscriptionLife {
    kind: ControlKind,
    context: u64,
    lifetime: Option<libkernel::ipc::message::LifetimeObserver>,
    source: libexecution::SourceRegistration,
    committed: bool,
    closed: bool,
    retry_at: Option<u64>,
}
pub struct Request {
    funding: crate::Funding,
    outbox: Option<Outbox>,
    domain: Domain,
    header: Header,
    access: Option<AccessSnapshot>,
    management: Option<ManagementSnapshot>,
    denied: Option<Status>,
    encoded: bool,
    prepared: bool,
    pending: Option<PreparedControl>,
    outbound: Option<PendingOutbound>,
    life: Option<SubscriptionLife>,
    abort: Option<Status>,
    stopping: bool,
    control_source_kind: u64,
    data_source_kind: u64,
    data_source: libexecution::SourceRegistration,
    data_source_active: bool,
    data_observed: erhino_shared::object::ObjectSignals,
    stream_deadline: Deadline,
    finish_context: Option<u64>,
    fees: Option<crate::funding::RequestFees>,
    _task_bytes: Charge,
}
impl Request {
    #[expect(clippy::result_large_err, reason = "拒收原样返还完整消息/请求 owner")]
    #[expect(
        clippy::too_many_arguments,
        reason = "实际接收边界显式借用两种独立授权表及原付款视图"
    )]
    pub fn accept(
        message: ReceivedMessage,
        grants: &GrantTable,
        management: &ManagementTable,
        backend: &MemoryBackend,
        streams: &crate::stream::StreamTable,
        admission_account: &AccountView<FsResource>,
        admission_execution: &AccountView<libexecution::ExecutionResource>,
        reply_source_kind: u64,
        control_source_kind: u64,
        data_source_kind: u64,
    ) -> Result<Self, Rejected> {
        let Some(domain) = Domain::from_kind(message.header.kind) else {
            return Err(Rejected::UnknownKind(message));
        };
        let context = RequestContext::decode(
            message,
            match domain {
                Domain::Oal => liboal::protocol::ID,
                Domain::Fs => protocol::ID,
                Domain::Management => crate::route::ID,
            },
        )
        .map_err(Rejected::Rpc)?;
        let header = match domain {
            Domain::Fs | Domain::Management => {
                Header::decode(&context.payload).map(|(h, _)| h).ok()
            }
            Domain::Oal => liboal::protocol::Header::decode(&context.payload)
                .map(|(h, _)| Header {
                    version: h.version,
                    op: h.op,
                    status: h.status,
                    body_len: h.body_len,
                    deadline: h.deadline,
                })
                .ok(),
        };
        let Some(header) = header.filter(|h| h.status == 0) else {
            return Err(Rejected::Header(context));
        };
        let known = header.version == 1
            && match domain {
                Domain::Oal => matches!(header.op, 1 | 2),
                Domain::Fs => protocol::Op::from_raw(header.op).is_some(),
                Domain::Management => matches!(header.op, 1 | 2),
            };
        if known {
            let expected = if (domain == Domain::Fs && header.op == protocol::Op::Rename as u16)
                || (domain == Domain::Management && header.op == 1)
            {
                3
            } else {
                1
            };
            if slots(&context) != expected {
                return Err(Rejected::Role(context));
            }
            // Rename 的第二槽由可信 grant 表判角色/撤销/跨 backing；
            // 错角色是领域 Permission，不能在 RPC 建立前丢成 Closed。
        }
        let (access, management_access, denied) = if domain == Domain::Management {
            match management.access(context.envelope.sender_context_id) {
                Ok(access) => (None, Some(access), None),
                Err(status) => (None, None, Some(status)),
            }
        } else {
            match grants.access(context.envelope.sender_context_id) {
                Ok(access) => (Some(access), None, None),
                Err(status) => (None, None, Some(status)),
            }
        };
        let funding = match grants
            .funding(context.envelope.sender_context_id)
            .or_else(|| management.funding(context.envelope.sender_context_id))
            .or_else(|| backend.watches.funding(context.envelope.sender_context_id))
            .or_else(|| streams.funding(context.envelope.sender_context_id))
        {
            Some(funding) => funding.clone(),
            None => match crate::Funding::new(admission_account, admission_execution) {
                Ok(funding) => funding,
                Err(error) => return Err(Rejected::Resource(ResponseFailure { error, context })),
            },
        };
        let (fees, task_bytes) = match funding.prepare_request::<Self>() {
            Ok(charges) => charges,
            Err(error) => return Err(Rejected::Resource(ResponseFailure { error, context })),
        };
        let outbox = Outbox::prepare(
            context,
            PAYLOAD_MAX - librpc::PREFIX_LEN,
            header.deadline,
            reply_source_kind,
            &libexecution::SourceFunding::Task,
        )
        .map_err(Rejected::Resource)?;
        Ok(Self {
            funding,
            outbox: Some(outbox),
            domain,
            header,
            access,
            management: management_access,
            denied,
            encoded: false,
            prepared: false,
            pending: None,
            outbound: None,
            life: None,
            abort: None,
            stopping: false,
            control_source_kind,
            data_source_kind,
            data_source: libexecution::SourceRegistration::new(),
            data_source_active: false,
            data_observed: erhino_shared::object::ObjectSignals::NONE,
            stream_deadline: Deadline::INFINITE,
            finish_context: None,
            fees: Some(fees),
            _task_bytes: task_bytes,
        })
    }

    pub fn execution_funding(&self) -> &AccountView<libexecution::ExecutionResource> {
        self.funding.execution()
    }
    fn prepare_subscription(
        &mut self,
        backend: &MemoryBackend,
        grants: &mut GrantTable,
        mailbox: &libkernel::ipc::message::Mailbox,
    ) -> Result<(), Status> {
        if self.domain != Domain::Fs
            || !matches!(
                protocol::Op::from_raw(self.header.op),
                Some(protocol::Op::Watch | protocol::Op::DeriveDirectory)
            )
            || self.header.version != 1
        {
            return Ok(());
        }
        let response = self
            .outbox
            .as_mut()
            .ok_or(Status::Internal)?
            .response_mut()
            .map_err(failure)?;
        let (context, _) = response.parts().map_err(failure)?;
        let (_, request) = protocol::decode_request(&context.payload, slots(context))?;
        let path = request.path().ok_or(Status::Invalid)?;
        let access = self
            .access
            .as_ref()
            .ok_or(self.denied.unwrap_or(Status::Permission))?;
        let Resolved::Local(selected) = backend.resolve(access, path, true)? else {
            // 仍由领域响应给出 Boundary，不为接续建立本地订阅。
            return Ok(());
        };
        if let FsRequest::DeriveDirectory {
            rights,
            sender_transport,
            output_transport,
            ..
        } = request
        {
            let policy = Issuance {
                rights,
                sender_transport,
                output_transport,
            };
            backend.check_directory_issue(access, &selected, policy)?;
            let prepared = grants
                .prepare_derive(
                    mailbox,
                    access,
                    selected.clone(),
                    policy,
                    backend.ceiling(&selected)?,
                    0,
                )
                .map_err(|failed| failure(failed.error))?;
            let (publication, sender, lifetime) = prepared.into_parts();
            let context = publication.context();
            self.pending = Some(PreparedControl::Directory(PreparedDirectory {
                publication,
                policy,
                sender,
            }));
            self.life = Some(SubscriptionLife {
                kind: ControlKind::Directory,
                context,
                lifetime: Some(lifetime),
                source: libexecution::SourceRegistration::new(),
                committed: false,
                closed: false,
                retry_at: None,
            });
            return Ok(());
        }
        let FsRequest::Watch { mask, .. } = request else {
            return Err(Status::Invalid);
        };
        backend.check(access, &selected, FsRights::WATCH)?;
        let mandatory = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
        let observer_required = Rights::WAIT | Rights::TRANSIT;
        let output = access.output_transport();
        if !output.contains(mandatory | observer_required) {
            return Err(Status::Permission);
        }
        let sender_rights = mandatory | (output & Rights::DUPLICATE);
        let observer_rights = observer_required | (output & Rights::DUPLICATE);
        let pair = libkernel::ipc::notification::create(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::SIGNAL,
        )
        .map_err(failure)?;
        let observer = pair.owner.mint_observer(observer_rights).map_err(failure)?;
        let minted = mailbox.mint(0, sender_rights).map_err(failure)?;
        let id = minted.sender.description().map_err(failure)?.object_id;
        let watch =
            backend
                .watches
                .prepare(id, access, selected.id(), mask, pair.owner, pair.peer)?;
        self.pending = Some(PreparedControl::Watch(PreparedSubscription {
            watch,
            selected,
            sender: minted.sender,
            observer,
            sender_rights,
            observer_rights,
        }));
        self.life = Some(SubscriptionLife {
            kind: ControlKind::Watch,
            context: id,
            lifetime: Some(minted.lifetime),
            source: libexecution::SourceRegistration::new(),
            committed: false,
            closed: false,
            retry_at: None,
        });
        Ok(())
    }
    fn encode_error(&mut self, status: Status) -> Result<(), SystemCallError> {
        let response = self
            .outbox
            .as_mut()
            .ok_or(SystemCallError::ObjectClosed)?
            .response_mut()?;
        response.drain_capabilities(|capability, _| drop(capability));
        let out = response.body_mut()?;
        let used = match self.domain {
            Domain::Oal => liboal::protocol::encode_response(
                self.header.op,
                &liboal::protocol::Response::Error(oal_status(status)),
                self.header.deadline,
                out,
            )
            .map_err(|_| SystemCallError::IllegalArgument)?,
            Domain::Fs => protocol::encode_response(
                self.header.op,
                &FsResponse::Error(status),
                self.header.deadline,
                out,
            )
            .map_err(|_| SystemCallError::IllegalArgument)?,
            Domain::Management => {
                crate::route::encode_response(self.header.op, status, self.header.deadline, out)
                    .map_err(|_| SystemCallError::IllegalArgument)?
            }
        };
        response.finish_body(used)?;
        self.encoded = true;
        Ok(())
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "实际任务借用 backing、授权、运输与 Runtime 推进边界"
    )]
    pub fn advance<F>(
        &mut self,
        backend: &mut MemoryBackend,
        grants: &mut GrantTable,
        mailbox: &libkernel::ipc::message::Mailbox,
        mut transport: Option<Transport<'_>>,
        mut stream_host: StreamHost<'_>,
        requests: &mut Requests<'_, F>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        use erhino_shared::object::ObjectSignals;
        use libexecution::runtime::Step;
        if budget == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        while let Some(event) = input.pull() {
            self.observe(event);
        }
        if input.take_timeout()
            && self.outbox.as_ref().is_some_and(|outbox| {
                outbox
                    .deadline()
                    .instant()
                    .ok()
                    .flatten()
                    .is_some_and(|at| at <= input.now_ns())
            })
        {
            self.timed_out();
        }
        if self.stopping {
            self.abort.get_or_insert(Status::Cancelled);
            if let Some(life) = self.life.as_mut() {
                life.closed = true;
            }
        }
        if self.life.as_ref().is_none_or(|life| !life.committed)
            && self
                .outbox
                .as_ref()
                .is_some_and(|outbox| outbox.result().is_some())
        {
            self.abort.get_or_insert(Status::Cancelled);
        }
        if let Some(outbox) = self.outbox.as_mut()
            && outbox.result().is_some()
            && !outbox.is_complete()
        {
            // 先取消在途下游并移出已终结回复的 CLOSED 电平来源。
            // completion、业务 pin 与费用仍由本请求保留至真实收束。
            if let Some(txid) = self.outbound.as_ref().and_then(PendingOutbound::txid)
                && let Some(transport) = transport.as_mut()
            {
                transport.dispatcher.cancel(txid);
                let _ = requests.wake(transport.driver);
            }
            let advance = outbox.drive(requests, budget)?;
            if !outbox.is_complete() {
                return Ok(advance);
            }
        }
        if self.finish_wait(stream_host.table, stream_host.driver, input.now_ns())? {
            return Ok(file_step(Step::Parked));
        }
        if !self.prepared && self.abort.is_none() {
            let outbox = self
                .outbox
                .as_mut()
                .expect("unprepared file request lost reply owner");
            if !outbox.is_admitted() {
                return Ok(outbox.admit(requests));
            }
            self.prepared = true;
            if let Err(status) = self.prepare_stream(backend, mailbox, &mut stream_host) {
                self.abort = Some(status);
            }
            if self.abort.is_none()
                && let Err(status) = self.prepare_subscription(backend, grants, mailbox)
            {
                self.abort = Some(status);
            }
            if self.abort.is_none()
                && let Err(status) = self.prepare_bind(backend, transport.as_mut(), requests)
            {
                self.abort = Some(status);
            }
        }
        if self.outbound.is_some() {
            return self.advance_outbound(backend, transport.as_mut(), requests);
        }
        if self.pending.is_some() && self.abort.is_none() {
            if !self.stream_precommit(&mut stream_host, requests, input.now_ns())? {
                return Ok(file_step(Step::Runnable));
            }
            let life = self
                .life
                .as_mut()
                .expect("prepared file subscription lost Lifetime");
            if life.closed {
                self.abort = Some(Status::Unavailable);
            } else {
                if life.source.is_empty() {
                    let handle = life
                        .lifetime
                        .as_ref()
                        .expect("prepared file Lifetime missing")
                        .as_handle();
                    if life
                        .source
                        .install(
                            requests,
                            libexecution::runtime::SourcePlan::new(handle, ObjectSignals::CLOSED),
                            self.control_source_kind,
                        )
                        .is_err()
                    {
                        return Ok(file_step(Step::Runnable));
                    }
                }
                if life.source.source().is_none() {
                    return Ok(file_step(Step::Runnable));
                }
                let closed = libkernel::ipc::wait::probe(
                    life.lifetime
                        .as_ref()
                        .expect("prepared file Lifetime missing")
                        .as_handle(),
                    ObjectSignals::CLOSED,
                )?
                .intersects(ObjectSignals::CLOSED);
                let pending = self
                    .pending
                    .as_ref()
                    .expect("prepared file subscription missing");
                let status = if libkernel::time::expired(self.header.deadline)? {
                    Some(Status::Expired)
                } else if closed {
                    Some(Status::Unavailable)
                } else {
                    match pending {
                        PreparedControl::Watch(pending) if !pending.watch.is_authorized() => {
                            Some(Status::Revoked)
                        }
                        PreparedControl::Watch(pending) if !backend.contains(&pending.selected) => {
                            Some(Status::NotFound)
                        }
                        PreparedControl::Watch(_) => None,
                        PreparedControl::Directory(pending) => backend
                            .check_directory_issue(
                                self.access
                                    .as_ref()
                                    .expect("prepared directory lost admitted authority"),
                                pending.publication.root(),
                                pending.policy,
                            )
                            .err(),
                        PreparedControl::Stream(_) => None,
                    }
                };
                if let Some(status) = status {
                    self.abort = Some(status);
                } else {
                    let pending = self
                        .pending
                        .take()
                        .expect("prepared file subscription missing");
                    let response = self
                        .outbox
                        .as_mut()
                        .expect("prepared file reply missing")
                        .response_mut()?;
                    let (_, out) = response.parts()?;
                    match pending {
                        PreparedControl::Watch(pending) => {
                            let used = protocol::encode_response(
                                self.header.op,
                                &FsResponse::Watch(pending.watch.info()),
                                self.header.deadline,
                                out,
                            )
                            .map_err(|_| SystemCallError::IllegalArgument)?;
                            response
                                .push(pending.sender.into_capability(), pending.sender_rights)
                                .map_err(|failed| failed.error)?;
                            response
                                .push(pending.observer.into_capability(), pending.observer_rights)
                                .map_err(|failed| failed.error)?;
                            response.finish_body(used)?;
                            backend.watches.commit(pending.watch);
                            backend.schedule_watches();
                        }
                        PreparedControl::Directory(pending) => {
                            let used = protocol::encode_response(
                                self.header.op,
                                &FsResponse::Empty,
                                self.header.deadline,
                                out,
                            )
                            .map_err(|_| SystemCallError::IllegalArgument)?;
                            // 表项仍未公开；能力先转入既有 Outbox 清理链，提交无分配。
                            response
                                .push(
                                    pending.sender.into_capability(),
                                    pending.policy.sender_transport,
                                )
                                .map_err(|failed| failed.error)?;
                            response.finish_body(used)?;
                            grants.install(pending.publication);
                        }
                        PreparedControl::Stream(pending) => {
                            // 短借用 life 在调用持 self 的提交步骤前终止。
                            self.commit_stream(pending, backend, &mut stream_host)?;
                            return Ok(file_step(Step::Runnable));
                        }
                    }
                    life.committed = true;
                    self.encoded = true;
                }
            }
        }
        if self.abort.is_some() && self.life.as_ref().is_none_or(|life| !life.committed) {
            if let Some(life) = self.life.as_mut()
                && !life.source.is_empty()
            {
                if life.source.remove(requests).is_err() {
                    return Ok(file_step(Step::Runnable));
                }
                return Ok(file_step(
                    if life.source.is_removing() || life.source.is_installing() {
                        Step::Parked
                    } else {
                        Step::Runnable
                    },
                ));
            }
            if !self.cleanup_prepared_stream(requests, input.now_ns())? {
                let step = if self
                    .stream_deadline
                    .instant()
                    .ok()
                    .flatten()
                    .is_some_and(|at| input.now_ns() < at)
                    || self.data_source.is_installing()
                    || self.data_source.is_removing()
                {
                    Step::Parked
                } else {
                    Step::Runnable
                };
                return Ok(file_step(step));
            }
            if let Some(life) = self.life.as_mut()
                && life.retry_at.is_some_and(|at| input.now_ns() < at)
            {
                return Ok(file_step(Step::Parked));
            }
            if let Some(life) = self.life.as_mut()
                && let Some(lifetime) = life.lifetime.take()
                && let Err((lifetime, _)) = lifetime.close()
            {
                life.lifetime = Some(lifetime);
                life.retry_at = Some(
                    input
                        .now_ns()
                        .saturating_add(libexecution::runtime::CLEANUP_RETRY_NS),
                );
                return Ok(file_step(Step::Parked));
            }
            self.life = None;
            self.pending = None;
            self.stream_deadline = Deadline::INFINITE;
            if !self.encoded
                && self
                    .outbox
                    .as_ref()
                    .is_some_and(|outbox| outbox.result().is_none())
            {
                self.encode_error(self.abort.expect("aborting file request lost reason"))?;
            }
        }
        if !self.encoded
            && self.abort.is_none()
            && self.pending.is_none()
            && self
                .outbox
                .as_ref()
                .is_some_and(|outbox| outbox.result().is_none())
        {
            if self.serve_stream_control(&mut stream_host, input.now_ns())? {
                return Ok(file_step(if self.finish_context.is_some() {
                    Step::Parked
                } else {
                    Step::Runnable
                }));
            }
            let response = self
                .outbox
                .as_mut()
                .expect("file reply missing")
                .response_mut()?;
            let (context, out) = response.parts()?;
            let mut export = None;
            let access = self
                .access
                .as_ref()
                .ok_or(self.denied.unwrap_or(Status::Permission));
            let used = match self.domain {
                Domain::Oal => serve_oal(
                    backend,
                    grants.mailbox_id(),
                    access,
                    self.header,
                    &context.payload,
                    out,
                    &mut export,
                ),
                Domain::Fs => serve_fs(
                    backend,
                    grants,
                    access,
                    self.header,
                    context,
                    out,
                    &mut export,
                ),
                Domain::Management => serve_unbind(
                    backend,
                    self.management
                        .as_ref()
                        .ok_or(self.denied.unwrap_or(Status::Permission)),
                    self.header,
                    context,
                    out,
                ),
            }
            .map_err(|_| SystemCallError::IllegalArgument)?;
            if matches!(export, Some(Export::Derive(_))) {
                let Some(Export::Derive(boundary)) = export.take() else {
                    unreachable!("validated deferred export changed");
                };
                match self.prepare_boundary(backend, boundary, used, transport.as_mut(), requests) {
                    Ok(()) => return Ok(file_step(Step::Runnable)),
                    Err(status) => {
                        self.abort = Some(status);
                        self.encode_error(status)?;
                        return Ok(file_step(Step::Runnable));
                    }
                }
            }
            if let Some(Export::Capability(capability, rights)) = export {
                response
                    .push(capability, rights)
                    .map_err(|failed| failed.error)?;
            }
            response.finish_body(used)?;
            self.encoded = true;
        }
        if let Some(life) = self.life.as_mut()
            && life.committed
            && matches!(life.kind, ControlKind::Stream)
            && life.closed
        {
            stream_host.table.stop(
                life.context,
                Status::Unavailable,
                if self.stopping {
                    protocol::StreamReason::ProviderStopped
                } else {
                    protocol::StreamReason::PeerClosed
                },
            );
            // 已锁存的 CLOSED 电平先移出观察；实际 owner/费用仍留到数据退休。
            if !life.source.is_empty() {
                if life.source.remove(requests).is_err() {
                    return Ok(file_step(Step::Runnable));
                }
                return Ok(file_step(
                    if life.source.is_removing() || life.source.is_installing() {
                        Step::Parked
                    } else {
                        Step::Runnable
                    },
                ));
            }
        }
        if let Some(life) = self.life.as_ref()
            && life.committed
            && matches!(life.kind, ControlKind::Stream)
        {
            let context = life.context;
            // 控制关闭的停止意图已在上方锁存并移出来源。
            if self.abort.is_some() && !life.closed {
                stream_host.table.stop(
                    context,
                    self.abort.unwrap_or(Status::Resource),
                    protocol::StreamReason::Backend,
                );
            }
            let observed = core::mem::replace(&mut self.data_observed, ObjectSignals::NONE);
            let advance = stream_host.table.advance(
                context,
                backend,
                &mut self.data_source,
                &mut self.data_source_active,
                self.data_source_kind,
                requests,
                input.now_ns(),
                observed,
            )?;
            self.stream_deadline = stream_host.table.deadline(context);
            if !stream_host.table.data_released(context)
                || !life.closed
                || stream_host.table.has_finish_waiter(context)
            {
                // 完成原建立回复的释放链，随后会话由同一个原任务继续推进。
                if let Some(reply) = self.finish_reply(requests, budget)? {
                    return Ok(reply);
                }
                return Ok(advance);
            }
        }
        if let Some(life) = self.life.as_mut()
            && life.committed
            && life.closed
        {
            match life.kind {
                ControlKind::Watch => {
                    let _ = backend.watches.cancel(life.context);
                    backend.schedule_watches();
                }
                ControlKind::Directory if self.stopping => {
                    grants.revoke(life.context);
                }
                ControlKind::Directory => {}
                ControlKind::Stream => {}
            }
            if !life.source.is_empty() {
                if life.source.remove(requests).is_err() {
                    return Ok(file_step(Step::Runnable));
                }
                return Ok(file_step(
                    if life.source.is_removing() || life.source.is_installing() {
                        Step::Parked
                    } else {
                        Step::Runnable
                    },
                ));
            }
            if life.retry_at.is_some_and(|at| input.now_ns() < at) {
                return Ok(file_step(Step::Parked));
            }
            if let Some(lifetime) = life.lifetime.take()
                && let Err((lifetime, _)) = lifetime.close()
            {
                life.lifetime = Some(lifetime);
                life.retry_at = Some(
                    input
                        .now_ns()
                        .saturating_add(libexecution::runtime::CLEANUP_RETRY_NS),
                );
                return Ok(file_step(Step::Parked));
            }
            match life.kind {
                ControlKind::Watch => {
                    backend.watches.remove(life.context);
                    backend.schedule_watches();
                }
                ControlKind::Directory => {
                    grants.remove(life.context);
                }
                ControlKind::Stream => stream_host.table.remove(life.context),
            }
            self.life = None;
            self.stream_deadline = Deadline::INFINITE;
        }
        if let Some(advance) = self.finish_reply(requests, budget)? {
            return Ok(advance);
        }
        Ok(file_step(if self.life.is_some() {
            Step::Parked
        } else {
            Step::Complete
        }))
    }

    /// 原回复来源实际退出后，才能释放请求费用和准入授权引用。
    fn finish_reply<F>(
        &mut self,
        requests: &mut Requests<'_, F>,
        budget: usize,
    ) -> Result<Option<Advance>, SystemCallError> {
        if let Some(outbox) = self.outbox.as_mut() {
            let advance = outbox.drive(requests, budget)?;
            if !outbox.is_complete() {
                return Ok(Some(advance));
            }
            self.outbox = None;
            self.fees = None;
            self.access = None;
            self.management = None;
        }
        Ok(None)
    }

    pub fn refused<F>(&mut self, failure: RequestFailure<F>) {
        if let RequestFailure::Source { kind, .. } = &failure
            && *kind == self.data_source_kind
        {
            self.data_source.refused(*kind);
            self.abort.get_or_insert(Status::Resource);
        } else if let RequestFailure::Source { kind, .. } = &failure
            && *kind == self.control_source_kind
        {
            if let Some(life) = self.life.as_mut() {
                life.source.refused(*kind);
            }
            self.abort.get_or_insert(Status::Resource);
        } else if let Some(outbox) = self.outbox.as_mut() {
            outbox.refused(&mut (), failure);
        }
    }
    pub fn registered(&mut self, kind: u64, source: SourceId) {
        if kind == self.data_source_kind {
            self.data_source.registered(kind, source);
        } else if kind == self.control_source_kind {
            if let Some(life) = self.life.as_mut() {
                life.source.registered(kind, source);
            }
        } else if let Some(outbox) = self.outbox.as_mut() {
            outbox.registered(&mut (), kind, source);
        }
    }
    pub fn unregistered(&mut self, kind: u64, source: SourceId) {
        if kind == self.data_source_kind {
            self.data_source.unregistered(source);
        } else if kind == self.control_source_kind {
            if let Some(life) = self.life.as_mut() {
                life.source.unregistered(source);
            }
        } else if let Some(outbox) = self.outbox.as_mut() {
            outbox.unregistered(&mut (), kind, source);
        }
    }
    pub fn stop(&mut self) {
        self.stopping = true;
        if let Some(outbox) = self.outbox.as_mut() {
            outbox.stop(&mut ());
        }
    }
    pub fn deadline(&self) -> Deadline {
        let reply = self
            .outbox
            .as_ref()
            .map_or(Deadline::INFINITE, Outbox::deadline);
        let stream = self
            .life
            .as_ref()
            .and_then(|life| life.retry_at)
            .map_or(self.stream_deadline, Deadline::at);
        match (
            reply.instant().ok().flatten(),
            stream.instant().ok().flatten(),
        ) {
            (Some(a), Some(b)) => Deadline::at(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Deadline::at(a),
            (None, None) => Deadline::INFINITE,
        }
    }
    pub fn reply_complete(&self) -> bool {
        self.outbox.as_ref().is_none_or(Outbox::is_complete)
    }
    pub fn reject(&mut self, status: Status) -> Result<(), SystemCallError> {
        self.abort = Some(status);
        self.encode_error(status)
    }
    pub fn observe(&mut self, event: libexecution::runtime::SourceEvent) {
        if event.kind == self.data_source_kind {
            self.data_observed |= event.observed;
        } else if event.kind == self.control_source_kind {
            if event
                .observed
                .intersects(erhino_shared::object::ObjectSignals::CLOSED)
                && let Some(life) = self.life.as_mut()
            {
                life.closed = true;
            }
        } else if let Some(outbox) = self.outbox.as_mut() {
            outbox.observe(event);
        }
    }
    pub fn timed_out(&mut self) {
        if let Some(outbox) = self.outbox.as_mut() {
            outbox.timed_out();
        }
    }
}
fn serve_unbind(
    backend: &mut MemoryBackend,
    access: Result<&ManagementSnapshot, Status>,
    header: Header,
    context: &RequestContext,
    out: &mut [u8],
) -> Result<usize, Status> {
    let result = (|| {
        let (_, request) = crate::route::decode_request(&context.payload, slots(context))?;
        let access = access?;
        let (parent, name) = backend.management_parent(access, request.path())?;
        let prepared = match request {
            crate::route::Request::Unbind { .. } => backend.prepare_unbind(access, parent, name)?,
            // Bind 在准入后进入 PendingOutbound，不落到本地同步提交入口。
            crate::route::Request::Bind { .. } => return Err(Status::Internal),
        };
        let used = crate::route::encode_response(header.op, Status::Ok, header.deadline, out)?;
        if libkernel::time::expired(header.deadline).map_err(failure)? {
            return Err(Status::Expired);
        }
        backend.commit(prepared).map_err(|failed| failed.status)?;
        Ok(used)
    })();
    match result {
        Ok(used) => Ok(used),
        Err(status) => crate::route::encode_response(header.op, status, header.deadline, out),
    }
}
fn file_step(step: libexecution::runtime::Step) -> Advance {
    Advance { work_done: 1, step }
}

fn slots(context: &RequestContext) -> u64 {
    let mut mask = 1u64;
    for i in 1..context.handles.len() {
        if context.handles.get(i).is_ok() {
            mask |= 1u64 << i;
        }
    }
    mask
}
fn failure(error: SystemCallError) -> Status {
    match error {
        SystemCallError::RightsDenied => Status::Permission,
        SystemCallError::QuotaExceeded => Status::Quota,
        SystemCallError::OutOfMemory => Status::Resource,
        SystemCallError::ObjectClosed => Status::Revoked,
        _ => Status::Resource,
    }
}
fn commit(
    backend: &mut MemoryBackend,
    prepared: PreparedMutation,
    deadline: Deadline,
) -> Result<(), Status> {
    if libkernel::time::expired(deadline).map_err(failure)? {
        return Err(Status::Expired);
    }
    drop(backend.commit(prepared).map_err(|f| f.status)?);
    Ok(())
}
fn oal_status(status: Status) -> liboal::protocol::Status {
    // 两个领域独立拥有固定 status 数字；这里是文件 provider 的明确跨接口映射。
    liboal::protocol::Status::from_raw(status as u32).expect("file status lacks discovery mapping")
}
fn boundary_export(
    backend: &MemoryBackend,
    access: &AccessSnapshot,
    boundary: &Resolved,
    required: FsRights,
    export: &mut Option<Export>,
) -> Result<bool, Status> {
    match boundary {
        Resolved::Link { .. } => Ok(true),
        Resolved::Local(_) => Err(Status::Internal),
        Resolved::Provider {
            binding, parent, ..
        } => {
            backend.check_binding_boundary(access, binding, parent)?;
            let effective = access.rights().intersect(binding.max_rights());
            if !effective.contains(required) {
                return Err(Status::Permission);
            }
            let output = access.output_transport() & binding.output_transport();
            let mandatory = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
            if !output.contains(mandatory) {
                return Err(Status::Permission);
            }
            if effective != binding.max_rights() || output != binding.output_transport() {
                return Ok(false);
            }
            let rights = mandatory | (output & Rights::DUPLICATE);
            let shared = backend.binding_sender(binding, true)?;
            let duplicate = shared
                .description()
                .and_then(|d| {
                    if !rights.is_subset_of(d.rights) {
                        return Err(SystemCallError::RightsDenied);
                    }
                    // 复制失败不改 binding；返回的临时 cap 在回复失败时由 Packet/Outbox 持有。
                    libkernel::ipc::object::duplicate(shared.as_handle(), rights).map(|handle| {
                        // SAFETY: duplicate 成功返回本函数唯一拥有的新、未映射 handle，
                        // 尚未交给任何 Packet/typed owner；from_raw 接管其完整关闭责任。
                        unsafe { Capability::from_raw(handle) }
                    })
                })
                .map_err(failure)?;
            *export = Some(Export::Capability(duplicate, rights));
            Ok(true)
        }
    }
}
fn serve_oal(
    backend: &MemoryBackend,
    incarnation: u64,
    access: Result<&AccessSnapshot, Status>,
    header: Header,
    payload: &[u8],
    out: &mut [u8],
    export: &mut Option<Export>,
) -> Result<usize, Status> {
    use liboal::protocol::{self as oal, Request as Q, Response as R};
    let encode = |r: R<'_>, out: &mut [u8]| {
        oal::encode_response(header.op, &r, header.deadline, out).map_err(|_| Status::Internal)
    };
    let decoded = oal::decode_request(payload)
        .map(|(_, q)| q)
        .map_err(|s| Status::from_raw(s as u32).unwrap_or(Status::Invalid));
    let mut scratch = [0u8; protocol::BODY_MAX];
    let result = (|| {
        let q = decoded?;
        let access = access?;
        if !access.is_active() {
            return Err(Status::Revoked);
        }
        match q {
            Q::Lookup { path } => match backend.lookup(access, path, &mut scratch)? {
                Discovery::Found(info) => encode(R::Found(info), out),
                Discovery::Boundary(boundary) => {
                    let ready = boundary_export(
                        backend,
                        access,
                        &boundary,
                        FsRights::TRAVERSE | FsRights::DESCRIBE,
                        export,
                    )?;
                    let used = encode(
                        R::Boundary(boundary.boundary().ok_or(Status::Internal)?),
                        out,
                    )?;
                    if !ready {
                        *export = Some(Export::Derive(boundary));
                    }
                    Ok(used)
                }
            },
            Q::Enumerate {
                path,
                cursor,
                max_entries,
            } => match backend.enumerate(
                access,
                incarnation,
                path,
                cursor,
                max_entries,
                &mut scratch,
            )? {
                Enumeration::Page(page) => encode(R::Page(page), out),
                Enumeration::Boundary(boundary) => {
                    let ready = boundary_export(
                        backend,
                        access,
                        &boundary,
                        FsRights::TRAVERSE | FsRights::ENUMERATE,
                        export,
                    )?;
                    let used = encode(
                        R::Boundary(boundary.boundary().ok_or(Status::Internal)?),
                        out,
                    )?;
                    if !ready {
                        *export = Some(Export::Derive(boundary));
                    }
                    Ok(used)
                }
            },
        }
    })();
    match result {
        Ok(n) => Ok(n),
        Err(status) => {
            *export = None;
            encode(R::Error(oal_status(status)), out)
        }
    }
}
fn serve_fs(
    backend: &mut MemoryBackend,
    grants: &GrantTable,
    access: Result<&AccessSnapshot, Status>,
    header: Header,
    context: &RequestContext,
    out: &mut [u8],
    export: &mut Option<Export>,
) -> Result<usize, Status> {
    let encode = |r: FsResponse<'_>, out: &mut [u8]| {
        protocol::encode_response(header.op, &r, header.deadline, out)
    };
    let result = (|| {
        let (_, request) = protocol::decode_request(&context.payload, slots(context))?;
        let control = context.envelope.sender_context_id;
        match request {
            FsRequest::QueryWatch => {
                return encode(FsResponse::Watch(backend.watches.query(control)?), out);
            }
            FsRequest::AckWatch { sequence } => {
                let info = backend.watches.ack(control, sequence)?;
                backend.schedule_watches();
                return encode(FsResponse::Watch(info), out);
            }
            FsRequest::CancelWatch => {
                let info = backend.watches.cancel(control)?;
                backend.schedule_watches();
                return encode(FsResponse::Watch(info), out);
            }
            _ => {}
        }
        let access = access?;
        if !access.is_active() {
            return Err(Status::Revoked);
        }
        if !request.op().has_path() {
            return Err(Status::Permission);
        }
        let required = match request {
            FsRequest::Get { .. } | FsRequest::ReadLink { .. } => FsRights::READ_PROPERTY,
            FsRequest::Set { .. } => FsRights::WRITE_PROPERTY,
            FsRequest::ReadAt { .. } => FsRights::READ_STREAM,
            FsRequest::WriteAt { .. } | FsRequest::SetLength { .. } => FsRights::WRITE_STREAM,
            FsRequest::Create { .. } | FsRequest::CreateOpen { .. } => FsRights::CREATE,
            FsRequest::Remove { .. } | FsRequest::Rename { .. } => FsRights::REMOVE,
            FsRequest::DeriveDirectory { .. } => FsRights::DERIVE,
            FsRequest::Watch { .. } => FsRights::WATCH,
            FsRequest::Open { options, .. } => match options.direction {
                protocol::Direction::Read => FsRights::READ_STREAM,
                protocol::Direction::Write => FsRights::WRITE_STREAM,
            },
            _ => FsRights::NONE,
        };
        if !access.rights().contains(required) {
            return Err(Status::Permission);
        }
        let parent = matches!(
            request,
            FsRequest::Create { .. }
                | FsRequest::CreateOpen { .. }
                | FsRequest::Remove { .. }
                | FsRequest::Rename { .. }
        );
        let path = request.path().ok_or(Status::Invalid)?;
        let (resolved, name) = if parent {
            backend.parent_for_operation(access, path, required)?
        } else {
            (
                backend.resolve(access, path, !matches!(request, FsRequest::ReadLink { .. }))?,
                "",
            )
        };
        let Resolved::Local(node) = resolved else {
            // 父路径遍历产生的边界补回尚未操作的最后名字，覆盖完整原请求路径。
            let boundary = if parent {
                append_final(resolved, name)?
            } else {
                resolved
            };
            let ready = boundary_export(backend, access, &boundary, required, export)?;
            let used = encode(
                FsResponse::Boundary(boundary.boundary().ok_or(Status::Internal)?),
                out,
            )?;
            if !ready {
                *export = Some(Export::Derive(boundary));
            }
            return Ok(used);
        };
        match request {
            FsRequest::Get { .. } => {
                encode(FsResponse::Value(backend.property(access, &node)?), out)
            }
            FsRequest::ReadLink { .. } => encode(
                FsResponse::LinkTarget(backend.read_link(access, &node)?),
                out,
            ),
            FsRequest::ReadAt { offset, count, .. } => {
                let mut bytes = [0u8; protocol::BODY_MAX - 24];
                let max = usize::try_from(count)
                    .map_err(|_| Status::Range)?
                    .min(bytes.len());
                let read = backend.read_at(access, &node, offset, &mut bytes[..max])?;
                let end = if count != 0 && read < max {
                    IoEnd::Eof
                } else {
                    IoEnd::Continue
                };
                encode(
                    FsResponse::Io(IoResult {
                        count: read as u64,
                        end,
                        cause: Status::Ok,
                        data: &bytes[..read],
                    }),
                    out,
                )
            }
            FsRequest::Set { value, .. } => {
                let prepared = backend.prepare_set(access, node, value)?;
                let used = encode(FsResponse::Empty, out)?;
                commit(backend, prepared, header.deadline)?;
                Ok(used)
            }
            FsRequest::WriteAt { offset, data, .. } => {
                let prepared = backend.prepare_write(access, node, offset, data)?;
                let used = encode(
                    FsResponse::Io(IoResult {
                        count: data.len() as u64,
                        end: IoEnd::Continue,
                        cause: Status::Ok,
                        data: &[],
                    }),
                    out,
                )?;
                commit(backend, prepared, header.deadline)?;
                Ok(used)
            }
            FsRequest::SetLength { length, .. } => {
                let prepared = backend.prepare_length(access, node, length)?;
                let used = encode(FsResponse::Empty, out)?;
                commit(backend, prepared, header.deadline)?;
                Ok(used)
            }
            FsRequest::Create {
                kind,
                rights,
                initial,
                ..
            } => {
                let prepared = backend.prepare_create(access, node, name, kind, rights, initial)?;
                let used = encode(FsResponse::Empty, out)?;
                commit(backend, prepared, header.deadline)?;
                Ok(used)
            }
            FsRequest::Remove { .. } => {
                let prepared = backend.prepare_remove(access, node, name)?;
                let used = encode(FsResponse::Empty, out)?;
                commit(backend, prepared, header.deadline)?;
                Ok(used)
            }
            FsRequest::Rename {
                destination_name,
                replace,
                ..
            } => {
                let cap = context.handles.get(1).map_err(failure)?;
                let destination = grants.validate_received(cap).map_err(|e| match e {
                    GrantError::CrossDevice => Status::CrossFilesystem,
                    GrantError::Revoked => Status::Revoked,
                    GrantError::WrongRole => Status::Permission,
                    GrantError::Transport(e) => failure(e),
                })?;
                let prepared = backend.prepare_rename(
                    access,
                    node,
                    name,
                    &destination,
                    destination_name,
                    replace,
                )?;
                let used = encode(FsResponse::Empty, out)?;
                commit(backend, prepared, header.deadline)?;
                Ok(used)
            }
            // 流/订阅/派生的任务接线属于 P2 后续；Sync 的内存 provider 合同为 Unsupported。
            _ => Err(Status::Unsupported),
        }
    })();
    match result {
        Ok(n) => Ok(n),
        Err(status) => {
            *export = None;
            encode(FsResponse::Error(status), out)
        }
    }
}
fn append_final(mut boundary: Resolved, name: &str) -> Result<Resolved, Status> {
    let remaining = match &mut boundary {
        Resolved::Provider { remaining, .. } | Resolved::Link { remaining, .. } => remaining,
        Resolved::Local(_) => return Err(Status::Internal),
    };
    let extra = name
        .len()
        .checked_add(usize::from(!remaining.is_empty()))
        .ok_or(Status::Invalid)?;
    remaining
        .try_reserve_exact(extra)
        .map_err(|_| Status::Resource)?;
    if !remaining.is_empty() {
        remaining.push('/');
    }
    remaining.push_str(name);
    Ok(boundary)
}
