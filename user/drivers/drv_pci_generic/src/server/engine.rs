//! 原激活按预算取得资源、扫描 ECAM、登记真实 function，停止后保留清理服务。
use super::*;
use libdevice::identity::DeviceId;
use libpci::{
    config::{Address, XHCI_IDENTITY},
    host::HOST_RESOURCES,
};
#[derive(Clone, Copy)]
enum Call {
    Get(usize),
    Register,
    Withdraw,
}
pub(super) struct Engine {
    pending: Option<(u64, Call)>,
    next_resource: usize,
    scan: u16,
    enumerated: bool,
    withdrawn: bool,
    retirement: SourceRegistration,
    waiting: Option<erhino_shared::object::Handle>,
}
impl Engine {
    pub fn new() -> Self {
        Self {
            pending: None,
            next_resource: 0,
            scan: 0,
            enumerated: false,
            withdrawn: false,
            retirement: SourceRegistration::new(),
            waiting: None,
        }
    }
    pub fn deadline(&self) -> Deadline {
        Deadline::INFINITE
    }
    pub fn registered(&mut self, kind: u64, id: SourceId) {
        if kind == 7 {
            self.retirement.registered(kind, id);
        }
    }
    pub fn unregistered(&mut self, id: SourceId) {
        self.retirement.unregistered(id);
    }
    fn wait_retirement(
        &mut self,
        world: &World,
        requests: &mut Requests<'_, Family>,
    ) -> Result<Advance, SystemCallError> {
        let waiting = world.host.retirement_wait();
        if waiting != self.waiting {
            self.retirement.remove(requests)?;
            if !self.retirement.is_empty() {
                return Ok(step(Step::Parked));
            }
            self.waiting = waiting;
        }
        if let Some(handle) = waiting {
            if self.retirement.is_empty() {
                self.retirement.install(
                    requests,
                    SourcePlan::new(handle, ObjectSignals::DONE),
                    7,
                )?;
                return Ok(step(Step::Runnable));
            }
            if self.retirement.is_installing() {
                return Ok(step(Step::Runnable));
            }
        }
        Ok(step(Step::Parked))
    }
    fn begin(
        &mut self,
        id: u64,
        world: &mut World,
        target: Arc<MailboxSender>,
        command: device::Command<'_>,
        kind: Call,
    ) -> Result<(), Error> {
        let deadline = libkernel::time::timeout_millis(5_000)?;
        let mut bytes = [0; device::HEADER_LEN + device::BODY_MAX];
        let used = command.encode(deadline, &mut bytes)?;
        let mut request = librpc::Request::new(device::ID, &bytes[..used])?;
        if matches!(kind, Call::Register) {
            let child = world.host.child.as_ref().ok_or(Error::NotReady)?;
            request
                .push(alias(&child.sender)?, TRANSPORT)
                .map_err(|f| f.error)?;
        }
        self.send(id, world, target, deadline, request, kind)
    }
    fn send(
        &mut self,
        id: u64,
        world: &mut World,
        target: Arc<MailboxSender>,
        deadline: Deadline,
        request: librpc::Request,
        kind: Call,
    ) -> Result<(), Error> {
        let txid = world
            .rpc
            .begin_for(
                ServiceOwner::Shared(target),
                deadline,
                request,
                Some(id),
                &world.funding,
            )
            .map_err(|_| Error::Unavailable)?;
        world.rpc_dirty = true;
        self.pending = Some((txid, kind));
        Ok(())
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
        while let Some(event) = input.pull() {
            if let Some(slot) = &mut world.slot {
                slot.event(event);
            }
        }
        if let Some(slot) = &mut world.slot
            && slot.advance_lifetimes(requests)?
        {
            return Ok(step(Step::Runnable));
        }
        input.take_timeout();
        if world.slot.as_ref().is_none_or(|slot| !slot.io_ready()) {
            return Ok(step(Step::Parked));
        }
        let Some(_a) = world.slot.as_ref().and_then(|slot| slot.activation()) else {
            return Ok(step(Step::Parked));
        };
        if let Some((txid, kind)) = self.pending {
            let Some(result) = world.rpc.take(txid) else {
                return Ok(step(Step::Parked));
            };
            self.pending = None;
            // Unknown 保留原 Provision/Binding 责任并由外层故障封口，不换轮或制造安全结果。
            let mut reply = result.result.map_err(|error| {
                libkernel::debug!(
                    "PCI RPC failed: txid={}, phase={:?}, cause={:?}; original binding retained",
                    txid,
                    error.phase,
                    error.cause
                );
                Error::Unavailable
            })?;
            match kind {
                Call::Get(index) => match world.host.adopt(index, reply) {
                    Ok(()) => {
                        self.next_resource = index + 1;
                        libkernel::debug!(
                            "PCI resource acquired and verified: {}",
                            HOST_RESOURCES[index]
                        );
                    }
                    Err(Error::Closed) => {
                        world
                            .slot
                            .as_mut()
                            .expect("PCI activation missing")
                            .stop()?;
                    }
                    Err(error) => return Err(error),
                },
                Call::Register => {
                    let (header, body) = device::Header::decode(&reply.payload)?;
                    if header.op == 3
                        && header.error == Some(Error::Closed)
                        && body.is_empty()
                        && reply.handles.remaining() == 0
                    {
                        world.slot.as_mut().ok_or(Error::NotReady)?.stop()?;
                        return Ok(step(Step::Runnable));
                    }
                    if header.op != 3 || header.error.is_some() || reply.handles.remaining() != 1 {
                        return Err(header.error.unwrap_or(Error::Invalid));
                    }
                    let child = world
                        .host
                        .child
                        .as_mut()
                        .expect("PCI registered function missing");
                    child.device = Some(DeviceId::decode(&mut librpc::bytes::Reader::new(body))?);
                    child.control = Some(
                        MailboxSender::from_capability(reply.handles.take(0)?)
                            .map_err(|f| f.error)?
                            .0,
                    );
                    libkernel::debug!(
                        "PCI xHCI function registered: serial={}",
                        child
                            .device
                            .expect("PCI function identity missing")
                            .serial()
                    );
                }
                Call::Withdraw => {
                    let (header, body) = device::Header::decode(&reply.payload)?;
                    if header.op != 4
                        || header.error.is_some()
                        || !body.is_empty()
                        || reply.handles.remaining() != 0
                    {
                        return Err(header.error.unwrap_or(Error::Invalid));
                    }
                    self.withdrawn = true;
                }
            }
        }
        if world.slot.as_ref().is_some_and(|slot| {
            slot.activation()
                .is_some_and(|a| a.snapshot().phase == protocol::Phase::Terminal)
        }) {
            return self.wait_retirement(world, requests).map_err(Error::from);
        }
        let stopping = world
            .slot
            .as_ref()
            .and_then(|slot| slot.activation())
            .expect("PCI activation missing")
            .snapshot()
            .phase
            == protocol::Phase::Stopping;
        if stopping {
            if world
                .host
                .child
                .as_ref()
                .is_some_and(|c| c.device.is_some())
                && !self.withdrawn
            {
                let registrar = world
                    .slot
                    .as_ref()
                    .and_then(|slot| slot.activation())
                    .and_then(|a| a.registrar())
                    .ok_or(SystemCallError::InternalError)?
                    .clone();
                self.begin(
                    id,
                    world,
                    registrar,
                    device::Command::WithdrawPublication { seq: 1 },
                    Call::Withdraw,
                )?;
                return Ok(step(Step::Parked));
            }
            if world.host.advance()? {
                return Ok(step(Step::Runnable));
            }
            if world.host.hardware_idle() {
                world.host.unmap()?;
                let slot = world.slot.as_mut().expect("PCI activation missing");
                // SAFETY: 所有子供应已实际 Closed（含 xHCI halt/DMA/IRQ/MMIO）；扫描与配置
                // 在此分支永久关闭，ECAM 已解除。保留入口仅拒迟到请求/交付已有软件结果。
                unsafe {
                    slot.consumer_stopped()?;
                }
                slot.terminal(None)?;
                libkernel::debug!(
                    "PCI consumer stopped: ECAM unmapped; child hardware responsibility closed"
                );
                return Ok(step(Step::Runnable));
            }
            return self.wait_retirement(world, requests).map_err(Error::from);
        }
        if self.next_resource < 4 {
            let index = self.next_resource;
            let target = world
                .slot
                .as_ref()
                .and_then(|slot| slot.activation())
                .expect("PCI activation missing")
                .usage()?
                .clone();
            self.begin(
                id,
                world,
                target,
                device::Command::GetResource {
                    name: HOST_RESOURCES[index],
                },
                Call::Get(index),
            )?;
            return Ok(step(Step::Parked));
        }
        if world.host.ecam.is_none() {
            world.host.map()?;
            libkernel::debug!("PCI host resources acquired; ECAM mapped");
            return Ok(step(Step::Runnable));
        }
        if self.scan < 256 {
            let address = Address {
                slot: (self.scan / 8) as u8,
                function: (self.scan % 8) as u8,
            };
            self.scan += 1;
            // SAFETY: 原激活尚未报告停止，上级 Adapter 等该事实才 Revoke；单 actor 独占 ECAM。
            if let Some(function) = unsafe {
                world
                    .host
                    .ecam
                    .as_ref()
                    .expect("PCI ECAM missing")
                    .function(address)
            }? {
                libkernel::debug!(
                    "PCI function: bus=0 slot={} fn={} vendor={:04x} device={:04x} class={:06x}",
                    address.slot,
                    address.function,
                    function.vendor,
                    function.device,
                    function.class
                );
                if function.header & 0x7f == 1 {
                    return Err(Error::Unsupported);
                }
                if function.identity() == XHCI_IDENTITY {
                    world.host.discovered(function, &world.control.mailbox)?;
                }
            }
            return Ok(step(Step::Runnable));
        }
        if !self.enumerated {
            if world
                .host
                .child
                .as_ref()
                .is_some_and(|c| c.device.is_none())
            {
                let registrar = world
                    .slot
                    .as_ref()
                    .and_then(|slot| slot.activation())
                    .and_then(|a| a.registrar())
                    .ok_or(SystemCallError::InternalError)?
                    .clone();
                let parent = world
                    .slot
                    .as_ref()
                    .and_then(|slot| slot.activation())
                    .expect("PCI activation missing")
                    .reference()
                    .device();
                self.begin(
                    id,
                    world,
                    registrar,
                    device::Command::Register {
                        seq: 1,
                        parent: Some(parent),
                        name: "xhci",
                        identity: XHCI_IDENTITY,
                        limits: supply::LIMITS,
                        claimable: true,
                        stop_requirement: libdevice::binding::StopRequirement::ConsumerQuiescence,
                    },
                    Call::Register,
                )?;
                return Ok(step(Step::Parked));
            }
            self.enumerated = true;
            world
                .slot
                .as_mut()
                .expect("PCI activation missing")
                .ready()?;
            libkernel::debug!("PCI activation ready; bus 0 enumeration complete");
        }
        if world.host.advance()? {
            return Ok(step(Step::Runnable));
        }
        self.wait_retirement(world, requests).map_err(Error::from)
    }
}
