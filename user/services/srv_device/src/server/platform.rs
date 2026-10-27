//! 平台 PCI host 的真实供应；登记、RPC、冻结表及停止均复用服务原 owner。
use super::*;
use alloc::sync::Arc;
use libdevice::{
    protocol::{self, Command, Header, Progress, ProvisionCommand},
    provision::{self, Provision},
    resource::{Collecting, Descriptor, Entry, Frozen, Limits},
};
use libkernel::ipc::{message::MailboxSender, wait};
use libpci::{
    host::{HOST_IDENTITY, HOST_NAME, HOST_RESOURCES, HostInfo},
    platform::{Control, Platform, ROOT_RIGHTS},
};
use librpc::{Request as RpcRequest, dispatcher::ServiceOwner};

pub(super) const LIMITS: Limits = Limits {
    entries: 4,
    text_bytes: (HOST_RESOURCES[0].len()
        + HOST_RESOURCES[1].len()
        + HOST_RESOURCES[2].len()
        + HOST_RESOURCES[3].len()
        + HostInfo::BYTES * 4) as u32,
};

pub(super) struct Adapter {
    input: Platform,
    host: Option<(HostInfo, [usize; 4])>,
    pub context: u64,
    provision: Provision,
    collecting: Option<Collecting<Capability>>,
    frozen: Option<Frozen<Capability>>,
    children: [Option<Control>; 4],
    sealed: usize,
    retiring: usize,
    pub device: Option<DeviceId>,
    funding: Funding,
    _bytes: Charge,
}
impl Adapter {
    pub fn new(input: Platform, context: u64, funding: Funding) -> Result<Self, Error> {
        let host = input.host()?;
        let bytes = funding.domain().acquire(
            DeviceResource::Bytes,
            core::mem::size_of::<Self>() + core::mem::size_of::<PlatformTask>(),
        )?;
        Ok(Self {
            input,
            host,
            context,
            provision: Provision::new(),
            collecting: None,
            frozen: None,
            children: core::array::from_fn(|_| None),
            sealed: 0,
            retiring: 0,
            device: None,
            funding,
            _bytes: bytes,
        })
    }
    pub fn has_host(&self) -> bool {
        self.host.is_some()
    }
    pub fn command(&mut self, command: ProvisionCommand) -> Result<Progress, Error> {
        let epoch = command.epoch();
        match command {
            ProvisionCommand::Prepare(_) => {
                if self.provision.prepare(epoch)? == provision::Prepare::Started {
                    match Collecting::reserve(
                        LIMITS,
                        protocol::RESOURCE_HEADER_LEN,
                        self.funding.domain(),
                    ) {
                        Ok(mut table) => {
                            table.prepared(LIMITS.entries)?;
                            self.collecting = Some(table);
                        }
                        Err(error) => {
                            self.provision.rejected(epoch)?;
                            return Ok(Progress::Rejected(error));
                        }
                    }
                }
            }
            ProvisionCommand::Stop(_) => {
                self.provision.stop(epoch)?;
            }
            ProvisionCommand::ConfirmStopped(_) => {
                self.provision.confirm_stopped(epoch)?;
            }
            ProvisionCommand::Get { .. } => return Err(Error::Invalid),
        }
        Ok(self.progress(epoch))
    }
    fn progress(&self, epoch: u64) -> Progress {
        if epoch <= self.provision.closed_epoch() {
            return Progress::Closed;
        }
        match self.provision.active() {
            Some(active) if active.epoch == epoch => match active.phase {
                provision::Phase::Preparing => Progress::Preparing,
                provision::Phase::Prepared { count } => Progress::Prepared(count),
                provision::Phase::Stopping => {
                    active.failure.map_or(Progress::Stopping, Progress::Failed)
                }
            },
            _ => Progress::Stopping,
        }
    }
    pub fn resource(
        &self,
        epoch: u64,
        index: u32,
    ) -> Result<libdevice::resource::Resource<'_, Capability>, Error> {
        let active = self.provision.active().ok_or(Error::Closed)?;
        if active.epoch != epoch {
            return Err(Error::StaleEpoch);
        }
        if !matches!(active.phase, provision::Phase::Prepared { .. }) {
            return Err(Error::NotReady);
        }
        self.frozen
            .as_ref()
            .and_then(|table| table.at(index as usize))
            .ok_or(Error::NotFound)
    }
    fn prepare_one(&mut self, epoch: u64) -> Result<(), Error> {
        let index = self
            .children
            .iter()
            .take_while(|child| child.is_some())
            .count();
        if index == HOST_RESOURCES.len() {
            let table = self.collecting.take().ok_or(Error::NotReady)?;
            self.frozen = Some(table.finish().map_err(|table| {
                self.collecting = Some(table);
                Error::Invalid
            })?);
            return self.provision.prepared(epoch, LIMITS.entries);
        }
        let (info, indices) = self.host.ok_or(Error::NotFound)?;
        let control = match &self.input.controls()[indices[index]] {
            Control::Mmio { control, ecam } => Control::Mmio {
                control: control.create_control(
                    0,
                    usize::try_from(control.query()?.bytes).map_err(|_| Error::Invalid)?,
                    ROOT_RIGHTS,
                )?,
                ecam: *ecam,
            },
            Control::Irq(control) => {
                Control::Irq(control.create_control(control.query()?.scope, ROOT_RIGHTS)?)
            }
            Control::Dma(control) => Control::Dma(control.create_control(ROOT_RIGHTS)?),
        };
        // 派生一旦成功先存入原轮；后续 duplicate/表校验失败也由此 owner 封口并等实际完成。
        self.children[index] = Some(control);
        let control = self.children[index]
            .as_ref()
            .expect("platform child just installed");
        let handle = object::duplicate(control.handle(), ROOT_RIGHTS)?;
        // SAFETY: duplicate 返回本进程的新 entry，唯一关闭责任移入冻结表。
        let cap = unsafe { Capability::from_raw(handle) };
        let descriptor = Descriptor {
            kind: match control {
                Control::Mmio { .. } => erhino_shared::object::ObjectKind::MmioControl,
                Control::Irq(_) => erhino_shared::object::ObjectKind::IrqControl,
                Control::Dma(_) => erhino_shared::object::ObjectKind::DmaDomainControl,
            },
            rights: ROOT_RIGHTS,
            protocol: 0,
        };
        descriptor.check_handle(&cap.description()?)?;
        self.collecting.as_mut().ok_or(Error::NotReady)?.push(
            &Entry {
                index: index as u32,
                descriptor,
                name: HOST_RESOURCES[index],
                metadata: &info.encode(),
            },
            cap,
        )
    }
    fn advance(
        &mut self,
        requests: &mut Requests<'_, Family>,
        source: &mut SourceRegistration,
    ) -> Result<Advance, SystemCallError> {
        let Some(active) = self.provision.active() else {
            return Ok(step(Step::Parked));
        };
        if active.phase == provision::Phase::Preparing {
            if let Err(error) = self.prepare_one(active.epoch) {
                self.provision
                    .failed(active.epoch, error)
                    .unwrap_or_else(|e| fatal(e));
            }
            return Ok(step(Step::Runnable));
        }
        if active.phase != provision::Phase::Stopping {
            return Ok(step(Step::Parked));
        }
        if self.sealed < self.children.len() {
            if let Some(child) = self.children[self.sealed].as_ref() {
                match child {
                    Control::Mmio { control, .. } => control.seal()?,
                    Control::Irq(control) => control.seal()?,
                    Control::Dma(control) => control.seal()?,
                }
            }
            self.sealed += 1;
            return Ok(step(Step::Runnable));
        }
        // 原轮消费者报告覆盖最后一次 ECAM 访问及仍需其资源的后代硬件清理；
        // 后续服务只保留不再触碰这些资源的软件收尾。资源 DONE 仍须独立观察。
        if !active.stopped_report {
            return Ok(step(Step::Parked));
        }
        if self.retiring < self.children.len() {
            let Some(child) = self.children[self.retiring].as_ref() else {
                self.retiring += 1;
                return Ok(step(Step::Runnable));
            };
            match child {
                Control::Mmio { control, .. } => control.revoke()?,
                Control::Irq(control) => control.revoke()?,
                Control::Dma(_) => {}
            }
            let handle = child.handle();
            if !wait::probe(handle, ObjectSignals::DONE)?.contains(ObjectSignals::DONE) {
                if source.is_empty() {
                    libkernel::debug!(
                        "platform resource retirement pending: index={}",
                        self.retiring
                    );
                    source.install(requests, SourcePlan::new(handle, ObjectSignals::DONE), 7)?;
                }
                return Ok(step(if source.source().is_some() {
                    Step::Parked
                } else {
                    Step::Runnable
                }));
            }
            source.remove(requests)?;
            if !source.is_empty() {
                return Ok(step(Step::Runnable));
            }
            self.children[self.retiring] = None;
            self.retiring += 1;
            return Ok(step(Step::Runnable));
        }
        self.collecting = None;
        self.frozen = None;
        self.provision
            .finish(active.epoch)
            .unwrap_or_else(|e| fatal(e));
        self.sealed = 0;
        self.retiring = 0;
        Ok(step(Step::Parked))
    }
}

