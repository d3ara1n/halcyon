//! xHCI 1.2b §4/§5/§6 的首批 coherent 控制器；运行调度与授权由实际 worker 持有。
use alloc::{sync::Arc, vec::Vec};
use erhino_shared::{call::SystemCallError, object::ObjectKind};
use libdevice::{Error, protocol as registry, resource::Entry};
use libkernel::{
    dma::DmaUse,
    ipc::message::MailboxSender,
    irq::IrqUse,
    mm::Placement,
    mmio::{MmioRegion, MmioUse},
};
use libpci::function::{FunctionInfo, RESOURCES};
mod device;
mod memory;
pub use device::Interface;
use memory::{PAGE, Page, Ring, Trb, fence};
pub struct Inputs {
    info: Option<FunctionInfo>,
    registers: Option<MmioUse>,
    interrupt: Option<IrqUse>,
    dma: Option<DmaUse>,
    function: Option<Arc<MailboxSender>>,
}
impl Default for Inputs {
    fn default() -> Self {
        Self::new()
    }
}
impl Inputs {
    pub const fn new() -> Self {
        Self {
            info: None,
            registers: None,
            interrupt: None,
            dma: None,
            function: None,
        }
    }
    pub fn adopt(&mut self, index: usize, mut reply: librpc::Reply) -> Result<(), Error> {
        if index >= RESOURCES.len() {
            return Err(Error::Invalid);
        }
        let (header, body) = registry::Header::decode(&reply.payload)?;
        if header.op != 10 {
            return Err(Error::Invalid);
        }
        if let Some(error) = header.error {
            if !body.is_empty() || reply.handles.remaining() != 0 {
                return Err(Error::Invalid);
            }
            return Err(error);
        }
        let entry = Entry::decode(body, registry::HEADER_LEN, reply.handles.remaining())?;
        if entry.name != RESOURCES[index] {
            return Err(Error::Conflict);
        }
        let info = FunctionInfo::decode(entry.metadata)?;
        if self.info.is_some_and(|old| old != info) {
            return Err(Error::Conflict);
        }
        let cap = reply.handles.take(0)?;
        entry.descriptor.check_handle(&cap.description()?)?;
        let expected = [
            ObjectKind::MmioUse,
            ObjectKind::IrqUse,
            ObjectKind::DmaUse,
            ObjectKind::MailboxSender,
        ][index];
        if entry.descriptor.kind != expected {
            return Err(Error::Permission);
        }
        // SAFETY: 上面以真实 HandleDescribe 核实际角色，唯一entry关闭权移入typed owner。
        unsafe {
            match index {
                0 => {
                    let usage = MmioUse::from_handle(cap.into_raw());
                    let shape = usage.query()?;
                    if shape.physical != info.registers_cpu
                        || shape.bytes != info.registers_bytes
                        || shape.flags != 0
                    {
                        return Err(Error::Permission);
                    }
                    self.registers = Some(usage);
                }
                1 => {
                    let usage = IrqUse::from_handle(cap.into_raw());
                    let shape = usage.query()?;
                    if shape.scope.first != info.interrupt
                        || shape.scope.count != 1
                        || shape.scope.targets != 1u64 << info.target
                    {
                        return Err(Error::Permission);
                    }
                    self.interrupt = Some(usage);
                }
                2 => {
                    let usage = DmaUse::from_handle(cap.into_raw());
                    let shape = usage.query()?;
                    if shape.mode != erhino_shared::dma::TRUSTED_PHYSICAL
                        || shape.coherency != erhino_shared::dma::COHERENT
                        || shape.flags != 0
                        || shape.members != 0
                    {
                        return Err(Error::Permission);
                    }
                    self.dma = Some(usage);
                }
                3 => {
                    if entry.descriptor.protocol != libpci::function::ID {
                        return Err(Error::Permission);
                    }
                    let (sender, _) = MailboxSender::from_capability(cap).map_err(|f| f.error)?;
                    self.function = Some(Arc::try_new(sender).map_err(|_| Error::Resource)?);
                }
                _ => return Err(Error::Invalid),
            }
        }
        self.info = Some(info);
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    Starting,
    EnableMaster,
    Ready,
    Stopping,
    Closed,
}
#[derive(Clone, Copy, Debug)]
pub enum Event {
    Port(u8),
    Command {
        pointer: u64,
        completion: u8,
        slot: u8,
    },
    Transfer {
        pointer: u64,
        completion: u8,
        slot: u8,
        dci: u8,
        residue: u32,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Cnr,
    Halt,
    WaitHalt,
    Reset,
    Allocate(usize),
    Configure,
    Enable,
    Run,
    WaitRun,
    Ready,
    Stop,
    WaitStop,
    Retire(usize),
    Closed,
}
pub struct Controller {
    registers: MmioRegion<MmioUse>,
    pub interrupt: IrqUse,
    dma: DmaUse,
    pub function: Arc<MailboxSender>,
    op: usize,
    runtime: usize,
    doorbells: usize,
    pub ports: u8,
    pub context_bytes: usize,
    phase: Phase,
    end: u64,
    master: bool,
    dcbaa: Option<Page>,
    command: Option<Ring>,
    events: Option<Page>,
    erst: Option<Page>,
    event_index: usize,
    event_cycle: bool,
    pending_command: Option<u64>,
    usb: Vec<Option<device::Device>>,
    slots: u8,
    usb_cursor: usize,
    command_cursor: usize,
    scan_port: u8,
    scan_remaining: u8,
    next_connection: u64,
}
impl Controller {
    pub fn running(&self) -> bool {
        self.phase == Phase::Ready
    }
    pub fn adopt(mut inputs: Inputs) -> Result<Self, Error> {
        let info = inputs.info.ok_or(Error::NotReady)?;
        // 当前只承诺固定QEMU qemu-xhci；其detach丢弃NAK TD的事实不能外推真实硬件。
        if info.vendor != 0x1b36 || info.device != 0x000d {
            return Err(Error::Unsupported);
        }
        let usage = inputs.registers.take().ok_or(Error::NotReady)?;
        let registers = usage
            .map_owned(0, info.registers_bytes as usize, true, Placement::Anywhere)
            .map_err(|(_, e)| e)?;
        let interrupt = inputs.interrupt.take().ok_or(Error::NotReady)?;
        libkernel::debug!(
            "xHCI IRQ routing adopted: source={}, target_slot={}",
            info.interrupt,
            info.target
        );
        let dma = inputs.dma.take().ok_or(Error::NotReady)?;
        let function = inputs.function.take().ok_or(Error::NotReady)?;
        // SAFETY: 原绑定已ResourcesReady，上级PCI在真实原轮静止报告前不撤此MMIO；单actor读cap。
        let (op, runtime, doorbells, hcs, hcs2, hcc) = unsafe {
            (
                registers.read_u8(0)? as usize,
                registers.read_u32(0x18)? as usize & !31,
                registers.read_u32(0x14)? as usize & !3,
                registers.read_u32(4)?,
                registers.read_u32(8)?,
                registers.read_u32(0x10)?,
            )
        };
        let ports = (hcs >> 24) as u8;
        let scratch = ((hcs2 >> 27) & 31) | ((hcs2 >> 16) & 0x3e0);
        if op < 0x20
            || ports == 0
            || hcs & 0xff == 0
            || (hcs >> 8) & 0x7ff == 0
            || hcc & 1 == 0
            || scratch != 0
            || op
                .checked_add(0x400 + usize::from(ports) * 16)
                .is_none_or(|end| end > registers.bytes())
            || runtime
                .checked_add(0x40)
                .is_none_or(|end| end > registers.bytes())
            || doorbells
                .checked_add(8)
                .is_none_or(|end| end > registers.bytes())
        {
            return Err(Error::Unsupported);
        }
        let context_bytes = if hcc & 4 != 0 { 64 } else { 32 };
        // 首批为两台直连键盘预付每台五页；DCBAA页与真实MaxSlots/端口共同约束硬件配置。
        let slots = usize::from(hcs as u8)
            .min(2)
            .min(usize::from(ports))
            .min(PAGE / 8 - 1);
        if doorbells
            .checked_add((slots + 1) * 4)
            .is_none_or(|end| end > registers.bytes())
        {
            return Err(Error::Unsupported);
        }
        let mut usb = Vec::new();
        usb.try_reserve_exact(slots).map_err(|_| Error::Resource)?;
        usb.resize_with(slots, || None);
        Ok(Self {
            registers,
            interrupt,
            dma,
            function,
            op,
            runtime,
            doorbells,
            ports,
            context_bytes,
            phase: Phase::Cnr,
            end: 0,
            master: false,
            dcbaa: None,
            command: None,
            events: None,
            erst: None,
            event_index: 0,
            event_cycle: true,
            pending_command: None,
            usb,
            slots: slots as u8,
            usb_cursor: 0,
            command_cursor: 0,
            scan_port: 0,
            scan_remaining: ports,
            next_connection: 0,
        })
    }
    fn read(&self, at: usize) -> Result<u32, SystemCallError> {
        // SAFETY: parent等待本原轮消费者静止；Controller存活且尚未返回Closed时映射不被撤。
        unsafe { self.registers.read_u32(at) }
    }
    fn write(&self, at: usize, value: u32) -> Result<(), SystemCallError> {
        fence();
        // SAFETY: 与read同一原轮寿命；寄存器字段/副作用由对应调用点按规范构造。
        unsafe {
            self.registers.write_u32(at, value)?;
        }
        fence();
        Ok(())
    }
    fn write64(&self, at: usize, value: u64) -> Result<(), SystemCallError> {
        self.write(at, value as u32)?;
        self.write(at + 4, (value >> 32) as u32)
    }
    fn limit(&mut self, now: u64, nanos: u64) -> Result<(), Error> {
        self.end = now.checked_add(nanos).ok_or(Error::Exhausted)?;
        Ok(())
    }
    pub fn master_enabled(&mut self) -> Result<(), Error> {
        if self.phase != Phase::Enable {
            return Err(Error::Closed);
        }
        self.master = true;
        Ok(())
    }
    pub fn step(&mut self, now: u64) -> Result<Progress, Error> {
        match self.phase {
            Phase::Cnr => {
                if self.read(self.op + 4)? & (1 << 11) == 0 {
                    self.phase = Phase::Halt;
                } else if self.end == 0 {
                    self.limit(now, 1_000_000_000)?;
                } else if now >= self.end {
                    return Err(Error::Unavailable);
                }
            }
            Phase::Halt => {
                self.write(self.op, self.read(self.op)? & !5)?;
                self.limit(now, 16_000_000)?;
                self.phase = Phase::WaitHalt;
            }
            Phase::WaitHalt => {
                if self.read(self.op + 4)? & 1 != 0 {
                    self.write(self.op, 2)?;
                    self.limit(now, 1_000_000_000)?;
                    self.phase = Phase::Reset;
                } else if now >= self.end {
                    return Err(Error::Unavailable);
                }
            }
            Phase::Reset => {
                if self.read(self.op)? & 2 == 0 && self.read(self.op + 4)? & (1 << 11) == 0 {
                    if self.read(self.op + 8)? & 1 == 0 {
                        return Err(Error::Unsupported);
                    }
                    self.phase = Phase::Allocate(0);
                } else if now >= self.end {
                    return Err(Error::Unavailable);
                }
            }
            Phase::Allocate(index) => {
                let ready = match index {
                    0 => {
                        if self.dcbaa.is_none() {
                            self.dcbaa = Some(Page::allocate()?);
                        }
                        self.dcbaa
                            .as_mut()
                            .ok_or(Error::NotReady)?
                            .prepare(&self.dma)?
                    }
                    1 => {
                        if self.command.is_none() {
                            self.command = Some(Ring::allocate()?);
                        }
                        self.command
                            .as_mut()
                            .ok_or(Error::NotReady)?
                            .prepare(&self.dma)?
                    }
                    2 => {
                        if self.events.is_none() {
                            self.events = Some(Page::allocate()?);
                        }
                        self.events
                            .as_mut()
                            .ok_or(Error::NotReady)?
                            .prepare(&self.dma)?
                    }
                    3 => {
                        if self.erst.is_none() {
                            self.erst = Some(Page::allocate()?);
                        }
                        self.erst
                            .as_mut()
                            .ok_or(Error::NotReady)?
                            .prepare(&self.dma)?
                    }
                    _ => {
                        self.command.as_mut().ok_or(Error::NotReady)?.reset()?;
                        let erst = self.erst.as_ref().ok_or(Error::NotReady)?;
                        erst.write64(0, self.events.as_ref().ok_or(Error::NotReady)?.address()?)?;
                        erst.write(8, (PAGE / 16) as u32)?;
                        fence();
                        self.phase = Phase::Enable;
                        return Ok(Progress::Starting);
                    }
                };
                if ready {
                    self.phase = Phase::Allocate(index + 1);
                }
            }
            Phase::Configure => {
                // ERSTBA发布会立即读DMA（QEMU在高DWORD写入时缓存ERST）；
                // 缓冲先完整pin/初始化，再启BME，最后发布地址，不能在BME关闭时配置ERSTBA。
                let dcbaa = self.dcbaa.as_ref().ok_or(Error::NotReady)?;
                let command = self.command.as_ref().ok_or(Error::NotReady)?;
                let events = self.events.as_ref().ok_or(Error::NotReady)?;
                let erst = self.erst.as_ref().ok_or(Error::NotReady)?;
                self.write(self.op + 0x38, u32::from(self.slots))?;
                self.write64(self.op + 0x30, dcbaa.address()?)?;
                self.write64(self.op + 0x18, command.dequeue()?)?;
                self.write(self.runtime + 0x28, 1)?;
                self.write64(self.runtime + 0x38, events.address()? | 8)?;
                self.write64(self.runtime + 0x30, erst.address()?)?;
                self.write(self.runtime + 0x24, 0)?;
                self.write(self.runtime + 0x20, 3)?;
                libkernel::debug!(
                    "xHCI rings configured: DCBAA={:#x}, CRCR={:#x}, ERST={:#x}, events={:#x}, USBSTS={:#x}",
                    dcbaa.address()?,
                    command.dequeue()?,
                    erst.address()?,
                    events.address()?,
                    self.read(self.op + 4)?
                );
                self.phase = Phase::Run;
            }
            Phase::Enable => {
                if !self.master {
                    return Ok(Progress::EnableMaster);
                }
                self.phase = Phase::Configure;
            }
            Phase::Run => {
                self.write(self.op, 5)?;
                self.limit(now, 16_000_000)?;
                self.phase = Phase::WaitRun;
            }
            Phase::WaitRun => {
                let status = self.read(self.op + 4)?;
                if status & ((1 << 12) | (1 << 2)) != 0 {
                    return Err(Error::Unavailable);
                }
                if status & 1 == 0 {
                    self.phase = Phase::Ready;
                    return Ok(Progress::Ready);
                }
                if now >= self.end {
                    return Err(Error::Unavailable);
                }
            }
            Phase::Ready => {
                if self.read(self.op + 4)? & ((1 << 12) | (1 << 2)) != 0 {
                    return Err(Error::Unavailable);
                }
                return Ok(Progress::Ready);
            }
            Phase::Stop => {
                if self.pending_command.is_some() {
                    return Err(Error::Busy);
                }
                self.write(self.op, self.read(self.op)? & !1)?;
                self.limit(now, 16_000_000)?;
                self.phase = Phase::WaitStop;
            }
            Phase::WaitStop => {
                if self.read(self.op + 4)? & 1 != 0 {
                    // 调用者已收所有定向命令/TD；halt后仍消费残事件，不能将HCH当作event ring空。
                    if self.events.is_none() || self.poll_event()?.is_none() {
                        self.write(self.runtime + 0x20, 1)?;
                        self.write(self.op + 4, 8)?;
                        if let Some(token) = self.interrupt.take()? {
                            self.interrupt.ack(token)?;
                        }
                        self.phase = Phase::Retire(0);
                    }
                } else if now >= self.end {
                    return Err(Error::Unavailable);
                }
            }
            Phase::Retire(index) => {
                self.dma.seal()?;
                let page = match index {
                    0 => self.dcbaa.as_mut(),
                    1 => self.command.as_mut().map(|ring| &mut ring.page),
                    2 => self.events.as_mut(),
                    3 => self.erst.as_mut(),
                    _ => {
                        self.phase = Phase::Closed;
                        return Ok(Progress::Closed);
                    }
                };
                // SAFETY: caller在begin_stop前完成endpoint/slot/command责任，已观察HCH并收清残事件；
                // 本phase永久禁止doorbell/提交，地址只留观察/退款，不再交硬件。
                if page.is_none() || unsafe { page.ok_or(Error::NotReady)?.retire(&self.dma)? } {
                    self.phase = Phase::Retire(index + 1);
                }
            }
            Phase::Closed => return Ok(Progress::Closed),
        }
        Ok(
            if matches!(self.phase, Phase::Stop | Phase::WaitStop | Phase::Retire(_)) {
                Progress::Stopping
            } else {
                Progress::Starting
            },
        )
    }
    pub fn poll_event(&mut self) -> Result<Option<Event>, Error> {
        let page = self.events.as_ref().ok_or(Error::NotReady)?;
        let at = self.event_index * 16;
        let control = page.read(at + 12)?;
        if control & 1 != u32::from(self.event_cycle) {
            return Ok(None);
        }
        fence();
        let event = Trb {
            parameter: u64::from(page.read(at)?) | (u64::from(page.read(at + 4)?) << 32),
            status: page.read(at + 8)?,
            control,
        };
        self.event_index += 1;
        if self.event_index == PAGE / 16 {
            self.event_index = 0;
            self.event_cycle = !self.event_cycle;
        }
        self.write64(
            self.runtime + 0x38,
            (page.address()? + (self.event_index * 16) as u64) | 8,
        )?;
        let result = match event.kind() {
            32 => Event::Transfer {
                pointer: event.parameter,
                completion: event.completion(),
                slot: event.slot(),
                dci: ((control >> 16) & 31) as u8,
                residue: event.status & 0xff_ffff,
            },
            33 => {
                if self.pending_command != Some(event.parameter) {
                    return Err(Error::Conflict);
                }
                self.pending_command = None;
                Event::Command {
                    pointer: event.parameter,
                    completion: event.completion(),
                    slot: event.slot(),
                }
            }
            34 => Event::Port((event.parameter >> 24) as u8),
            _ => return Err(Error::Unavailable),
        };
        Ok(Some(result))
    }
    /// 事件已按预算消费；仍有事件则保持电平，下轮继续，不凭IRQ token证明TD完成。
    pub fn acknowledge_interrupt(&self) -> Result<(), Error> {
        if let Some(token) = self.interrupt.take()? {
            self.write(self.op + 4, 8)?;
            self.write(self.runtime + 0x20, 3)?;
            self.interrupt.ack(token)?;
            libkernel::debug!(
                "xHCI INTx acknowledged: generation={}, sequence={}",
                token.generation,
                token.sequence
            );
        }
        Ok(())
    }
    pub fn port(&self, port: u8) -> Result<u32, Error> {
        if port == 0 || port > self.ports {
            return Err(Error::Invalid);
        }
        Ok(self.read(self.op + 0x400 + usize::from(port - 1) * 16)?)
    }
    pub fn reset_port(&self, port: u8) -> Result<(), Error> {
        let value = self.port(port)?;
        let writable = value & ((1 << 9) | (3 << 14) | (7 << 25));
        self.write(
            self.op + 0x400 + usize::from(port - 1) * 16,
            writable | (1 << 4),
        )?;
        Ok(())
    }
    pub fn acknowledge_port(&self, port: u8) -> Result<(), Error> {
        let value = self.port(port)?;
        self.write(
            self.op + 0x400 + usize::from(port - 1) * 16,
            (value & ((1 << 9) | (3 << 14) | (7 << 25))) | (value & 0x00fe_0000),
        )?;
        Ok(())
    }
    pub fn command(
        &mut self,
        kind: u32,
        slot: u8,
        parameter: u64,
        flags: u32,
    ) -> Result<u64, Error> {
        if self.phase != Phase::Ready || self.pending_command.is_some() {
            return Err(Error::Busy);
        }
        let pointer = self.command.as_mut().ok_or(Error::NotReady)?.push(Trb {
            parameter,
            status: 0,
            control: (kind << 10) | (u32::from(slot) << 24) | flags,
        })?;
        self.pending_command = Some(pointer);
        self.write(self.doorbells, 0)?;
        libkernel::debug!(
            "xHCI command submitted: kind={}, slot={}, pointer={:#x}, USBSTS={:#x}, CRCR={:#x}, IMAN={:#x}",
            kind,
            slot,
            pointer,
            self.read(self.op + 4)?,
            self.read(self.op + 0x18)?,
            self.read(self.runtime + 0x20)?
        );
        Ok(pointer)
    }
    /// # Safety
    /// 全部已准入子session已封口；endpoint/slot/TD/command责任均已结束，不再需要寄存器清理。
    pub unsafe fn begin_stop(&mut self) -> Result<(), Error> {
        if !self.usb_idle() {
            return Err(Error::Busy);
        }
        if matches!(
            self.phase,
            Phase::Stop | Phase::WaitStop | Phase::Retire(_) | Phase::Closed
        ) {
            return Ok(());
        }
        if self.pending_command.is_some() {
            return Err(Error::Busy);
        }
        if self.phase != Phase::Closed {
            self.phase = Phase::Stop;
        }
        Ok(())
    }
    /// # Safety
    /// 已返回Closed且上级仍等原轮停止报告，MMIO尚未撤销或复用；解除本映射后永久不用寄存器。
    #[expect(
        clippy::result_large_err,
        reason = "解除失败返还实际Controller全部资源与原映射，不为错误路径分配"
    )]
    pub unsafe fn unmap(self) -> Result<(), (Self, SystemCallError)> {
        // SAFETY: 由实际原轮owner证明Closed与上级撤销的因果顺序。
        unsafe { self.registers.unmap() }
            .map_err(|(registers, error)| (Self { registers, ..self }, error))
    }
}
