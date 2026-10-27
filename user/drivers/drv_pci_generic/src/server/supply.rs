//! PCI host 原激活的真实配置、子供应及资源收束 owner。
use super::*;
use libdevice::{
    protocol::{Progress, ProvisionCommand},
    provision::{self, Provision},
    resource::{Collecting, Descriptor, DeviceResource, Entry, Frozen, Limits},
};
use libkernel::{dma::DmaDomainControl, ipc::wait, irq::IrqControl, mmio::MmioControl};
use libpci::{
    config::{self, Ecam, Function},
    function::{FunctionInfo, RESOURCES},
    host::{HOST_RESOURCES, HostInfo},
    platform::ROOT_RIGHTS,
};
const USE: Rights =
    Rights::from_raw(ROOT_RIGHTS.raw() & !Rights::CREATE.raw() & !Rights::GRANT.raw());
pub(super) const LIMITS: Limits = Limits {
    entries: 4,
    text_bytes: ("registersinterruptdmapci-function".len() + FunctionInfo::BYTES * 4) as u32,
};
pub(super) struct FunctionSupply {
    pub function: Function,
    pub context: u64,
    pub sender: Arc<MailboxSender>,
    function_entry: Option<Arc<MailboxSender>>,
    pub device: Option<libdevice::identity::DeviceId>,
    pub control: Option<MailboxSender>,
    pub provision: Provision,
    pub info: Option<FunctionInfo>,
    collecting: Option<Collecting<Capability>>,
    frozen: Option<Frozen<Capability>>,
    mmio: Option<MmioControl>,
    irq: Option<IrqControl>,
    dma: Option<DmaDomainControl>,
    sealed: bool,
    retired: usize,
}
pub(super) struct Host {
    pub info: Option<HostInfo>,
    ecam_control: Option<MmioControl>,
    window: Option<MmioControl>,
    irq: Option<IrqControl>,
    dma: Option<DmaDomainControl>,
    pub ecam: Option<Ecam>,
    pub child: Option<FunctionSupply>,
    funding: AccountView<DeviceResource>,
    _account: Arc<libbudget::Account>,
    _budget: Arc<Budget>,
}
impl Host {
    pub fn is_provider_context(&self, context: u64) -> bool {
        self.child
            .as_ref()
            .is_some_and(|child| child.context == context)
    }
    pub fn new() -> Result<Self, Error> {
        // 首批只有一个物理 xHCI function；表的实物容量由四项及其布局给出。
        let bytes = Collecting::<Capability>::storage_bytes(LIMITS)?;
        let limits = [0, 0, bytes, 0, 0, 0, 0, 0, 0, 0];
        let budget = Budget::new(&limits, 1)?;
        let account = budget.account(&limits)?;
        let slots =
            core::array::from_fn::<_, 10, _>(|i| budget.slot(i).expect("PCI domain slot missing"));
        let funding = account.view(&slots)?;
        Ok(Self {
            info: None,
            ecam_control: None,
            window: None,
            irq: None,
            dma: None,
            ecam: None,
            child: None,
            funding,
            _account: account,
            _budget: budget,
        })
    }
    pub fn retirement_wait(&self) -> Option<erhino_shared::object::Handle> {
        let child = self.child.as_ref()?;
        let active = child.provision.active()?;
        if active.phase != provision::Phase::Stopping || !active.consumer_quiescent {
            return None;
        }
        match child.retired {
            0 => child.irq.as_ref().map(|control| control.handle()),
            1 => child.mmio.as_ref().map(|control| control.handle()),
            2 => child.dma.as_ref().map(|control| control.handle()),
            _ => None,
        }
    }
    pub fn adopt(&mut self, index: usize, mut reply: librpc::Reply) -> Result<(), Error> {
        let (header, body) = device::Header::decode(&reply.payload)?;
        if header.op != 10 {
            return Err(Error::Invalid);
        }
        if let Some(e) = header.error {
            if !body.is_empty() || reply.handles.remaining() != 0 {
                return Err(Error::Invalid);
            }
            return Err(e);
        }
        let entry = Entry::decode(body, device::HEADER_LEN, reply.handles.remaining())?;
        if entry.name != HOST_RESOURCES[index] {
            return Err(Error::Conflict);
        }
        let info = HostInfo::decode(entry.metadata)?;
        if self.info.is_some_and(|old| old != info) {
            return Err(Error::Conflict);
        }
        let cap = reply.handles.take(0)?;
        entry.descriptor.check_handle(&cap.description()?)?;
        let expected = match index {
            0 | 1 => erhino_shared::object::ObjectKind::MmioControl,
            2 => erhino_shared::object::ObjectKind::IrqControl,
            _ => erhino_shared::object::ObjectKind::DmaDomainControl,
        };
        if entry.descriptor.kind != expected || !entry.descriptor.rights.contains(ROOT_RIGHTS) {
            return Err(Error::Permission);
        }
        self.info = Some(info);
        // SAFETY: 真实 kind/rights 与表的领域名字已核，移交唯一 owner。
        unsafe {
            match index {
                0 => {
                    let control = MmioControl::from_handle(cap.into_raw());
                    let geometry = control.query()?;
                    if geometry.physical != info.ecam_cpu || geometry.bytes != info.ecam_bytes {
                        return Err(Error::Invalid);
                    }
                    self.ecam_control = Some(control);
                }
                1 => {
                    let control = MmioControl::from_handle(cap.into_raw());
                    let geometry = control.query()?;
                    if geometry.physical != info.window_cpu || geometry.bytes != info.window_bytes {
                        return Err(Error::Invalid);
                    }
                    self.window = Some(control);
                }
                2 => {
                    let control = IrqControl::from_handle(cap.into_raw());
                    let scope = control.query()?.scope;
                    if info.intx.iter().any(|n| {
                        *n < scope.first
                            || u64::from(*n) >= u64::from(scope.first) + u64::from(scope.count)
                    }) || scope.targets & info.targets != info.targets
                    {
                        return Err(Error::Permission);
                    }
                    self.irq = Some(control);
                }
                3 => {
                    let control = DmaDomainControl::from_handle(cap.into_raw());
                    let snapshot = control.query()?;
                    if snapshot.mode != erhino_shared::dma::TRUSTED_PHYSICAL
                        || snapshot.coherency != erhino_shared::dma::COHERENT
                        || snapshot.flags != 0
                        || snapshot.members != 0
                        || snapshot.reserved != 0
                    {
                        return Err(Error::Permission);
                    }
                    self.dma = Some(control);
                }
                _ => return Err(Error::Invalid),
            }
        }
        Ok(())
    }
    pub fn map(&mut self) -> Result<(), Error> {
        let usage = self
            .ecam_control
            .as_ref()
            .ok_or(Error::NotReady)?
            .create_use(0, 1 << 20, config::CONFIG_USE_RIGHTS)?;
        self.ecam = Some(Ecam::map(usage, self.info.ok_or(Error::NotReady)?).map_err(|(_, e)| e)?);
        Ok(())
    }
    pub fn discovered(&mut self, function: Function, mailbox: &Mailbox) -> Result<(), Error> {
        if self.child.is_some() {
            return Err(Error::Unsupported);
        }
        let sender = mailbox.mint(0, TRANSPORT)?.sender;
        let context = sender.description()?.object_id;
        self.child = Some(FunctionSupply {
            function,
            context,
            sender: Arc::try_new(sender).map_err(|_| Error::Resource)?,
            function_entry: None,
            device: None,
            control: None,
            provision: Provision::new(),
            info: None,
            collecting: None,
            frozen: None,
            mmio: None,
            irq: None,
            dma: None,
            sealed: false,
            retired: 0,
        });
        Ok(())
    }
    pub fn command(
        &mut self,
        command: ProvisionCommand,
        signal: Option<Capability>,
        mailbox: &Mailbox,
    ) -> Result<Progress, Error> {
        let child = self.child.as_mut().ok_or(Error::NotFound)?;
        let epoch = command.epoch();
        match command {
            ProvisionCommand::Prepare(_) => {
                if child
                    .provision
                    .prepare(epoch, signal.ok_or(Error::Invalid)?)?
                    == provision::Prepare::Started
                {
                    match Collecting::reserve(LIMITS, device::RESOURCE_HEADER_LEN, &self.funding) {
                        Ok(mut table) => {
                            table.prepared(4)?;
                            let entry = match mailbox.mint(0, TRANSPORT).map(|minted| minted.sender)
                            {
                                Ok(entry) => entry,
                                Err(error) => {
                                    child.provision.rejected(epoch)?;
                                    return Ok(Progress::Rejected(error.into()));
                                }
                            };
                            child.function_entry =
                                Some(Arc::try_new(entry).map_err(|_| Error::Resource)?);
                            child.collecting = Some(table);
                        }
                        Err(e) => {
                            child.provision.rejected(epoch)?;
                            return Ok(Progress::Rejected(e));
                        }
                    }
                }
            }
            ProvisionCommand::Stop {
                consumer_quiescent, ..
            } => {
                child.provision.stop(epoch, consumer_quiescent)?;
            }
            ProvisionCommand::Query(_) => {}
            ProvisionCommand::Get { .. } => return Err(Error::Invalid),
        }
        Ok(self.progress(epoch))
    }
    fn progress(&self, epoch: u64) -> Progress {
        let child = self.child.as_ref().expect("PCI child missing");
        if epoch <= child.provision.closed_epoch() {
            return Progress::Closed;
        }
        match child.provision.active() {
            Some(a) if a.epoch == epoch => match a.phase {
                provision::Phase::Preparing => Progress::Preparing,
                provision::Phase::Prepared { count } => Progress::Prepared(count),
                provision::Phase::Stopping => {
                    a.failure.map_or(Progress::Stopping, Progress::Failed)
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
        let child = self.child.as_ref().ok_or(Error::NotFound)?;
        let active = child.provision.active().ok_or(Error::Closed)?;
        if active.epoch != epoch {
            return Err(Error::StaleEpoch);
        }
        if !matches!(active.phase, provision::Phase::Prepared { .. }) {
            return Err(Error::NotReady);
        }
        child
            .frozen
            .as_ref()
            .and_then(|t| t.at(index as usize))
            .ok_or(Error::NotFound)
    }
    pub fn enable_master(&self, context: u64) -> Result<(), Error> {
        let child = self.child.as_ref().ok_or(Error::NotFound)?;
        if !child
            .function_entry
            .as_ref()
            .is_some_and(|entry| entry.description().is_ok_and(|d| d.object_id == context))
        {
            return Err(Error::Permission);
        }
        if !child
            .provision
            .active()
            .is_some_and(|a| matches!(a.phase, provision::Phase::Prepared { .. }))
        {
            return Err(Error::Closed);
        }
        // SAFETY: actor 独占 ECAM，原绑定仍准备完成，父撤销等待本进程永久停止报告。
        unsafe {
            self.ecam.as_ref().ok_or(Error::Closed)?.command(
                child.function.address,
                config::COMMAND_MASTER,
                config::COMMAND_INTX_DISABLE,
            )?;
        }
        libkernel::debug!(
            "PCI EnableMaster committed: epoch={}, context={}",
            child.provision.active().ok_or(Error::Closed)?.epoch,
            context
        );
        Ok(())
    }
    fn prepare_one(&mut self) -> Result<(), Error> {
        let child = self.child.as_mut().ok_or(Error::NotFound)?;
        let epoch = child.provision.active().ok_or(Error::Closed)?.epoch;
        if child.info.is_none() {
            let ecam = self.ecam.as_ref().ok_or(Error::Closed)?;
            // SAFETY: 本 function 尚未交付使用权，原轮 ECAM owner 排除撤销与配置竞争。
            let bar =
                unsafe { ecam.probe_bar(child.function.address, 0)? }.ok_or(Error::Unsupported)?;
            let host = self.info.ok_or(Error::NotReady)?;
            let (bus, cpu) = bar.placement(host, host.window_bus)?;
            // 先保有排他子控制，才提交 BAR 硬件效果；后续失败仍由原 Provision 停止。
            child.mmio = Some(
                self.window
                    .as_ref()
                    .ok_or(Error::NotReady)?
                    .create_control(
                        (cpu - host.window_cpu) as usize,
                        bar.bytes as usize,
                        ROOT_RIGHTS,
                    )?,
            );
            child.info = Some(FunctionInfo {
                address: child.function.address,
                vendor: child.function.vendor,
                device: child.function.device,
                registers_cpu: cpu,
                registers_bytes: bar.bytes,
                interrupt: host.irq(child.function.address.slot, child.function.interrupt_pin)?,
                target: host.targets.trailing_zeros(),
            });
            // SAFETY: BAR范围已在授权窗口排他保留；BME 仍关闭，解码只供该轮资源消费者。
            unsafe {
                ecam.install_bar(child.function.address, bar, bus)?;
            }
            return Ok(());
        }
        let info = child.info.ok_or(Error::NotReady)?;
        let index = child.collecting.as_ref().ok_or(Error::NotReady)?.received();
        let (cap, kind, rights, protocol) = match index {
            0 => {
                let usage = child.mmio.as_ref().ok_or(Error::NotReady)?.create_use(
                    0,
                    info.registers_bytes as usize,
                    USE,
                )?;
                (
                    unsafe_cap(usage.into_handle()),
                    erhino_shared::object::ObjectKind::MmioUse,
                    USE,
                    0,
                )
            }
            1 => {
                let host = self.info.ok_or(Error::NotReady)?;
                child.irq = Some(self.irq.as_ref().ok_or(Error::NotReady)?.create_control(
                    erhino_shared::irq::IrqScope {
                        first: info.interrupt,
                        count: 1,
                        targets: host.targets,
                    },
                    ROOT_RIGHTS,
                )?);
                let usage = child.irq.as_ref().ok_or(Error::NotReady)?.create_use(
                    info.interrupt,
                    info.target,
                    USE,
                )?;
                (
                    unsafe_cap(usage.into_handle()),
                    erhino_shared::object::ObjectKind::IrqUse,
                    USE,
                    0,
                )
            }
            2 => {
                child.dma = Some(
                    self.dma
                        .as_ref()
                        .ok_or(Error::NotReady)?
                        .create_control(ROOT_RIGHTS)?,
                );
                let usage = child
                    .dma
                    .as_ref()
                    .ok_or(Error::NotReady)?
                    .create_use(ROOT_RIGHTS & !Rights::GRANT)?;
                (
                    unsafe_cap(usage.into_handle()),
                    erhino_shared::object::ObjectKind::DmaUse,
                    ROOT_RIGHTS & !Rights::GRANT,
                    0,
                )
            }
            3 => (
                alias(child.function_entry.as_ref().ok_or(Error::NotReady)?)?,
                erhino_shared::object::ObjectKind::MailboxSender,
                TRANSPORT,
                libpci::function::ID,
            ),
            4 => {
                let table = child.collecting.take().ok_or(Error::NotReady)?;
                child.frozen = Some(table.finish().map_err(|_| Error::Invalid)?);
                child.provision.prepared(epoch, 4)?;
                libkernel::debug!("PCI function resources prepared: epoch={}", epoch);
                return Ok(());
            }
            _ => return Err(Error::Invalid),
        };
        child.collecting.as_mut().ok_or(Error::NotReady)?.push(
            &Entry {
                index: index as u32,
                descriptor: Descriptor {
                    kind,
                    rights,
                    protocol,
                },
                name: RESOURCES[index],
                metadata: &info.encode(),
            },
            cap,
        )?;
        Ok(())
    }
    pub fn advance(&mut self) -> Result<bool, Error> {
        let Some(active) = self.child.as_ref().and_then(|c| c.provision.active()) else {
            return Ok(false);
        };
        if active.phase == provision::Phase::Preparing {
            if let Err(error) = self.prepare_one() {
                self.child
                    .as_mut()
                    .ok_or(Error::NotFound)?
                    .provision
                    .failed(active.epoch, error)?;
            }
            return Ok(true);
        }
        if active.phase != provision::Phase::Stopping {
            return Ok(false);
        }
        let child = self.child.as_mut().ok_or(Error::NotFound)?;
        if !child.sealed {
            if let Some(c) = &child.mmio {
                c.seal()?;
            }
            if let Some(c) = &child.irq {
                c.seal()?;
            }
            if let Some(c) = &child.dma {
                c.seal()?;
            }
            child.sealed = true;
            return Ok(true);
        }
        // 即使已清 BME 也不能推导既往 DMA 安全；仅在真实 xHCI 静止报告后撤资源。
        if !active.consumer_quiescent {
            return Ok(false);
        }
        if child.retired == 0 {
            // SAFETY: xHCI 消费者报告包括 halt/事件与下游硬件清理；父轮 ECAM 仍存活。
            unsafe {
                self.ecam.as_ref().ok_or(Error::Closed)?.command(
                    child.function.address,
                    config::COMMAND_INTX_DISABLE,
                    config::COMMAND_MASTER | config::COMMAND_MEMORY,
                )?;
            }
        }
        let handle = match child.retired {
            0 => {
                if let Some(c) = &child.irq {
                    c.revoke()?;
                    Some(c.handle())
                } else {
                    None
                }
            }
            1 => {
                if let Some(c) = &child.mmio {
                    c.revoke()?;
                    Some(c.handle())
                } else {
                    None
                }
            }
            2 => child.dma.as_ref().map(|c| c.handle()),
            _ => {
                child.frozen = None;
                child.function_entry = None;
                child.collecting = None;
                child.mmio = None;
                child.irq = None;
                child.dma = None;
                child.info = None;
                child.provision.finish(active.epoch)?;
                child.retired = 0;
                child.sealed = false;
                libkernel::debug!("PCI function supply closed: epoch={}", active.epoch);
                return Ok(false);
            }
        };
        if let Some(handle) = handle
            && !wait::probe(handle, ObjectSignals::DONE)?.contains(ObjectSignals::DONE)
        {
            return Ok(false);
        }
        child.retired += 1;
        Ok(true)
    }
    pub fn hardware_idle(&self) -> bool {
        self.child
            .as_ref()
            .is_none_or(|c| c.provision.active().is_none())
    }
    pub fn unmap(&mut self) -> Result<(), Error> {
        if let Some(ecam) = self.ecam.take() {
            // SAFETY: 子硬件责任均完成，Engine 已停止扫描/配置；上级还在等本轮停止报告。
            if let Err((ecam, error)) = unsafe { ecam.unmap() } {
                self.ecam = Some(ecam);
                return Err(error.into());
            }
        }
        Ok(())
    }
}
fn unsafe_cap(handle: erhino_shared::object::Handle) -> Capability {
    // SAFETY: 调用者刚消费 typed 内核资源 owner；唯一 entry 关闭责任移入表。
    unsafe { Capability::from_raw(handle) }
}
