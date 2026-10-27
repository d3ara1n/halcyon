//! 根和派生来源的实际 Lifetime owner；入口只在两个观察来源准入后交付。
use super::*;
use libdevice::{
    protocol,
    registry::{Access, Scope, SourceIds},
};
use libkernel::ipc::packet::Packet;

pub(super) struct Source {
    ids: SourceIds,
    issuer: Option<u64>,
    binding: Option<libdevice::identity::BindingRef>,
    binding_owner: Option<Binding>,
    recovered: bool,
    scope: Scope,
    access: Access,
    use_sender: Option<MailboxSender>,
    admin_sender: Option<MailboxSender>,
    use_life: Observed,
    admin_life: Observed,
    committed: bool,
    aborted: bool,
    bootstrap: Option<MailboxSender>,
    published: bool,
    use_reported: bool,
    admin_reported: bool,
    funding: Funding,
    _bytes: Charge,
}
impl Source {
    pub fn observations_pending(&self) -> bool {
        self.use_life.is_pending() || self.admin_life.is_pending()
    }
    pub fn registration_entry(&self) -> Result<MailboxSender, Error> {
        let (sender, _) =
            MailboxSender::from_capability(alias(self.use_sender.as_ref().ok_or(Error::Closed)?)?)
                .map_err(|failure| Error::from(failure.error))?;
        Ok(sender)
    }
    pub fn root(world: &World, bootstrap: MailboxSender) -> Result<Self, Error> {
        let mut value = Self::prepare(
            world,
            None,
            Scope::Root,
            Access::ALL,
            world.funding.clone(),
            None,
        )?;
        value.bootstrap = Some(bootstrap);
        Ok(value)
    }
    pub fn prepare(
        world: &World,
        issuer: Option<u64>,
        scope: Scope,
        access: Access,
        funding: Funding,
        binding: Option<libdevice::identity::BindingRef>,
    ) -> Result<Self, Error> {
        if let Some(issuer) = issuer {
            if let Some(reference) = binding {
                world.registry.check_child_source(issuer, reference)?;
            } else {
                world.registry.check_derive_source(issuer, scope, access)?;
            }
        }
        let bytes = funding
            .domain()
            .acquire(DeviceResource::Bytes, core::mem::size_of::<Self>())?;
        let binding_owner = binding
            .map(
                |reference| match &world.existing_binding(reference).ok_or(Error::Closed)?.kind {
                    RoleKind::Binding(binding) => Ok(binding.clone()),
                    _ => Err(Error::Permission),
                },
            )
            .transpose()?;
        let use_role = world.mailbox.mint(0, TRANSPORT)?;
        let admin_role = world.mailbox.mint(0, TRANSPORT)?;
        let ids = SourceIds {
            use_context: use_role.sender.description()?.object_id,
            admin_context: admin_role.sender.description()?.object_id,
        };
        Ok(Self {
            ids,
            issuer,
            binding,
            binding_owner,
            recovered: false,
            scope,
            access,
            use_sender: Some(use_role.sender),
            admin_sender: Some(admin_role.sender),
            use_life: Observed::lifetime(use_role.lifetime, 4),
            admin_life: Observed::lifetime(admin_role.lifetime, 5),
            committed: false,
            aborted: false,
            bootstrap: None,
            published: false,
            use_reported: false,
            admin_reported: false,
            funding,
            _bytes: bytes,
        })
    }
    pub fn observe(&mut self, event: libexecution::runtime::SourceEvent) {
        self.use_life.observe(event);
        self.admin_life.observe(event);
    }
    pub fn registered(&mut self, kind: u64, id: SourceId) {
        self.use_life.registered(kind, id);
        self.admin_life.registered(kind, id);
    }
    pub fn unregistered(&mut self, id: SourceId) {
        self.use_life.unregistered(id);
        self.admin_life.unregistered(id);
    }
    pub fn deadline(&self) -> Deadline {
        Deadline::INFINITE
    }
    pub fn stop(&mut self, world: &mut World) {
        if self.committed {
            let _ = world.registry.seal_source(self.ids.admin_context);
            world.dirty = true;
        } else {
            self.abort();
        }
    }
    pub fn abort(&mut self) {
        self.aborted = true;
        self.use_sender = None;
        self.admin_sender = None;
        self.use_life.closed = true;
        self.admin_life.closed = true;
    }
    pub fn admit(
        &mut self,
        id: u64,
        world: &mut World,
        requests: &mut Requests<'_, Family>,
    ) -> Result<bool, Error> {
        if self.aborted {
            return Err(Error::Closed);
        }
        let a = self.use_life.drive(requests)?;
        let b = self.admin_life.drive(requests)?;
        if !self.committed && (!a || !b) {
            return Ok(false);
        }
        if !self.committed {
            // 并发重复发行在同一 actor 提交点恢复赢家，不另造子来源。
            if let Some(binding) = &self.binding_owner
                && let Some((a, b)) = binding.registrar()?
            {
                self.use_sender = Some(a);
                self.admin_sender = Some(b);
                self.use_life.closed = true;
                self.admin_life.closed = true;
                self.recovered = true;
                self.committed = true;
                self.binding_owner = None;
                return Ok(true);
            }
            let mothers = self
                .binding_owner
                .as_ref()
                .map(|_| {
                    let a = MailboxSender::from_capability(alias(
                        self.use_sender.as_ref().ok_or(Error::Closed)?,
                    )?)
                    .map_err(|failure| Error::from(failure.error))?
                    .0;
                    let b = MailboxSender::from_capability(alias(
                        self.admin_sender.as_ref().ok_or(Error::Closed)?,
                    )?)
                    .map_err(|failure| Error::from(failure.error))?
                    .0;
                    Ok::<_, Error>((a, b))
                })
                .transpose()?;
            match self.issuer {
                Some(issuer) => {
                    if let Some(reference) = self.binding {
                        world
                            .registry
                            .derive_child_source(issuer, reference, self.ids, id)?;
                    } else {
                        world.registry.derive_source(
                            issuer,
                            self.ids,
                            id,
                            self.scope,
                            self.access,
                        )?;
                    }
                }
                None => world
                    .registry
                    .bootstrap(self.ids, id, self.funding.clone())?,
            }
            self.committed = true;
            if let Some(binding) = self.binding_owner.take() {
                binding.install_registrar(mothers.expect("child source mothers missing"));
            }
        }
        Ok(true)
    }
    pub fn take_senders(&mut self) -> Result<(MailboxSender, MailboxSender), Error> {
        if !self.committed || self.published {
            return Err(Error::NotReady);
        }
        let a = self.use_sender.take().ok_or(Error::NotReady)?;
        let b = self.admin_sender.take().ok_or(Error::NotReady)?;
        self.published = true;
        Ok((a, b))
    }
    pub fn advance(
        &mut self,
        id: u64,
        world: &mut World,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            self.observe(event);
        }
        input.take_timeout();
        if self.aborted {
            self.use_life.drive(requests)?;
            self.admin_life.drive(requests)?;
            return Ok(step(
                if self.use_life.source.is_empty() && self.admin_life.source.is_empty() {
                    Step::Complete
                } else {
                    Step::Runnable
                },
            ));
        }
        if self.recovered {
            self.use_life.drive(requests)?;
            self.admin_life.drive(requests)?;
            return Ok(step(
                if self.use_life.source.is_empty() && self.admin_life.source.is_empty() {
                    Step::Complete
                } else {
                    Step::Parked
                },
            ));
        }
        if !self
            .admit(id, world, requests)
            .unwrap_or_else(|error| fatal(error))
        {
            return Ok(step(Step::Runnable));
        }
        if self.bootstrap.is_some() && !self.published && world.platform_ready {
            let (a, b) = self.take_senders().unwrap_or_else(|error| fatal(error));
            let incarnation = object::query(world.mailbox.as_handle())?.object_id;
            let mut packet = Packet::new(protocol::BOOT_READY, &incarnation.to_le_bytes())?;
            packet
                .push(a.into_capability(), TRANSPORT)
                .map_err(|f| f.error)?;
            packet
                .push(b.into_capability(), TRANSPORT)
                .map_err(|f| f.error)?;
            // 私有 bootstrap 只有这一条就绪消息；发送失败由 init 的预付启动监督收束。
            packet
                .try_send(
                    self.bootstrap.as_ref().expect("device bootstrap missing"),
                    Deadline::INFINITE,
                )
                .map_err(|f| f.error)?;
            self.bootstrap = None;
            libkernel::debug!("device manager ready; registry={}", incarnation);
        }
        if self.use_life.closed && !self.use_reported {
            world
                .registry
                .role_closed(self.ids.use_context)
                .unwrap_or_else(|error| fatal(error));
            world.dirty = true;
            self.use_reported = true;
        }
        world
            .registry
            .synchronize_source(self.ids.use_context)
            .unwrap_or_else(|e| fatal(e));
        if self.admin_life.closed && !self.admin_reported {
            world
                .registry
                .role_closed(self.ids.admin_context)
                .unwrap_or_else(|error| fatal(error));
            self.admin_reported = true;
        }
        if self.use_life.closed
            && self.admin_life.closed
            && self.use_life.source.is_empty()
            && self.admin_life.source.is_empty()
        {
            match world.registry.retire_source(self.ids.use_context) {
                Ok(()) => {
                    world.dirty = true;
                    if self.issuer.is_none() {
                        world.finished = true;
                    }
                    return Ok(step(Step::Complete));
                }
                Err(Error::Busy) => {}
                Err(error) => fatal(error),
            }
        }
        Ok(step(if self.observations_pending() {
            Step::Runnable
        } else {
            Step::Parked
        }))
    }
}
