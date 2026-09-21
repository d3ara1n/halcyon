//! srv_fs 验收 provider 的长期 Runtime 装配。

use crate::watch::{self, ControlError, Effect, Effects};
use alloc::vec::Vec;
use alloc::{rc::Rc, string::String};
use erhino_shared::{
    call::SystemCallError,
    message::PAYLOAD_MAX,
    object::{Handle, HandleRole, ObjectSignals, Rights},
    time::Deadline,
};
use libbudget::{Budget, Charge, Taxonomy};
use libexecution::{
    ExecutionResource,
    runtime::{
        Advance, DriveState, Input, RequestFailure, Requests, Runtime, SourceId, SourceKind, Step,
        Task,
    },
    wake::NotificationWake,
};
use libfal::{
    authority::{AccessSnapshot, FalRights},
    backend::{BackendError, Body, CommitResult, MemoryBackend, PreparedTake},
    bytes::Writer,
    data::Data,
    grant::{GrantObserver, GrantTable, Issuance, PreparedGrant},
    node::{NodeKind, validate_path},
    protocol,
    resource::FalResource,
    route,
    store::{NodeId, NodeRef},
    value::{ExportPolicy, Protocol as ValueProtocol, StoredHandle, StoredValue, TakenValue},
};
use librpc::dispatcher::{Completion, Dispatcher};
use librpc::{CallCause, CallError, Outbox, OutboxResult, Request as RpcRequest, RequestContext};
use rinlib::ipc::{
    capability::Capability,
    message::{Mailbox, MailboxSender, MessageStorage, ReceiveBuffer},
    notification,
    object::duplicate,
    packet::Packet,
    wait::wait_until,
    wait_set::WaitSet,
};

const KIND_MAILBOX: SourceKind = 1;
const KIND_REPLY: SourceKind = 1;
const KIND_GRANT_LIFETIME: SourceKind = 2;
const KIND_RETIRE: SourceKind = 3;
const KIND_ROUTE: SourceKind = 4;
const KIND_RELEASE: SourceKind = 5;
const KIND_WATCH_OWNER: SourceKind = 6;
const WATCH_LIMIT: usize = watch::LIMIT;
const RETIRE_BIT: u64 = 1;
const DISPATCH_LIMIT: usize = 8;

struct RouteBinding {
    name: String,
    target: MailboxSender,
    rights: FalRights,
}

struct DispatchSubmission {
    waiter: u64,
    service: Capability,
    deadline: Deadline,
    request: RpcRequest,
}

struct World {
    mailbox: Mailbox,
    route_mailbox: Handle,
    route: Option<RouteBinding>,
    dispatch_submission: Option<DispatchSubmission>,
    dispatcher_task: u64,
    backend: Option<MemoryBackend<Capability>>,
    grants: Option<GrantTable>,
    root_sender: Option<MailboxSender>,
    retire_task: u64,
    retire_ready: bool,
    backend_sealed: bool,
    committed: u64,
    abandoned: u64,
    downstream_abandoned: u64,
    watches: watch::Table,
    stop: bool,
    failed: bool,
}

struct WatchOwner {
    id: u64,
    context: u64,
    node: NodeRef,
    access: AccessSnapshot,
    path: String,
    mask: protocol::WatchMask,
    signaler: Option<Capability>,
    _watch_charge: Charge,
    _source_charge: Charge,
}

struct WatchTask {
    owner: WatchOwner,
    outbox: Option<Outbox>,
    info: Option<protocol::SubscriptionInfo>,
    task_id: Option<u64>,
    source: Option<SourceId>,
    requested: bool,
    removing: bool,
    encoded: bool,
    recorded: bool,
    stopping: bool,
    provider_stopping: bool,
    registration_error: Option<protocol::Status>,
}

impl WatchTask {
    fn new(owner: WatchOwner, outbox: Outbox) -> Self {
        Self {
            owner,
            outbox: Some(outbox),
            info: None,
            task_id: None,
            source: None,
            requested: false,
            removing: false,
            encoded: false,
            recorded: false,
            stopping: false,
            provider_stopping: false,
            registration_error: None,
        }
    }

    fn record_outbox(&mut self, world: &mut World) -> Result<(), SystemCallError> {
        let Some(outbox) = self.outbox.as_ref() else {
            return Ok(());
        };
        if !outbox.is_complete() || self.recorded {
            return Ok(());
        }
        world.committed = world
            .committed
            .checked_add(1)
            .ok_or(SystemCallError::ReachLimit)?;
        if let Some(OutboxResult::Abandoned(cause)) = outbox.result() {
            world.abandoned = world
                .abandoned
                .checked_add(1)
                .ok_or(SystemCallError::ReachLimit)?;
            rinlib::debug!(
                "fs: Subscribe response abandoned after install: {:?}",
                cause
            );
        }
        self.recorded = true;
        Ok(())
    }

    fn encode_reply(&mut self) -> Result<(), SystemCallError> {
        if self.encoded {
            return Ok(());
        }
        let outbox = self.outbox.as_mut().expect("Watch response owner missing");
        let status = self.registration_error.unwrap_or(protocol::Status::Ok);
        let response = self
            .info
            .map(protocol::Response::Subscription)
            .unwrap_or(protocol::Response::Empty);
        let used = protocol::encode_response(
            protocol::Op::Subscribe,
            status,
            outbox.deadline(),
            &response,
            outbox.response_mut()?.body_mut()?,
        )
        .ok_or(SystemCallError::InternalError)?;
        outbox.response_mut()?.finish_body(used)?;
        self.encoded = true;
        Ok(())
    }

    fn close_signaler(&mut self) -> Result<(), SystemCallError> {
        let Some(signaler) = self.owner.signaler.take() else {
            return Ok(());
        };
        signaler.close().map_err(|(_, error)| error)
    }
}

impl Task<World> for WatchTask {
    type Family = ServiceTask;

    fn advance(
        &mut self,
        id: u64,
        world: &mut World,
        requests: &mut Requests<ServiceTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        self.task_id.get_or_insert(id);
        while let Some(event) = input.pull() {
            if event.kind == KIND_REPLY {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.observe(event);
                }
            } else if event.kind == KIND_WATCH_OWNER
                && (event.error != 0 || event.observed.intersects(ObjectSignals::CLOSED))
            {
                self.stopping = true;
            }
        }
        if input.take_timeout()
            && let Some(outbox) = self.outbox.as_mut()
        {
            outbox.timed_out();
        }

        if !self.stopping && self.info.is_none() && self.registration_error.is_none() {
            if self.source.is_none() && !self.requested {
                requests.add_source(
                    self.owner
                        .signaler
                        .as_ref()
                        .expect("Watch signaler missing before registration")
                        .as_handle(),
                    ObjectSignals::CLOSED,
                    KIND_WATCH_OWNER,
                )?;
                self.requested = true;
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }

        if !self.stopping && self.info.is_some() && !world.watches.contains(self.owner.id) {
            self.stopping = true;
        }

        if self.stopping {
            if world.watches.contains(self.owner.id) {
                if self.provider_stopping {
                    let mut bits = world
                        .watches
                        .take_pending(self.owner.id)
                        .map_or(protocol::WatchMask::NONE, |(pending, _)| pending);
                    bits |= protocol::WatchMask::TERMINATED;
                    match notification::signal(
                        self.owner
                            .signaler
                            .as_ref()
                            .expect("installed Watch lost its signaler")
                            .as_handle(),
                        bits.raw(),
                    ) {
                        Ok(()) | Err(SystemCallError::ObjectClosed) => {}
                        Err(error) => return Err(error),
                    }
                }
                world.watches.remove(self.owner.id);
            }
            if let Some(outbox) = self.outbox.as_mut() {
                if !outbox.is_complete() {
                    outbox.stop(world);
                    let advance = outbox.drive(requests, budget)?;
                    if !outbox.is_complete() {
                        return Ok(advance);
                    }
                }
                self.record_outbox(world)?;
                self.outbox = None;
            }
            if let Some(source) = self.source
                && !self.removing
            {
                requests.remove(source)?;
                self.removing = true;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                });
            }
            if self.source.is_none() && !self.requested {
                self.close_signaler()?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }

        if let Some((pending, _)) = world.watches.take_pending(self.owner.id)
            && !pending.is_empty()
        {
            match notification::signal(
                self.owner
                    .signaler
                    .as_ref()
                    .expect("installed Watch lost its signaler")
                    .as_handle(),
                pending.raw(),
            ) {
                Ok(()) => {}
                Err(SystemCallError::ObjectClosed) => {
                    self.stopping = true;
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Runnable,
                    });
                }
                Err(error) => return Err(error),
            }
        }

        if self.outbox.is_some() {
            if self
                .outbox
                .as_ref()
                .is_some_and(|outbox| !outbox.is_admitted() && outbox.result().is_none())
            {
                return Ok(self
                    .outbox
                    .as_mut()
                    .expect("Watch response owner missing")
                    .admit(requests));
            }
            if !self.encoded
                && self
                    .outbox
                    .as_ref()
                    .is_some_and(|outbox| outbox.result().is_none())
            {
                self.encode_reply()?;
            }
            let advance = self
                .outbox
                .as_mut()
                .expect("Watch response owner missing")
                .drive(requests, budget)?;
            if self.outbox.as_ref().is_some_and(Outbox::is_complete) {
                let abandoned = matches!(
                    self.outbox.as_ref().and_then(Outbox::result),
                    Some(OutboxResult::Abandoned(_))
                );
                self.record_outbox(world)?;
                self.outbox = None;
                if abandoned || self.registration_error.is_some() {
                    self.stopping = true;
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Runnable,
                    });
                }
            } else {
                return Ok(advance);
            }
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Parked,
        })
    }

    fn refused(&mut self, world: &mut World, failure: RequestFailure<ServiceTask>) {
        match failure {
            RequestFailure::Source {
                kind: KIND_WATCH_OWNER,
                error,
            } => {
                self.requested = false;
                self.registration_error = Some(match error {
                    SystemCallError::ObjectClosed | SystemCallError::StaleHandle => {
                        protocol::Status::Cancelled
                    }
                    SystemCallError::QuotaExceeded => protocol::Status::Quota,
                    SystemCallError::OutOfMemory => protocol::Status::Resource,
                    _ => protocol::Status::Internal,
                });
            }
            failure => {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.refused(world, failure);
                }
            }
        }
    }

    fn registered(&mut self, world: &mut World, kind: SourceKind, source: SourceId) {
        if kind == KIND_REPLY {
            if let Some(outbox) = self.outbox.as_mut() {
                outbox.registered(world, kind, source);
            }
            return;
        }
        if kind != KIND_WATCH_OWNER {
            return;
        }
        self.requested = false;
        self.source = Some(source);
        let installation = (|| {
            let backend = world
                .backend
                .as_ref()
                .expect("provider backend missing during Watch installation");
            let current = resolve_v2(backend, &self.owner.access, &self.owner.path)
                .map_err(backend_status)?;
            if current.id() != self.owner.node.id() {
                return Err(protocol::Status::Conflict);
            }
            let node = backend.get(&current).ok_or(protocol::Status::NotFound)?;
            if !self
                .owner
                .access
                .rights()
                .intersect(node.rights())
                .contains(FalRights::WATCH)
            {
                return Err(protocol::Status::Permission);
            }
            Ok(node.version())
        })();
        match installation {
            Ok(generation) => {
                let info = world.watches.install(
                    self.owner.id,
                    self.owner.context,
                    self.owner.node.id(),
                    self.task_id.expect("Watch task id missing at installation"),
                    generation,
                    self.owner.mask,
                );
                self.info = Some(info);
            }
            Err(status) => self.registration_error = Some(status),
        }
    }

    fn unregistered(&mut self, world: &mut World, kind: SourceKind, source: SourceId) {
        if kind == KIND_REPLY {
            if let Some(outbox) = self.outbox.as_mut() {
                outbox.unregistered(world, kind, source);
            }
        } else if kind == KIND_WATCH_OWNER && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.removing = false;
        }
    }

    fn stop(&mut self, _world: &mut World) {
        self.provider_stopping = true;
        self.stopping = true;
    }

    fn deadline(&self) -> Deadline {
        self.outbox
            .as_ref()
            .map_or(Deadline::INFINITE, Outbox::deadline)
    }
}

struct GrantTask {
    prepared: Option<PreparedGrant>,
    observer: Option<GrantObserver>,
    publication: GrantPublication,
    outbox: Option<Outbox>,
    source: Option<SourceId>,
    requested: bool,
    removing: bool,
    stopping: bool,
}

