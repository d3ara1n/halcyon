use alloc::string::String;

use erhino_shared::{
    call::SystemCallError,
    object::{ObjectSignals, Rights},
    time::Deadline,
};
use libbudget::Charge;
use libexecution::runtime::{Advance, Input, RequestFailure, Requests, SourceId, SourceKind, Step};
use librpc::{Outbox, OutboxResult};
use rinlib::ipc::{
    capability::Capability,
    message::{Mailbox, MailboxSender},
    notification,
};

use crate::{
    authority::AccessSnapshot,
    backend::{BackendError, ProviderBackend},
    grant::{GrantObserver, GrantTable, PreparedGrant},
    protocol,
    store::NodeRef,
    watch,
};

pub struct State<B> {
    pub mailbox: Mailbox,
    pub backend: Option<B>,
    pub grants: Option<GrantTable>,
    pub watches: watch::Table,
    pub retire_task: u64,
    pub retire_ready: bool,
    pub backend_sealed: bool,
    pub committed: u64,
    pub abandoned: u64,
}

impl<B> State<B> {
    pub fn record_response(
        &mut self,
        committed: bool,
        abandoned: bool,
    ) -> Result<(), SystemCallError> {
        let committed_count = self
            .committed
            .checked_add(u64::from(committed))
            .ok_or(SystemCallError::ReachLimit)?;
        let abandoned_count = self
            .abandoned
            .checked_add(u64::from(abandoned))
            .ok_or(SystemCallError::ReachLimit)?;
        self.committed = committed_count;
        self.abandoned = abandoned_count;
        Ok(())
    }
}

impl<B: ProviderBackend<Capability>> State<B> {
    pub fn seal_when_grants_drained(&mut self, stopping: bool) -> bool {
        if !stopping
            || self.backend_sealed
            || !self
                .grants
                .as_ref()
                .expect("grant table missing during shutdown")
                .is_empty()
        {
            return false;
        }
        self.backend
            .as_mut()
            .expect("backend missing during shutdown")
            .seal();
        self.backend_sealed = true;
        true
    }

    pub fn retire_step(&mut self, budget: usize) -> Result<bool, SystemCallError> {
        let backend = self
            .backend
            .as_mut()
            .expect("backend missing during retirement");
        if !backend.has_retire_work() {
            return Ok(false);
        }
        backend.retire_step(budget)?;
        Ok(true)
    }

    pub fn backend_is_empty(&self) -> bool {
        self.backend_sealed
            && self
                .backend
                .as_ref()
                .expect("backend missing during retirement")
                .is_empty()
    }

    pub fn close_backend(&mut self) -> Result<(), SystemCallError> {
        let backend = self
            .backend
            .take()
            .expect("backend disappeared before close");
        if let Err(backend) = backend.close() {
            self.backend = Some(backend);
            return Err(SystemCallError::ObjectBusy);
        }
        Ok(())
    }
}

pub struct Retirement {
    owner: Capability,
    source_kind: SourceKind,
    signal_bit: u64,
    source: Option<SourceId>,
    requested: bool,
    removing: bool,
    rearm_needed: bool,
    stopping: bool,
}

impl Retirement {
    pub fn new(owner: Capability, source_kind: SourceKind, signal_bit: u64) -> Self {
        Self {
            owner,
            source_kind,
            signal_bit,
            source: None,
            requested: false,
            removing: false,
            rearm_needed: false,
            stopping: false,
        }
    }

