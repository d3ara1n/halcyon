use super::*;

pub(super) struct PendingRegistration {
    pub(super) authority_identity: u64,
    pub(super) name: String,
    pub(super) protocol: u64,
    pub(super) version: u32,
    pub(super) policy: ExportPolicy,
    pub(super) establish_deadline: Deadline,
    pub(super) endpoint: Capability,
    pub(super) instance: u64,
}

pub(super) struct IssuedRegistration {
    pub(super) pending: PendingRegistration,
    pub(super) sender: MailboxSender,
    pub(super) lifetime: Capability,
    pub(super) endpoint_observer: Capability,
    pub(super) lifetime_charge: Charge,
    pub(super) endpoint_charge: Charge,
}

pub(super) struct RegistrationReplyTask {
    outbox: Option<Outbox>,
    op: service_protocol::Op,
    status: service_protocol::Status,
    response: service_protocol::Response,
    action: Option<(u64, RegistrationCommand)>,
    pending: Option<PendingRegistration>,
    sender: Option<MailboxSender>,
    lifetime: Option<Capability>,
    endpoint_observer: Option<Capability>,
    lifetime_charge: Option<Charge>,
    endpoint_charge: Option<Charge>,
    control: Option<RegistrationControl>,
    lifetime_source: Option<SourceId>,
    endpoint_source: Option<SourceId>,
    lifetime_requested: bool,
    endpoint_requested: bool,
    lifetime_removing: bool,
    endpoint_removing: bool,
    endpoint_released: bool,
    endpoint_retiring: bool,
    retire_wake: bool,
    stopping: bool,
    establish_deadline: Deadline,
    encoded: bool,
    wakes: WakeBatch,
}

impl RegistrationReplyTask {
    pub(super) fn new(
        outbox: Outbox,
        op: service_protocol::Op,
        status: service_protocol::Status,
        response: service_protocol::Response,
        issued: Option<IssuedRegistration>,
        action: Option<(u64, RegistrationCommand)>,
        wakes: WakeBatch,
    ) -> Self {
        let establish_deadline = issued.as_ref().map_or(Deadline::INFINITE, |issued| {
            issued.pending.establish_deadline
        });
        let (pending, sender, lifetime, endpoint_observer, lifetime_charge, endpoint_charge) =
            if let Some(issued) = issued {
                (
                    Some(issued.pending),
                    Some(issued.sender),
                    Some(issued.lifetime),
                    Some(issued.endpoint_observer),
                    Some(issued.lifetime_charge),
                    Some(issued.endpoint_charge),
                )
            } else {
                (None, None, None, None, None, None)
            };
        let endpoint_released = endpoint_observer.is_none();
        Self {
            outbox: Some(outbox),
            op,
            status,
            response,
            action,
            pending,
            sender,
            lifetime,
            endpoint_observer,
            lifetime_charge,
            endpoint_charge,
            control: None,
            lifetime_source: None,
            endpoint_source: None,
            lifetime_requested: false,
            endpoint_requested: false,
            lifetime_removing: false,
            endpoint_removing: false,
            endpoint_released,
            endpoint_retiring: false,
            retire_wake: false,
            stopping: false,
            establish_deadline,
            encoded: false,
            wakes,
        }
    }

    pub(super) fn take_refused_outbox(&mut self) -> Option<(Outbox, service_protocol::Op)> {
        self.outbox.take().map(|outbox| (outbox, self.op))
    }