enum GrantPublication {
    Root,
    Reply {
        info: protocol::NodeInfo,
        sender: Option<MailboxSender>,
        encoded: bool,
    },
}

impl GrantTask {
    fn root(prepared: PreparedGrant) -> Self {
        Self {
            prepared: Some(prepared),
            observer: None,
            publication: GrantPublication::Root,
            outbox: None,
            source: None,
            requested: false,
            removing: false,
            stopping: false,
        }
    }

    fn reply(prepared: PreparedGrant, outbox: Outbox, info: protocol::NodeInfo) -> Self {
        Self {
            prepared: Some(prepared),
            observer: None,
            publication: GrantPublication::Reply {
                info,
                sender: None,
                encoded: false,
            },
            outbox: Some(outbox),
            source: None,
            requested: false,
            removing: false,
            stopping: false,
        }
    }

    fn drive_publication(
        &mut self,
        requests: &mut Requests<ServiceTask>,
        budget: usize,
    ) -> Result<Option<Advance>, SystemCallError> {
        let GrantPublication::Reply {
            info,
            sender,
            encoded,
        } = &mut self.publication
        else {
            return Ok(None);
        };
        let Some(active) = self.outbox.as_mut() else {
            return Ok(None);
        };
        if active.result().is_none() && !active.is_admitted() {
            return Ok(Some(active.admit(requests)));
        }
        if active.result().is_none() && !*encoded {
            let deadline = active.deadline();
            let response = active.response_mut()?;
            let used = protocol::encode_response(
                protocol::Op::Derive,
                protocol::Status::Ok,
                deadline,
                &protocol::Response::Node(*info),
                response.body_mut()?,
            )
            .ok_or(SystemCallError::InternalError)?;
            let grant = sender
                .take()
                .expect("derived grant installation lost its sender");
            response
                .push(
                    grant.into_capability(),
                    Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
                )
                .map_err(|failure| failure.error)?;
            response.finish_body(used)?;
            *encoded = true;
        }
        let advance = active.drive(requests, budget)?;
        if active.is_complete() {
            let abandoned = !matches!(active.result(), Some(OutboxResult::Sent));
            self.outbox = None;
            if abandoned {
                *sender = None;
                self.stopping = true;
            }
            return Ok(Some(Advance {
                work_done: advance.work_done,
                step: if self.stopping {
                    Step::Runnable
                } else {
                    Step::Parked
                },
            }));
        }
        Ok(Some(advance))
    }
}

impl Task<World> for GrantTask {
    type Family = ServiceTask;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World,
        requests: &mut Requests<ServiceTask>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            if event.kind == KIND_REPLY {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.observe(event);
                }
            } else if event.kind == KIND_GRANT_LIFETIME
                && (event.error != 0 || event.observed.intersects(ObjectSignals::CLOSED))
            {
                self.stopping = true;
            }
        }
        if input.take_timeout()
            && let Some(outbox) = self.outbox.as_mut()
        {
            outbox.timed_out();
        }

        if self.stopping {
            if let GrantPublication::Reply { sender, .. } = &mut self.publication {
                if let Some(active) = self.outbox.as_mut() {
                    active.stop(world);
                    let advance = active.drive(requests, 1)?;
                    if !active.is_complete() {
                        return Ok(advance);
                    }
                }
                self.outbox = None;
                *sender = None;
            }
            if self.prepared.is_some() && !self.requested {
                self.prepared = None;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            if let Some(source) = self.source
                && !self.removing
            {
                requests.remove(source)?;
                self.removing = true;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                });
            }
            if self.source.is_none()
                && !self.requested
                && let Some(observer) = self.observer.take()
            {
                let context = observer.context();
                requests.wake(world.retire_task)?;
                let removed = world
                    .grants
                    .as_mut()
                    .expect("grant table missing during retirement")
                    .remove(context);
                assert!(removed, "retiring grant missing from table");
                match observer.close() {
                    Ok(()) => {
                        return Ok(Advance {
                            work_done: 1,
                            step: Step::Complete,
                        });
                    }
                    Err((observer, error)) => {
                        self.observer = Some(observer);
                        return Err(error);
                    }
                }
            }
        }

        if self.observer.is_some() {
            if let Some(advance) = self.drive_publication(requests, 1)? {
                return Ok(advance);
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        if !world.retire_ready {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if !self.requested {
            let prepared = self
                .prepared
                .as_ref()
                .expect("grant installation lost its prepared owner");
            requests.add_source(
                prepared.lifetime_handle(),
                ObjectSignals::CLOSED,
                KIND_GRANT_LIFETIME,
            )?;
            self.requested = true;
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Runnable,
        })
    }

    fn refused(&mut self, world: &mut World, failure: RequestFailure<ServiceTask>) {
        match failure {
            RequestFailure::Source {
                kind: KIND_GRANT_LIFETIME,
                ..
            } => {
                self.requested = false;
                self.stopping = true;
                if !world.stop {
                    world.failed = true;
                }
            }
            failure => {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.refused(world, failure);
                }
            }
        }
    }

    fn registered(&mut self, world: &mut World, kind: SourceKind, source: SourceId) {
        if kind == KIND_REPLY {
            if let Some(outbox) = self.outbox.as_mut() {
                outbox.registered(world, kind, source);
            }
            return;
        }
        if kind != KIND_GRANT_LIFETIME {
            return;
        }
        self.requested = false;
        self.source = Some(source);
        let prepared = self
            .prepared
            .take()
            .expect("grant source registered without prepared owner");
        let installed = world
            .grants
            .as_mut()
            .expect("grant table missing during installation")
            .install(prepared);
        match &mut self.publication {
            GrantPublication::Root => assert!(
                world.root_sender.replace(installed.sender).is_none(),
                "root grant published more than once"
            ),
            GrantPublication::Reply { sender, .. } => {
                assert!(
                    sender.replace(installed.sender).is_none(),
                    "derived grant sender published more than once"
                );
            }
        }
        self.observer = Some(installed.observer);
    }

    fn unregistered(&mut self, _world: &mut World, kind: SourceKind, source: SourceId) {
        if kind == KIND_REPLY {
            if let Some(outbox) = self.outbox.as_mut() {
                outbox.unregistered(_world, kind, source);
            }
        } else if kind == KIND_GRANT_LIFETIME && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.removing = false;
        }
    }

    fn stop(&mut self, _world: &mut World) {
        self.stopping = true;
    }

    fn deadline(&self) -> Deadline {
        match &self.publication {
            GrantPublication::Root => Deadline::INFINITE,
            GrantPublication::Reply { .. } => self
                .outbox
                .as_ref()
                .map_or(Deadline::INFINITE, Outbox::deadline),
        }
    }
}

struct RetireTask {
    owner: Capability,
    source: Option<SourceId>,
    requested: bool,
    removing: bool,
    rearm_needed: bool,
    stopping: bool,
}

impl RetireTask {
    fn new(owner: Capability) -> Self {
        Self {
            owner,
            source: None,
            requested: false,
            removing: false,
            rearm_needed: false,
            stopping: false,
        }
    }
}

impl Task<World> for RetireTask {
    type Family = ServiceTask;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World,
        requests: &mut Requests<ServiceTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        let mut signaled = false;
        while let Some(event) = input.pull() {
            if event.kind != KIND_RETIRE {
                continue;
            }
            if event.error != 0 || event.observed.intersects(ObjectSignals::CLOSED) {
                world.failed = true;
                self.stopping = true;
            } else if event.observed.intersects(ObjectSignals::READABLE) {
                signaled = true;
                self.rearm_needed = true;
            }
        }
        if signaled {
            let _ = notification::take(self.owner.as_handle(), RETIRE_BIT)?;
        }

        if self.stopping
            && world
                .grants
                .as_ref()
                .expect("grant table missing during shutdown")
                .is_empty()
            && !world.backend_sealed
        {
            rinlib::debug!("fs provider shutdown: sealing backend");
            world
                .backend
                .as_mut()
                .expect("backend missing during shutdown")
                .seal();
            world.backend_sealed = true;
        }

        let backend = world
            .backend
            .as_mut()
            .expect("backend missing during retirement");
        if backend.has_retire_work() {
            backend.retire_step(budget)?;
            return Ok(Advance {
                work_done: budget,
                step: Step::Runnable,
            });
        }

        if self.stopping && world.backend_sealed && backend.is_empty() {
            if let Some(source) = self.source
                && !self.removing
            {
                rinlib::debug!("fs provider shutdown: removing retirement source");
                requests.remove(source)?;
                self.removing = true;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                });
            }
            if self.source.is_none() && !self.requested {
                let backend = world
                    .backend
                    .take()
                    .expect("backend disappeared before close");
                if let Err(backend) = backend.close() {
                    world.backend = Some(backend);
                    return Err(SystemCallError::ObjectBusy);
                }
                rinlib::debug!("fs provider shutdown: backend retired");
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
        }

        if self.source.is_none() && !self.requested {
            requests.add_source(
                self.owner.as_handle(),
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                KIND_RETIRE,
            )?;
            self.requested = true;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if self.rearm_needed
            && let Some(source) = self.source
        {
            requests.rearm(source)?;
            self.rearm_needed = false;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Parked,
        })
    }

    fn refused(&mut self, world: &mut World, failure: RequestFailure<ServiceTask>) {
        if let RequestFailure::Source {
            kind: KIND_RETIRE, ..
        } = failure
        {
            self.requested = false;
            if !world.stop {
                world.failed = true;
            }
        }
    }

    fn registered(&mut self, world: &mut World, kind: SourceKind, source: SourceId) {
        if kind == KIND_RETIRE {
            self.requested = false;
            self.source = Some(source);
            world.retire_ready = true;
        }
    }

    fn unregistered(&mut self, _world: &mut World, kind: SourceKind, source: SourceId) {
        if kind == KIND_RETIRE && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.removing = false;
            self.rearm_needed = false;
        }
    }

    fn stop(&mut self, _world: &mut World) {
        self.stopping = true;
    }
}

struct ReleaseTask {
    owner: Option<Capability>,
    source: Option<SourceId>,
    requested: bool,
    removing: bool,
    released: bool,
}

impl ReleaseTask {
    fn new(owner: Capability) -> Self {
        Self {
            owner: Some(owner),
            source: None,
            requested: false,
            removing: false,
            released: false,
        }
    }
}

impl Task<World> for ReleaseTask {
    type Family = ServiceTask;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World,
        requests: &mut Requests<ServiceTask>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            if event.error != 0 {
                world.failed = true;
            } else if event.observed.intersects(ObjectSignals::READABLE) {
                let owner = self.owner.as_ref().expect("release source lost its owner");
                notification::take(owner.as_handle(), 1)?;
                self.released = true;
                world.stop = true;
            } else if event.observed.intersects(ObjectSignals::CLOSED) {
                world.failed = true;
            }
        }
        if self.released {
            if let Some(source) = self.source
                && !self.removing
            {
                requests.remove(source)?;
                self.removing = true;
            }
            if self.source.is_none() && !self.requested {
                let owner = self.owner.take().expect("release owner already closed");
                owner.close().map_err(|(_, error)| error)?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        if self.source.is_none() && !self.requested {
            requests.add_source(
                self.owner
                    .as_ref()
                    .expect("release task lost its owner")
                    .as_handle(),
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                KIND_RELEASE,
            )?;
            self.requested = true;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Parked,
        })
    }

    fn refused(&mut self, world: &mut World, _failure: RequestFailure<ServiceTask>) {
        world.failed = true;
    }

    fn registered(&mut self, _world: &mut World, kind: SourceKind, source: SourceId) {
        if kind == KIND_RELEASE {
            self.requested = false;
            self.source = Some(source);
        }
    }

    fn unregistered(&mut self, _world: &mut World, kind: SourceKind, source: SourceId) {
        if kind == KIND_RELEASE && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.removing = false;
        }
    }

    fn stop(&mut self, _world: &mut World) {
        self.released = true;
    }
}

struct RouteIngress {
    buffer: ReceiveBuffer,
    source: Option<SourceId>,
    requested: bool,
    removing: bool,
    stopping: bool,
}

impl RouteIngress {
    fn new() -> Self {
        Self {
            buffer: ReceiveBuffer::new().expect("route receive buffer creation failed"),
            source: None,
            requested: false,
            removing: false,
            stopping: false,
        }
    }
}