    pub fn advance<B: ProviderBackend<Capability>, F>(
        &mut self,
        state: &mut State<B>,
        requests: &mut Requests<F>,
        input: &mut Input<'_>,
        budget: usize,
        mut report_failure: impl FnMut(),
    ) -> Result<Advance, SystemCallError> {
        let mut signaled = false;
        while let Some(event) = input.pull() {
            if event.kind != self.source_kind {
                continue;
            }
            if event.error != 0 || event.observed.intersects(ObjectSignals::CLOSED) {
                report_failure();
                self.stopping = true;
            } else if event.observed.intersects(ObjectSignals::READABLE) {
                signaled = true;
                self.rearm_needed = true;
            }
        }
        if signaled {
            let _ = notification::take(self.owner.as_handle(), self.signal_bit)?;
        }

        if state.seal_when_grants_drained(self.stopping) {
            rinlib::debug!("FAL provider shutdown: sealing backend");
        }
        if state.retire_step(budget)? {
            return Ok(Advance {
                work_done: budget,
                step: Step::Runnable,
            });
        }
        if self.stopping && state.backend_is_empty() {
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
                state.close_backend()?;
                rinlib::debug!("FAL provider shutdown: backend retired");
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
                self.source_kind,
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

    pub fn refused<F>(&mut self, failure: RequestFailure<F>) -> bool {
        if let RequestFailure::Source { kind, .. } = failure
            && kind == self.source_kind
        {
            self.requested = false;
            return true;
        }
        false
    }

    pub fn registered<B>(&mut self, state: &mut State<B>, kind: SourceKind, source: SourceId) {
        if kind == self.source_kind {
            self.requested = false;
            self.source = Some(source);
            state.retire_ready = true;
        }
    }

    pub fn unregistered(&mut self, kind: SourceKind, source: SourceId) {
        if kind == self.source_kind && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.removing = false;
            self.rearm_needed = false;
        }
    }

    pub fn stop(&mut self) {
        self.stopping = true;
    }
}

pub trait Host<B> {
    fn provider(&self) -> &State<B>;
    fn provider_mut(&mut self) -> &mut State<B>;
    fn publish_initial_grant(&mut self, task_id: u64, sender: MailboxSender);
    fn stopping(&self) -> bool;
    fn fail(&mut self);
}

enum GrantPublication {
    Root,
    Reply {
        info: protocol::NodeInfo,
        sender: Option<MailboxSender>,
        encoded: bool,
    },
}

pub struct Grant {
    prepared: Option<PreparedGrant>,
    task_id: Option<u64>,
    observer: Option<GrantObserver>,
    publication: GrantPublication,
    outbox: Option<Outbox>,
    source: Option<SourceId>,
    requested: bool,
    removing: bool,
    stopping: bool,
    reply_kind: SourceKind,
    lifetime_kind: SourceKind,
}

impl Grant {
    pub fn root(prepared: PreparedGrant, lifetime_kind: SourceKind) -> Self {
        Self {
            prepared: Some(prepared),
            task_id: None,
            observer: None,
            publication: GrantPublication::Root,
            outbox: None,
            source: None,
            requested: false,
            removing: false,
            stopping: false,
            reply_kind: 0,
            lifetime_kind,
        }
    }

    pub fn reply(
        prepared: PreparedGrant,
        outbox: Outbox,
        info: protocol::NodeInfo,
        reply_kind: SourceKind,
        lifetime_kind: SourceKind,
    ) -> Self {
        Self {
            prepared: Some(prepared),
            task_id: None,
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
            reply_kind,
            lifetime_kind,
        }
    }

