use super::*;
use libservice::authority::PreparedAuthority;
use rinlib::ipc::message::MintedSender;

pub(super) enum AuthorityPublication {
    Root,
    Reply,
}

pub(super) struct AuthorityTask {
    prepared: Option<PreparedAuthority>,
    sender: Option<MailboxSender>,
    lifetime: Option<Capability>,
    charge: Option<Charge>,
    identity: u64,
    info: Option<service_protocol::AuthorityInfo>,
    publication: AuthorityPublication,
    outbox: Option<Outbox>,
    source: Option<SourceId>,
    requested: bool,
    removing: bool,
    stopping: bool,
    encoded: bool,
}

impl AuthorityTask {
    fn new(
        prepared: PreparedAuthority,
        minted: MintedSender,
        charge: Charge,
        publication: AuthorityPublication,
        outbox: Option<Outbox>,
    ) -> Self {
        let identity = minted
            .sender
            .description()
            .expect("minted authority description failed")
            .object_id;
        Self {
            prepared: Some(prepared),
            sender: Some(minted.sender),
            lifetime: Some(minted.lifetime),
            charge: Some(charge),
            identity,
            info: None,
            publication,
            outbox,
            source: None,
            requested: false,
            removing: false,
            stopping: false,
            encoded: false,
        }
    }

    pub(super) fn root(prepared: PreparedAuthority, minted: MintedSender, charge: Charge) -> Self {
        Self::new(prepared, minted, charge, AuthorityPublication::Root, None)
    }

    pub(super) fn reply(
        prepared: PreparedAuthority,
        minted: MintedSender,
        charge: Charge,
        outbox: Outbox,
    ) -> Self {
        Self::new(
            prepared,
            minted,
            charge,
            AuthorityPublication::Reply,
            Some(outbox),
        )
    }

    pub(super) fn take_refused_outbox(&mut self) -> Option<Outbox> {
        self.outbox.take()
    }
}

impl<B: RuntimeBackend> Task<World<B>> for AuthorityTask {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            if event.kind == KIND_AUTHORITY_LIFETIME {
                if event.error != 0 || event.observed.intersects(ObjectSignals::CLOSED) {
                    self.stopping = true;
                }
            } else if let Some(outbox) = self.outbox.as_mut() {
                outbox.observe(event);
            }
        }
        if input.take_timeout()
            && let Some(outbox) = self.outbox.as_mut()
        {
            outbox.timed_out();
        }
        if self.stopping {
            if let Some(outbox) = self.outbox.as_mut() {
                outbox.stop(world);
            }
            self.sender = None;
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
                if self.info.take().is_some() {
                    world
                        .provider
                        .backend
                        .as_mut()
                        .and_then(RuntimeBackend::registry_mut)
                        .expect("authority retired after Registry disappeared")
                        .remove_authority(self.identity)
                        .expect("observed authority disappeared before retirement");
                }
                self.prepared = None;
                self.lifetime = None;
                self.charge = None;
            }
        } else if self.source.is_none() && !self.requested {
            requests.add_source(
                self.lifetime
                    .as_ref()
                    .expect("authority lost Lifetime")
                    .as_handle(),
                ObjectSignals::CLOSED,
                KIND_AUTHORITY_LIFETIME,
            )?;
            self.requested = true;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if let Some(outbox) = self.outbox.as_mut() {
            if let Some(info) = self.info
                && !self.stopping
                && outbox.result().is_none()
            {
                if !outbox.is_admitted() {
                    return Ok(outbox.admit(requests));
                }
                if !self.encoded {
                    let used = service_protocol::encode_response(
                        service_protocol::Op::DelegateName,
                        service_protocol::Status::Ok,
                        outbox.deadline(),
                        service_protocol::Response::Authority(info),
                        outbox.response_mut()?.body_mut()?,
                    )
                    .ok_or(SystemCallError::InternalError)?;
                    let sender = self
                        .sender
                        .take()
                        .expect("authority reply lost unique sender");
                    outbox
                        .response_mut()?
                        .push(
                            sender.into_capability(),
                            Rights::WRITE
                                | Rights::WAIT
                                | Rights::TRANSIT
                                | Rights::DUPLICATE
                                | Rights::GRANT,
                        )
                        .map_err(|failure| failure.error)?;
                    outbox.response_mut()?.finish_body(used)?;
                    self.encoded = true;
                }
            }
            let advance = outbox.drive(requests, budget)?;
            if outbox.is_complete() && !matches!(outbox.result(), Some(OutboxResult::Sent)) {
                self.stopping = true;
            }
            if !self.stopping && !outbox.is_complete() {
                return Ok(advance);
            }
        }
        Ok(Advance {
            work_done: 1,
            step: if self.stopping {
                if self.source.is_none()
                    && !self.requested
                    && self.outbox.as_ref().is_none_or(Outbox::is_complete)
                {
                    Step::Complete
                } else {
                    Step::Runnable
                }
            } else {
                Step::Parked
            },
        })
    }

    fn refused(&mut self, world: &mut World<B>, failure: RequestFailure<ServiceTask<B>>) {
        match failure {
            RequestFailure::Source {
                kind: KIND_AUTHORITY_LIFETIME,
                ..
            } => {
                self.requested = false;
                self.stopping = true;
                if matches!(self.publication, AuthorityPublication::Root) {
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

    fn registered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        if kind != KIND_AUTHORITY_LIFETIME {
            if let Some(outbox) = self.outbox.as_mut() {
                outbox.registered(world, kind, source);
            }
            return;
        }
        self.source = Some(source);
        self.requested = false;
        if self.stopping {
            return;
        }
        let prepared = self
            .prepared
            .take()
            .expect("authority source lost prepared owner");
        let registry = world
            .provider
            .backend
            .as_mut()
            .and_then(RuntimeBackend::registry_mut)
            .expect("authority requires Registry");
        match registry.install_authority(prepared, self.identity) {
            Ok(info) => {
                self.info = Some(info);
                if matches!(self.publication, AuthorityPublication::Root) {
                    let sender = self
                        .sender
                        .take()
                        .expect("root authority lost unique sender");
                    assert!(world.registration_root_sender.replace(sender).is_none());
                }
            }
            Err(failure) => {
                drop(failure.prepared);
                self.stopping = true;
                if matches!(self.publication, AuthorityPublication::Root) && !world.stop {
                    world.failed = true;
                }
            }
        }
    }

    fn unregistered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        if kind == KIND_AUTHORITY_LIFETIME && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.removing = false;
        } else if let Some(outbox) = self.outbox.as_mut() {
            outbox.unregistered(world, kind, source);
        }
    }

    fn stop(&mut self, world: &mut World<B>) {
        self.stopping = true;
        if matches!(self.publication, AuthorityPublication::Root) {
            world.registration_root_sender = None;
        }
        if let Some(outbox) = self.outbox.as_mut() {
            outbox.stop(world);
        }
    }

    fn deadline(&self) -> Deadline {
        self.outbox
            .as_ref()
            .map_or(Deadline::INFINITE, Outbox::deadline)
    }
}