pub(super) struct PlatformTask {
    entry: Option<Arc<MailboxSender>>,
    provider: Option<MailboxSender>,
    control: Option<MailboxSender>,
    call: Option<u64>,
    source: SourceRegistration,
    stopping: bool,
}
impl PlatformTask {
    pub fn new(entry: MailboxSender, provider: MailboxSender) -> Result<Self, Error> {
        Ok(Self {
            entry: Some(Arc::try_new(entry).map_err(|_| Error::Resource)?),
            provider: Some(provider),
            control: None,
            call: None,
            source: SourceRegistration::new(),
            stopping: false,
        })
    }
    pub fn registered(&mut self, kind: u64, id: SourceId) {
        if kind == 7 {
            self.source.registered(kind, id);
        }
    }
    pub fn unregistered(&mut self, id: SourceId) {
        self.source.unregistered(id);
    }
    pub fn stop(&mut self) {
        self.stopping = true;
    }
    pub fn advance(
        &mut self,
        id: u64,
        world: &mut World,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        while input.pull().is_some() {}
        input.take_timeout();
        if let Some(txid) = self.call {
            let Some(result) = world.rpc.take(txid) else {
                return Ok(step(Step::Parked));
            };
            self.call = None;
            let mut reply = result.result.unwrap_or_else(|_| fatal(Error::Unavailable));
            let (header, body) = Header::decode(&reply.payload).unwrap_or_else(|e| fatal(e));
            if header.op != 3
                || header.error.is_some()
                || body.len() != 16
                || reply.handles.remaining() != 1
            {
                fatal(Error::Invalid);
            }
            let device = DeviceId::decode(&mut librpc::bytes::Reader::new(body))
                .unwrap_or_else(|e| fatal(e));
            let (control, _) =
                MailboxSender::from_capability(reply.handles.take(0)?).map_err(|f| f.error)?;
            self.control = Some(control);
            world
                .platform
                .as_mut()
                .expect("platform adapter missing")
                .device = Some(device);
            self.entry = None;
            world.platform_ready = true;
            requests.wake(world.root_task)?;
            libkernel::debug!("platform PCI host registered: serial={}", device.serial());
        }
        if let Some(entry) = self.entry.as_ref() {
            let context = entry.description()?.object_id;
            if world.registry.funding(context).is_err() {
                return Ok(step(Step::Runnable));
            }
            let deadline = libkernel::time::timeout_millis(5_000)?;
            let mut bytes = [0; protocol::HEADER_LEN + protocol::BODY_MAX];
            let used = Command::Register {
                seq: 1,
                parent: None,
                name: HOST_NAME,
                identity: HOST_IDENTITY,
                limits: LIMITS,
                claimable: true,
            }
            .encode(deadline, &mut bytes)
            .unwrap_or_else(|e| fatal(e));
            let mut request = RpcRequest::new(protocol::ID, &bytes[..used])?;
            request
                .push(
                    self.provider
                        .take()
                        .expect("platform Provision sender missing")
                        .into_capability(),
                    TRANSPORT,
                )
                .map_err(|f| f.error)?;
            let txid = world
                .rpc
                .begin_for(
                    ServiceOwner::Shared(entry.clone()),
                    deadline,
                    request,
                    Some(id),
                    world.funding.execution(),
                )
                .unwrap_or_else(|_| fatal(Error::Unavailable));
            self.call = Some(txid);
            requests.wake(world.transport_task)?;
            return Ok(step(Step::Parked));
        }
        let mut adapter = world.platform.take().expect("platform adapter missing");
        if let Some(device) = adapter.device
            && world
                .registry
                .control_snapshot(device)
                .is_ok_and(|info| !info.present)
        {
            self.control = None;
        }
        let result = adapter.advance(requests, &mut self.source);
        world.platform = Some(adapter);
        if self.stopping {
            self.source.remove(requests)?;
            return Ok(step(if self.source.is_empty() {
                Step::Complete
            } else {
                Step::Runnable
            }));
        }
        result
    }
}