    fn drive_publication<H, B, F>(
        &mut self,
        _host: &mut H,
        requests: &mut Requests<F>,
        budget: usize,
    ) -> Result<Option<Advance>, SystemCallError>
    where
        H: Host<B>,
        B: ProviderBackend<Capability>,
    {
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

    pub fn advance<H, B, F>(
        &mut self,
        id: u64,
        host: &mut H,
        requests: &mut Requests<F>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Result<Advance, SystemCallError>
    where
        H: Host<B>,
        B: ProviderBackend<Capability>,
    {
        self.task_id = Some(id);
        while let Some(event) = input.pull() {
            if event.kind == self.reply_kind {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.observe(event);
                }
            } else if event.kind == self.lifetime_kind
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
                    active.stop(host);
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
                let retire_task = host.provider().retire_task;
                requests.wake(retire_task)?;
                let removed = host
                    .provider_mut()
                    .grants
                    .as_mut()
                    .expect("grant table missing during retirement")
                    .remove(observer.context());
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
            if let Some(advance) = self.drive_publication(host, requests, 1)? {
                return Ok(advance);
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        if !host.provider().retire_ready {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if !self.requested {
            let prepared = self
                .prepared
                .as_ref()
                .expect("grant installation lost prepared owner");
            requests.add_source(
                prepared.lifetime_handle(),
                ObjectSignals::CLOSED,
                self.lifetime_kind,
            )?;
            self.requested = true;
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Runnable,
        })
    }

    pub fn refused<H, B, F>(&mut self, host: &mut H, failure: RequestFailure<F>)
    where
        H: Host<B>,
        B: ProviderBackend<Capability>,
    {
        match failure {
            RequestFailure::Source { kind, .. } if kind == self.lifetime_kind => {
                self.requested = false;
                self.stopping = true;
                if !host.stopping() {
                    host.fail();
                }
            }
            failure => {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.refused(host, failure);
                }
            }
        }
    }

    pub fn registered<H, B>(&mut self, host: &mut H, kind: SourceKind, source: SourceId)
    where
        H: Host<B>,
        B: ProviderBackend<Capability>,
    {
        if kind == self.reply_kind {
            if let Some(outbox) = self.outbox.as_mut() {
                outbox.registered(host, kind, source);
            }
            return;
        }
        if kind != self.lifetime_kind {
            return;
        }
        self.requested = false;
        self.source = Some(source);
        let prepared = self
            .prepared
            .take()
            .expect("grant source registered without prepared owner");
        let installed = host
            .provider_mut()
            .grants
            .as_mut()
            .expect("grant table missing during installation")
            .install(prepared);
        match &mut self.publication {
            GrantPublication::Root => host.publish_initial_grant(
                self.task_id.expect("initial grant task id missing"),
                installed.sender,
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

    pub fn unregistered<H>(&mut self, host: &mut H, kind: SourceKind, source: SourceId) {
        if kind == self.reply_kind {
            if let Some(outbox) = self.outbox.as_mut() {
                outbox.unregistered(host, kind, source);
            }
        } else if kind == self.lifetime_kind && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.removing = false;
        }
    }

    pub fn take_refused_outbox(&mut self) -> Outbox {
        self.outbox.take().expect("grant refusal lost reply owner")
    }
    pub fn stop(&mut self) {
        self.stopping = true;
    }
    pub fn deadline(&self) -> Deadline {
        match &self.publication {
            GrantPublication::Root => Deadline::INFINITE,
            GrantPublication::Reply { .. } => self
                .outbox
                .as_ref()
                .map_or(Deadline::INFINITE, Outbox::deadline),
        }
    }
}

pub fn backend_status(error: BackendError) -> protocol::Status {
    match error {
        BackendError::NotFound => protocol::Status::NotFound,
        BackendError::NotDirectory => protocol::Status::NotDirectory,
        BackendError::Permission => protocol::Status::Permission,
        BackendError::Exists => protocol::Status::Exists,
        BackendError::NotEmpty => protocol::Status::NotEmpty,
        BackendError::Conflict => protocol::Status::Conflict,
        BackendError::InvalidName | BackendError::Cycle | BackendError::WrongType => {
            protocol::Status::Invalid
        }
        BackendError::Unsupported => protocol::Status::Unsupported,
        BackendError::CrossDevice => protocol::Status::CrossDevice,
        BackendError::Busy | BackendError::Closed => protocol::Status::Busy,
        BackendError::Resource(SystemCallError::QuotaExceeded) => protocol::Status::Quota,
        BackendError::Resource(SystemCallError::OutOfMemory) => protocol::Status::Resource,
        BackendError::Resource(_) => protocol::Status::Internal,
    }
}

pub struct WatchOwner {
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

impl WatchOwner {
    #[expect(
        clippy::too_many_arguments,
        reason = "WatchOwner 一次接管节点、访问、signaler 与两笔 Charge，避免未完成 owner 的中间结构"
    )]
    pub fn new(
        id: u64,
        context: u64,
        node: NodeRef,
        access: AccessSnapshot,
        path: String,
        mask: protocol::WatchMask,
        signaler: Capability,
        watch_charge: Charge,
        source_charge: Charge,
    ) -> Self {
        Self {
            id,
            context,
            node,
            access,
            path,
            mask,
            signaler: Some(signaler),
            _watch_charge: watch_charge,
            _source_charge: source_charge,
        }
    }
}

pub struct Watch {
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
    reply_kind: SourceKind,
    owner_kind: SourceKind,
}

impl Watch {
    pub fn new(
        owner: WatchOwner,
        outbox: Outbox,
        reply_kind: SourceKind,
        owner_kind: SourceKind,
    ) -> Self {
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
            reply_kind,
            owner_kind,
        }
    }

    fn record_outbox<H, B>(&mut self, host: &mut H) -> Result<(), SystemCallError>
    where
        H: Host<B>,
        B: ProviderBackend<Capability>,
    {
        let Some(outbox) = self.outbox.as_ref() else {
            return Ok(());
        };
        if !outbox.is_complete() || self.recorded {
            return Ok(());
        };
        host.provider_mut().record_response(
            true,
            matches!(outbox.result(), Some(OutboxResult::Abandoned(_))),
        )?;
        self.recorded = true;
        Ok(())
    }

    fn encode_reply(&mut self) -> Result<(), SystemCallError> {
        if self.encoded {
            return Ok(());
        };
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

    pub fn advance<H, B, F>(
        &mut self,
        id: u64,
        host: &mut H,
        requests: &mut Requests<F>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError>
    where
        H: Host<B>,
        B: ProviderBackend<Capability>,
    {
        self.task_id.get_or_insert(id);
        while let Some(event) = input.pull() {
            if event.kind == self.reply_kind {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.observe(event);
                }
            } else if event.kind == self.owner_kind
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
                let signaler = self
                    .owner
                    .signaler
                    .as_ref()
                    .expect("Watch signaler missing before registration");
                requests.add_source(
                    signaler.as_handle(),
                    ObjectSignals::CLOSED,
                    self.owner_kind,
                )?;
                self.requested = true;
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if !self.stopping && self.info.is_some() && !host.provider().watches.contains(self.owner.id)
        {
            self.stopping = true;
        }
        if self.stopping {
            if host.provider().watches.contains(self.owner.id) {
                if self.provider_stopping {
                    let bits = host
                        .provider_mut()
                        .watches
                        .take_pending(self.owner.id)
                        .map_or(protocol::WatchMask::NONE, |(pending, _)| pending)
                        | protocol::WatchMask::TERMINATED;
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
                host.provider_mut().watches.remove(self.owner.id);
            }
            if let Some(outbox) = self.outbox.as_mut() {
                if !outbox.is_complete() {
                    outbox.stop(host);
                    let advance = outbox.drive(requests, budget)?;
                    if !outbox.is_complete() {
                        return Ok(advance);
                    }
                }
                self.record_outbox(host)?;
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
        if let Some((pending, _)) = host.provider_mut().watches.take_pending(self.owner.id)
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
                self.record_outbox(host)?;
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

    pub fn refused<H, B, F>(&mut self, host: &mut H, failure: RequestFailure<F>)
    where
        H: Host<B>,
        B: ProviderBackend<Capability>,
    {
        match failure {
            RequestFailure::Source { kind, error } if kind == self.owner_kind => {
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
                    outbox.refused(host, failure);
                }
            }
        }
    }

    pub fn registered<H, B>(&mut self, host: &mut H, kind: SourceKind, source: SourceId)
    where
        H: Host<B>,
        B: ProviderBackend<Capability>,
    {
        if kind == self.reply_kind {
            if let Some(outbox) = self.outbox.as_mut() {
                outbox.registered(host, kind, source);
            }
            return;
        }
        if kind != self.owner_kind {
            return;
        }
        self.requested = false;
        self.source = Some(source);
        let generation = {
            let backend = host
                .provider()
                .backend
                .as_ref()
                .expect("provider backend missing during Watch installation");
            let (current, generation) =
                match backend.watch_snapshot(&self.owner.access, &self.owner.path) {
                    Ok(result) => result,
                    Err(error) => {
                        self.registration_error = Some(backend_status(error));
                        return;
                    }
                };
            if current.id() != self.owner.node.id() {
                self.registration_error = Some(protocol::Status::Conflict);
                return;
            }
            generation
        };
        let info = host.provider_mut().watches.install(
            self.owner.id,
            self.owner.context,
            self.owner.node.id(),
            self.task_id.expect("Watch task id missing at installation"),
            generation,
            self.owner.mask,
        );
        self.info = Some(info);
    }

    pub fn unregistered<H>(&mut self, host: &mut H, kind: SourceKind, source: SourceId) {
        if kind == self.reply_kind {
            if let Some(outbox) = self.outbox.as_mut() {
                outbox.unregistered(host, kind, source);
            }
        } else if kind == self.owner_kind && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.removing = false;
        }
    }

    pub fn stop(&mut self) {
        self.provider_stopping = true;
        self.stopping = true;
    }
    pub fn deadline(&self) -> Deadline {
        self.outbox
            .as_ref()
            .map_or(Deadline::INFINITE, Outbox::deadline)
    }
    pub fn take_refused_outbox(&mut self) -> Outbox {
        self.outbox.take().expect("Watch refusal lost reply owner")
    }
}