    fn execute_action<B: RuntimeBackend>(
        &mut self,
        world: &mut World<B>,
        now_ns: u64,
    ) -> Result<(), SystemCallError> {
        let (sender_context, command) = self.action.take().ok_or(SystemCallError::InternalError)?;
        let mut effects = Effects::default();
        let mut wake_control = None;
        let result = match command {
            RegistrationCommand::Withdraw(name, instance, generation) => {
                let registry = world
                    .provider
                    .backend
                    .as_mut()
                    .and_then(RuntimeBackend::registry_mut)
                    .ok_or(RegistryError::Closed);
                registry.and_then(|registry| {
                    let transition =
                        registry.withdraw(sender_context, &name, instance, generation)?;
                    effects = transition.effects;
                    wake_control = world
                        .registration_controls
                        .iter()
                        .find(|control| control.instance == instance)
                        .map(|control| control.task_id);
                    Ok(None)
                })
            }
            RegistrationCommand::PublishReady | RegistrationCommand::BeginDrain => {
                let control = world
                    .registration_controls
                    .iter()
                    .copied()
                    .find(|control| control.instance == sender_context)
                    .ok_or(RegistryError::Permission);
                control.and_then(|control| {
                    let registry = world
                        .provider
                        .backend
                        .as_mut()
                        .and_then(RuntimeBackend::registry_mut)
                        .ok_or(RegistryError::Closed)?;
                    let transition = if matches!(command, RegistrationCommand::PublishReady) {
                        registry.publish_ready(control.instance, now_ns)?
                    } else {
                        registry.begin_drain(
                            control.instance,
                            service_protocol::TerminalReason::Withdrawn,
                        )?
                    };
                    effects = transition.effects;
                    wake_control = Some(control.task_id);
                    Ok(Some(transition.info))
                })
            }
            _ => return Err(SystemCallError::InternalError),
        };
        match result {
            Ok(Some(info)) => self.response = service_protocol::Response::Instance(info),
            Ok(None) => {}
            Err(error) => self.status = service_status(error),
        }
        publish_watch_events(world, &effects, &mut self.wakes);
        if let Some(task_id) = wake_control {
            self.wakes.push_unique(task_id);
        }
        Ok(())
    }

    fn drain<B: RuntimeBackend>(
        &mut self,
        world: &mut World<B>,
        reason: service_protocol::TerminalReason,
    ) {
        if let Some(control) = self.control {
            let effects = drain_registration(world, control.instance, reason);
            publish_watch_events(world, &effects, &mut self.wakes);
        }
        self.establish_deadline = Deadline::INFINITE;
        self.endpoint_retiring = true;
    }

    fn close_control<B: RuntimeBackend>(
        &mut self,
        world: &mut World<B>,
        reason: service_protocol::TerminalReason,
    ) {
        if let Some(control) = self.control {
            let effects = remove_registration_control(world, control, reason);
            publish_watch_events(world, &effects, &mut self.wakes);
        }
        self.establish_deadline = Deadline::INFINITE;
        self.endpoint_retiring = true;
        self.stopping = true;
        self.pending = None;
        self.sender = None;
        if let Some(outbox) = self.outbox.as_mut() {
            outbox.stop(world);
        }
    }

    fn prepare_sources<B: RuntimeBackend>(
        &mut self,
        requests: &mut Requests<ServiceTask<B>>,
    ) -> Result<Option<Advance>, SystemCallError> {
        if self.pending.is_none() {
            return Ok(None);
        }
        let outbox = self
            .outbox
            .as_mut()
            .expect("pending registration lost Outbox");
        if outbox.result().is_some() {
            self.stopping = true;
            self.pending = None;
            self.sender = None;
            self.endpoint_retiring = true;
            return Ok(None);
        }
        if !outbox.is_admitted() {
            return Ok(Some(outbox.admit(requests)));
        }
        if self.lifetime_source.is_none() && !self.lifetime_requested {
            requests.add_source(
                self.lifetime
                    .as_ref()
                    .expect("pending registration lost control Lifetime")
                    .as_handle(),
                ObjectSignals::CLOSED,
                KIND_REGISTRATION_LIFETIME,
            )?;
            self.lifetime_requested = true;
            return Ok(Some(Advance {
                work_done: 1,
                step: Step::Runnable,
            }));
        }
        if self.endpoint_source.is_none() && !self.endpoint_requested {
            requests.add_source(
                self.endpoint_observer
                    .as_ref()
                    .expect("pending registration lost endpoint observer")
                    .as_handle(),
                ObjectSignals::CLOSED,
                KIND_REGISTRATION_ENDPOINT,
            )?;
            self.endpoint_requested = true;
            return Ok(Some(Advance {
                work_done: 1,
                step: Step::Runnable,
            }));
        }
        Ok(None)
    }