impl Task<World> for RouteIngress {
    type Family = ServiceTask;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World,
        requests: &mut Requests<ServiceTask>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if self.stopping {
            if let Some(source) = self.source
                && !self.removing
            {
                requests.remove(source)?;
                self.removing = true;
            }
            return Ok(Advance {
                work_done: 1,
                step: if self.source.is_none() && !self.requested {
                    Step::Complete
                } else {
                    Step::Parked
                },
            });
        }
        if self.source.is_none() && !self.requested {
            requests.add_source(
                world.route_mailbox,
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                KIND_ROUTE,
            )?;
            self.requested = true;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        let mut ready = false;
        while let Some(event) = input.pull() {
            if event.error != 0 || event.observed.intersects(ObjectSignals::CLOSED) {
                self.stopping = true;
            } else {
                ready = true;
            }
        }
        if self.stopping {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if !ready {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        match self.buffer.receive(world.route_mailbox) {
            Ok(()) => {}
            Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => {
                requests.rearm(self.source.expect("route source remains registered"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            Err(error) => return Err(error),
        }
        let message = MessageStorage::new()?.take(&mut self.buffer)?;
        let mut context = match RequestContext::decode(message, route::ID) {
            Ok(context) => context,
            Err(_) => {
                requests.rearm(self.source.expect("route source remains registered"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
        };
        let mut status = route::Status::Invalid;
        if context.handles.remaining() == 1
            && let Ok(binding) = route::Bind::decode(&context.payload)
            && binding.rights.contains(FalRights::TRAVERSE)
            && let Ok(capability) = context.handles.get(1)
        {
            let rights = Rights::WRITE | Rights::WAIT | Rights::DUPLICATE;
            if let Ok(description) = capability.description()
                && description.rights.contains(rights)
            {
                let capability = context
                    .handles
                    .take(1)
                    .expect("validated route target disappeared");
                match MailboxSender::from_capability(capability) {
                    Ok((target, _)) => {
                        world.route = Some(RouteBinding {
                            name: String::from(binding.name),
                            target,
                            rights: binding.rights,
                        });
                        status = route::Status::Ok;
                    }
                    Err(_) => status = route::Status::Invalid,
                }
            } else {
                status = route::Status::Permission;
            }
        }
        let outbox = Outbox::prepare(context, route::RESPONSE_LEN, Deadline::INFINITE, KIND_REPLY)
            .map_err(|_| SystemCallError::InternalError)?;
        requests
            .spawn(
                ServiceTask::Request(RequestTask {
                    outbox,
                    mode: RequestMode::RouteAck(status),
                    committed: false,
                    recorded: false,
                }),
                1,
            )
            .map_err(|_| SystemCallError::ReachLimit)?;
        requests.rearm(self.source.expect("route source remains registered"))?;
        Ok(Advance {
            work_done: 1,
            step: Step::Parked,
        })
    }

    fn refused(&mut self, world: &mut World, _failure: RequestFailure<ServiceTask>) {
        world.failed = true;
    }

    fn registered(&mut self, _world: &mut World, kind: SourceKind, source: SourceId) {
        if kind == KIND_ROUTE {
            self.requested = false;
            self.source = Some(source);
        }
    }

    fn unregistered(&mut self, _world: &mut World, kind: SourceKind, source: SourceId) {
        if kind == KIND_ROUTE && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.removing = false;
        }
    }

    fn stop(&mut self, _world: &mut World) {
        self.stopping = true;
    }
}

struct Ingress {
    buffer: ReceiveBuffer,
    source: Option<SourceId>,
    requested: bool,
    removing: bool,
    stopping: bool,
}

impl Ingress {
    fn new() -> Self {
        Self {
            buffer: ReceiveBuffer::new().expect("provider receive buffer creation failed"),
            source: None,
            requested: false,
            removing: false,
            stopping: false,
        }
    }
}

impl Task<World> for Ingress {
    type Family = ServiceTask;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World,
        requests: &mut Requests<ServiceTask>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if self.stopping {
            if let Some(source) = self.source
                && !self.removing
            {
                requests.remove(source)?;
                self.removing = true;
            }
            return Ok(Advance {
                work_done: 1,
                step: if self.source.is_none() && !self.requested {
                    Step::Complete
                } else {
                    Step::Parked
                },
            });
        }
        if self.source.is_none() && !self.requested {
            requests.add_source(
                world.mailbox.as_handle(),
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                KIND_MAILBOX,
            )?;
            self.requested = true;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        let mut ready = false;
        while let Some(event) = input.pull() {
            if event.error != 0 || event.observed.intersects(ObjectSignals::CLOSED) {
                world.failed = true;
                self.stopping = true;
            } else {
                ready = true;
            }
        }
        if self.stopping {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if !ready {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        match self.buffer.receive(world.mailbox.as_handle()) {
            Ok(()) => {}
            Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => {
                requests.rearm(self.source.expect("provider source remains registered"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            Err(error) => return Err(error),
        }
        let message = MessageStorage::new()?.take(&mut self.buffer)?;
        let mut context = match RequestContext::decode(message, protocol::ID) {
            Ok(context) => context,
            Err(_) => {
                requests.rearm(self.source.expect("provider source remains registered"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
        };
        let parsed = protocol::decode_request(&context.payload);
        let (header, request) = match parsed {
            Ok((header, request)) => {
                let expected_handles = match request {
                    protocol::Request::Move { .. } | protocol::Request::Subscribe { .. } => Some(1),
                    protocol::Request::Create {
                        kind: NodeKind::Property,
                        ..
                    }
                    | protocol::Request::Write { .. } => None,
                    _ => Some(0),
                };
                if expected_handles.is_none()
                    || context.handles.remaining() == expected_handles.unwrap()
                {
                    (header, request)
                } else {
                    requests.rearm(self.source.expect("provider source remains registered"))?;
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Parked,
                    });
                }
            }
            Err(_) => {
                requests.rearm(self.source.expect("provider source remains registered"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
        };
        let Some(access) = world
            .grants
            .as_ref()
            .expect("grant table missing during request admission")
            .snapshot(context.envelope.sender_context_id)
        else {
            requests.rearm(self.source.expect("provider source remains registered"))?;
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        };
        let sender_context = context.envelope.sender_context_id;
        let mut watch_failure = None;
        let watch_owner = if let protocol::Request::Subscribe { path, mask } = request {
            let prepared = (|| {
                let capability = context
                    .handles
                    .take(1)
                    .map_err(|_| protocol::Status::Invalid)?;
                let description = capability
                    .description()
                    .map_err(|_| protocol::Status::Internal)?;
                let required = Rights::SIGNAL | Rights::WAIT | Rights::TRANSIT;
                if description.role != HandleRole::NotificationSignaler as u32
                    || !description.rights.contains(required)
                {
                    return Err(protocol::Status::Invalid);
                }
                let backend = world
                    .backend
                    .as_ref()
                    .expect("provider backend missing during Watch preparation");
                let node = resolve_v2(backend, &access, path).map_err(backend_status)?;
                let state = backend.get(&node).ok_or(protocol::Status::NotFound)?;
                if !access
                    .rights()
                    .intersect(state.rights())
                    .contains(FalRights::WATCH)
                {
                    return Err(protocol::Status::Permission);
                }
                let mut watch_path = String::new();
                watch_path
                    .try_reserve_exact(path.len())
                    .map_err(|_| protocol::Status::Resource)?;
                watch_path.push_str(path);
                let watch_charge = access.account().acquire(FalResource::Watch, 1).map_err(
                    |error| match error {
                        SystemCallError::QuotaExceeded => protocol::Status::Quota,
                        SystemCallError::OutOfMemory => protocol::Status::Resource,
                        _ => protocol::Status::Internal,
                    },
                )?;
                let source_charge = access
                    .account()
                    .acquire(FalResource::WaitSource, 1)
                    .map_err(|error| match error {
                        SystemCallError::QuotaExceeded => protocol::Status::Quota,
                        SystemCallError::OutOfMemory => protocol::Status::Resource,
                        _ => protocol::Status::Internal,
                    })?;
                let id = world
                    .watches
                    .allocate_id()
                    .ok_or(protocol::Status::Resource)?;
                Ok(WatchOwner {
                    id,
                    context: sender_context,
                    node,
                    access: access.clone(),
                    path: watch_path,
                    mask,
                    signaler: Some(capability),
                    _watch_charge: watch_charge,
                    _source_charge: source_charge,
                })
            })();
            match prepared {
                Ok(owner) => Some(owner),
                Err(status) => {
                    watch_failure = Some(status);
                    None
                }
            }
        } else {
            None
        };
        let mut move_failure = None;
        let move_destination = if matches!(request, protocol::Request::Move { .. }) {
            let capability = context
                .handles
                .take(1)
                .map_err(|_| SystemCallError::IllegalArgument)?;
            match world
                .grants
                .as_ref()
                .expect("grant table missing during request admission")
                .validate_received(&capability)
            {
                Ok(destination) => Some(destination),
                Err(error) => {
                    move_failure = Some(match error {
                        libfal::grant::GrantError::CrossDevice => protocol::Status::CrossDevice,
                        libfal::grant::GrantError::Revoked => protocol::Status::GrantRevoked,
                        libfal::grant::GrantError::WrongRole => protocol::Status::Invalid,
                        libfal::grant::GrantError::Transport(_) => protocol::Status::Internal,
                    });
                    None
                }
            }
        } else {
            None
        };
        let mut delegate_failure = None;
        let delegated = if let protocol::Request::Lookup { path } = request {
            match world.route.as_ref() {
                Some(binding) => match prepare_delegate(binding, &access, path, header) {
                    Ok(delegate) => delegate,
                    Err(status) => {
                        delegate_failure = Some(status);
                        None
                    }
                },
                None => None,
            }
        } else {
            None
        };
        let mut derive_failure = None;
        let derived = if let protocol::Request::Derive { path, rights } = request {
            let prepared = (|| {
                if !access.rights().contains(rights) {
                    return Err(protocol::Status::Permission);
                }
                let backend = world
                    .backend
                    .as_ref()
                    .expect("provider backend missing during grant derivation");
                let root = resolve_v2(backend, &access, path).map_err(backend_status)?;
                let info = node_info_v2(backend, &root, rights).map_err(backend_status)?;
                if info.kind != NodeKind::Directory {
                    return Err(protocol::Status::NotDirectory);
                }
                let prepared = world
                    .grants
                    .as_mut()
                    .expect("grant table missing during derivation")
                    .prepare_derive(
                        &world.mailbox,
                        context.envelope.sender_context_id,
                        root,
                        rights,
                        context.envelope.sender_context_id,
                    )
                    .map_err(|failure| match failure.error {
                        SystemCallError::RightsDenied => protocol::Status::Permission,
                        SystemCallError::QuotaExceeded => protocol::Status::Quota,
                        SystemCallError::OutOfMemory => protocol::Status::Resource,
                        SystemCallError::ObjectClosed => protocol::Status::GrantRevoked,
                        _ => protocol::Status::Internal,
                    })?;
                Ok((prepared, info))
            })();
            match prepared {
                Ok(prepared) => Some(prepared),
                Err(status) => {
                    derive_failure = Some(status);
                    None
                }
            }
        } else {
            None
        };
        let move_operation = if let protocol::Request::Move {
            source_parent,
            source_name,
            destination_name,
            expected,
        } = request
        {
            let expected = if expected.identity == 0 {
                if expected.version != 0 {
                    move_failure = Some(protocol::Status::Invalid);
                    None
                } else {
                    None
                }
            } else {
                match NodeId::from_raw(expected.identity) {
                    Some(identity) => Some((identity, expected.version)),
                    None => {
                        move_failure = Some(protocol::Status::Invalid);
                        None
                    }
                }
            };
            match move_destination {
                Some(destination) if move_failure.is_none() => Some(MoveOperation {
                    access: access.clone(),
                    destination,
                    header,
                    source_parent: String::from(source_parent),
                    source_name: String::from(source_name),
                    destination_name: String::from(destination_name),
                    expected,
                    prepared: None,
                    source_parent_ref: None,
                    target_ref: None,
                }),
                _ => None,
            }
        } else {
            None
        };
        let take_operation = if let protocol::Request::Take { path } = request {
            Some(TakeOperation {
                access: access.clone(),
                header,
                path: String::from(path),
                prepared: None,
                reference: None,
                bytes: None,
                policies: Vec::new(),
                restore_handles: Vec::new(),
                encoded: false,
                failed: false,
            })
        } else {
            None
        };
        let watch_control = match request {
            protocol::Request::QuerySubscription { id } => Some(WatchControlOperation {
                context: sender_context,
                header,
                kind: WatchControlKind::Query(id),
                encoded: false,
            }),
            protocol::Request::Unsubscribe { id } => Some(WatchControlOperation {
                context: sender_context,
                header,
                kind: WatchControlKind::Unsubscribe(id),
                encoded: false,
            }),
            _ => None,
        };
        let mode = if let Some(status) = watch_failure
            .or(move_failure)
            .or(derive_failure)
            .or(delegate_failure)
        {
            RequestMode::V2Failure { header, status }
        } else if let Some(operation) = move_operation {
            RequestMode::Move(operation)
        } else if let Some(operation) = take_operation {
            RequestMode::Take(operation)
        } else if let Some(operation) = watch_control {
            RequestMode::WatchControl(operation)
        } else {
            RequestMode::V2 { access, header }
        };
        let (mode, deadline, derived, delegated) = (mode, header.deadline, derived, delegated);
        let outbox = match Outbox::prepare(
            context,
            PAYLOAD_MAX - librpc::PREFIX_LEN,
            deadline,
            KIND_REPLY,
        ) {
            Ok(outbox) => outbox,
            Err(_) => {
                requests.rearm(self.source.expect("provider source remains registered"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
        };
        let (task, max_sources) = if let Some(delegate) = delegated {
            (ServiceTask::Delegate(delegate.with_outbox(outbox)), 1)
        } else if let Some((prepared, info)) = derived {
            (
                ServiceTask::Grant(GrantTask::reply(prepared, outbox, info)),
                2,
            )
        } else if let Some(owner) = watch_owner {
            (ServiceTask::Watch(WatchTask::new(owner, outbox)), 2)
        } else {
            (
                ServiceTask::Request(RequestTask {
                    outbox,
                    mode,
                    committed: false,
                    recorded: false,
                }),
                1,
            )
        };
        requests
            .spawn(task, max_sources)
            .map_err(|_| SystemCallError::ReachLimit)?;
        requests.rearm(self.source.expect("provider source remains registered"))?;
        Ok(Advance {
            work_done: 1,
            step: Step::Parked,
        })
    }

    fn refused(&mut self, world: &mut World, failure: RequestFailure<ServiceTask>) {
        match failure {
            RequestFailure::Spawn { .. }
            | RequestFailure::Source { .. }
            | RequestFailure::Wake { .. } => world.failed = true,
        }
    }

    fn registered(&mut self, _world: &mut World, kind: SourceKind, source: SourceId) {
        if kind == KIND_MAILBOX {
            self.requested = false;
            self.source = Some(source);
        }
    }

    fn unregistered(&mut self, _world: &mut World, kind: SourceKind, source: SourceId) {
        if kind == KIND_MAILBOX && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.removing = false;
        }
    }

    fn stop(&mut self, _world: &mut World) {
        self.stopping = true;
    }
}

fn publish_watch_events(
    world: &mut World,
    requests: &mut Requests<ServiceTask>,
    effects: &Effects,
) -> Result<(), SystemCallError> {
    if effects.is_empty() {
        return Ok(());
    }
    let wakes = world.watches.publish(effects);
    for task in wakes.iter() {
        requests.wake(task)?;
    }
    Ok(())
}

fn backend_status(error: BackendError) -> protocol::Status {
    match error {
        BackendError::NotFound => protocol::Status::NotFound,
        BackendError::NotDirectory => protocol::Status::NotDirectory,
        BackendError::Permission => protocol::Status::Permission,
        BackendError::Exists => protocol::Status::Exists,
        BackendError::NotEmpty => protocol::Status::NotEmpty,
        BackendError::Conflict => protocol::Status::Conflict,
        BackendError::InvalidName | BackendError::Cycle => protocol::Status::Invalid,
        BackendError::Busy | BackendError::Closed => protocol::Status::Busy,
        BackendError::Resource(SystemCallError::QuotaExceeded) => protocol::Status::Quota,
        BackendError::Resource(SystemCallError::OutOfMemory) => protocol::Status::Resource,
        BackendError::Resource(_) => protocol::Status::Internal,
    }
}

fn value_status(error: libfal::value::ValueError) -> protocol::Status {
    match error {
        libfal::value::ValueError::Budget
        | libfal::value::ValueError::Encoding
        | libfal::value::ValueError::DuplicateField
        | libfal::value::ValueError::HandleSlots => protocol::Status::Invalid,
        libfal::value::ValueError::Allocation => protocol::Status::Resource,
        libfal::value::ValueError::Affine => protocol::Status::Busy,
        libfal::value::ValueError::Unsupported => protocol::Status::Unsupported,
        libfal::value::ValueError::Capability(SystemCallError::QuotaExceeded) => {
            protocol::Status::Quota
        }
        libfal::value::ValueError::Capability(SystemCallError::OutOfMemory) => {
            protocol::Status::Resource
        }
        libfal::value::ValueError::Capability(_) => protocol::Status::Internal,
    }
}

fn resolve_v2(
    backend: &MemoryBackend<Capability>,
    access: &AccessSnapshot,
    path: &str,
) -> Result<NodeRef, BackendError> {
    if !validate_path(path.as_bytes()) {
        return Err(BackendError::InvalidName);
    }
    let mut current = access.root().clone();
    if path.is_empty() {
        return Ok(current);
    }
    for component in path.split('/') {
        current = backend.lookup_child(&current, component, access)?;
    }
    Ok(current)
}

enum LookupV2 {
    Found(NodeRef),
    Link {
        reference: NodeRef,
        consumed: String,
        target: String,
        remaining: String,
    },
}

fn lookup_v2(
    backend: &MemoryBackend<Capability>,
    access: &AccessSnapshot,
    path: &str,
) -> Result<LookupV2, BackendError> {
    if !validate_path(path.as_bytes()) {
        return Err(BackendError::InvalidName);
    }
    let mut current = access.root().clone();
    if path.is_empty() {
        return Ok(LookupV2::Found(current));
    }
    let components = path.split('/').collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        current = backend.lookup_child(&current, component, access)?;
        let node = backend.get(&current).ok_or(BackendError::NotFound)?;
        if let Body::Link(target) = node.body() {
            return Ok(LookupV2::Link {
                reference: current,
                consumed: components[..index].join("/"),
                target: target.clone(),
                remaining: components[index + 1..].join("/"),
            });
        }
    }
    Ok(LookupV2::Found(current))
}

fn node_info_v2(
    backend: &MemoryBackend<Capability>,
    reference: &NodeRef,
    ceiling: FalRights,
) -> Result<protocol::NodeInfo, BackendError> {
    let node = backend.get(reference).ok_or(BackendError::NotFound)?;
    if node.take_reserved() {
        return Err(BackendError::Busy);
    }
    let size = match node.body() {
        Body::Directory(_) => 0,
        Body::Property(value) => value.bytes.len() as u64,
        Body::Stream(data) => data.len(),
        Body::Link(target) => target.len() as u64,
    };
    Ok(protocol::NodeInfo {
        identity: reference.id().raw(),
        version: node.version(),
        kind: node.kind(),
        rights: node.rights().intersect(ceiling),
        size,
    })
}

fn encode_v2(
    out: &mut [u8],
    op: protocol::Op,
    status: protocol::Status,
    deadline: Deadline,
    response: protocol::Response<'_>,
) -> Result<usize, SystemCallError> {
    protocol::encode_response(op, status, deadline, &response, out)
        .ok_or(SystemCallError::InternalError)
}

fn push_watch_effect(
    backend: &MemoryBackend<Capability>,
    effects: &mut Effects,
    node: &NodeRef,
    events: protocol::WatchMask,
    terminal: Option<protocol::WatchReason>,
) -> Result<(), SystemCallError> {
    let generation = backend
        .get(node)
        .ok_or(SystemCallError::InternalError)?
        .version();
    effects.push(Effect {
        node: node.id(),
        generation,
        events,
        terminal,
    });
    Ok(())
}

fn drive_move(
    backend: &mut MemoryBackend<Capability>,
    operation: &mut MoveOperation,
    out: &mut [u8],
    budget: usize,
    effects: &mut Effects,
) -> Result<Option<usize>, SystemCallError> {
    let mut failure = |status| {
        encode_v2(
            out,
            protocol::Op::Move,
            status,
            operation.header.deadline,
            protocol::Response::Empty,
        )
    };
    if operation.prepared.is_none() {
        let parent = match resolve_v2(backend, &operation.access, &operation.source_parent) {
            Ok(parent) => parent,
            Err(error) => return failure(backend_status(error)).map(Some),
        };
        let target = match backend.lookup_child(&parent, &operation.source_name, &operation.access)
        {
            Ok(target) => target,
            Err(error) => return failure(backend_status(error)).map(Some),
        };
        let source = match backend.position(&parent, &operation.source_name, operation.expected) {
            Ok(source) => source,
            Err(error) => return failure(backend_status(error)).map(Some),
        };
        let mutation = match backend.prepare_move(
            source,
            &operation.access,
            &operation.destination,
            &operation.destination_name,
        ) {
            Ok(mutation) => mutation,
            Err(error) => return failure(backend_status(error)).map(Some),
        };
        operation.source_parent_ref = Some(parent);
        operation.target_ref = Some(target);
        operation.prepared = Some(mutation);
        return Ok(None);
    }

    let checked = match backend.validate_move_step(
        operation
            .prepared
            .as_mut()
            .expect("move mutation exists before validation"),
        budget.max(1),
    ) {
        Ok(checked) => checked,
        Err(error) => {
            operation.prepared.take();
            return failure(backend_status(error)).map(Some);
        }
    };
    if !checked {
        return Ok(None);
    }

    let mutation = operation
        .prepared
        .take()
        .expect("checked move mutation remains owned");
    match backend.commit(mutation) {
        Ok(CommitResult::Moved(_)) => {
            let source = operation
                .source_parent_ref
                .as_ref()
                .expect("committed Move lost its source parent");
            let target = operation
                .target_ref
                .as_ref()
                .expect("committed Move lost its target");
            let destination = operation.destination.root();
            effects.push(Effect {
                node: source.id(),
                generation: backend
                    .get(source)
                    .expect("committed Move source parent disappeared")
                    .version(),
                events: protocol::WatchMask::RENAME,
                terminal: None,
            });
            effects.push(Effect {
                node: destination.id(),
                generation: backend
                    .get(destination)
                    .expect("committed Move destination parent disappeared")
                    .version(),
                events: protocol::WatchMask::RENAME,
                terminal: None,
            });
            effects.push(Effect {
                node: target.id(),
                generation: backend
                    .get(target)
                    .expect("committed Move target disappeared")
                    .version(),
                events: protocol::WatchMask::RENAME,
                terminal: None,
            });
            failure(protocol::Status::Ok).map(Some)
        }
        Ok(_) => failure(protocol::Status::Internal).map(Some),
        Err(commit) => failure(backend_status(commit.error)).map(Some),
    }
}

struct ServeTransfers<'a> {
    input: &'a mut Vec<Capability>,
    output: &'a mut Vec<(Capability, Rights)>,
    effects: &'a mut Effects,
}

fn serve_v2(
    backend: &mut MemoryBackend<Capability>,
    access: &AccessSnapshot,
    expected: protocol::Header,
    payload: &[u8],
    transfers: &mut ServeTransfers<'_>,
    out: &mut [u8],
) -> Result<usize, SystemCallError> {
    let (header, request) =
        protocol::decode_request(payload).map_err(|_| SystemCallError::IllegalArgument)?;
    if header != expected {
        return Err(SystemCallError::IllegalArgument);
    }
    let failure = |out: &mut [u8], status| {
        encode_v2(
            out,
            header.op,
            status,
            header.deadline,
            protocol::Response::Empty,
        )
    };
    match request {
        protocol::Request::Lookup { path } => {
            let lookup = match lookup_v2(backend, access, path) {
                Ok(lookup) => lookup,
                Err(error) => return failure(out, backend_status(error)),
            };
            match lookup {
                LookupV2::Found(reference) => {
                    let info = match node_info_v2(backend, &reference, access.rights()) {
                        Ok(info) => info,
                        Err(error) => return failure(out, backend_status(error)),
                    };
                    encode_v2(
                        out,
                        header.op,
                        protocol::Status::Ok,
                        header.deadline,
                        protocol::Response::Node(info),
                    )
                }
                LookupV2::Link {
                    reference,
                    consumed,
                    target,
                    remaining,
                } => {
                    let info = match node_info_v2(backend, &reference, access.rights()) {
                        Ok(info) => info,
                        Err(error) => return failure(out, backend_status(error)),
                    };
                    encode_v2(
                        out,
                        header.op,
                        protocol::Status::Ok,
                        header.deadline,
                        protocol::Response::LinkBoundary {
                            node: info,
                            consumed: &consumed,
                            target: &target,
                            remaining: &remaining,
                        },
                    )
                }
            }
        }
        protocol::Request::Create {
            name,
            kind,
            rights,
            value,
        } => {
            if name.contains('/') || name.is_empty() {
                return failure(out, protocol::Status::Invalid);
            }
            let body = match kind {
                NodeKind::Directory if value.is_empty() => backend.directory_body(),
                NodeKind::Property => {
                    let owners = core::mem::take(transfers.input);
                    match StoredValue::prepare(value, owners, 1, PAYLOAD_MAX, access.account()) {
                        Ok(value) => Body::Property(value),
                        Err(failure_value) => {
                            transfers.input.extend(failure_value.handles);
                            return failure(out, value_status(failure_value.error));
                        }
                    }
                }
                NodeKind::Stream if value.is_empty() => match Data::new(access.account()) {
                    Ok(data) => Body::Stream(data),
                    Err(SystemCallError::QuotaExceeded) => {
                        return failure(out, protocol::Status::Quota);
                    }
                    Err(SystemCallError::OutOfMemory) => {
                        return failure(out, protocol::Status::Resource);
                    }
                    Err(_) => return failure(out, protocol::Status::Internal),
                },
                NodeKind::Directory | NodeKind::Stream => {
                    return failure(out, protocol::Status::Invalid);
                }
                NodeKind::SymbolicLink => {
                    return failure(out, protocol::Status::Unsupported);
                }
            };
            let position = match backend.position(access.root(), name, None) {
                Ok(position) => position,
                Err(error) => return failure(out, backend_status(error)),
            };
            let mutation = match backend.prepare_create(position, access, body, rights) {
                Ok(mutation) => mutation,
                Err(create) => return failure(out, backend_status(create.error)),
            };
            let created = match backend.commit(mutation) {
                Ok(CommitResult::Created(created)) => created,
                Ok(_) => return failure(out, protocol::Status::Internal),
                Err(commit) => return failure(out, backend_status(commit.error)),
            };
            push_watch_effect(
                backend,
                transfers.effects,
                access.root(),
                protocol::WatchMask::CREATE,
                None,
            )?;
            let info = node_info_v2(backend, &created, access.rights())
                .map_err(|_| SystemCallError::InternalError)?;
            encode_v2(
                out,
                header.op,
                protocol::Status::Ok,
                header.deadline,
                protocol::Response::Node(info),
            )
        }
        protocol::Request::Link {
            name,
            target,
            rights,
        } => {
            if name.contains('/') || name.is_empty() || !validate_path(target.as_bytes()) {
                return failure(out, protocol::Status::Invalid);
            }
            let position = match backend.position(access.root(), name, None) {
                Ok(position) => position,
                Err(error) => return failure(out, backend_status(error)),
            };
            let mutation =
                match backend.prepare_create(position, access, Body::Link(target.into()), rights) {
                    Ok(mutation) => mutation,
                    Err(create) => return failure(out, backend_status(create.error)),
                };
            let created = match backend.commit(mutation) {
                Ok(CommitResult::Created(created)) => created,
                Ok(_) => return failure(out, protocol::Status::Internal),
                Err(commit) => return failure(out, backend_status(commit.error)),
            };
            push_watch_effect(
                backend,
                transfers.effects,
                access.root(),
                protocol::WatchMask::CREATE,
                None,
            )?;
            let info = node_info_v2(backend, &created, access.rights())
                .map_err(|_| SystemCallError::InternalError)?;
            encode_v2(
                out,
                header.op,
                protocol::Status::Ok,
                header.deadline,
                protocol::Response::Node(info),
            )
        }
        protocol::Request::Enumerate {
            path,
            cursor,
            limit,
        } => {
            let parent = match resolve_v2(backend, access, path) {
                Ok(parent) => parent,
                Err(error) => return failure(out, backend_status(error)),
            };
            let page_limit = usize::from(limit).min(7);
            let body = &mut out[protocol::HEADER_LEN..];
            let mut writer = Writer::new(body);
            writer.u64(0);
            writer.u16(0);
            writer.u16(0);
            let mut count = 0u16;
            let next = match backend.enumerate(
                &parent,
                access,
                cursor,
                page_limit,
                |name, reference, node| {
                    writer.sized_bytes(name.as_bytes());
                    writer.u64(reference.id().raw());
                    writer.u64(node.version());
                    writer.u32(node.kind() as u32);
                    writer.u32(node.rights().intersect(access.rights()).raw());
                    let size = match node.body() {
                        Body::Directory(_) => 0,
                        Body::Property(value) => value.bytes.len() as u64,
                        Body::Stream(data) => data.len(),
                        Body::Link(target) => target.len() as u64,
                    };
                    writer.u64(size);
                    count += 1;
                },
            ) {
                Ok(next) => next,
                Err(error) => return failure(out, backend_status(error)),
            };
            let used = writer.written();
            body[..8].copy_from_slice(&next.to_le_bytes());
            body[8..10].copy_from_slice(&count.to_le_bytes());
            body[10..12].copy_from_slice(&0u16.to_le_bytes());
            protocol::Header {
                op: header.op,
                status: protocol::Status::Ok,
                body_len: used,
                deadline: header.deadline,
            }
            .encode(&mut out[..protocol::HEADER_LEN]);
            Some(protocol::HEADER_LEN + used).ok_or(SystemCallError::InternalError)
        }
        protocol::Request::Read { path } | protocol::Request::Take { path } => {
            let reference = match resolve_v2(backend, access, path) {
                Ok(reference) => reference,
                Err(error) => return failure(out, backend_status(error)),
            };
            let node = backend
                .get(&reference)
                .ok_or(SystemCallError::InternalError)?;
            if !access
                .rights()
                .intersect(node.rights())
                .contains(FalRights::READ_PROPERTY)
            {
                return failure(out, protocol::Status::Permission);
            }
            let Body::Property(value) = node.body() else {
                return failure(out, protocol::Status::Invalid);
            };
            if node.take_reserved() {
                return failure(out, protocol::Status::Busy);
            }
            if !value.handles.is_empty()
                && !access
                    .rights()
                    .intersect(node.rights())
                    .contains(FalRights::ACQUIRE_CAPABILITY)
            {
                return failure(out, protocol::Status::Permission);
            }
            let (value_bytes, handles) = match value.duplicate_for_reply() {
                Ok(exported) => exported,
                Err(libfal::value::ValueError::Affine) => {
                    return failure(out, protocol::Status::Busy);
                }
                Err(libfal::value::ValueError::Unsupported) => {
                    return failure(out, protocol::Status::Unsupported);
                }
                Err(error) => return failure(out, value_status(error)),
            };
            let used = encode_v2(
                out,
                header.op,
                protocol::Status::Ok,
                header.deadline,
                protocol::Response::Value(&value_bytes),
            )?;
            transfers.output.extend(handles);
            Ok(used)
        }
        protocol::Request::Write { path, value } => {
            let reference = match resolve_v2(backend, access, path) {
                Ok(reference) => reference,
                Err(error) => return failure(out, backend_status(error)),
            };
            let owners = core::mem::take(transfers.input);
            let stored = match StoredValue::prepare(value, owners, 1, PAYLOAD_MAX, access.account())
            {
                Ok(stored) => stored,
                Err(store) => {
                    transfers.input.extend(store.handles);
                    return failure(out, value_status(store.error));
                }
            };
            let mutation = match backend.prepare_property(reference.clone(), access, stored) {
                Ok(mutation) => mutation,
                Err(property) => return failure(out, backend_status(property.error)),
            };
            let mut old = match backend.commit(mutation) {
                Ok(CommitResult::PropertyReplaced(old)) => old,
                Ok(_) => return failure(out, protocol::Status::Internal),
                Err(commit) => return failure(out, backend_status(commit.error)),
            };
            push_watch_effect(
                backend,
                transfers.effects,
                &reference,
                protocol::WatchMask::MODIFY,
                None,
            )?;
            if !old
                .retire_step(usize::MAX)
                .map_err(|_| SystemCallError::InternalError)?
            {
                return failure(out, protocol::Status::Internal);
            }
            encode_v2(
                out,
                header.op,
                protocol::Status::Ok,
                header.deadline,
                protocol::Response::Empty,
            )
        }
        protocol::Request::ReadAt {
            path,
            offset,
            count,
        } => {
            let reference = match resolve_v2(backend, access, path) {
                Ok(reference) => reference,
                Err(error) => return failure(out, backend_status(error)),
            };
            let node = backend
                .get(&reference)
                .ok_or(SystemCallError::InternalError)?;
            if !access
                .rights()
                .intersect(node.rights())
                .contains(FalRights::READ_STREAM)
            {
                return failure(out, protocol::Status::Permission);
            }
            let Body::Stream(data) = node.body() else {
                return failure(out, protocol::Status::Invalid);
            };
            let maximum = PAYLOAD_MAX - librpc::PREFIX_LEN - protocol::HEADER_LEN - 2;
            if count as usize > maximum {
                return failure(out, protocol::Status::Invalid);
            }
            let mut bytes = [0; PAYLOAD_MAX];
            let read = data.read(offset, &mut bytes[..count as usize]);
            encode_v2(
                out,
                header.op,
                protocol::Status::Ok,
                header.deadline,
                protocol::Response::Value(&bytes[..read]),
            )
        }
        protocol::Request::WriteAt {
            path,
            offset,
            value,
        } => {
            let reference = match resolve_v2(backend, access, path) {
                Ok(reference) => reference,
                Err(error) => return failure(out, backend_status(error)),
            };
            let mutation = match backend.prepare_write(reference.clone(), access, offset, value) {
                Ok(mutation) => mutation,
                Err(error) => return failure(out, backend_status(error)),
            };
            match backend.commit(mutation) {
                Ok(CommitResult::Written) => {}
                Ok(_) => return failure(out, protocol::Status::Internal),
                Err(commit) => return failure(out, backend_status(commit.error)),
            }
            push_watch_effect(
                backend,
                transfers.effects,
                &reference,
                protocol::WatchMask::MODIFY,
                None,
            )?;
            encode_v2(
                out,
                header.op,
                protocol::Status::Ok,
                header.deadline,
                protocol::Response::Written(value.len() as u32),
            )
        }
        protocol::Request::Delete { name, expected } => {
            if name.contains('/') || name.is_empty() {
                return failure(out, protocol::Status::Invalid);
            }
            let expected = if expected.identity == 0 {
                None
            } else {
                let Some(identity) = NodeId::from_raw(expected.identity) else {
                    return failure(out, protocol::Status::Invalid);
                };
                Some((identity, expected.version))
            };
            let parent = access.root().clone();
            let target = match backend.lookup_child(&parent, name, access) {
                Ok(target) => target,
                Err(error) => return failure(out, backend_status(error)),
            };
            let position = match backend.position(access.root(), name, expected) {
                Ok(position) => position,
                Err(error) => return failure(out, backend_status(error)),
            };
            let mutation = match backend.prepare_delete(position, access) {
                Ok(mutation) => mutation,
                Err(error) => return failure(out, backend_status(error)),
            };
            let deleted = match backend.commit(mutation) {
                Ok(CommitResult::Deleted(reference)) => reference,
                Ok(_) => return failure(out, protocol::Status::Internal),
                Err(commit) => return failure(out, backend_status(commit.error)),
            };
            assert_eq!(deleted.id(), target.id(), "Delete returned the wrong node");
            push_watch_effect(
                backend,
                transfers.effects,
                &parent,
                protocol::WatchMask::DELETE,
                None,
            )?;
            push_watch_effect(
                backend,
                transfers.effects,
                &deleted,
                protocol::WatchMask::DELETE,
                Some(protocol::WatchReason::NodeDeleted),
            )?;
            encode_v2(
                out,
                header.op,
                protocol::Status::Ok,
                header.deadline,
                protocol::Response::Empty,
            )
        }
        protocol::Request::Derive { .. }
        | protocol::Request::Move { .. }
        | protocol::Request::Subscribe { .. }
        | protocol::Request::QuerySubscription { .. }
        | protocol::Request::Unsubscribe { .. } => failure(out, protocol::Status::Internal),
    }
}

struct RequestTask {
    outbox: Outbox,
    mode: RequestMode,
    committed: bool,
    recorded: bool,
}

enum RequestMode {
    RouteAck(route::Status),
    V2 {
        access: AccessSnapshot,
        header: protocol::Header,
    },
    Move(MoveOperation),
    Take(TakeOperation),
    WatchControl(WatchControlOperation),
    V2Failure {
        header: protocol::Header,
        status: protocol::Status,
    },
}

enum WatchControlKind {
    Query(u64),
    Unsubscribe(u64),
}

struct WatchControlOperation {
    context: u64,
    header: protocol::Header,
    kind: WatchControlKind,
    encoded: bool,
}

impl WatchControlOperation {
    fn drive(
        &mut self,
        world: &mut World,
        requests: &mut Requests<ServiceTask>,
        out: &mut [u8],
    ) -> Result<usize, SystemCallError> {
        if self.encoded {
            return Ok(0);
        }
        let (status, response) = match self.kind {
            WatchControlKind::Query(id) => match world.watches.query(id, self.context) {
                Ok(info) => (protocol::Status::Ok, protocol::Response::Subscription(info)),
                Err(ControlError::NotFound) => {
                    (protocol::Status::NotFound, protocol::Response::Empty)
                }
                Err(ControlError::Permission) => {
                    (protocol::Status::Permission, protocol::Response::Empty)
                }
            },
            WatchControlKind::Unsubscribe(id) => match world.watches.cancel(id, self.context) {
                Ok(task) => {
                    requests.wake(task)?;
                    (protocol::Status::Ok, protocol::Response::Empty)
                }
                Err(ControlError::NotFound) => {
                    (protocol::Status::NotFound, protocol::Response::Empty)
                }
                Err(ControlError::Permission) => {
                    (protocol::Status::Permission, protocol::Response::Empty)
                }
            },
        };
        let used = encode_v2(out, self.header.op, status, self.header.deadline, response)?;
        self.encoded = true;
        Ok(used)
    }
}

struct MoveOperation {
    access: AccessSnapshot,
    destination: AccessSnapshot,
    header: protocol::Header,
    source_parent: String,
    source_name: String,
    destination_name: String,
    expected: Option<(NodeId, u64)>,
    prepared: Option<libfal::backend::PreparedMutation<Capability>>,
    source_parent_ref: Option<NodeRef>,
    target_ref: Option<NodeRef>,
}

struct TakeOperation {
    access: AccessSnapshot,
    header: protocol::Header,
    path: String,
    prepared: Option<PreparedTake<Capability>>,
    reference: Option<NodeRef>,
    bytes: Option<Vec<u8>>,
    policies: Vec<ExportPolicy>,
    restore_handles: Vec<StoredHandle<Capability>>,
    encoded: bool,
    failed: bool,
}

impl TakeOperation {
    fn drive(
        &mut self,
        backend: &mut MemoryBackend<Capability>,
        outbox: &mut Outbox,
    ) -> Result<usize, SystemCallError> {
        if self.encoded {
            return Ok(0);
        }
        if self.prepared.is_none() {
            let reference = match resolve_v2(backend, &self.access, &self.path) {
                Ok(reference) => reference,
                Err(error) => {
                    self.failed = true;
                    self.encoded = true;
                    return encode_v2(
                        outbox.response_mut()?.body_mut()?,
                        self.header.op,
                        backend_status(error),
                        self.header.deadline,
                        protocol::Response::Empty,
                    );
                }
            };
            let value = match backend.get(&reference).map(|node| node.body()) {
                Some(Body::Property(value)) => value,
                Some(_) => {
                    self.failed = true;
                    self.encoded = true;
                    return encode_v2(
                        outbox.response_mut()?.body_mut()?,
                        self.header.op,
                        protocol::Status::Invalid,
                        self.header.deadline,
                        protocol::Response::Empty,
                    );
                }
                None => {
                    self.failed = true;
                    self.encoded = true;
                    return encode_v2(
                        outbox.response_mut()?.body_mut()?,
                        self.header.op,
                        protocol::Status::NotFound,
                        self.header.deadline,
                        protocol::Response::Empty,
                    );
                }
            };
            if value
                .handles
                .iter()
                .any(|handle| handle.policy.protocol == ValueProtocol::Directory)
            {
                self.failed = true;
                self.encoded = true;
                return encode_v2(
                    outbox.response_mut()?.body_mut()?,
                    self.header.op,
                    protocol::Status::Unsupported,
                    self.header.deadline,
                    protocol::Response::Empty,
                );
            }
            if self
                .policies
                .try_reserve_exact(value.handles.len())
                .is_err()
                || self
                    .restore_handles
                    .try_reserve_exact(value.handles.len())
                    .is_err()
            {
                self.failed = true;
                self.encoded = true;
                return encode_v2(
                    outbox.response_mut()?.body_mut()?,
                    self.header.op,
                    protocol::Status::Resource,
                    self.header.deadline,
                    protocol::Response::Empty,
                );
            }
            self.policies
                .extend(value.handles.iter().map(|handle| handle.policy));
            let watch_reference = reference.clone();
            let mut prepared = match backend.prepare_take(reference, &self.access) {
                Ok(prepared) => prepared,
                Err(error) => {
                    self.failed = true;
                    self.encoded = true;
                    return encode_v2(
                        outbox.response_mut()?.body_mut()?,
                        self.header.op,
                        backend_status(error),
                        self.header.deadline,
                        protocol::Response::Empty,
                    );
                }
            };
            self.reference = Some(watch_reference);
            let taken = MemoryBackend::take_value(&mut prepared);
            self.bytes = Some(taken.bytes);
            self.prepared = Some(prepared);

            let bytes = self.bytes.as_ref().expect("take bytes prepared");
            let response = outbox.response_mut()?;
            let used = encode_v2(
                response.body_mut()?,
                self.header.op,
                protocol::Status::Ok,
                self.header.deadline,
                protocol::Response::Value(bytes),
            )?;
            let mut handles = taken.handles;
            let mut index = 0usize;
            while !handles.is_empty() {
                let handle = handles.remove(0);
                let policy = self.policies[index];
                if let Err(failure) = response.push(handle.owner, policy.transport) {
                    handles.insert(
                        0,
                        StoredHandle {
                            owner: failure.capability,
                            policy,
                        },
                    );
                    let mut prior = index;
                    response.drain_capabilities(|owner, _| {
                        prior -= 1;
                        handles.insert(
                            0,
                            StoredHandle {
                                owner,
                                policy: self.policies[prior],
                            },
                        );
                    });
                    assert_eq!(prior, 0, "take reply recovery omitted a capability");
                    let taken = TakenValue {
                        bytes: self.bytes.take().expect("take bytes on push failure"),
                        handles,
                    };
                    let prepared = self.prepared.take().expect("take reservation exists");
                    backend.rollback_take(prepared, taken);
                    return Err(failure.error);
                }
                index += 1;
            }
            self.restore_handles = handles;
            self.encoded = true;
            return Ok(used);
        }
        Ok(0)
    }

    fn finalize(
        &mut self,
        backend: &mut MemoryBackend<Capability>,
        outbox: &mut Outbox,
        effects: &mut Effects,
    ) -> Result<bool, SystemCallError> {
        if self.failed {
            return Ok(false);
        }
        let prepared = self.prepared.take().expect("take reservation exists");
        if matches!(outbox.result(), Some(OutboxResult::Sent)) {
            backend.commit_take(prepared);
            push_watch_effect(
                backend,
                effects,
                self.reference
                    .as_ref()
                    .expect("committed Take lost its target"),
                protocol::WatchMask::MODIFY,
                None,
            )?;
            self.bytes = None;
            self.policies.clear();
            return Ok(true);
        }
        let mut index = self.policies.len();
        outbox.drain_capabilities(|owner, _| {
            index -= 1;
            self.restore_handles.insert(
                0,
                StoredHandle {
                    owner,
                    policy: self.policies[index],
                },
            );
        });
        assert_eq!(index, 0, "take rollback omitted a capability");
        assert_eq!(
            self.restore_handles.len(),
            self.policies.len(),
            "take rollback capability count mismatch"
        );
        let taken = TakenValue {
            bytes: self.bytes.take().expect("take bytes retained"),
            handles: core::mem::take(&mut self.restore_handles),
        };
        backend.rollback_take(prepared, taken);
        self.policies.clear();
        Ok(false)
    }
}

impl Task<World> for RequestTask {
    type Family = ServiceTask;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World,
        requests: &mut Requests<ServiceTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            self.outbox.observe(event);
        }
        if input.take_timeout() {
            self.outbox.timed_out();
        }
        if !self.committed && self.outbox.result().is_none() {
            if !self.outbox.is_admitted() {
                return Ok(self.outbox.admit(requests));
            }
            let used = match &mut self.mode {
                RequestMode::RouteAck(status) => {
                    route::encode_status(*status, self.outbox.response_mut()?.body_mut()?)
                        .ok_or(SystemCallError::InternalError)?
                }
                RequestMode::V2 { access, header } => {
                    let mut effects = Effects::default();
                    let used = {
                        let response = self.outbox.response_mut()?;
                        let (context, body) = response.parts()?;
                        let mut input_handles = context.handles.take_all();
                        let mut output_handles = Vec::new();
                        let mut transfers = ServeTransfers {
                            input: &mut input_handles,
                            output: &mut output_handles,
                            effects: &mut effects,
                        };
                        let used = serve_v2(
                            world
                                .backend
                                .as_mut()
                                .expect("provider backend missing during request"),
                            access,
                            *header,
                            &context.payload,
                            &mut transfers,
                            body,
                        )?;
                        for (capability, rights) in output_handles {
                            response
                                .push(capability, rights)
                                .map_err(|failure| failure.error)?;
                        }
                        used
                    };
                    publish_watch_events(world, requests, &effects)?;
                    used
                }
                RequestMode::Move(operation) => {
                    let mut effects = Effects::default();
                    let Some(used) = drive_move(
                        world
                            .backend
                            .as_mut()
                            .expect("provider backend missing during move"),
                        operation,
                        self.outbox.response_mut()?.body_mut()?,
                        budget,
                        &mut effects,
                    )?
                    else {
                        return Ok(Advance {
                            work_done: 1,
                            step: Step::Runnable,
                        });
                    };
                    publish_watch_events(world, requests, &effects)?;
                    used
                }
                RequestMode::Take(operation) => operation.drive(
                    world
                        .backend
                        .as_mut()
                        .expect("provider backend missing during take"),
                    &mut self.outbox,
                )?,
                RequestMode::WatchControl(operation) => {
                    operation.drive(world, requests, self.outbox.response_mut()?.body_mut()?)?
                }
                RequestMode::V2Failure { header, status } => encode_v2(
                    self.outbox.response_mut()?.body_mut()?,
                    header.op,
                    *status,
                    header.deadline,
                    protocol::Response::Empty,
                )?,
            };
            self.outbox.response_mut()?.finish_body(used)?;
            self.committed = true;
        }
        let advance = self.outbox.drive(requests, budget)?;
        if advance.step == Step::Complete && self.committed && !self.recorded {
            let business_committed = if let RequestMode::Take(operation) = &mut self.mode {
                let mut effects = Effects::default();
                let committed = operation.finalize(
                    world
                        .backend
                        .as_mut()
                        .expect("provider backend missing during take finalization"),
                    &mut self.outbox,
                    &mut effects,
                )?;
                publish_watch_events(world, requests, &effects)?;
                committed
            } else {
                true
            };
            if business_committed {
                world.committed = world
                    .committed
                    .checked_add(1)
                    .ok_or(SystemCallError::ReachLimit)?;
            }
            if let Some(OutboxResult::Abandoned(cause)) = self.outbox.result() {
                world.abandoned = world
                    .abandoned
                    .checked_add(1)
                    .ok_or(SystemCallError::ReachLimit)?;
                rinlib::debug!("fs: provider response abandoned after commit: {:?}", cause);
            }
            self.recorded = true;
        }
        Ok(advance)
    }

    fn refused(&mut self, world: &mut World, failure: RequestFailure<ServiceTask>) {
        self.outbox.refused(world, failure);
    }

    fn registered(&mut self, world: &mut World, kind: SourceKind, source: SourceId) {
        self.outbox.registered(world, kind, source);
    }

    fn unregistered(&mut self, world: &mut World, kind: SourceKind, source: SourceId) {
        self.outbox.unregistered(world, kind, source);
    }

    fn stop(&mut self, world: &mut World) {
        self.outbox.stop(world);
    }

    fn deadline(&self) -> Deadline {
        self.outbox.deadline()
    }
}

struct DelegateSetup {
    header: protocol::Header,
    consumed: String,
    remaining: String,
    service: Capability,
    request: RpcRequest,
}

impl DelegateSetup {
    fn with_outbox(self, outbox: Outbox) -> DelegateTask {
        DelegateTask {
            outbox,
            header: self.header,
            consumed: self.consumed,
            remaining: self.remaining,
            dispatch: DelegateDispatch::Ready {
                service: self.service,
                request: self.request,
            },
            committed: false,
            recorded: false,
        }
    }
}

fn prepare_delegate(
    binding: &RouteBinding,
    access: &AccessSnapshot,
    path: &str,
    header: protocol::Header,
) -> Result<Option<DelegateSetup>, protocol::Status> {
    let remaining = if path == binding.name {
        ""
    } else {
        let Some(remaining) = path
            .strip_prefix(binding.name.as_str())
            .and_then(|suffix| suffix.strip_prefix('/'))
        else {
            return Ok(None);
        };
        remaining
    };
    if !access
        .rights()
        .contains(FalRights::TRAVERSE | FalRights::ACQUIRE_CAPABILITY)
    {
        return Err(protocol::Status::Permission);
    }
    let rights = access.rights().intersect(binding.rights);
    if !rights.contains(FalRights::TRAVERSE) {
        return Err(protocol::Status::Permission);
    }
    let request = protocol::Request::Derive { path: "", rights };
    let capacity = protocol::HEADER_LEN
        .checked_add(request.encoded_len().ok_or(protocol::Status::Invalid)?)
        .ok_or(protocol::Status::Invalid)?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(capacity)
        .map_err(|_| protocol::Status::Resource)?;
    payload.resize(capacity, 0);
    let used = protocol::encode_request(&request, header.deadline, &mut payload)
        .ok_or(protocol::Status::Internal)?;
    payload.truncate(used);
    let rpc = RpcRequest::new(protocol::ID, &payload).map_err(|_| protocol::Status::Resource)?;
    let transport = Rights::WRITE | Rights::WAIT;
    let duplicated =
        duplicate(binding.target.as_handle(), transport).map_err(|error| match error {
            SystemCallError::QuotaExceeded => protocol::Status::Quota,
            SystemCallError::OutOfMemory => protocol::Status::Resource,
            SystemCallError::ObjectClosed => protocol::Status::GrantRevoked,
            _ => protocol::Status::Internal,
        })?;
    // SAFETY: ObjectDuplicate returned a fresh affine entry owned by this submission.
    let service = unsafe { Capability::from_raw(duplicated) };
    Ok(Some(DelegateSetup {
        header,
        consumed: binding.name.clone(),
        remaining: String::from(remaining),
        service,
        request: rpc,
    }))
}

enum DelegateDispatch {
    Ready {
        service: Capability,
        request: RpcRequest,
    },
    Queued,
    Pending(u64),
    Complete(Completion),
}

struct DelegateTask {
    outbox: Outbox,
    header: protocol::Header,
    consumed: String,
    remaining: String,
    dispatch: DelegateDispatch,
    committed: bool,
    recorded: bool,
}

impl DelegateTask {
    fn submitted(&mut self, result: Result<u64, Completion>) {
        assert!(
            matches!(self.dispatch, DelegateDispatch::Queued),
            "Delegate submission completed from an invalid state"
        );
        self.dispatch = match result {
            Ok(txid) => DelegateDispatch::Pending(txid),
            Err(completion) => DelegateDispatch::Complete(completion),
        };
    }

    fn completed(&mut self, txid: u64, completion: Completion) {
        assert!(
            matches!(self.dispatch, DelegateDispatch::Pending(pending) if pending == txid),
            "Delegate completion txid mismatch"
        );
        self.dispatch = DelegateDispatch::Complete(completion);
    }

    fn encode_completion(&mut self) -> Result<(), SystemCallError> {
        let completion = match core::mem::replace(&mut self.dispatch, DelegateDispatch::Queued) {
            DelegateDispatch::Complete(completion) => completion,
            other => {
                self.dispatch = other;
                return Ok(());
            }
        };
        drop(completion.service);
        let mut derived = None;
        let mut info = None;
        let status = match completion.result {
            Ok(mut reply) => {
                let decoded = protocol::decode_response(&reply.payload);
                match decoded {
                    Ok((header, response))
                        if header.op == protocol::Op::Derive
                            && header.status == protocol::Status::Ok
                            && matches!(response, protocol::Response::Node(_))
                            && reply.handles.remaining() == 1 =>
                    {
                        let protocol::Response::Node(node) = response else {
                            unreachable!()
                        };
                        let capability = reply
                            .handles
                            .take(0)
                            .map_err(|_| SystemCallError::InternalError)?;
                        match MailboxSender::from_capability(capability) {
                            Ok((sender, _)) => {
                                derived = Some(sender);
                                info = Some(node);
                                protocol::Status::Ok
                            }
                            Err(_) => protocol::Status::Internal,
                        }
                    }
                    Ok((header, _))
                        if header.op == protocol::Op::Derive
                            && header.status != protocol::Status::Ok
                            && reply.handles.is_empty() =>
                    {
                        header.status
                    }
                    _ => protocol::Status::Internal,
                }
            }
            Err(CallError { cause, .. }) => match cause {
                CallCause::Timeout | CallCause::Shutdown => protocol::Status::Cancelled,
                CallCause::ServiceClosed => protocol::Status::GrantRevoked,
                CallCause::Frame(_) | CallCause::System(_) => protocol::Status::Internal,
            },
        };
        let response = self.outbox.response_mut()?;
        let used = if status == protocol::Status::Ok {
            let sender = derived
                .take()
                .expect("successful Delegate lost its derived grant");
            response
                .push(
                    sender.into_capability(),
                    Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
                )
                .map_err(|failure| failure.error)?;
            protocol::encode_response(
                protocol::Op::Lookup,
                status,
                self.header.deadline,
                &protocol::Response::Delegate {
                    node: info.expect("successful Delegate lost its node information"),
                    consumed: &self.consumed,
                    remaining: &self.remaining,
                },
                response.body_mut()?,
            )
        } else {
            protocol::encode_response(
                protocol::Op::Lookup,
                status,
                self.header.deadline,
                &protocol::Response::Empty,
                response.body_mut()?,
            )
        }
        .ok_or(SystemCallError::InternalError)?;
        response.finish_body(used)?;
        self.committed = true;
        Ok(())
    }
}

impl Task<World> for DelegateTask {
    type Family = ServiceTask;

    fn advance(
        &mut self,
        id: u64,
        world: &mut World,
        requests: &mut Requests<ServiceTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            self.outbox.observe(event);
        }
        if input.take_timeout() {
            self.outbox.timed_out();
        }
        if self.outbox.result().is_none() && !self.outbox.is_admitted() {
            return Ok(self.outbox.admit(requests));
        }
        if matches!(self.dispatch, DelegateDispatch::Ready { .. }) {
            if world.dispatch_submission.is_some() {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                });
            }
            requests.wake(world.dispatcher_task)?;
            let ready = core::mem::replace(&mut self.dispatch, DelegateDispatch::Queued);
            let DelegateDispatch::Ready { service, request } = ready else {
                unreachable!()
            };
            world.dispatch_submission = Some(DispatchSubmission {
                waiter: id,
                service,
                deadline: self.header.deadline,
                request,
            });
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        if !self.committed && matches!(self.dispatch, DelegateDispatch::Complete(_)) {
            self.encode_completion()?;
        }
        if !self.committed {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        let advance = self.outbox.drive(requests, budget)?;
        if advance.step == Step::Complete && self.committed && !self.recorded {
            world.committed = world
                .committed
                .checked_add(1)
                .ok_or(SystemCallError::ReachLimit)?;
            if let Some(OutboxResult::Abandoned(cause)) = self.outbox.result() {
                world.abandoned = world
                    .abandoned
                    .checked_add(1)
                    .ok_or(SystemCallError::ReachLimit)?;
                rinlib::debug!("fs: Delegate response abandoned after commit: {:?}", cause);
            }
            self.recorded = true;
        }
        Ok(advance)
    }

    fn refused(&mut self, world: &mut World, failure: RequestFailure<ServiceTask>) {
        self.outbox.refused(world, failure);
    }

    fn registered(&mut self, world: &mut World, kind: SourceKind, source: SourceId) {
        self.outbox.registered(world, kind, source);
    }

    fn unregistered(&mut self, world: &mut World, kind: SourceKind, source: SourceId) {
        self.outbox.unregistered(world, kind, source);
    }

    fn stop(&mut self, world: &mut World) {
        self.outbox.stop(world);
    }

    fn deadline(&self) -> Deadline {
        self.outbox.deadline()
    }
}

enum ServiceTask {
    Delegate(DelegateTask),
    Dispatcher(Dispatcher),
    Grant(GrantTask),
    Ingress(Ingress),
    Release(ReleaseTask),
    Request(RequestTask),
    Retire(RetireTask),
    Route(RouteIngress),
    Watch(WatchTask),
}

impl Task<World> for ServiceTask {
    type Family = Self;

    fn advance(
        &mut self,
        id: u64,
        world: &mut World,
        requests: &mut Requests<Self>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        match self {
            Self::Delegate(task) => task.advance(id, world, requests, input, budget),
            Self::Dispatcher(task) => task.advance(requests, input, budget),
            Self::Grant(task) => task.advance(id, world, requests, input, budget),
            Self::Ingress(task) => task.advance(id, world, requests, input, budget),
            Self::Release(task) => task.advance(id, world, requests, input, budget),
            Self::Request(task) => task.advance(id, world, requests, input, budget),
            Self::Retire(task) => task.advance(id, world, requests, input, budget),
            Self::Route(task) => task.advance(id, world, requests, input, budget),
            Self::Watch(task) => task.advance(id, world, requests, input, budget),
        }
    }

    fn refused(&mut self, world: &mut World, failure: RequestFailure<Self>) {
        match self {
            Self::Delegate(task) => task.refused(world, failure),
            Self::Dispatcher(task) => task.refused(world, failure),
            Self::Grant(task) => task.refused(world, failure),
            Self::Ingress(task) => task.refused(world, failure),
            Self::Release(task) => task.refused(world, failure),
            Self::Request(task) => task.refused(world, failure),
            Self::Retire(task) => task.refused(world, failure),
            Self::Route(task) => task.refused(world, failure),
            Self::Watch(task) => task.refused(world, failure),
        }
    }

    fn registered(&mut self, world: &mut World, kind: SourceKind, source: SourceId) {
        match self {
            Self::Delegate(task) => task.registered(world, kind, source),
            Self::Dispatcher(task) => task.registered(world, kind, source),
            Self::Grant(task) => task.registered(world, kind, source),
            Self::Ingress(task) => task.registered(world, kind, source),
            Self::Release(task) => task.registered(world, kind, source),
            Self::Request(task) => task.registered(world, kind, source),
            Self::Retire(task) => task.registered(world, kind, source),
            Self::Route(task) => task.registered(world, kind, source),
            Self::Watch(task) => task.registered(world, kind, source),
        }
    }

    fn unregistered(&mut self, world: &mut World, kind: SourceKind, source: SourceId) {
        match self {
            Self::Delegate(task) => task.unregistered(world, kind, source),
            Self::Dispatcher(task) => task.unregistered(world, kind, source),
            Self::Grant(task) => task.unregistered(world, kind, source),
            Self::Ingress(task) => task.unregistered(world, kind, source),
            Self::Release(task) => task.unregistered(world, kind, source),
            Self::Request(task) => task.unregistered(world, kind, source),
            Self::Retire(task) => task.unregistered(world, kind, source),
            Self::Route(task) => task.unregistered(world, kind, source),
            Self::Watch(task) => task.unregistered(world, kind, source),
        }
    }

    fn stop(&mut self, world: &mut World) {
        match self {
            Self::Delegate(task) => task.stop(world),
            Self::Dispatcher(task) => task.stop(world),
            Self::Grant(task) => task.stop(world),
            Self::Ingress(task) => task.stop(world),
            Self::Release(task) => task.stop(world),
            Self::Request(task) => task.stop(world),
            Self::Retire(task) => task.stop(world),
            Self::Route(task) => task.stop(world),
            Self::Watch(task) => task.stop(world),
        }
    }

    fn deadline(&self) -> Deadline {
        match self {
            Self::Delegate(task) => task.deadline(),
            Self::Dispatcher(task) => task.deadline(),
            Self::Grant(task) => task.deadline(),
            Self::Watch(task) => task.deadline(),
            Self::Ingress(_) | Self::Release(_) | Self::Retire(_) | Self::Route(_) => {
                Deadline::INFINITE
            }
            Self::Request(task) => task.deadline(),
        }
    }
}

fn progress_dispatch(
    runtime: &mut Runtime<ServiceTask, WaitSet>,
    world: &mut World,
    bindings: &mut Vec<(u64, u64)>,
) -> Result<(), SystemCallError> {
    if let Some(submission) = world.dispatch_submission.take() {
        let waiter = submission.waiter;
        let started = {
            let Some(ServiceTask::Dispatcher(dispatcher)) =
                runtime.get_task_mut(world.dispatcher_task)
            else {
                return Err(SystemCallError::InternalError);
            };
            dispatcher.begin_for(
                submission.service,
                submission.deadline,
                submission.request,
                Some(waiter),
            )
        };
        match started {
            Ok(txid) => {
                if bindings.len() == bindings.capacity() {
                    return Err(SystemCallError::ReachLimit);
                }
                bindings.push((txid, waiter));
                let Some(ServiceTask::Delegate(task)) = runtime.get_task_mut(waiter) else {
                    return Err(SystemCallError::InternalError);
                };
                task.submitted(Ok(txid));
                runtime.wake(world.dispatcher_task)?;
            }
            Err(failure) => {
                let completion = Completion {
                    service: failure.service,
                    result: Err(failure.error),
                };
                let Some(ServiceTask::Delegate(task)) = runtime.get_task_mut(waiter) else {
                    return Err(SystemCallError::InternalError);
                };
                task.submitted(Err(completion));
                runtime.wake(waiter)?;
            }
        }
    }
    if bindings.is_empty() {
        return Ok(());
    }
    loop {
        let completed = {
            let Some(ServiceTask::Dispatcher(dispatcher)) =
                runtime.get_task_mut(world.dispatcher_task)
            else {
                return Err(SystemCallError::InternalError);
            };
            dispatcher.pop_completed()
        };
        let Some((txid, completion)) = completed else {
            break;
        };
        let Some(index) = bindings.iter().position(|(pending, _)| *pending == txid) else {
            return Err(SystemCallError::InternalError);
        };
        let (_, waiter) = bindings.swap_remove(index);
        if matches!(
            &completion.result,
            Err(CallError {
                phase: librpc::CallPhase::Sent,
                ..
            })
        ) {
            world.downstream_abandoned = world
                .downstream_abandoned
                .checked_add(1)
                .ok_or(SystemCallError::ReachLimit)?;
        }
        let Some(ServiceTask::Delegate(task)) = runtime.get_task_mut(waiter) else {
            return Err(SystemCallError::InternalError);
        };
        task.completed(txid, completion);
        runtime.wake(waiter)?;
    }
    Ok(())
}

pub(super) fn run(
    mailbox: Mailbox,
    bootstrap: MailboxSender,
    route_mailbox: Handle,
    release: Handle,
) {
    let task_limit = 32;
    let source_limit = 64;
    let input_bytes = Runtime::<ServiceTask, WaitSet>::input_budget(source_limit)
        .expect("provider Runtime input budget calculation failed");
    let execution_layout = [0, 1];
    let fal_layout = [2, 3, 4, 5, 6, 7, 8, 9, 10];
    let mut limits = [0; ExecutionResource::COUNT + FalResource::COUNT];
    limits[execution_layout[ExecutionResource::Task.slot()]] = task_limit;
    limits[execution_layout[ExecutionResource::InputBytes.slot()]] = input_bytes;
    limits[fal_layout[FalResource::Node.slot()]] = 64;
    limits[fal_layout[FalResource::Bytes.slot()]] = 2 * 1024 * 1024;
    limits[fal_layout[FalResource::Grant.slot()]] = 16;
    limits[fal_layout[FalResource::Watch.slot()]] = WATCH_LIMIT;
    limits[fal_layout[FalResource::WaitSource.slot()]] = 48;
    let budget = Budget::new(&limits, 1).expect("provider Runtime budget creation failed");
    let execution_binding = execution_layout.map(|index| budget.slot(index).unwrap());
    let fal_binding = fal_layout.map(|index| budget.slot(index).unwrap());
    let account = budget
        .account(&limits)
        .expect("provider Runtime account creation failed");
    let execution_account = account
        .view::<ExecutionResource>(&execution_binding)
        .expect("provider execution budget binding failed");
    let fal_account = account
        .view::<FalResource>(&fal_binding)
        .expect("provider FAL budget binding failed");
    let event = notification::create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL)
        .expect("provider retirement notification creation failed");
    // SAFETY: NotificationCreate returned two fresh affine entries; this worker owns both.
    let retire_owner = unsafe { Capability::from_raw(event.owner) };
    // SAFETY: the peer entry is the unique signaler owner and is transferred into NotificationWake.
    let retire_signaler = unsafe { Capability::from_raw(event.peer) };
    let wake = NotificationWake::new(retire_signaler, RETIRE_BIT)
        .map_err(|(_, error)| error)
        .expect("provider retirement wake validation failed");
    let backend = MemoryBackend::new(&fal_account, 64, Rc::new(wake))
        .expect("provider backend creation failed");
    let mut grants = GrantTable::new(&mailbox, 16).expect("provider grant table creation failed");
    let root = backend
        .root()
        .expect("provider backend root missing")
        .clone();
    let prepared_root = grants
        .prepare_issue(
            &mailbox,
            root,
            Issuance {
                rights: FalRights::ALL,
                sender_transport: Rights::WRITE
                    | Rights::WAIT
                    | Rights::TRANSIT
                    | Rights::DUPLICATE,
                output_transport: Rights::TRANSIT,
            },
            fal_account.clone(),
            0,
        )
        .map_err(|failure| failure.error)
        .expect("provider root grant preparation failed");
    let set = WaitSet::create(source_limit).expect("provider Runtime WaitSet creation failed");
    let mut runtime =
        Runtime::<ServiceTask, WaitSet>::new(set, task_limit, source_limit, &execution_account)
            .expect("provider Runtime creation failed");
    let retire_task = runtime
        .spawn(ServiceTask::Retire(RetireTask::new(retire_owner)), 1)
        .map_err(|failure| failure.error)
        .expect("provider retirement task admission failed");
    let dispatcher = Dispatcher::new(DISPATCH_LIMIT).expect("provider Dispatcher creation failed");
    let dispatcher_task = runtime
        .spawn(ServiceTask::Dispatcher(dispatcher), DISPATCH_LIMIT * 2 + 1)
        .map_err(|failure| failure.error)
        .expect("provider Dispatcher admission failed");
    runtime
        .spawn(ServiceTask::Grant(GrantTask::root(prepared_root)), 1)
        .map_err(|failure| failure.error)
        .expect("provider root grant task admission failed");
    runtime
        .spawn(ServiceTask::Ingress(Ingress::new()), 1)
        .map_err(|failure| failure.error)
        .expect("provider ingress admission failed");
    runtime
        .spawn(ServiceTask::Route(RouteIngress::new()), 1)
        .map_err(|failure| failure.error)
        .expect("provider route ingress admission failed");
    // SAFETY: StartupBlock transferred the unique release notification owner to this process.
    let release = unsafe { Capability::from_raw(release) };
    runtime
        .spawn(ServiceTask::Release(ReleaseTask::new(release)), 1)
        .map_err(|failure| failure.error)
        .expect("provider release task admission failed");
    let mut world = World {
        mailbox,
        route_mailbox,
        route: None,
        dispatch_submission: None,
        dispatcher_task,
        backend: Some(backend),
        grants: Some(grants),
        root_sender: None,
        retire_task,
        retire_ready: false,
        backend_sealed: false,
        committed: 0,
        abandoned: 0,
        downstream_abandoned: 0,
        watches: watch::Table::new().expect("provider Watch table allocation failed"),
        stop: false,
        failed: false,
    };
    let mut dispatch_bindings = Vec::new();
    dispatch_bindings
        .try_reserve_exact(DISPATCH_LIMIT)
        .expect("provider dispatch binding allocation failed");
    let mut sealing = false;
    loop {
        assert!(!world.failed, "provider Runtime entered a fatal state");
        if world.stop && !sealing {
            world
                .grants
                .as_mut()
                .expect("provider grant table missing during seal")
                .seal();
            runtime.seal();
            sealing = true;
        }
        let result = if sealing {
            runtime.shutdown_turn(&mut world, 1)
        } else {
            runtime.turn(&mut world, 1)
        };
        result.expect("provider Runtime failed");
        progress_dispatch(&mut runtime, &mut world, &mut dispatch_bindings)
            .expect("provider dispatch integration failed");
        if let Some(sender) = world.root_sender.take() {
            let mut packet = Packet::new(protocol::ROOT_GRANT_KIND, &[])
                .expect("provider root grant packet creation failed");
            packet
                .push(
                    sender.into_capability(),
                    Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
                )
                .map_err(|failure| failure.error)
                .expect("provider root grant packet preparation failed");
            packet
                .try_send(&bootstrap, Deadline::INFINITE)
                .map_err(|failure| failure.error)
                .expect("provider root grant publication failed");
            bootstrap
                .send(protocol::PROVIDER_READY_KIND, &[])
                .expect("provider Ready publication failed");
        }
        match runtime.drive_state() {
            DriveState::Drained => break,
            DriveState::Runnable => {}
            DriveState::Waiting(deadline) => {
                wait_until(&[runtime.wait_item(0)], deadline)
                    .expect("provider Runtime wait failed");
                runtime.notified();
            }
        }
    }
    runtime
        .close()
        .map_err(|(_, error)| error)
        .expect("provider Runtime close failed");
    assert!(
        dispatch_bindings.is_empty() && world.dispatch_submission.is_none(),
        "provider dispatch owners remained after shutdown"
    );
    assert!(
        world.watches.is_empty(),
        "provider Watch records remained after Runtime shutdown"
    );
    rinlib::debug!("fs provider shutdown: Runtime closed");
    world
        .grants
        .take()
        .expect("provider grant table missing at close")
        .close()
        .map_err(|_| SystemCallError::ObjectBusy)
        .expect("provider grant table close failed");
    rinlib::debug!("fs provider shutdown: GrantTable closed");
    assert!(
        world.backend.is_none(),
        "provider backend remained after shutdown"
    );
    // SAFETY: StartupBlock transferred the unique route mailbox owner to this process,
    // and Runtime has removed its final WaitSet source before reaching this point.
    unsafe { rinlib::ipc::object::close(world.route_mailbox) }
        .expect("provider route mailbox close failed");
    rinlib::debug!("fs provider shutdown: route mailbox closed");
    for kind in [
        FalResource::Node,
        FalResource::Bytes,
        FalResource::Grant,
        FalResource::Watch,
        FalResource::WaitSource,
    ] {
        assert_eq!(
            fal_account.usage(kind).0,
            0,
            "provider FAL account did not refund {kind:?}"
        );
    }
    for kind in [ExecutionResource::Task, ExecutionResource::InputBytes] {
        assert_eq!(
            execution_account.usage(kind).0,
            0,
            "provider execution account did not refund {kind:?}"
        );
    }
    rinlib::debug!("fs provider shutdown: account refunded");
    let report = protocol::ProviderReport {
        committed: world.committed,
        abandoned: world.abandoned,
        downstream_abandoned: world.downstream_abandoned,
    };
    let mut payload = [0; protocol::PROVIDER_REPORT_LEN];
    let used = report
        .encode(&mut payload)
        .expect("provider report encoding failed");
    bootstrap
        .send(protocol::PROVIDER_STOPPED_KIND, &payload[..used])
        .expect("provider shutdown report failed");
    rinlib::debug!(
        "fs provider shutdown report: committed={}, abandoned={}, downstream_abandoned={}",
        report.committed,
        report.abandoned,
        report.downstream_abandoned
    );
}
