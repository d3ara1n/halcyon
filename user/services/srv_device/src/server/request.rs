//! 入站调用及其提交后长期 owner；原任务贯穿回复、准备、停止和控制壳退休。
use super::*;
use alloc::{sync::Arc, vec::Vec};
use libdevice::{
    binding::{Commit, Phase},
    binding_work::Work,
    protocol::{self, Command, Header},
    registry::{EndpointIdentity, RegisterCommit, Registration},
};
use libkernel::ipc::message::ReceivedMessage;
use librpc::{exchange::RequestContext, outbox::Outbox};

const REPLY: u64 = 2;
enum Owner {
    None,
    Source(Box<Source>),
    Device {
        id: DeviceId,
        context: u64,
        control: Observed,
        provider: Observed,
        withdrawn: bool,
    },
    Binding {
        context: u64,
        control: Observed,
        work: Box<Work>,
        removed: bool,
    },
    Use {
        context: u64,
        control: Observed,
        task: u64,
        removed: bool,
    },
    Watch {
        context: u64,
        control: Observed,
        removed: bool,
    },
}
pub(super) struct Request {
    oal: bool,
    provision: bool,
    wire: Vec<u8>,
    header: Header,
    context: u64,
    input_handles: usize,
    funding: Funding,
    outbox: Option<Outbox>,
    encoded: bool,
    started: bool,
    error: Option<Error>,
    owner: Owner,
    owner_done: bool,
    bound: Option<Binding>,
    bound_control: bool,
    wake: Option<u64>,
    _request: Option<Charge>,
    control_request: bool,
    _bytes: Charge,
}
impl Request {
    pub fn accept(message: ReceivedMessage, world: &World) -> Result<Self, ()> {
        let oal = message.header.kind == liboal::protocol::ID;
        let provision = message.header.kind == protocol::PROVISION_ID;
        let mut context = RequestContext::decode(
            message,
            if oal {
                liboal::protocol::ID
            } else if provision {
                protocol::PROVISION_ID
            } else {
                protocol::ID
            },
        )
        .map_err(|_| ())?;
        let (header, _) = Header::decode(&context.payload).map_err(|_| ())?;
        let input_handles = context.handles.remaining();
        let decoded = if oal {
            Err(Error::Unsupported)
        } else {
            Command::decode(&context.payload, input_handles)
        };
        let error = if oal {
            None
        } else if provision {
            protocol::ProvisionCommand::decode(&context.payload, input_handles).err()
        } else {
            decoded.as_ref().err().copied()
        };
        let who = context.envelope.sender_context_id;
        let role = world.roles.get(who);
        let source_funding = world
            .registry
            .funding(who)
            .ok()
            .or_else(|| role.map(|r| &r.funding))
            .unwrap_or(&world.funding)
            .clone();
        let (bound, bound_control) = match role.map(|r| &r.kind) {
            Some(RoleKind::Binding(b)) => (Some(b.clone()), true),
            Some(RoleKind::Use(b)) => (Some(b.clone()), false),
            _ => (None, false),
        };
        let access = world.registry.source_access(who).ok();
        // 只有实际控制角色可使用管理预留；自报一个 Stop opcode 不获得预留额度。
        let control = if provision {
            world
                .platform
                .as_ref()
                .is_some_and(|adapter| adapter.context == who)
                && protocol::ProvisionCommand::decode(&context.payload, input_handles).is_ok_and(
                    |(_, command)| matches!(command, protocol::ProvisionCommand::Stop { .. }),
                )
        } else {
            decoded.as_ref().is_ok_and(|(_, command)| match command {
                Command::SealSource | Command::QuerySourceStop => {
                    world.registry.is_source_admin(who)
                }
                Command::WithdrawPublication { .. } => {
                    access.is_some_and(|a| a.contains(libdevice::registry::Access::REGISTER))
                }
                Command::StopClaim { .. } => access.is_some_and(|a| {
                    a.contains(libdevice::registry::Access::CLAIM)
                        || a.contains(libdevice::registry::Access::ADMIN)
                }),
                Command::StopBinding => bound_control,
                Command::ReportQuiescent => {
                    !bound_control
                        && bound.as_ref().is_some_and(|binding| {
                            binding.use_access().is_ok_and(|access| {
                                access.contains(libdevice::binding::UseAccess::REPORT_QUIESCENT)
                            })
                        })
                }
                Command::WithdrawDevice => {
                    role.is_some_and(|r| matches!(r.kind, RoleKind::Device { .. }))
                }
                Command::AckWatch { .. } | Command::CancelWatch => {
                    role.is_some_and(|r| matches!(r.kind, RoleKind::Watch { .. }))
                }
                _ => false,
            })
        };
        let funding = if control {
            world.management.clone()
        } else {
            source_funding
        };
        let request = funding
            .domain()
            .acquire(
                if control {
                    DeviceResource::ControlRequest
                } else {
                    DeviceResource::Request
                },
                1,
            )
            .map_err(|_| ())?;
        let bytes = funding
            .domain()
            .acquire(
                if control {
                    DeviceResource::ControlBytes
                } else {
                    DeviceResource::Bytes
                },
                core::mem::size_of::<Self>() + PAYLOAD_MAX * 2,
            )
            .map_err(|_| ())?;
        let wire = core::mem::take(&mut context.payload);
        let outbox = Outbox::prepare(
            context,
            PAYLOAD_MAX - librpc::PREFIX_LEN,
            header.deadline,
            REPLY,
            &libexecution::SourceFunding::Task,
        )
        .map_err(|_| ())?;
        Ok(Self {
            oal,
            provision,
            wire,
            header,
            context: who,
            input_handles,
            funding,
            outbox: Some(outbox),
            encoded: false,
            started: false,
            error,
            owner: Owner::None,
            owner_done: false,
            bound,
            bound_control,
            wake: None,
            _request: Some(request),
            control_request: control,
            _bytes: bytes,
        })
    }
    pub fn funding(&self) -> &Funding {
        &self.funding
    }
    pub fn max_sources(&self) -> usize {
        if self.control_request {
            CONTROL_SOURCES
        } else {
            REQUEST_SOURCES
        }
    }
    pub fn reject(&mut self, error: Error) {
        self.error = Some(error);
    }
    pub fn observe(&mut self, event: libexecution::runtime::SourceEvent) {
        if let Some(outbox) = self.outbox.as_mut() {
            outbox.observe(event);
        }
        match &mut self.owner {
            Owner::Source(s) => s.observe(event),
            Owner::Device {
                control, provider, ..
            } => {
                control.observe(event);
                provider.observe(event);
            }
            Owner::Binding { control, work, .. } => {
                control.observe(event);
                work.observe(event);
            }
            Owner::Use { control, .. } | Owner::Watch { control, .. } => control.observe(event),
            Owner::None => {}
        }
    }
    pub fn timed_out(&mut self) {
        if let Some(outbox) = self.outbox.as_mut() {
            outbox.timed_out();
        }
    }
    pub fn registered(&mut self, kind: u64, id: SourceId) {
        if let Some(outbox) = self.outbox.as_mut() {
            outbox.registered(&mut (), kind, id);
        }
        match &mut self.owner {
            Owner::Source(s) => s.registered(kind, id),
            Owner::Device {
                control, provider, ..
            } => {
                control.registered(kind, id);
                provider.registered(kind, id);
            }
            Owner::Binding { control, work, .. } => {
                control.registered(kind, id);
                work.registered(kind, id);
            }
            Owner::Use { control, .. } | Owner::Watch { control, .. } => {
                control.registered(kind, id)
            }
            Owner::None => {}
        }
    }
    pub fn unregistered(&mut self, kind: u64, id: SourceId) {
        if let Some(outbox) = self.outbox.as_mut() {
            outbox.unregistered(&mut (), kind, id);
        }
        match &mut self.owner {
            Owner::Source(s) => s.unregistered(id),
            Owner::Device {
                control, provider, ..
            } => {
                control.unregistered(id);
                provider.unregistered(id);
            }
            Owner::Binding { control, work, .. } => {
                control.unregistered(id);
                work.unregistered(id);
            }
            Owner::Use { control, .. } | Owner::Watch { control, .. } => control.unregistered(id),
            Owner::None => {}
        }
    }
    pub fn deadline(&self) -> Deadline {
        let reply = self
            .outbox
            .as_ref()
            .filter(|o| !o.is_complete())
            .map_or(Deadline::INFINITE, Outbox::deadline);
        let work = match &self.owner {
            Owner::Binding { work, .. } => work.deadline(),
            _ => Deadline::INFINITE,
        };
        match (
            reply.instant().ok().flatten(),
            work.instant().ok().flatten(),
        ) {
            (Some(a), Some(b)) => Deadline::at(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Deadline::at(a),
            (None, None) => Deadline::INFINITE,
        }
    }
    pub fn stop(&mut self, world: &mut World) {
        if let Some(outbox) = self.outbox.as_mut() {
            outbox.stop(&mut ());
        }
        match &mut self.owner {
            Owner::Source(s) => s.stop(world),
            Owner::Device { id, .. } => {
                let _ = world.registry.control_withdraw(*id);
                world.dirty = true;
            }
            Owner::Binding { work, .. } => {
                if let Ok(binding) = work.binding() {
                    let _ = binding.stop(&mut world.registry);
                }
                world.dirty = true;
            }
            Owner::None => {
                self.error = Some(Error::Closed);
            }
            Owner::Use { .. } => {}
            Owner::Watch { context, .. } => {
                let _ = world.registry.cancel_watch(*context);
            }
        }
    }
    fn encode(&mut self, error: Option<Error>, body: &[u8]) -> Result<(), Error> {
        let response = self.outbox.as_mut().ok_or(Error::Closed)?.response_mut()?;
        if self.oal {
            let used = liboal::protocol::encode_response(
                self.header.op,
                &liboal::protocol::Response::Error(libdevice::registry::oal::status(
                    error.unwrap_or(Error::Invalid),
                )),
                self.header.deadline,
                response.body_mut()?,
            )
            .map_err(|_| Error::Invalid)?;
            response.finish_body(used)?;
            self.encoded = true;
            return Ok(());
        }
        let used = Header {
            op: self.header.op,
            error,
            deadline: Deadline::INFINITE,
        }
        .encode(body, response.body_mut()?)?;
        response.finish_body(used)?;
        self.encoded = true;
        Ok(())
    }
    fn push(&mut self, capability: Capability, rights: Rights) -> Result<(), Error> {
        self.outbox
            .as_mut()
            .ok_or(Error::Closed)?
            .response_mut()?
            .push(capability, rights)
            .map_err(|f| f.error.into())
    }
    fn bound(&self, control: bool) -> Result<Binding, Error> {
        if control && !self.bound_control {
            return Err(Error::Permission);
        }
        self.bound.clone().ok_or(Error::Permission)
    }
    fn existing_reply(&mut self, role: &Role, body: &[u8]) -> Result<(), Error> {
        let sender = role.sender.as_ref().ok_or(Error::Closed)?;
        self.push(alias(sender)?, TRANSPORT)?;
        self.encode(None, body)
    }
    fn execute(&mut self, id: u64, now: u64, world: &mut World) -> Result<(), Error> {
        // 移动原输入 Vec，既不复制消息也不让字段借用跨越提交步骤。
        let wire = core::mem::take(&mut self.wire);
        let result = self.execute_wire(&wire, id, now, world);
        self.wire = wire;
        result
    }
    fn execute_wire(
        &mut self,
        wire: &[u8],
        id: u64,
        now: u64,
        world: &mut World,
    ) -> Result<(), Error> {
        if self.oal {
            return self.execute_oal(wire, world);
        }
        if self.provision {
            return self.execute_provision(wire, world);
        }
        let (_, command) = Command::decode(wire, self.input_handles)?;
        let mut body = [0; protocol::BODY_MAX];
        match command {
            Command::IssueSource { scope, access } => {
                let source = Source::prepare(
                    world,
                    Some(self.context),
                    scope,
                    access,
                    self.funding.clone(),
                    None,
                )?;
                self.owner = Owner::Source(Box::try_new(source).map_err(|_| Error::Resource)?);
            }
            Command::SealSource => {
                let task = world.registry.seal_source(self.context)?;
                self.wake = Some(task);
                world.dirty = true;
                self.encode(None, &[])?;
            }
            Command::QuerySourceStop => {
                let status = world.registry.query_source_stop(self.context)?;
                self.encode(None, &protocol::encode_source_stop(status))?;
            }
            Command::Register {
                seq,
                parent,
                name,
                identity,
                limits,
                claimable,
                stop_requirement,
            } => {
                let cap = {
                    let (context, _) = self
                        .outbox
                        .as_mut()
                        .ok_or(Error::Closed)?
                        .response_mut()?
                        .parts()?;
                    context.handles.take(1)?
                };
                let (sender, description) =
                    MailboxSender::from_capability(cap).map_err(|f| Error::from(f.error))?;
                let endpoint = EndpointIdentity::from_description(&description)?;
                let registration = Registration {
                    seq,
                    parent,
                    name,
                    identity,
                    limits,
                    claimable,
                    stop_requirement,
                    endpoint,
                };
                if let Some(device) = world
                    .registry
                    .existing_registration(self.context, &registration)?
                {
                    device.encode(&mut librpc::bytes::Writer::new(&mut body));
                    return self.existing_reply(
                        world.existing_device(device).ok_or(Error::Closed)?,
                        &body[..16],
                    );
                }
                let sender = Arc::try_new(sender).map_err(|_| Error::Resource)?;
                let provider_handle = object::duplicate(sender.as_handle(), Rights::WAIT)?;
                // SAFETY: duplicate 返回独占的新 entry；这里只拥有观察，不取得供应控制权。
                let provider = Observed::new(unsafe { Capability::from_raw(provider_handle) }, 4);
                let candidate =
                    world
                        .registry
                        .prepare_registration(self.context, &registration, sender, id)?;
                let device = candidate.id();
                let minted = world.mailbox.mint(0, TRANSPORT)?;
                let context = minted.sender.description()?.object_id;
                let role = world.prepare_role(
                    context,
                    RoleKind::Device { id: device },
                    minted.sender,
                    self.funding.clone(),
                )?;
                self.push(
                    alias(role.value().sender.as_ref().ok_or(Error::Closed)?)?,
                    TRANSPORT,
                )?;
                match world.registry.commit_registration(candidate) {
                    Ok(RegisterCommit::Created(device)) => {
                        world.roles.insert_prepared(role);
                        self.owner = Owner::Device {
                            id: device,
                            context,
                            control: Observed::lifetime(minted.lifetime, 3),
                            provider,
                            withdrawn: false,
                        };
                        device.encode(&mut librpc::bytes::Writer::new(&mut body));
                        self.encode(None, &body[..16])
                            .unwrap_or_else(|error| fatal(error));
                        world.dirty = true;
                    }
                    Ok(RegisterCommit::Existing(_)) => {
                        unreachable!("device registration changed within actor commit")
                    }
                    Err(failure) => return Err(failure.error),
                }
            }
            Command::WithdrawPublication { seq } => {
                if let Some(task) = world.registry.withdraw_publication(self.context, seq)? {
                    self.wake = Some(task);
                }
                world.dirty = true;
                self.encode(None, &[])?;
            }
            Command::QueryDevice { id: device } => {
                let info = match world.roles.get(self.context).map(|r| &r.kind) {
                    Some(RoleKind::Device { id, .. }) if *id == device => {
                        world.registry.control_snapshot(device)?
                    }
                    Some(_) => return Err(Error::Permission),
                    None => world.registry.query(self.context, device)?,
                };
                let response = self.outbox.as_mut().ok_or(Error::Closed)?.response_mut()?;
                let used = protocol::encode_device(&info, response.body_mut()?)?;
                response.finish_body(used)?;
                self.encoded = true;
            }
            Command::Claim { reference } => {
                if let libdevice::binding::Claim::Existing(snapshot) =
                    world.registry.claim(self.context, reference)?
                {
                    let used = protocol::encode_snapshot(snapshot, &mut body)?;
                    return self.existing_reply(
                        world.existing_binding(reference).ok_or(Error::Closed)?,
                        &body[..used],
                    );
                }
                let work = Work::prepare(&world.registry, self.context, reference, POLICY)?;
                let mut work = Box::try_new(work).map_err(|_| Error::Resource)?;
                let minted = world.mailbox.mint(0, TRANSPORT)?;
                let context = minted.sender.description()?.object_id;
                // 控制索引在提交前预留；绑定共享体尚未公开，但同一任务已持其实际存储。
                let binding = work.prepared_binding();
                let role = world.prepare_role(
                    context,
                    RoleKind::Binding(binding),
                    minted.sender,
                    self.funding.clone(),
                )?;
                self.push(
                    alias(role.value().sender.as_ref().ok_or(Error::Closed)?)?,
                    TRANSPORT,
                )?;
                let snapshot = match work.commit(id, now, &mut world.registry)? {
                    Commit::Created(snapshot) => snapshot,
                    Commit::Existing(_) => {
                        unreachable!("device binding changed within actor commit")
                    }
                };
                libkernel::debug!(
                    "device binding owner committed: task={}, reference={:?}",
                    id,
                    reference
                );
                world.roles.insert_prepared(role);
                self.owner = Owner::Binding {
                    context,
                    control: Observed::lifetime(minted.lifetime, 3),
                    work,
                    removed: false,
                };
                let used = protocol::encode_snapshot(snapshot, &mut body)
                    .unwrap_or_else(|error| fatal(error));
                self.encode(None, &body[..used])
                    .unwrap_or_else(|error| fatal(error));
                world.dirty = true;
            }
            Command::StopClaim { reference } => {
                let result = world.registry.stop_claim(self.context, reference)?;
                world.dirty = true;
                let used = protocol::encode_stop_claim(result, reference, &mut body)?;
                self.encode(None, &body[..used])?;
            }
            Command::QueryBinding => {
                let snapshot = self.bound(false)?.snapshot(&world.registry)?;
                let used = protocol::encode_snapshot(snapshot, &mut body)?;
                self.encode(None, &body[..used])?;
            }
            Command::QueryUse => {
                if self.bound_control {
                    return Err(Error::Permission);
                }
                let binding = self.bound(false)?;
                let info = protocol::UseInfo {
                    binding: binding.snapshot(&world.registry)?,
                    stop_requirement: binding.stop_requirement(),
                    access: binding.use_access()?,
                };
                let used = info.encode(&mut body)?;
                self.encode(None, &body[..used])?;
            }
            Command::IssueUse => {
                let binding = self.bound(true)?;
                let snapshot = binding.snapshot(&world.registry)?;
                if snapshot.phase == Phase::Released {
                    return Err(Error::Closed);
                }
                if let Some(entry) = binding.use_entry()? {
                    self.push(entry, TRANSPORT)?;
                    return self.encode(None, &[]);
                }
                world.registry.binding_usable(snapshot.reference)?;
                binding.use_access()?;
                let minted = world.mailbox.mint(0, TRANSPORT)?;
                let context = minted.sender.description()?.object_id;
                let role = world.prepare_role(
                    context,
                    RoleKind::Use(binding.clone()),
                    minted.sender,
                    self.funding.clone(),
                )?;
                self.push(
                    alias(role.value().sender.as_ref().ok_or(Error::Closed)?)?,
                    TRANSPORT,
                )?;
                world.roles.insert_prepared(role);
                // 原轮只提交一个 Use；母本归 Binding，Released 时关闭。
                let sender = world
                    .roles
                    .get_mut(context)
                    .expect("device use role disappeared")
                    .sender
                    .take()
                    .expect("device use sender missing");
                binding.install_use(sender);
                self.owner = Owner::Use {
                    context,
                    control: Observed::lifetime(minted.lifetime, 3),
                    task: binding.task(),
                    removed: false,
                };
                self.encode(None, &[]).unwrap_or_else(|error| fatal(error));
            }
            Command::GetResource { name } => {
                if self.bound_control {
                    return Err(Error::Permission);
                }
                let (used, capability, rights) =
                    self.bound(false)?
                        .resource(&world.registry, name, &mut body)?;
                self.push(capability, rights)?;
                self.encode(None, &body[..used])?;
            }
            Command::StopBinding => {
                self.wake = Some(self.bound(true)?.stop(&mut world.registry)?);
                world.dirty = true;
                self.encode(None, &[])?;
            }
            Command::ReportQuiescent => {
                if self.bound_control {
                    return Err(Error::Permission);
                }
                self.wake = Some(self.bound(false)?.report_quiescent(&world.registry)?);
                self.encode(None, &[])?;
            }
            Command::WithdrawDevice => {
                let RoleKind::Device { id, .. } = &world.role(self.context)?.kind else {
                    return Err(Error::Permission);
                };
                self.wake = world.registry.control_withdraw(*id)?;
                world.dirty = true;
                self.encode(None, &[])?;
            }
            Command::Enumerate { scope, after, end } => {
                let scan = world.registry.enumerate(self.context, scope, after, end)?;
                let used = protocol::encode_scan(scan, &mut body)?;
                self.encode(None, &body[..used])?;
            }
            Command::WatchScope { scope } => {
                self.create_watch(
                    id,
                    world,
                    libdevice::registry::WatchTarget::Scope(scope),
                    None,
                )?;
            }
            Command::WatchBinding => {
                let binding = self.bound(false)?;
                let snapshot = binding.snapshot(&world.registry)?;
                self.create_watch(
                    id,
                    world,
                    libdevice::registry::WatchTarget::Binding(snapshot),
                    Some(binding),
                )?;
            }
            Command::IssueRegistrar => {
                let binding = self.bound(true)?;
                let snapshot = binding.snapshot(&world.registry)?;
                if snapshot.phase == Phase::Released {
                    return Err(Error::Closed);
                }
                if let Some((a, b)) = binding.registrar()? {
                    self.push(a.into_capability(), TRANSPORT)?;
                    self.push(b.into_capability(), TRANSPORT)?;
                    return self.encode(None, &[]);
                }
                let source = Source::prepare(
                    world,
                    Some(snapshot.subject.context()),
                    libdevice::registry::Scope::Subtree(snapshot.reference.device()),
                    libdevice::registry::Access::REGISTER
                        .union(libdevice::registry::Access::OBSERVE),
                    self.funding.clone(),
                    Some(snapshot.reference),
                )?;
                self.owner = Owner::Source(Box::try_new(source).map_err(|_| Error::Resource)?);
            }
            Command::InstallFunction { name, description } => {
                let reference = self.bound(true)?.snapshot(&world.registry)?.reference;
                let (entry, endpoint) = self.take_entry()?;
                world.registry.install_function(
                    reference,
                    libdevice::registry::FunctionRegistration {
                        name,
                        description,
                        endpoint,
                    },
                    entry,
                )?;
                self.encode(None, &[])?;
            }
            Command::OpenFunction { reference, name } => {
                let function = world
                    .registry
                    .open_function(self.context, reference, name)?;
                let entry = alias_with(function.entry, ENTRY_RIGHTS)?;
                let mut w = librpc::bytes::Writer::new(&mut body);
                w.u64(liboal::protocol::ID);
                w.bytes32(function.description);
                let used = w.written();
                self.push(entry, ENTRY_RIGHTS)?;
                self.encode(None, &body[..used])?;
            }
            Command::InstallActivation { protocol } => {
                let reference = self.bound(true)?.snapshot(&world.registry)?.reference;
                let (entry, endpoint) = self.take_entry()?;
                world
                    .registry
                    .install_activation(reference, endpoint, protocol, entry)?;
                self.encode(None, &[])?;
            }
            Command::GetActivation { reference } => {
                let (protocol, entry) = world.registry.activation(self.context, reference)?;
                let entry = alias_with(entry, ENTRY_RIGHTS)?;
                self.push(entry, ENTRY_RIGHTS)?;
                self.encode(None, &protocol.to_le_bytes())?;
            }
            Command::QueryWatch | Command::AckWatch { .. } | Command::CancelWatch => {
                if !matches!(world.role(self.context)?.kind, RoleKind::Watch { .. }) {
                    return Err(Error::Permission);
                }
                match command {
                    Command::AckWatch { sequence } => {
                        world.registry.ack_watch(self.context, sequence)?;
                    }
                    Command::CancelWatch => {
                        world.registry.cancel_watch(self.context)?;
                    }
                    _ => {}
                }
                let info = world.signal_watch(self.context)?;
                let used = info.encode(&mut body)?;
                self.encode(None, &body[..used])?;
            }
        }
        Ok(())
    }
    fn execute_provision(&mut self, wire: &[u8], world: &mut World) -> Result<(), Error> {
        let (_, command) = protocol::ProvisionCommand::decode(wire, self.input_handles)?;
        let adapter = world
            .platform
            .as_mut()
            .filter(|adapter| adapter.context == self.context)
            .ok_or(Error::Permission)?;
        if let protocol::ProvisionCommand::Get { epoch, index } = command {
            let resource = adapter.resource(epoch, index)?;
            let mut body = [0; protocol::BODY_MAX];
            body[..8].copy_from_slice(&epoch.to_le_bytes());
            let used = libdevice::resource::Entry {
                index,
                descriptor: resource.descriptor,
                name: resource.name,
                metadata: resource.metadata,
            }
            .encode(protocol::RESOURCE_HEADER_LEN, &mut body[8..])?;
            let handle =
                object::duplicate(resource.capability.as_handle(), resource.descriptor.rights)?;
            // SAFETY: 刚获得本进程独占的新 entry；回复完成后由实际运输 owner 关闭。
            self.push(
                unsafe { Capability::from_raw(handle) },
                resource.descriptor.rights,
            )?;
            return self.encode(None, &body[..8 + used]);
        }
        let signal = if matches!(command, protocol::ProvisionCommand::Prepare(_)) {
            Some(
                self.outbox
                    .as_mut()
                    .ok_or(Error::Closed)?
                    .response_mut()?
                    .parts()?
                    .0
                    .handles
                    .take(1)?,
            )
        } else {
            None
        };
        let progress = adapter.command(command, signal)?;
        let mut bytes = [0; protocol::HEADER_LEN + 16];
        let used = progress.encode(command, &mut bytes)?;
        let (_, body) = Header::decode(&bytes[..used])?;
        self.wake = Some(world.platform_task);
        self.encode(None, body)
    }
    fn execute_oal(&mut self, wire: &[u8], world: &World) -> Result<(), Error> {
        let response = self.outbox.as_mut().ok_or(Error::Closed)?.response_mut()?;
        let result = if let Some(binding) = &self.bound {
            binding
                .discover_resources(
                    &world.registry,
                    wire,
                    self.input_handles,
                    response.body_mut()?,
                )
                .map(|used| libdevice::registry::oal::Projection { used, entry: None })
        } else {
            world
                .registry
                .discover(self.context, wire, self.input_handles, response.body_mut()?)
        };
        match result {
            Ok(projection) => {
                response.finish_body(projection.used)?;
                if let Some(entry) = projection.entry {
                    self.push(alias_with(&entry, ENTRY_RIGHTS)?, ENTRY_RIGHTS)?;
                }
            }
            Err(status) => {
                let used = liboal::protocol::encode_response(
                    self.header.op,
                    &liboal::protocol::Response::Error(status),
                    self.header.deadline,
                    response.body_mut()?,
                )
                .map_err(|_| Error::Invalid)?;
                response.finish_body(used)?;
            }
        }
        self.encoded = true;
        Ok(())
    }
    fn create_watch(
        &mut self,
        task: u64,
        world: &mut World,
        target: libdevice::registry::WatchTarget,
        binding: Option<Binding>,
    ) -> Result<(), Error> {
        let source = if let Some(binding) = &binding {
            binding.snapshot(&world.registry)?.subject.context()
        } else {
            self.context
        };
        let (signal, observer) = watch::Signal::create()?;
        let minted = world.mailbox.mint(0, TRANSPORT)?;
        let context = minted.sender.description()?.object_id;
        let role = world.prepare_role(
            context,
            RoleKind::Watch { signal, binding },
            minted.sender,
            self.funding.clone(),
        )?;
        self.push(
            alias(role.value().sender.as_ref().ok_or(Error::Closed)?)?,
            TRANSPORT,
        )?;
        self.push(
            observer.into_capability(),
            Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )?;
        match target {
            libdevice::registry::WatchTarget::Binding(snapshot) => {
                world
                    .registry
                    .create_binding_watch(snapshot, context, task, &self.funding)?;
            }
            libdevice::registry::WatchTarget::Scope(_) => {
                world.registry.create_watch(source, target, context, task)?;
            }
        }
        world.roles.insert_prepared(role);
        let info = world.signal_watch(context)?;
        world
            .roles
            .get_mut(context)
            .expect("watch role disappeared")
            .sender = None;
        self.owner = Owner::Watch {
            context,
            control: Observed::lifetime(minted.lifetime, 3),
            removed: false,
        };
        let mut body = [0; libdevice::watch::Info::ENCODED_LEN];
        let used = info.encode(&mut body).unwrap_or_else(|e| fatal(e));
        self.encode(None, &body[..used])
            .unwrap_or_else(|e| fatal(e));
        Ok(())
    }
    fn take_entry(&mut self) -> Result<(Arc<MailboxSender>, EndpointIdentity), Error> {
        let (context, _) = self
            .outbox
            .as_mut()
            .ok_or(Error::Closed)?
            .response_mut()?
            .parts()?;
        let (sender, description) = MailboxSender::from_capability(context.handles.take(1)?)
            .map_err(|e| Error::from(e.error))?;
        let endpoint = EndpointIdentity::from_description(&description)?;
        Ok((Arc::try_new(sender).map_err(|_| Error::Resource)?, endpoint))
    }
    fn owner_advance(
        &mut self,
        id: u64,
        world: &mut World,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        match &mut self.owner {
            Owner::None => Ok(step(Step::Complete)),
            Owner::Source(source) => source.advance(id, world, requests, input),
            Owner::Device {
                id,
                context,
                control,
                provider,
                withdrawn,
            } => {
                control.drive(requests)?;
                provider.drive(requests)?;
                if provider.closed
                    || !world
                        .registry
                        .control_snapshot(*id)
                        .unwrap_or_else(|e| fatal(e))
                        .present
                {
                    if !*withdrawn {
                        world
                            .registry
                            .control_withdraw(*id)
                            .unwrap_or_else(|e| fatal(e));
                        *withdrawn = true;
                        world.dirty = true;
                    }
                    if let Some(role) = world.roles.get_mut(*context) {
                        role.sender = None;
                    }
                }
                if control.closed {
                    world
                        .registry
                        .control_closed(*id)
                        .unwrap_or_else(|e| fatal(e));
                    if !*withdrawn {
                        world
                            .registry
                            .control_withdraw(*id)
                            .unwrap_or_else(|e| fatal(e));
                        *withdrawn = true;
                        world.dirty = true;
                    }
                }
                if *withdrawn && control.closed && control.source.is_empty() {
                    // 先从观察集合移除 provider，设备 backing 仍持真实 sender。
                    provider.closed = true;
                    provider.drive(requests)?;
                    if provider.source.is_empty() {
                        match world.registry.retire_device(*id) {
                            Ok(provider) => {
                                drop(provider);
                                world.roles.remove(*context);
                                world.dirty = true;
                                return Ok(step(Step::Complete));
                            }
                            Err(Error::Busy) => {}
                            Err(e) => fatal(e),
                        }
                    }
                }
                Ok(step(Step::Parked))
            }
            Owner::Binding {
                context,
                control,
                work,
                removed,
            } => {
                control.drive(requests)?;
                if control.closed && control.source.is_empty() && !*removed {
                    world.roles.remove(*context);
                    *removed = true;
                }
                let result = work.advance(
                    &mut world.registry,
                    &mut world.rpc,
                    world.transport_task,
                    requests,
                    input,
                );
                if work.has_fault() {
                    libkernel::debug!("device binding responsibility failed: {:?}", work);
                    fatal(Error::Unavailable);
                }
                if let Ok(binding) = work.binding()
                    && binding
                        .snapshot(&world.registry)
                        .is_ok_and(|s| s.phase == Phase::Released)
                    && let Some(role) = world.roles.get_mut(*context)
                {
                    role.sender = None;
                }
                if result.step == Step::Complete {
                    libkernel::debug!("device binding owner retired: {:?}", work);
                    world.dirty = true;
                }
                Ok(result)
            }
            Owner::Use {
                context,
                control,
                task,
                removed,
            } => {
                control.drive(requests)?;
                if control.closed && control.source.is_empty() {
                    if !*removed {
                        world.roles.remove(*context);
                        *removed = true;
                        self.wake = Some(*task);
                    }
                    return Ok(step(Step::Complete));
                }
                Ok(step(Step::Parked))
            }
            Owner::Watch {
                context,
                control,
                removed,
            } => {
                control.drive(requests)?;
                if control.closed && control.source.is_empty() {
                    if !*removed {
                        world
                            .registry
                            .retire_watch(*context)
                            .unwrap_or_else(|e| fatal(e));
                        if let Some(role) = world.roles.remove(*context) {
                            if let RoleKind::Watch {
                                binding: Some(binding),
                                ..
                            } = &role.value().kind
                            {
                                self.wake = Some(binding.task());
                            }
                            drop(role);
                        }
                        *removed = true;
                    }
                    return Ok(step(Step::Complete));
                }
                world.signal_watch(*context).unwrap_or_else(|e| fatal(e));
                Ok(step(Step::Parked))
            }
        }
    }
    fn sources_ready(&self) -> bool {
        let ready = |o: &Observed| o.closed || o.source.source().is_some();
        match &self.owner {
            Owner::None => true,
            Owner::Source(_) => self.encoded,
            Owner::Device {
                control, provider, ..
            } => ready(control) && ready(provider),
            Owner::Binding { control, .. }
            | Owner::Use { control, .. }
            | Owner::Watch { control, .. } => ready(control),
        }
    }
    fn observations_pending(&self) -> bool {
        match &self.owner {
            Owner::Device {
                control, provider, ..
            } => control.is_pending() || provider.is_pending(),
            Owner::Binding { control, .. }
            | Owner::Use { control, .. }
            | Owner::Watch { control, .. } => control.is_pending(),
            Owner::Source(source) => source.observations_pending(),
            Owner::None => false,
        }
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
        if input.take_timeout()
            && self.outbox.as_ref().is_some_and(|outbox| {
                outbox
                    .deadline()
                    .instant()
                    .is_ok_and(|deadline| deadline.is_some_and(|at| at <= input.now_ns()))
            })
        {
            self.timed_out();
        }
        if let Some(task) = self.wake {
            requests.wake(task)?;
            self.wake = None;
        }
        if !self.started {
            let outbox = self.outbox.as_mut().expect("device request reply missing");
            if outbox.result().is_some() {
                self.started = true;
                self.error = Some(Error::Closed);
            } else if !outbox.is_admitted() {
                return Ok(outbox.admit(requests));
            } else {
                self.started = true;
                if self.error.is_none() {
                    self.error = self.execute(id, input.now_ns(), world).err();
                }
            }
        }
        if self.error.is_some() && !self.encoded {
            if let Some(outbox) = self.outbox.as_mut() {
                outbox.drain_capabilities(|cap, _| drop(cap));
            }
            if self.outbox.as_ref().is_some_and(|o| o.result().is_none()) {
                self.encode(self.error, &[])
                    .unwrap_or_else(|error| fatal(error));
            } else {
                self.encoded = true;
            }
        }
        if !self.encoded && matches!(self.owner, Owner::Source(_)) {
            let ready = if let Owner::Source(source) = &mut self.owner {
                source.admit(id, world, requests)
            } else {
                unreachable!()
            };
            match ready {
                Ok(false) => return Ok(step(Step::Runnable)),
                Err(error) => {
                    if let Owner::Source(source) = &mut self.owner {
                        source.abort();
                    }
                    self.encode(Some(error), &[]).unwrap_or_else(|e| fatal(e));
                }
                Ok(true) => {
                    let (a, b) = match &mut self.owner {
                        Owner::Source(s) => s.take_senders().unwrap_or_else(|e| fatal(e)),
                        _ => unreachable!(),
                    };
                    // 已提交来源的出口异常只能保留 Unknown 并进入故障，不能回复未提交 Quota。
                    self.push(a.into_capability(), TRANSPORT)
                        .unwrap_or_else(|e| fatal(e));
                    self.push(b.into_capability(), TRANSPORT)
                        .unwrap_or_else(|e| fatal(e));
                    self.encode(None, &[]).unwrap_or_else(|e| fatal(e));
                    world.dirty = true;
                }
            }
        }
        let mut next = Step::Parked;
        if !self.owner_done {
            let result = self.owner_advance(id, world, requests, input)?;
            if result.step == Step::Complete {
                self.owner_done = true;
            } else {
                next = result.step;
            }
        }
        if self.sources_ready()
            && let Some(outbox) = self.outbox.as_mut()
        {
            let result = outbox.drive(requests, 1)?;
            if result.step == Step::Runnable {
                next = Step::Runnable;
            }
            if outbox.is_complete() {
                self.outbox = None;
                self.wire = Vec::new();
                if !matches!(self.owner, Owner::None) {
                    self._request = None;
                }
                self._bytes.shrink_to(core::mem::size_of::<Self>());
            }
        }
        if self.observations_pending() {
            next = Step::Runnable;
        }
        if self.outbox.is_none()
            && let Some(binding) = self.bound.take()
        {
            let task = binding.task();
            drop(binding);
            self.wake = Some(task);
        }
        if self.wake.is_some() {
            return Ok(step(Step::Runnable));
        }
        if self.owner_done && self.outbox.is_none() {
            return Ok(step(Step::Complete));
        }
        Ok(step(next))
    }
}
