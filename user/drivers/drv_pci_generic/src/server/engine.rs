//! 原激活按预算取得资源、扫描 ECAM、登记真实 function，停止后保留清理服务。
use super::*;
use libdevice::{binding::Phase as BindingPhase, identity::DeviceId};
use libpci::{
    config::{Address, XHCI_IDENTITY},
    host::HOST_RESOURCES,
};
#[derive(Clone, Copy)]
enum Call {
    Describe(usize, Deadline),
    Get(usize, libdevice::resource::Descriptor),
    Register,
    Withdraw,
    Query,
}
pub(super) struct Engine {
    pending: Option<(u64, Call)>,
    next_resource: usize,
    scan: u16,
    enumerated: bool,
    withdrawn: bool,
    next_poll: Deadline,
}
impl Engine {
    pub fn new() -> Self {
        Self {
            pending: None,
            next_resource: 0,
            scan: 0,
            enumerated: false,
            withdrawn: false,
            next_poll: Deadline::INFINITE,
        }
    }
    pub fn deadline(&self) -> Deadline {
        self.next_poll
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
        let Some(_a) = world
            .slot
            .as_ref()
            .and_then(|slot| slot.activation.as_ref())
        else {
            return Ok(step(Step::Parked));
        };
        if let Some((txid, kind)) = self.pending {
            let Some(result) = world.rpc.take(txid) else {
                return Ok(step(Step::Parked));
            };
            self.pending = None;
            // Unknown 保留原 Provision/Binding 责任并由外层故障封口，不换轮或制造安全结果。
            let mut reply = result.result.unwrap_or_else(|error| {
                libkernel::debug!(
                    "PCI RPC failed: txid={}, phase={:?}, cause={:?}; original binding retained",
                    txid,
                    error.phase,
                    error.cause
                );
                fatal(Error::Unavailable)
            });
            match kind {
                Call::Describe(index, deadline) => {
                    if reply.handles.remaining() != 0 {
                        fatal(Error::Invalid);
                    }
                    let request = liboal::protocol::Request::Lookup {
                        path: HOST_RESOURCES[index],
                    };
                    match liboal::protocol::decode_response(&reply.payload, &request, deadline, 0)
                        .unwrap_or_else(|_| fatal(Error::Invalid))
                    {
                        liboal::protocol::Response::Found(info) => {
                            let info = libdevice::resource::oal::Info::decode(info)
                                .unwrap_or_else(|_| fatal(Error::Invalid));
                            let activation = world
                                .slot
                                .as_ref()
                                .and_then(|s| s.activation.as_ref())
                                .expect("PCI activation missing");
                            if info.reference != activation.reference() {
                                fatal(Error::StaleEpoch);
                            }
                            let descriptor =
                                info.descriptor.unwrap_or_else(|| fatal(Error::Invalid));
                            if !matches!(
                                activation.snapshot().phase,
                                protocol::Phase::Stopping | protocol::Phase::Terminal
                            ) {
                                let usage = activation.usage().unwrap_or_else(|e| fatal(e)).clone();
                                self.begin(
                                    id,
                                    world,
                                    usage,
                                    device::Command::GetResource {
                                        name: HOST_RESOURCES[index],
                                    },
                                    Call::Get(index, descriptor),
                                )
                                .unwrap_or_else(|e| fatal(e));
                                return Ok(step(Step::Parked));
                            }
                        }
                        liboal::protocol::Response::Error(liboal::protocol::Status::Revoked) => {
                            world
                                .slot
                                .as_mut()
                                .and_then(|s| s.activation.as_mut())
                                .expect("PCI activation missing")
                                .stop();
                            world.changed()?;
                        }
                        _ => fatal(Error::Invalid),
                    }
                }
                Call::Get(index, descriptor) => match world.host.adopt(index, descriptor, reply) {
                    Ok(()) => {
                        self.next_resource = index + 1;
                        libkernel::debug!(
                            "PCI resource Describe/Get matched: {}",
                            HOST_RESOURCES[index]
                        );
                    }
                    Err(Error::Closed) => {
                        world
                            .slot
                            .as_mut()
                            .and_then(|s| s.activation.as_mut())
                            .expect("PCI activation missing")
                            .stop();
                        world.changed()?;
                    }
                    Err(error) => fatal(error),
                },
                Call::Register => {
                    let (header, body) =
                        device::Header::decode(&reply.payload).unwrap_or_else(|e| fatal(e));
                    if header.op != 3 || header.error.is_some() || reply.handles.remaining() != 1 {
                        fatal(header.error.unwrap_or(Error::Invalid));
                    }
                    let child = world
                        .host
                        .child
                        .as_mut()
                        .expect("PCI registered function missing");
                    child.device = Some(
                        DeviceId::decode(&mut librpc::bytes::Reader::new(body))
                            .unwrap_or_else(|e| fatal(e)),
                    );
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
                    let (header, body) =
                        device::Header::decode(&reply.payload).unwrap_or_else(|e| fatal(e));
                    if header.op != 4
                        || header.error.is_some()
                        || !body.is_empty()
                        || reply.handles.remaining() != 0
                    {
                        fatal(header.error.unwrap_or(Error::Invalid));
                    }
                    self.withdrawn = true;
                }
                Call::Query => {
                    let (header, body) =
                        device::Header::decode(&reply.payload).unwrap_or_else(|e| fatal(e));
                    if header.op != 8 || header.error.is_some() || reply.handles.remaining() != 0 {
                        fatal(header.error.unwrap_or(Error::Invalid));
                    }
                    let state = device::decode_snapshot(body).unwrap_or_else(|e| fatal(e));
                    let activation = world
                        .slot
                        .as_mut()
                        .and_then(|slot| slot.activation.as_mut())
                        .expect("PCI activation missing");
                    if state.reference != activation.reference() {
                        fatal(Error::StaleEpoch);
                    }
                    if state.phase == BindingPhase::Released
                        && activation.snapshot().phase == protocol::Phase::Terminal
                    {
                        world
                            .slot
                            .as_mut()
                            .expect("PCI slot missing")
                            .release(state)
                            .unwrap_or_else(|e| fatal(e));
                    } else if matches!(state.phase, BindingPhase::Stopping | BindingPhase::Released)
                    {
                        activation.stop();
                        world.changed()?;
                    }
                }
            }
        }
        if world.slot.as_ref().is_some_and(|slot| {
            slot.activation
                .as_ref()
                .is_some_and(|a| a.snapshot().phase == protocol::Phase::Terminal)
        }) {
            if !world.slot.as_ref().is_some_and(ActivationSlot::active) {
                self.next_poll = Deadline::INFINITE;
                return Ok(step(Step::Parked));
            }
            if self.next_poll == Deadline::INFINITE || libkernel::time::expired(self.next_poll)? {
                let usage = world
                    .slot
                    .as_ref()
                    .and_then(|s| s.activation.as_ref())
                    .expect("PCI activation missing")
                    .usage()
                    .unwrap_or_else(|e| fatal(e))
                    .clone();
                self.next_poll = libkernel::time::timeout_millis(1)?;
                self.begin(id, world, usage, device::Command::QueryBinding, Call::Query)
                    .unwrap_or_else(|e| fatal(e));
            }
            return Ok(step(Step::Parked));
        }
        let stopping = world
            .slot
            .as_ref()
            .and_then(|slot| slot.activation.as_ref())
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
                    .and_then(|slot| slot.activation.as_ref())
                    .and_then(|a| a.registrar())
                    .ok_or(SystemCallError::InternalError)?
                    .clone();
                self.begin(
                    id,
                    world,
                    registrar,
                    device::Command::WithdrawPublication { seq: 1 },
                    Call::Withdraw,
                )
                .unwrap_or_else(|e| fatal(e));
                return Ok(step(Step::Parked));
            }
            if world.host.advance().unwrap_or_else(|e| fatal(e)) {
                return Ok(step(Step::Runnable));
            }
            if world.host.hardware_idle() {
                world.host.unmap().unwrap_or_else(|e| fatal(e));
                let activation = world
                    .slot
                    .as_mut()
                    .and_then(|slot| slot.activation.as_mut())
                    .expect("PCI activation missing");
                // SAFETY: 所有子供应已实际 Closed（含 xHCI halt/DMA/IRQ/MMIO）；扫描与配置
                // 在此分支永久关闭，ECAM 已解除。保留入口仅拒迟到请求/交付已有软件结果。
                unsafe {
                    activation.consumer_stopped().unwrap_or_else(|e| fatal(e));
                }
                activation.terminal(None);
                world.changed()?;
                libkernel::debug!(
                    "PCI consumer stopped: ECAM unmapped; child hardware responsibility closed"
                );
                self.next_poll = Deadline::INFINITE;
                return Ok(step(Step::Runnable));
            }
            self.next_poll = libkernel::time::timeout_millis(1)?;
            return Ok(step(Step::Parked));
        }
        if self.next_resource < 4 {
            let index = self.next_resource;
            let target = world
                .slot
                .as_ref()
                .and_then(|slot| slot.activation.as_ref())
                .expect("PCI activation missing")
                .usage()
                .unwrap_or_else(|e| fatal(e))
                .clone();
            let deadline = libkernel::time::timeout_millis(5_000)?;
            let mut wire = [0; liboal::protocol::HEADER_LEN + liboal::protocol::BODY_MAX];
            let used = liboal::protocol::encode_request(
                &liboal::protocol::Request::Lookup {
                    path: HOST_RESOURCES[index],
                },
                deadline,
                &mut wire,
            )
            .unwrap_or_else(|_| fatal(Error::Invalid));
            let request = librpc::Request::new(liboal::protocol::ID, &wire[..used])?;
            self.send(
                id,
                world,
                target,
                deadline,
                request,
                Call::Describe(index, deadline),
            )
            .unwrap_or_else(|e| fatal(e));
            return Ok(step(Step::Parked));
        }
        if world.host.ecam.is_none() {
            world.host.map().unwrap_or_else(|e| fatal(e));
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
            }
            .unwrap_or_else(|e| fatal(e.into()))
            {
                libkernel::debug!(
                    "PCI function: bus=0 slot={} fn={} vendor={:04x} device={:04x} class={:06x}",
                    address.slot,
                    address.function,
                    function.vendor,
                    function.device,
                    function.class
                );
                if function.header & 0x7f == 1 {
                    fatal(Error::Unsupported);
                }
                if function.identity() == XHCI_IDENTITY {
                    world
                        .host
                        .discovered(function, &world.control.mailbox)
                        .unwrap_or_else(|e| fatal(e));
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
                    .and_then(|slot| slot.activation.as_ref())
                    .and_then(|a| a.registrar())
                    .ok_or(SystemCallError::InternalError)?
                    .clone();
                let parent = world
                    .slot
                    .as_ref()
                    .and_then(|slot| slot.activation.as_ref())
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
                    },
                    Call::Register,
                )
                .unwrap_or_else(|e| fatal(e));
                return Ok(step(Step::Parked));
            }
            self.enumerated = true;
            world
                .slot
                .as_mut()
                .and_then(|slot| slot.activation.as_mut())
                .expect("PCI activation missing")
                .ready()
                .unwrap_or_else(|e| fatal(e));
            world.changed()?;
            libkernel::debug!("PCI activation ready; bus 0 enumeration complete");
        }
        if world.host.advance().unwrap_or_else(|e| fatal(e)) {
            return Ok(step(Step::Runnable));
        }
        if self.next_poll == Deadline::INFINITE || libkernel::time::expired(self.next_poll)? {
            let usage = world
                .slot
                .as_ref()
                .and_then(|slot| slot.activation.as_ref())
                .expect("PCI activation missing")
                .usage()
                .unwrap_or_else(|e| fatal(e))
                .clone();
            self.next_poll = libkernel::time::timeout_millis(1)?;
            self.begin(id, world, usage, device::Command::QueryBinding, Call::Query)
                .unwrap_or_else(|e| fatal(e));
        }
        let _ = requests;
        Ok(step(Step::Parked))
    }
}