    fn install<B: RuntimeBackend>(&mut self, id: u64, world: &mut World<B>, now_ns: u64) {
        if self.lifetime_source.is_none() || self.endpoint_source.is_none() {
            return;
        }
        let pending = self
            .pending
            .take()
            .expect("registration installation lost input");
        let deadline = pending
            .establish_deadline
            .instant()
            .expect("validated establish deadline changed")
            .expect("validated establish deadline became infinite");
        if now_ns >= deadline {
            self.status = service_protocol::Status::Expired;
            self.sender = None;
            self.endpoint_retiring = true;
            return;
        }
        let result = world
            .provider
            .backend
            .as_mut()
            .and_then(RuntimeBackend::registry_mut)
            .expect("RegistrationControl requires Registry")
            .register(
                pending.authority_identity,
                &pending.name,
                pending.instance,
                pending.protocol,
                pending.version,
                pending.policy,
                pending.establish_deadline,
                pending.endpoint,
            );
        match result {
            Ok(info) => {
                assert!(
                    world.registration_controls.len() < world.registration_controls.capacity(),
                    "registration control admission exceeded prepared capacity"
                );
                let control = RegistrationControl {
                    instance: pending.instance,
                    task_id: id,
                };
                world.registration_controls.push(control);
                self.control = Some(control);
                self.response = service_protocol::Response::Instance(info);
            }
            Err(failure) => {
                self.status = service_status(failure.error);
                drop(failure.endpoint);
                self.sender = None;
                self.endpoint_retiring = true;
            }
        }
    }

    fn release_sources<B: RuntimeBackend>(
        &mut self,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
    ) -> bool {
        let mut retry = false;
        if self.endpoint_retiring || self.stopping {
            if let Some(source) = self.endpoint_source
                && !self.endpoint_removing
            {
                if requests.remove(source).is_ok() {
                    self.endpoint_removing = true;
                } else {
                    retry = true;
                }
            }
            if self.endpoint_source.is_none() && !self.endpoint_requested && !self.endpoint_released
            {
                self.endpoint_observer = None;
                self.endpoint_charge = None;
                if let Some(control) = self.control
                    && let Some(registry) = world
                        .provider
                        .backend
                        .as_mut()
                        .and_then(RuntimeBackend::registry_mut)
                {
                    registry.release_endpoint_observation(control.instance);
                    self.retire_wake = true;
                }
                self.endpoint_released = true;
            }
        }
        if self.stopping {
            if let Some(source) = self.lifetime_source
                && !self.lifetime_removing
            {
                if requests.remove(source).is_ok() {
                    self.lifetime_removing = true;
                } else {
                    retry = true;
                }
            }
            if self.lifetime_source.is_none() && !self.lifetime_requested {
                self.lifetime = None;
                self.lifetime_charge = None;
            }
        }
        if self.retire_wake {
            if requests.wake(world.provider.retire_task).is_ok() {
                self.retire_wake = false;
            } else {
                retry = true;
            }
        }
        retry
    }
}

