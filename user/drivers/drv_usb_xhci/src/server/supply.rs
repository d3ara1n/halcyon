//! 受限USB接口的登记/供应/请求原owner；物理slot/TD仍只由Controller保存。
use super::*;
use alloc::vec::Vec;
use libbudget::Charge;
use libdevice::{
    identity::DeviceId,
    protocol::{Progress, ProvisionCommand},
    provision::{self, Provision},
    resource::{Collecting, Descriptor, DeviceResource, Entry, Frozen, Limits},
};
use libkernel::ipc::{message::LifetimeObserver, wait};
use libusb::{
    descriptor::Keyboard,
    protocol as usb,
    xhci::{Controller, Inputs, Interface},
};
const RECORDS: usize = 32;
pub(super) const LIMITS: Limits = Limits {
    entries: 1,
    text_bytes: ("usb-interface".len() + Keyboard::BYTES) as u32,
};
struct Transfer {
    sequence: u64,
    request: Option<usb::Transfer>,
    end: Option<u64>,
    result: Option<usb::Progress>,
}
pub(super) struct InterfaceSupply {
    pub connection: u64,
    pub keyboard: Keyboard,
    pub publication: u64,
    pub context: u64,
    pub provider: Option<Arc<MailboxSender>>,
    life: LifetimeObserver,
    pub device: Option<DeviceId>,
    pub control: Option<MailboxSender>,
    pub withdrawn: bool,
    provision: Provision,
    end: Deadline,
    collecting: Option<Collecting<Capability>>,
    frozen: Option<Frozen<Capability>>,
    session: Option<Arc<MailboxSender>>,
    session_context: u64,
    current: Option<Transfer>,
}
pub(super) struct Host {
    pub inputs: Option<Inputs>,
    pub controller: Option<Controller>,
    pub records: Vec<InterfaceSupply>,
    pub publication: u64,
    funding: AccountView<DeviceResource>,
    _storage: Charge,
    _account: Arc<libbudget::Account>,
    _budget: Arc<Budget>,
    gc: usize,
}
impl Host {
    pub fn is_provider_context(&self, context: u64) -> bool {
        self.records.iter().any(|record| record.context == context)
    }
    pub fn new() -> Result<Self, Error> {
        // 域额度支付真实记录数组和每项冻结表；与硬件端口数/中央占用分别建模。
        let mut records = Vec::new();
        records
            .try_reserve_exact(RECORDS)
            .map_err(|_| Error::Resource)?;
        let storage = records
            .capacity()
            .checked_mul(core::mem::size_of::<InterfaceSupply>())
            .ok_or(Error::Quota)?;
        let tables = Collecting::<Capability>::storage_bytes(LIMITS)?
            .checked_mul(RECORDS)
            .ok_or(Error::Quota)?;
        let limits = [
            0,
            0,
            storage.checked_add(tables).ok_or(Error::Quota)?,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
        ];
        let budget = Budget::new(&limits, 1)?;
        let account = budget.account(&limits)?;
        let slots =
            core::array::from_fn::<_, 10, _>(|i| budget.slot(i).expect("USB domain slot missing"));
        let funding = account.view(&slots)?;
        let charge = funding.acquire(DeviceResource::Bytes, storage)?;
        Ok(Self {
            inputs: Some(Inputs::new()),
            controller: None,
            records,
            publication: 0,
            funding,
            _storage: charge,
            _account: account,
            _budget: budget,
            gc: 0,
        })
    }
    pub fn discover(&mut self, interface: Interface, mailbox: &Mailbox) -> Result<(), Error> {
        if self
            .records
            .iter()
            .any(|r| r.connection == interface.connection)
        {
            return Ok(());
        }
        if self.records.len() == RECORDS {
            return Err(Error::Quota);
        }
        let publication = self.publication.checked_add(1).ok_or(Error::Exhausted)?;
        let minted = mailbox.mint(0, TRANSPORT)?;
        let context = minted.sender.description()?.object_id;
        let provider = Arc::try_new(minted.sender).map_err(|_| Error::Resource)?;
        // publication只在原Register回执确认后推进；Unknown停止原父轮，不发替代序号。
        self.records.push(InterfaceSupply {
            connection: interface.connection,
            keyboard: interface.keyboard,
            publication,
            context,
            provider: Some(provider),
            life: minted.lifetime,
            device: None,
            control: None,
            withdrawn: false,
            provision: Provision::new(),
            end: Deadline::INFINITE,
            collecting: None,
            frozen: None,
            session: None,
            session_context: 0,
            current: None,
        });
        Ok(())
    }
    fn by_context(&self, context: u64) -> Result<usize, Error> {
        self.records
            .iter()
            .position(|r| r.context == context)
            .ok_or(Error::Permission)
    }
    pub fn command(
        &mut self,
        context: u64,
        command: ProvisionCommand,
        deadline: Deadline,
        mailbox: &Mailbox,
        open: bool,
    ) -> Result<Progress, Error> {
        let index = self.by_context(context)?;
        let record = &mut self.records[index];
        let epoch = command.epoch();
        match command {
            ProvisionCommand::Prepare(_) => {
                if !open && record.provision.active().is_none() {
                    record.provision.stop(epoch)?;
                    return Ok(Progress::Closed);
                }
                if record.provision.prepare(epoch)? == provision::Prepare::Started {
                    let prepared = (|| {
                        let mut table = Collecting::reserve(
                            LIMITS,
                            device::RESOURCE_HEADER_LEN,
                            &self.funding,
                        )?;
                        table.prepared(1)?;
                        let sender = Arc::try_new(mailbox.mint(0, TRANSPORT)?.sender)
                            .map_err(|_| Error::Resource)?;
                        let session_context = sender.description()?.object_id;
                        Ok::<_, Error>((table, sender, session_context))
                    })();
                    match prepared {
                        Ok((table, sender, session_context)) => {
                            record.collecting = Some(table);
                            record.session = Some(sender);
                            record.session_context = session_context;
                            record.current = None;
                            record.end = deadline;
                        }
                        Err(error) => {
                            record.provision.rejected(epoch)?;
                            return Ok(Progress::Rejected(error));
                        }
                    }
                }
                if !open {
                    record.provision.stop(epoch)?;
                    record.end = deadline;
                }
            }
            ProvisionCommand::Stop(_) => {
                let stopping = record
                    .provision
                    .active()
                    .is_some_and(|a| a.phase == provision::Phase::Stopping);
                record.provision.stop(epoch)?;
                if !stopping {
                    record
                        .log_stopping(epoch, self.controller.as_ref().ok_or(Error::Unavailable)?)?;
                    record.end = deadline;
                }
            }
            ProvisionCommand::ConfirmStopped(_) => record.provision.confirm_stopped(epoch)?,
            ProvisionCommand::Get { .. } => return Err(Error::Invalid),
        }
        Ok(record.progress(epoch))
    }
    pub fn resource(
        &self,
        context: u64,
        epoch: u64,
        index: u32,
    ) -> Result<libdevice::resource::Resource<'_, Capability>, Error> {
        let record = &self.records[self.by_context(context)?];
        let active = record.provision.active().ok_or(Error::Closed)?;
        if active.epoch != epoch {
            return Err(Error::StaleEpoch);
        }
        if !matches!(active.phase, provision::Phase::Prepared { .. }) {
            return Err(Error::NotReady);
        }
        record
            .frozen
            .as_ref()
            .and_then(|table| table.at(index as usize))
            .ok_or(Error::NotFound)
    }
    pub fn transfer(
        &mut self,
        context: u64,
        command: usb::Command,
        deadline: Deadline,
        open: bool,
    ) -> Result<usb::Progress, Error> {
        let record = self
            .records
            .iter_mut()
            .find(|r| r.session_context != 0 && r.session_context == context)
            .ok_or(Error::Permission)?;
        if record.provision.active().is_none() {
            return Ok(usb::Progress::Closed);
        }
        let sequence = command.sequence();
        if let Some(current) = record.current.as_ref() {
            if sequence < current.sequence {
                return Err(Error::Closed);
            }
            if sequence == current.sequence {
                if let usb::Command::Transfer { request, .. } = command
                    && current.request.is_some_and(|old| old != request)
                {
                    return Err(Error::Conflict);
                }
                if let usb::Command::Cancel { .. } = command
                    && current.result.is_none()
                {
                    self.controller
                        .as_mut()
                        .ok_or(Error::Unavailable)?
                        .usb_cancel(record.connection, deadline)?;
                }
                return Ok(current.result.unwrap_or(usb::Progress::Pending));
            }
            if sequence != current.sequence.checked_add(1).ok_or(Error::Exhausted)?
                || current.result.is_none()
            {
                return Err(Error::Busy);
            }
        } else if sequence != 1 {
            return Err(Error::Invalid);
        }
        if !open
            || !record
                .provision
                .active()
                .is_some_and(|a| matches!(a.phase, provision::Phase::Prepared { .. }))
        {
            return Err(Error::Closed);
        }
        match command {
            usb::Command::Query { .. } => Err(Error::NotFound),
            usb::Command::Cancel { .. } => {
                record.current = Some(Transfer {
                    sequence,
                    request: None,
                    end: None,
                    result: Some(usb::Progress::Cancelled),
                });
                Ok(usb::Progress::Cancelled)
            }
            usb::Command::Transfer { request, .. } => {
                let controller = self.controller.as_mut().ok_or(Error::Unavailable)?;
                controller.usb_can_transfer(record.connection)?;
                // 原seq/输入/结果容量先入记录，再提交doorbell；写入失败也不抹去潜在硬件效果。
                record.current = Some(Transfer {
                    sequence,
                    request: Some(request),
                    end: deadline.instant().map_err(|_| Error::Invalid)?,
                    result: None,
                });
                controller.usb_transfer(record.connection, request)?;
                Ok(usb::Progress::Pending)
            }
        }
    }
    pub fn advance(&mut self, now: u64, open: bool) -> Result<(), Error> {
        let Some(controller) = self.controller.as_mut() else {
            return Ok(());
        };
        for record in &mut self.records {
            let Some(mut active) = record.provision.active() else {
                continue;
            };
            if !open && active.phase != provision::Phase::Stopping {
                record.provision.stop(active.epoch)?;
                record.log_stopping(active.epoch, controller)?;
                record.end = operation_end()?;
                active = record.provision.active().ok_or(Error::Closed)?;
            }
            if active.phase == provision::Phase::Preparing {
                let prepared = match controller.usb_prepare(record.connection, record.end) {
                    Ok(ready) => ready,
                    Err(Error::Closed) => {
                        record.provision.failed(active.epoch, Error::Closed)?;
                        record.log_stopping(active.epoch, controller)?;
                        record.end = operation_end()?;
                        false
                    }
                    Err(error) => return Err(error),
                };
                if prepared {
                    let interface = controller
                        .interface(record.connection)?
                        .ok_or(Error::NotReady)?;
                    if interface.connection != record.connection
                        || interface.keyboard != record.keyboard
                    {
                        return Err(Error::Conflict);
                    }
                    let sender = record.session.as_ref().ok_or(Error::NotReady)?;
                    let cap = alias(sender)?;
                    let table = record.collecting.as_mut().ok_or(Error::NotReady)?;
                    table.push(
                        &Entry {
                            index: 0,
                            descriptor: Descriptor {
                                kind: erhino_shared::object::ObjectKind::MailboxSender,
                                rights: TRANSPORT,
                                protocol: usb::ID,
                            },
                            name: "usb-interface",
                            metadata: &record.keyboard.encode(),
                        },
                        cap,
                    )?;
                    record.frozen = Some(
                        record
                            .collecting
                            .take()
                            .ok_or(Error::NotReady)?
                            .finish()
                            .map_err(|_| Error::Invalid)?,
                    );
                    record.provision.prepared(active.epoch, 1)?;
                }
            }
            if let Some(current) = &mut record.current
                && current.result.is_none()
            {
                if let Some(result) = controller.usb_result(record.connection)? {
                    current.result = Some(match result {
                        Ok((bytes, count)) => usb::Progress::Complete { bytes, count },
                        Err(Error::Closed) => usb::Progress::Cancelled,
                        Err(error) => return Err(error),
                    });
                } else if current.end.is_some_and(|end| now >= end) {
                    controller.usb_cancel(record.connection, operation_end()?)?;
                }
            }
            if active.phase == provision::Phase::Stopping {
                controller.usb_stop(record.connection, record.end)?;
                if controller.usb_stopped(record.connection)? {
                    record.frozen = None;
                    record.collecting = None;
                    record.session = None;
                    record.current = None;
                    record.provision.finish(active.epoch)?;
                    libkernel::debug!(
                        "USB interface supply closed: connection={}, epoch={}",
                        record.connection,
                        active.epoch
                    );
                }
            }
        }
        // 每轮只观察一个旧来源；真实Registry/Binding/Delivery关系退休后才移除观察壳。
        if !self.records.is_empty() {
            self.gc %= self.records.len();
            let record = &self.records[self.gc];
            if record.withdrawn
                && record.provision.active().is_none()
                && record.provider.is_none()
                && wait::probe(record.life.as_handle(), ObjectSignals::CLOSED)?
                    .contains(ObjectSignals::CLOSED)
            {
                self.records.remove(self.gc);
            } else {
                self.gc += 1;
            }
        }
        Ok(())
    }
    pub fn connection_in_use(&self, connection: u64) -> bool {
        self.records
            .iter()
            .any(|record| record.connection == connection && record.provision.active().is_some())
    }
    pub fn has_open_provisions(&self) -> bool {
        self.records.iter().any(|r| r.provision.active().is_some())
    }
}
impl InterfaceSupply {
    fn log_stopping(&self, epoch: u64, controller: &Controller) -> Result<(), Error> {
        libkernel::debug!(
            "USB interface stopping: connection={}, epoch={}, descriptor_ready={}",
            self.connection,
            epoch,
            controller.interface(self.connection)?.is_some()
        );
        Ok(())
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
}
