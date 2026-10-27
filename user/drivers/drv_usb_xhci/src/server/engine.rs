//! 实际控制器/USB任务与同一Dispatcher接线；等待管理RPC时仍消费事件/IRQ和硬件清理。
use super::*;
use libdevice::identity::DeviceId;
use libusb::xhci::{Controller, Progress};
#[derive(Clone, Copy)]
enum Call {
    Get(usize),
    Master(Deadline),
    Register(u64),
    Withdraw(u64),
}
pub(super) struct Engine {
    pending: Option<(u64, Call)>,
    next_resource: usize,
    source: SourceRegistration,
    next_poll: Deadline,
}
impl Engine {
    pub fn new() -> Self {
        Self {
            pending: None,
            next_resource: 0,
            source: SourceRegistration::new(),
            next_poll: Deadline::INFINITE,
        }
    }
    pub fn registered(&mut self, kind: u64, id: SourceId) {
        if kind == 7 {
            self.source.registered(kind, id);
        }
    }
    pub fn unregistered(&mut self, id: SourceId) {
        self.source.unregistered(id);
    }
    pub fn deadline(&self) -> Deadline {
        self.next_poll
    }
    fn begin(
        &mut self,
        id: u64,
        world: &mut World,
        target: Arc<MailboxSender>,
        request: librpc::Request,
        deadline: Deadline,
        operation: Call,
    ) -> Result<(), Error> {
        let tx = world
            .rpc
            .begin_for(
                ServiceOwner::Shared(target),
                deadline,
                request,
                Some(id),
                &world.funding,
            )
            .map_err(|_| Error::Unavailable)?;
        self.pending = Some((tx, operation));
        world.rpc_dirty = true;
        Ok(())
    }
    fn device(
        &mut self,
        id: u64,
        world: &mut World,
        command: device::Command<'_>,
        operation: Call,
    ) -> Result<(), Error> {
        let target = world
            .slot
            .as_ref()
            .and_then(|slot| slot.activation())
            .ok_or(Error::NotReady)?
            .usage()
            .unwrap_or_else(|e| fatal(e))
            .clone();
        let deadline = operation_end()?;
        let mut bytes = [0; device::HEADER_LEN + device::BODY_MAX];
        let used = command.encode(deadline, &mut bytes)?;
        self.begin(
            id,
            world,
            target,
            librpc::Request::new(device::ID, &bytes[..used])?,
            deadline,
            operation,
        )
    }
    fn master(&mut self, id: u64, world: &mut World, deadline: Deadline) -> Result<(), Error> {
        if libkernel::time::expired(deadline)? {
            return Err(Error::Unavailable);
        }
        let target = world
            .host
            .controller
            .as_ref()
            .ok_or(Error::NotReady)?
            .function
            .clone();
        let reference = world
            .slot
            .as_ref()
            .and_then(|s| s.activation())
            .ok_or(Error::NotReady)?
            .reference();
        libkernel::debug!(
            "xHCI EnableMaster submitted: device={}, epoch={}, context={}, deadline={}",
            reference.device().serial(),
            reference.epoch(),
            target.description()?.object_id,
            deadline.at_ns
        );
        let mut wire = [0; device::HEADER_LEN];
        let used = device::Header {
            op: 1,
            error: None,
            deadline,
        }
        .encode(&[], &mut wire)?;
        self.begin(
            id,
            world,
            target,
            librpc::Request::new(libpci::function::ID, &wire[..used])?,
            deadline,
            Call::Master(deadline),
        )
    }
    fn publication(
        &mut self,
        id: u64,
        world: &mut World,
        connection: u64,
        withdraw: bool,
    ) -> Result<(), Error> {
        let record = world
            .host
            .records
            .iter()
            .find(|r| r.connection == connection)
            .ok_or(Error::NotFound)?;
        let deadline = operation_end()?;
        let mut bytes = [0; device::HEADER_LEN + device::BODY_MAX];
        // 名字属于父设备的目录；同一父轮多连接不能共用一个固定分量。
        let name = alloc::format!("keyboard-{}", record.connection);
        let command = if withdraw {
            device::Command::WithdrawPublication {
                seq: record.publication,
            }
        } else {
            device::Command::Register {
                seq: record.publication,
                parent: Some(
                    world
                        .slot
                        .as_ref()
                        .and_then(|slot| slot.activation())
                        .ok_or(Error::NotReady)?
                        .reference()
                        .device(),
                ),
                name: &name,
                identity: libusb::descriptor::BOOT_KEYBOARD_IDENTITY,
                limits: supply::LIMITS,
                claimable: true,
                stop_requirement: libdevice::binding::StopRequirement::ProviderOwned,
            }
        };
        let used = command.encode(deadline, &mut bytes)?;
        let mut request = librpc::Request::new(device::ID, &bytes[..used])?;
        if !withdraw {
            request
                .push(
                    alias(record.provider.as_ref().ok_or(Error::Closed)?)?,
                    TRANSPORT,
                )
                .map_err(|f| f.error)?;
        }
        let target = world
            .slot
            .as_ref()
            .and_then(|slot| slot.activation())
            .and_then(|a| a.registrar())
            .ok_or(Error::Permission)?
            .clone();
        self.begin(
            id,
            world,
            target,
            request,
            deadline,
            if withdraw {
                Call::Withdraw(connection)
            } else {
                Call::Register(connection)
            },
        )
    }
    fn hardware(world: &mut World, now: u64, open: bool) -> Result<bool, Error> {
        let Some(controller) = world.host.controller.as_mut() else {
            return Ok(false);
        };
        if !controller.running() {
            return Ok(false);
        }
        // 复用Controller原Ready健康门，先拒HCE/HSE再消费/提交本轮USB动作。
        controller.step(now)?;
        let mut more = false;
        for _ in 0..16 {
            if let Some(event) = controller.poll_event()? {
                libkernel::debug!("xHCI hardware event: {:?}", event);
                controller.usb_event(event)?;
                more = true;
            } else {
                more = false;
                break;
            }
        }
        if !more {
            controller.acknowledge_interrupt()?;
        }
        controller.usb_step(now, open, operation_end()?)?;
        world.host.advance(now, open)?;
        // 按真实连接的供应责任保留物理owner，不让另一连接的活绑定阻挡已安全连接退休。
        let closed = world.host.controller.as_ref().and_then(|controller| {
            controller
                .closed_connections()
                .find(|connection| !world.host.connection_in_use(*connection))
        });
        if let Some(connection) = closed {
            world
                .host
                .controller
                .as_mut()
                .ok_or(Error::NotReady)?
                .usb_forget_disconnected(connection)?;
        }
        Ok(more)
    }
    pub fn advance(
        &mut self,
        id: u64,
        world: &mut World,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        match self.drive(id, world, requests, input) {
            Ok(advance) => Ok(advance),
            Err(error) => {
                let slot = world.slot.as_mut().unwrap_or_else(|| fatal(error));
                if !matches!(
                    slot.activation()
                        .ok_or(Error::NotReady)
                        .unwrap_or_else(|e| fatal(e))
                        .snapshot()
                        .phase,
                    protocol::Phase::Starting | protocol::Phase::Ready
                ) {
                    fatal(error);
                }
                slot.fail(error).unwrap_or_else(|e| fatal(e));
                Ok(step(Step::Runnable))
            }
        }
    }
    fn drive(
        &mut self,
        id: u64,
        world: &mut World,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, Error> {
        let mut irq_hint = false;
        while let Some(event) = input.pull() {
            if let Some(slot) = &mut world.slot {
                slot.event(event);
            }
            irq_hint |= event.kind == 7
                && self.source.source() == Some(event.source)
                && event.observed.intersects(ObjectSignals::READABLE);
        }
        if let Some(slot) = &mut world.slot
            && slot.advance_lifetimes(requests)?
        {
            return Ok(step(Step::Runnable));
        }
        if world.host.retire_sources(requests)? {
            return Ok(step(Step::Runnable));
        }
        let timed = input.take_timeout();
        if world.slot.as_ref().is_none_or(|slot| !slot.io_ready()) {
            return Ok(step(Step::Parked));
        }
        if irq_hint && !timed {
            libkernel::debug!("xHCI IRQ source woke engine before timer");
        }
        let Some(activation) = world.slot.as_ref().and_then(|slot| slot.activation()) else {
            return Ok(step(Step::Parked));
        };
        let stopping = activation.snapshot().phase == protocol::Phase::Stopping;
        let running = world
            .host
            .controller
            .as_ref()
            .is_some_and(Controller::running);
        let mut more = Self::hardware(world, input.now_ns(), !stopping)?;
        if running {
            let due = world
                .host
                .controller
                .as_ref()
                .and_then(|controller| controller.usb_deadline(input.now_ns()));
            self.next_poll = due
                .into_iter()
                .chain(world.host.transfer_deadline())
                .min()
                .map_or(Deadline::INFINITE, Deadline::at);
            more |= due.is_some_and(|at| at <= input.now_ns());
        }
        if let Some((tx, operation)) = self.pending {
            let Some(result) = world.rpc.take(tx) else {
                return Ok(step(if more { Step::Runnable } else { Step::Parked }));
            };
            self.pending = None;
            let mut reply = match result.result {
                Ok(reply) => reply,
                Err(error) if matches!(operation, Call::Master(_)) => {
                    libkernel::debug!(
                        "xHCI EnableMaster reply failed: txid={}, phase={:?}, cause={:?}; original controller retained",
                        tx,
                        error.phase,
                        error.cause
                    );
                    if error.phase != librpc::CallPhase::Sent {
                        return Err(Error::Unavailable);
                    }
                    let recoverable = matches!(
                        error.cause,
                        librpc::CallCause::ServiceClosed | librpc::CallCause::Timeout
                    );
                    // 本次RPC注销后只复用原function及绝对期限；停止先到则沿原硬件清理，
                    // 不重新配置Controller、派生DMA域或以另一轮掩盖BME的未知效果。
                    if !stopping {
                        if !recoverable {
                            return Err(Error::Unavailable);
                        }
                        let Call::Master(deadline) = operation else {
                            unreachable!("Master failure lost original deadline");
                        };
                        self.master(id, world, deadline)?;
                    }
                    return Ok(step(if stopping {
                        Step::Runnable
                    } else {
                        Step::Parked
                    }));
                }
                Err(_) => fatal(Error::Unavailable),
            };
            if matches!(operation, Call::Master(_) | Call::Register(_)) {
                let (header, body) = device::Header::decode(&reply.payload)?;
                let expected = if matches!(operation, Call::Master(_)) {
                    1
                } else {
                    3
                };
                if header.op != expected {
                    return Err(Error::Invalid);
                }
                if header.error == Some(Error::Closed) {
                    if !body.is_empty() || reply.handles.remaining() != 0 {
                        return Err(Error::Invalid);
                    }
                    if let Call::Register(connection) = operation {
                        let record = world
                            .host
                            .records
                            .iter_mut()
                            .find(|record| record.connection == connection)
                            .ok_or(Error::NotFound)?;
                        // 明确拒绝未发布登记，关闭本地出口；真实 USB 清理由原 Controller 继续持有。
                        record.provider = None;
                        record.withdrawn = true;
                    }
                    world.slot.as_mut().ok_or(Error::NotReady)?.stop()?;
                    return Ok(step(Step::Runnable));
                }
            }
            match operation {
                Call::Get(index) => {
                    match world
                        .host
                        .inputs
                        .as_mut()
                        .ok_or(SystemCallError::InternalError)?
                        .adopt(index, reply)
                    {
                        Ok(()) => self.next_resource = index + 1,
                        Err(Error::Closed) => {
                            // 供应封新取得不表示硬件清理失败；原已取得输入仍交停止 owner。
                            world.slot.as_mut().ok_or(Error::NotReady)?.stop()?;
                            return Ok(step(Step::Runnable));
                        }
                        Err(error) => return Err(error),
                    }
                }
                Call::Master(_) => {
                    let (header, body) = device::Header::decode(&reply.payload)?;
                    if header.op != 1
                        || header.error.is_some()
                        || !body.is_empty()
                        || reply.handles.remaining() != 0
                    {
                        return Err(header.error.unwrap_or(Error::Invalid));
                    }
                    world
                        .host
                        .controller
                        .as_mut()
                        .ok_or(SystemCallError::InternalError)?
                        .master_enabled()?;
                }
                Call::Register(connection) => {
                    let (header, body) = device::Header::decode(&reply.payload)?;
                    if header.op != 3 || header.error.is_some() || reply.handles.remaining() != 1 {
                        return Err(header.error.unwrap_or(Error::Invalid));
                    }
                    let device = DeviceId::decode(&mut librpc::bytes::Reader::new(body))?;
                    let record = world
                        .host
                        .records
                        .iter_mut()
                        .find(|r| r.connection == connection)
                        .ok_or(SystemCallError::InternalError)?;
                    record.control = Some(
                        MailboxSender::from_capability(reply.handles.take(0)?)
                            .map_err(|f| f.error)?
                            .0,
                    );
                    record.device = Some(device);
                    record.provider = None;
                    world.host.publication = record.publication;
                    libkernel::debug!(
                        "USB boot keyboard registered: connection={}, serial={}",
                        connection,
                        device.serial()
                    );
                }
                Call::Withdraw(connection) => {
                    let (header, body) = device::Header::decode(&reply.payload)?;
                    if header.op != 4
                        || header.error.is_some()
                        || !body.is_empty()
                        || reply.handles.remaining() != 0
                    {
                        return Err(header.error.unwrap_or(Error::Invalid));
                    }
                    let record = world
                        .host
                        .records
                        .iter_mut()
                        .find(|r| r.connection == connection)
                        .ok_or(SystemCallError::InternalError)?;
                    record.withdrawn = true;
                    record.control = None;
                    libkernel::debug!("USB interface withdrawn: connection={}", connection);
                }
            }
        }
        if world.slot.as_ref().is_some_and(|slot| {
            slot.activation()
                .is_some_and(|a| a.snapshot().phase == protocol::Phase::Terminal)
        }) {
            self.next_poll = Deadline::INFINITE;
            return Ok(step(Step::Parked));
        }
        if stopping {
            if let Some(connection) = world
                .host
                .records
                .iter()
                .find(|r| r.device.is_some() && !r.withdrawn)
                .map(|r| r.connection)
            {
                self.publication(id, world, connection, true)?;
                return Ok(step(Step::Parked));
            }
            let provisions_open = world.host.has_open_provisions();
            if let Some(controller) = world.host.controller.as_mut() {
                if !controller.usb_idle() || provisions_open {
                    return Ok(step(if more { Step::Runnable } else { Step::Parked }));
                }
                // SAFETY: 同一owner已收完全部USB端点/slot/TD/command责任；中央软件壳仍可继续查询。
                unsafe {
                    controller.begin_stop()?;
                }
                if controller.step(input.now_ns())? != Progress::Closed {
                    self.next_poll = libkernel::time::timeout_millis(1)?;
                    return Ok(step(Step::Parked));
                }
                self.source.remove(requests)?;
                if !self.source.is_empty() {
                    return Ok(step(Step::Runnable));
                }
                let controller = world
                    .host
                    .controller
                    .take()
                    .ok_or(SystemCallError::InternalError)?;
                // SAFETY: HCH/残事件/逐页DMA DONE已成立；父PCI仍等原轮静止报告才撤MMIO。
                if let Err((controller, error)) = unsafe { controller.unmap() } {
                    world.host.controller = Some(controller);
                    return Err(error.into());
                }
            }
            world.host.inputs = None;
            let slot = world.slot.as_mut().ok_or(SystemCallError::InternalError)?;
            // SAFETY: 本轮硬件/USB子清理均结束，入口只保留迟到拒绝/既有软件结果。
            unsafe {
                slot.consumer_stopped()?;
            }
            slot.terminal(None)?;
            self.next_poll = Deadline::INFINITE;
            libkernel::debug!(
                "xHCI consumer stopped: HCH, event drain, DMA DONE and register unmap confirmed"
            );
            return Ok(step(Step::Runnable));
        }
        if self.next_resource < 4 {
            let index = self.next_resource;
            self.device(
                id,
                world,
                device::Command::GetResource {
                    name: libpci::function::RESOURCES[index],
                },
                Call::Get(index),
            )?;
            return Ok(step(Step::Parked));
        }
        if world.host.controller.is_none() {
            world.host.controller = Some(Controller::adopt(
                world
                    .host
                    .inputs
                    .take()
                    .ok_or(SystemCallError::InternalError)?,
            )?);
            libkernel::debug!("xHCI PCI resources acquired; registers mapped");
            return Ok(step(Step::Runnable));
        }
        let controller = world
            .host
            .controller
            .as_mut()
            .ok_or(SystemCallError::InternalError)?;
        if self.source.is_empty() {
            self.source.install(
                requests,
                SourcePlan::new(
                    controller.interrupt.handle(),
                    ObjectSignals::READABLE | ObjectSignals::CLOSED,
                ),
                7,
            )?;
            return Ok(step(Step::Runnable));
        }
        if self.source.is_installing() {
            return Ok(step(Step::Runnable));
        }
        match controller.step(input.now_ns())? {
            Progress::EnableMaster => {
                self.master(id, world, operation_end()?)?;
                return Ok(step(Step::Parked));
            }
            Progress::Starting => {
                self.next_poll = libkernel::time::timeout_millis(1)?;
                return Ok(step(Step::Parked));
            }
            Progress::Ready => {}
            _ => return Err(Error::Invalid),
        }
        if !running {
            more |= Self::hardware(world, input.now_ns(), true)?;
        }
        if world
            .slot
            .as_ref()
            .and_then(|slot| slot.activation())
            .ok_or(SystemCallError::InternalError)?
            .snapshot()
            .phase
            == protocol::Phase::Starting
        {
            world
                .slot
                .as_mut()
                .ok_or(SystemCallError::InternalError)?
                .ready()?;
            libkernel::debug!(
                "xHCI activation ready: controller running with coherent rings and INTx"
            );
        }
        let mut withdraw = None;
        for record in &world.host.records {
            if record.device.is_some()
                && !record.withdrawn
                && !world
                    .host
                    .controller
                    .as_ref()
                    .ok_or(SystemCallError::InternalError)?
                    .connection_present(record.connection)?
            {
                withdraw = Some(record.connection);
                break;
            }
        }
        if let Some(connection) = withdraw {
            self.publication(id, world, connection, true)?;
            return Ok(step(Step::Parked));
        }
        let mut discovered = None;
        for interface in world
            .host
            .controller
            .as_ref()
            .ok_or(SystemCallError::InternalError)?
            .interfaces()
        {
            let interface = interface?;
            if interface.ready
                && interface.present
                && !world.host.records.iter().any(|r| {
                    r.connection == interface.connection && (r.device.is_some() || r.withdrawn)
                })
            {
                discovered = Some(interface);
                break;
            }
        }
        if let Some(interface) = discovered {
            world.host.discover(interface, &world.control.mailbox)?;
            self.publication(id, world, interface.connection, false)?;
            return Ok(step(Step::Parked));
        }
        let due = world
            .host
            .controller
            .as_ref()
            .and_then(|controller| controller.usb_deadline(input.now_ns()));
        self.next_poll = due
            .into_iter()
            .chain(world.host.transfer_deadline())
            .min()
            .map_or(Deadline::INFINITE, Deadline::at);
        more |= due.is_some_and(|at| at <= input.now_ns());
        Ok(step(if more { Step::Runnable } else { Step::Parked }))
    }
}