impl<B: RuntimeBackend> Task<World<B>> for RegistrationReplyTask {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        id: u64,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if !self.wakes.is_empty() && !self.wakes.drive(requests)? {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        let mut control_closed = false;
        let mut endpoint_closed = false;
        while let Some(event) = input.pull() {
            match event.kind {
                KIND_REGISTRATION_LIFETIME => {
                    control_closed |=
                        event.error != 0 || event.observed.intersects(ObjectSignals::CLOSED);
                }
                KIND_REGISTRATION_ENDPOINT => {
                    endpoint_closed |=
                        event.error != 0 || event.observed.intersects(ObjectSignals::CLOSED);
                }
                _ => {
                    if let Some(outbox) = self.outbox.as_mut() {
                        outbox.observe(event);
                    }
                }
            }
        }
        if control_closed {
            self.close_control(world, service_protocol::TerminalReason::ControlClosed);
        } else if endpoint_closed {
            self.drain(world, service_protocol::TerminalReason::EndpointClosed);
            if self.pending.take().is_some() {
                self.status = service_protocol::Status::Cancelled;
                self.sender = None;
            }
        }
        if input.take_timeout() {
            let now_ns = input.now_ns();
            if let Some(outbox) = self.outbox.as_mut()
                && outbox
                    .deadline()
                    .instant()
                    .ok()
                    .flatten()
                    .is_some_and(|deadline| now_ns >= deadline)
            {
                outbox.timed_out();
            }
            if let Some(control) = self.control
                && !self.stopping
                && self
                    .establish_deadline
                    .instant()
                    .ok()
                    .flatten()
                    .is_some_and(|deadline| now_ns >= deadline)
            {
                let instance = control.instance;
                let expired = world
                    .provider
                    .backend
                    .as_mut()
                    .and_then(RuntimeBackend::registry_mut)
                    .ok_or(SystemCallError::InternalError)?
                    .expire_if_starting(instance, now_ns)
                    .map_err(|_| SystemCallError::InternalError)?;
                self.establish_deadline = Deadline::INFINITE;
                self.endpoint_retiring |= expired;
            }
        }
        if !self.stopping && self.action.is_some() {
            let outbox = self.outbox.as_mut().ok_or(SystemCallError::InternalError)?;
            if outbox.result().is_none() {
                if !outbox.is_admitted() {
                    return Ok(outbox.admit(requests));
                }
                self.execute_action(world, input.now_ns())?;
            } else {
                self.action = None;
            }
        }
        if !self.stopping {
            if let Some(advance) = self.prepare_sources(requests)? {
                return Ok(advance);
            }
            if self.pending.is_some()
                && self.lifetime_source.is_some()
                && self.endpoint_source.is_some()
            {
                self.install(id, world, input.now_ns());
            }
            if let Some(control) = self.control
                && let Some(registry) = world
                    .provider
                    .backend
                    .as_ref()
                    .and_then(RuntimeBackend::registry)
                && let Ok(info) = registry.query(control.instance)
            {
                match info.state {
                    service_protocol::State::Starting => {}
                    service_protocol::State::Ready => self.establish_deadline = Deadline::INFINITE,
                    service_protocol::State::Draining | service_protocol::State::Terminal => {
                        self.establish_deadline = Deadline::INFINITE;
                        self.endpoint_retiring = true;
                        if self
                            .outbox
                            .as_ref()
                            .is_some_and(|outbox| outbox.result().is_none())
                        {
                            self.outbox.as_mut().unwrap().stop(world);
                        }
                    }
                }
            }
        }
        let mut outbox_runnable = false;
        if let Some(outbox) = self.outbox.as_mut() {
            if !self.stopping && self.pending.is_none() && outbox.result().is_none() {
                if !outbox.is_admitted() {
                    return Ok(outbox.admit(requests));
                }
                if !self.encoded {
                    let used = service_protocol::encode_response(
                        self.op,
                        self.status,
                        outbox.deadline(),
                        self.response,
                        outbox.response_mut()?.body_mut()?,
                    )
                    .ok_or(SystemCallError::InternalError)?;
                    if let Some(sender) = self.sender.take() {
                        outbox
                            .response_mut()?
                            .push(
                                sender.into_capability(),
                                Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
                            )
                            .map_err(|failure| failure.error)?;
                    }
                    outbox.response_mut()?.finish_body(used)?;
                    self.encoded = true;
                }
            }
            outbox_runnable = matches!(outbox.drive(requests, budget)?.step, Step::Runnable);
            if outbox.is_complete() {
                let sent = matches!(outbox.result(), Some(OutboxResult::Sent));
                self.outbox = None;
                if !sent || self.control.is_none() {
                    self.close_control(world, service_protocol::TerminalReason::ControlClosed);
                }
            } else if matches!(outbox.result(), Some(OutboxResult::Abandoned(_))) && !self.stopping
            {
                self.close_control(world, service_protocol::TerminalReason::ControlClosed);
            }
        }
        self.action = None;
        if self.stopping {
            self.pending = None;
            self.sender = None;
            if let Some(outbox) = self.outbox.as_mut() {
                outbox.stop(world);
            }
        }
        let retry = self.release_sources(world, requests);
        let wakes_done = self.wakes.drive(requests)?;
        let done = self.stopping
            && self.outbox.is_none()
            && self.pending.is_none()
            && self.endpoint_released
            && self.lifetime_source.is_none()
            && !self.lifetime_requested
            && self.lifetime.is_none()
            && wakes_done
            && !self.retire_wake;
        let runnable = outbox_runnable
            || retry
            || !wakes_done
            || self.retire_wake
            || self.lifetime_requested
            || self.endpoint_requested
            || self.lifetime_removing
            || self.endpoint_removing
            || self
                .outbox
                .as_ref()
                .is_some_and(|outbox| outbox.result().is_some());
        Ok(Advance {
            work_done: 1,
            step: if done {
                Step::Complete
            } else if runnable {
                Step::Runnable
            } else {
                Step::Parked
            },
        })
    }

    fn refused(&mut self, world: &mut World<B>, failure: RequestFailure<ServiceTask<B>>) {
        match failure {
            RequestFailure::Source {
                kind: KIND_REGISTRATION_LIFETIME,
                ..
            } => {
                self.lifetime_requested = false;
                self.close_control(world, service_protocol::TerminalReason::ControlClosed);
            }
            RequestFailure::Source {
                kind: KIND_REGISTRATION_ENDPOINT,
                ..
            } => {
                self.endpoint_requested = false;
                self.close_control(world, service_protocol::TerminalReason::ControlClosed);
            }
            RequestFailure::Wake { .. } => self.retire_wake = true,
            failure => {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.refused(world, failure);
                }
            }
        }
    }

    fn registered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        match kind {
            KIND_REGISTRATION_LIFETIME => {
                self.lifetime_source = Some(source);
                self.lifetime_requested = false;
            }
            KIND_REGISTRATION_ENDPOINT => {
                self.endpoint_source = Some(source);
                self.endpoint_requested = false;
            }
            _ => {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.registered(world, kind, source);
                }
            }
        }
    }

    fn unregistered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        match kind {
            KIND_REGISTRATION_LIFETIME if self.lifetime_source == Some(source) => {
                self.lifetime_source = None;
                self.lifetime_requested = false;
                self.lifetime_removing = false;
            }
            KIND_REGISTRATION_ENDPOINT if self.endpoint_source == Some(source) => {
                self.endpoint_source = None;
                self.endpoint_requested = false;
                self.endpoint_removing = false;
            }
            _ => {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.unregistered(world, kind, source);
                }
            }
        }
    }

    fn stop(&mut self, world: &mut World<B>) {
        self.close_control(world, service_protocol::TerminalReason::ProviderStopping);
    }

    fn deadline(&self) -> Deadline {
        let reply = self
            .outbox
            .as_ref()
            .map_or(Deadline::INFINITE, Outbox::deadline);
        if self.control.is_none() || self.stopping {
            return reply;
        }
        match (
            reply.instant().ok().flatten(),
            self.establish_deadline.instant().ok().flatten(),
        ) {
            (Some(reply), Some(establish)) => Deadline::at(reply.min(establish)),
            (None, _) => self.establish_deadline,
            _ => reply,
        }
    }
}
