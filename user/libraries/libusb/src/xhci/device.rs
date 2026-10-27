//! 一个根端口直连 boot keyboard 的真实 slot/context/TD owner；不拥有设备登记或能力。
use super::*;
use crate::descriptor::{self, Keyboard, Setup};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AfterStop {
    Transfer,
    Interface,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Reset,
    WaitReset,
    Allocate(usize),
    Enable,
    WaitEnable,
    Address,
    WaitAddress,
    Descriptor8,
    WaitDescriptor8,
    Evaluate,
    WaitEvaluate,
    DeviceDescriptor,
    WaitDeviceDescriptor,
    ConfigHeader,
    WaitConfigHeader,
    Config,
    WaitConfig,
    SetConfig,
    WaitSetConfig,
    Configure,
    WaitConfigure,
    Ready,
    StopEndpoint(AfterStop),
    WaitStopEndpoint(AfterStop),
    WaitStoppedTransfer(AfterStop),
    Rewind(AfterStop),
    WaitRewind(AfterStop),
    Disable,
    WaitDisable,
    Retire(usize),
    Closed,
}
#[derive(Clone, Copy, Debug)]
pub struct Interface {
    pub connection: u64,
    pub port: u8,
    pub keyboard: Keyboard,
    pub ready: bool,
    pub present: bool,
}
pub type TransferResult = Result<([u8; 8], u8), Error>;
struct Transfer {
    ending: u64,
    data: u64,
    dci: u8,
    requested: usize,
    short: Option<usize>,
    result: Option<Result<usize, Error>>,
}
pub(super) struct Device {
    end: u64,
    stop_end: Option<u64>,
    connection: u64,
    port: u8,
    slot: u8,
    speed: u8,
    mps: u16,
    stage: Stage,
    command: Option<u64>,
    input: Option<Page>,
    output: Option<Page>,
    ep0: Option<Ring>,
    interrupt: Option<Ring>,
    data: Option<Page>,
    transfer: Option<Transfer>,
    keyboard: Option<Keyboard>,
    configuration_bytes: usize,
    disconnected: bool,
}
impl Device {
    fn new(connection: u64, port: u8, end: u64) -> Self {
        Self {
            end,
            stop_end: None,
            connection,
            port,
            slot: 0,
            speed: 0,
            mps: 8,
            stage: Stage::Reset,
            command: None,
            input: None,
            output: None,
            ep0: None,
            interrupt: None,
            data: None,
            transfer: None,
            keyboard: None,
            configuration_bytes: 0,
            disconnected: false,
        }
    }
    fn closing(&self) -> bool {
        matches!(
            self.stage,
            Stage::StopEndpoint(_)
                | Stage::WaitStopEndpoint(_)
                | Stage::WaitStoppedTransfer(_)
                | Stage::Rewind(_)
                | Stage::WaitRewind(_)
                | Stage::Disable
                | Stage::WaitDisable
                | Stage::Retire(_)
                | Stage::Closed
        )
    }
    fn stop(&mut self) {
        // 父停止升级原端点取消的后续动作，已经提交的同一条清理命令不重发。
        self.stage = match self.stage {
            Stage::StopEndpoint(_) => Stage::StopEndpoint(AfterStop::Interface),
            Stage::WaitStopEndpoint(_) => Stage::WaitStopEndpoint(AfterStop::Interface),
            Stage::WaitStoppedTransfer(_) => Stage::WaitStoppedTransfer(AfterStop::Interface),
            Stage::Rewind(_) => Stage::Rewind(AfterStop::Interface),
            Stage::WaitRewind(_) => Stage::WaitRewind(AfterStop::Interface),
            stage => stage,
        };
        if self.closing() {
            return;
        }
        // 在途定向命令必须先取完成；事件处理后step会根据停止意图转收束，不能取消命令地址。
        if self.command.is_none() {
            self.stage = if self.slot == 0 {
                Stage::Retire(0)
            } else if self.stage == Stage::Address {
                Stage::Disable
            } else {
                Stage::StopEndpoint(AfterStop::Interface)
            };
        }
    }
    fn page(&self) -> Result<&Page, Error> {
        self.data.as_ref().ok_or(Error::NotReady)
    }
    fn copy(&self, out: &mut [u8]) -> Result<(), Error> {
        for (index, byte) in out.iter_mut().enumerate() {
            let word = self.page()?.read(index & !3)?;
            *byte = (word >> ((index & 3) * 8)) as u8;
        }
        Ok(())
    }
    fn command(
        &mut self,
        controller: &mut Controller,
        kind: u32,
        parameter: u64,
        flags: u32,
        wait: Stage,
    ) -> Result<(), Error> {
        if controller.pending_command.is_some() {
            return Ok(());
        }
        self.command = Some(controller.command(kind, self.slot, parameter, flags)?);
        self.stage = wait;
        Ok(())
    }
    fn control(&mut self, controller: &Controller, setup: Setup) -> Result<(), Error> {
        if self.transfer.is_some() {
            return Err(Error::Busy);
        }
        let bytes = usize::from(setup.length);
        if bytes > PAGE {
            return Err(Error::Unsupported);
        }
        let address = self.page()?.address()?;
        let ring = self.ep0.as_mut().ok_or(Error::NotReady)?;
        let direction = setup.request_type & 0x80 != 0;
        let trt = if bytes == 0 {
            0
        } else if direction {
            3
        } else {
            2
        };
        ring.push(Trb {
            parameter: u64::from_le_bytes(setup.encode()),
            status: 8,
            control: (2 << 10) | (1 << 6) | (trt << 16),
        })?;
        let data = if bytes != 0 {
            ring.push(Trb {
                parameter: address,
                status: bytes as u32,
                control: (3 << 10) | (u32::from(direction) << 16) | (1 << 2),
            })?
        } else {
            0
        };
        let ending = ring.push(Trb {
            parameter: 0,
            status: 0,
            control: (4 << 10) | (1 << 5) | (u32::from(bytes == 0 || !direction) << 16),
        })?;
        self.transfer = Some(Transfer {
            ending,
            data,
            dci: 1,
            requested: bytes,
            short: None,
            result: None,
        });
        controller.write(controller.doorbells + usize::from(self.slot) * 4, 1)?;
        Ok(())
    }
    fn result(&mut self) -> Option<Result<usize, Error>> {
        let result = self.transfer.as_ref().and_then(|td| td.result);
        if result.is_some() {
            self.transfer = None;
        }
        result
    }
    fn input_address(&self) -> Result<u64, Error> {
        Ok(self.input.as_ref().ok_or(Error::NotReady)?.address()?)
    }
    fn slot_context(&self, controller: &Controller, entries: u8) -> Result<(), Error> {
        let input = self.input.as_ref().ok_or(Error::NotReady)?;
        let at = controller.context_bytes;
        input.write(
            at,
            (u32::from(self.speed) << 20) | (u32::from(entries) << 27),
        )?;
        input.write(at + 4, u32::from(self.port) << 16)?;
        Ok(())
    }
    fn update_intent(
        &mut self,
        controller: &mut Controller,
        now: u64,
        allow_new: bool,
        operation_end: u64,
    ) -> Result<(), Error> {
        let port = controller.port(self.port)?;
        if port & 1 == 0 {
            self.disconnected = true;
        }
        if !allow_new
            || self.disconnected
            || (self.stop_end.is_some() && !self.closing())
            || (!self.closing() && self.stage != Stage::Ready && now >= self.end)
        {
            if self.stop_end.is_none() {
                self.stop_end = Some(operation_end);
            }
            self.stop();
        }
        if self.stage != Stage::Closed && self.stop_end.is_some_and(|end| now >= end) {
            return Err(Error::Unavailable);
        }
        Ok(())
    }
    fn needs_command(&self) -> bool {
        matches!(
            self.stage,
            Stage::Enable
                | Stage::Address
                | Stage::Evaluate
                | Stage::Configure
                | Stage::StopEndpoint(_)
                | Stage::Rewind(_)
                | Stage::Disable
        )
    }
    fn step(&mut self, controller: &mut Controller, now: u64) -> Result<(), Error> {
        let port = controller.port(self.port)?;
        match self.stage {
            Stage::Reset => {
                controller.acknowledge_port(self.port)?;
                controller.reset_port(self.port)?;
                self.stage = Stage::WaitReset;
            }
            Stage::WaitReset => {
                if port & (1 << 4) == 0 && port & 2 != 0 {
                    self.speed = ((port >> 10) & 15) as u8;
                    self.mps = match self.speed {
                        1 | 2 => 8,
                        3 => 64,
                        _ => return Err(Error::Unsupported),
                    };
                    controller.acknowledge_port(self.port)?;
                    self.stage = Stage::Allocate(0);
                }
            }
            Stage::Allocate(index) => {
                let ready = match index {
                    0 => {
                        if self.input.is_none() {
                            self.input = Some(Page::allocate()?);
                        }
                        self.input
                            .as_mut()
                            .ok_or(Error::NotReady)?
                            .prepare(&controller.dma)?
                    }
                    1 => {
                        if self.output.is_none() {
                            self.output = Some(Page::allocate()?);
                        }
                        self.output
                            .as_mut()
                            .ok_or(Error::NotReady)?
                            .prepare(&controller.dma)?
                    }
                    2 => {
                        if self.ep0.is_none() {
                            self.ep0 = Some(Ring::allocate()?);
                        }
                        self.ep0
                            .as_mut()
                            .ok_or(Error::NotReady)?
                            .prepare(&controller.dma)?
                    }
                    3 => {
                        if self.interrupt.is_none() {
                            self.interrupt = Some(Ring::allocate()?);
                        }
                        self.interrupt
                            .as_mut()
                            .ok_or(Error::NotReady)?
                            .prepare(&controller.dma)?
                    }
                    4 => {
                        if self.data.is_none() {
                            self.data = Some(Page::allocate()?);
                        }
                        self.data
                            .as_mut()
                            .ok_or(Error::NotReady)?
                            .prepare(&controller.dma)?
                    }
                    _ => {
                        self.stage = Stage::Enable;
                        return Ok(());
                    }
                };
                if ready {
                    self.stage = Stage::Allocate(index + 1);
                }
            }
            Stage::Enable => self.command(controller, 9, 0, 0, Stage::WaitEnable)?,
            Stage::Address => {
                let input = self.input.as_ref().ok_or(Error::NotReady)?;
                input.zero()?;
                input.write(4, 3)?;
                self.slot_context(controller, 1)?;
                let ep = controller.context_bytes * 2;
                input.write(ep + 4, (u32::from(self.mps) << 16) | (4 << 3) | (3 << 1))?;
                input.write64(ep + 8, self.ep0.as_ref().ok_or(Error::NotReady)?.dequeue()?)?;
                input.write(ep + 16, 8)?;
                controller.dcbaa.as_ref().ok_or(Error::NotReady)?.write64(
                    usize::from(self.slot) * 8,
                    self.output.as_ref().ok_or(Error::NotReady)?.address()?,
                )?;
                fence();
                self.command(controller, 11, self.input_address()?, 0, Stage::WaitAddress)?;
            }
            Stage::Descriptor8 => {
                self.control(controller, Setup::descriptor(1, 8))?;
                self.stage = Stage::WaitDescriptor8;
            }
            Stage::WaitDescriptor8 => {
                if let Some(result) = self.result() {
                    if result? != 8 {
                        return Err(Error::Invalid);
                    }
                    let mut bytes = [0; 8];
                    self.copy(&mut bytes)?;
                    if bytes[0] != 18 || bytes[1] != 1 || !matches!(bytes[7], 8 | 16 | 32 | 64) {
                        return Err(Error::Unsupported);
                    }
                    if self.mps != u16::from(bytes[7]) {
                        self.mps = u16::from(bytes[7]);
                        self.stage = Stage::Evaluate;
                    } else {
                        self.stage = Stage::DeviceDescriptor;
                    }
                }
            }
            Stage::Evaluate => {
                let input = self.input.as_ref().ok_or(Error::NotReady)?;
                input.zero()?;
                input.write(4, 2)?;
                input.write(controller.context_bytes * 2 + 4, u32::from(self.mps) << 16)?;
                self.command(
                    controller,
                    13,
                    self.input_address()?,
                    0,
                    Stage::WaitEvaluate,
                )?;
            }
            Stage::DeviceDescriptor => {
                self.control(controller, Setup::descriptor(1, 18))?;
                self.stage = Stage::WaitDeviceDescriptor;
            }
            Stage::WaitDeviceDescriptor => {
                if let Some(result) = self.result() {
                    if result? != 18 {
                        return Err(Error::Invalid);
                    }
                    let mut bytes = [0; 18];
                    self.copy(&mut bytes)?;
                    if bytes[0] != 18 || bytes[1] != 1 || bytes[17] != 1 {
                        return Err(Error::Unsupported);
                    }
                    self.stage = Stage::ConfigHeader;
                }
            }
            Stage::ConfigHeader => {
                self.control(controller, Setup::descriptor(2, 9))?;
                self.stage = Stage::WaitConfigHeader;
            }
            Stage::WaitConfigHeader => {
                if let Some(result) = self.result() {
                    if result? != 9 {
                        return Err(Error::Invalid);
                    }
                    let mut bytes = [0; 9];
                    self.copy(&mut bytes)?;
                    let total = usize::from(u16::from_le_bytes([bytes[2], bytes[3]]));
                    if bytes[0] != 9 || bytes[1] != 2 || !(9..=PAGE).contains(&total) {
                        return Err(Error::Unsupported);
                    }
                    self.configuration_bytes = total;
                    self.stage = Stage::Config;
                }
            }
            Stage::Config => {
                self.control(
                    controller,
                    Setup::descriptor(2, self.configuration_bytes as u16),
                )?;
                self.stage = Stage::WaitConfig;
            }
            Stage::WaitConfig => {
                if let Some(result) = self.result() {
                    if result? != self.configuration_bytes {
                        return Err(Error::Invalid);
                    }
                    let mut bytes = [0; PAGE];
                    self.copy(&mut bytes[..self.configuration_bytes])?;
                    self.keyboard = Some(descriptor::keyboard_configuration(
                        &bytes[..self.configuration_bytes],
                    )?);
                    self.stage = Stage::SetConfig;
                }
            }
            Stage::SetConfig => {
                self.control(
                    controller,
                    Setup::configuration(self.keyboard.ok_or(Error::NotReady)?.configuration),
                )?;
                self.stage = Stage::WaitSetConfig;
            }
            Stage::WaitSetConfig => {
                if let Some(result) = self.result() {
                    if result? != 0 {
                        return Err(Error::Invalid);
                    }
                    self.stage = Stage::Configure;
                }
            }
            Stage::Configure => {
                let keyboard = self.keyboard.ok_or(Error::NotReady)?;
                let input = self.input.as_ref().ok_or(Error::NotReady)?;
                input.zero()?;
                input.write(4, 1 | (1u32 << keyboard.dci()))?;
                self.slot_context(controller, keyboard.dci())?;
                let at = (usize::from(keyboard.dci()) + 1) * controller.context_bytes;
                let interval = if self.speed == 3 {
                    u32::from(keyboard.interval - 1)
                } else {
                    31 - (u32::from(keyboard.interval) * 8).leading_zeros()
                };
                if interval > 15 {
                    return Err(Error::Unsupported);
                }
                input.write(at, interval << 16)?;
                input.write(
                    at + 4,
                    (u32::from(keyboard.max_packet) << 16) | (7 << 3) | (3 << 1),
                )?;
                input.write64(
                    at + 8,
                    self.interrupt.as_ref().ok_or(Error::NotReady)?.dequeue()?,
                )?;
                input.write(at + 16, 8 | (u32::from(keyboard.max_packet) << 16))?;
                fence();
                self.command(
                    controller,
                    12,
                    self.input_address()?,
                    0,
                    Stage::WaitConfigure,
                )?;
            }
            Stage::StopEndpoint(after) => {
                let dci = self
                    .transfer
                    .as_ref()
                    .map_or_else(|| self.keyboard.map_or(1, Keyboard::dci), |td| td.dci);
                self.command(
                    controller,
                    15,
                    0,
                    u32::from(dci) << 16,
                    Stage::WaitStopEndpoint(after),
                )?;
            }
            Stage::Rewind(after) => {
                let dci = self
                    .transfer
                    .as_ref()
                    .map_or_else(|| self.keyboard.map_or(1, Keyboard::dci), |td| td.dci);
                let address = if dci == 1 {
                    self.ep0.as_ref().ok_or(Error::NotReady)?.dequeue()?
                } else {
                    self.interrupt.as_ref().ok_or(Error::NotReady)?.dequeue()?
                };
                self.command(
                    controller,
                    16,
                    address,
                    u32::from(dci) << 16,
                    Stage::WaitRewind(after),
                )?;
            }
            Stage::Disable => self.command(controller, 10, 0, 0, Stage::WaitDisable)?,
            Stage::Retire(index) => {
                let page = match index {
                    0 => self.input.as_ref(),
                    1 => self.output.as_ref(),
                    2 => self.ep0.as_ref().map(|r| &r.page),
                    3 => self.interrupt.as_ref().map(|r| &r.page),
                    4 => self.data.as_ref(),
                    _ => {
                        self.stage = Stage::Closed;
                        return Ok(());
                    }
                };
                // SAFETY: slot从未启用，或Stop Endpoint/SetDequeue/DisableSlot完成、旧事件已收；
                // DCBAA已经清除，不再提交这组页的任何地址。与上级Controller ring寿命分开。
                if page.is_none()
                    || unsafe { page.ok_or(Error::NotReady)?.retire(&controller.dma)? }
                {
                    self.stage = Stage::Retire(index + 1);
                }
            }
            _ => {}
        }
        let _ = now;
        Ok(())
    }
    fn event(&mut self, controller: &Controller, event: Event) -> Result<(), Error> {
        match event {
            Event::Port(_) => {}
            Event::Command {
                pointer,
                completion,
                slot,
            } => {
                if self.command != Some(pointer) {
                    return Err(Error::Conflict);
                }
                self.command = None;
                if completion != 1 {
                    return Err(Error::Unavailable);
                }
                self.stage = match self.stage {
                    Stage::WaitEnable => {
                        if slot == 0
                            || slot > controller.slots
                            || controller
                                .usb
                                .iter()
                                .flatten()
                                .any(|device| device.slot == slot)
                        {
                            return Err(Error::Unsupported);
                        }
                        self.slot = slot;
                        Stage::Address
                    }
                    Stage::WaitAddress => Stage::Descriptor8,
                    Stage::WaitEvaluate => Stage::DeviceDescriptor,
                    Stage::WaitConfigure => {
                        libkernel::debug!(
                            "USB boot keyboard enumerated: port={}, slot={}, interface={}, endpoint={}",
                            self.port,
                            self.slot,
                            self.keyboard.ok_or(Error::NotReady)?.interface,
                            self.keyboard.ok_or(Error::NotReady)?.endpoint
                        );
                        Stage::Ready
                    }
                    Stage::WaitStopEndpoint(after) => {
                        if self.transfer.as_ref().is_some_and(|td| td.result.is_none())
                            && !self.disconnected
                        {
                            Stage::WaitStoppedTransfer(after)
                        } else {
                            Stage::Rewind(after)
                        }
                    }
                    Stage::WaitRewind(after) => {
                        if let Some(td) = self.transfer.as_mut() {
                            td.result.get_or_insert(Err(Error::Closed));
                        }
                        match after {
                            AfterStop::Interface => Stage::Disable,
                            AfterStop::Transfer => {
                                self.stop_end = None;
                                Stage::Ready
                            }
                        }
                    }
                    Stage::WaitDisable => {
                        controller
                            .dcbaa
                            .as_ref()
                            .ok_or(Error::NotReady)?
                            .write64(usize::from(self.slot) * 8, 0)?;
                        fence();
                        self.slot = 0;
                        Stage::Retire(0)
                    }
                    _ => return Err(Error::Invalid),
                };
            }
            Event::Transfer {
                pointer,
                completion,
                slot,
                dci,
                residue,
            } => {
                if slot != self.slot {
                    return Err(Error::Conflict);
                }
                let closing = self.closing();
                let Some(td) = self.transfer.as_mut() else {
                    return Err(Error::Invalid);
                };
                if dci != td.dci {
                    return Err(Error::Conflict);
                }
                if matches!(completion, 26 | 28) && closing {
                    td.result = Some(Err(Error::Closed));
                    if let Stage::WaitStoppedTransfer(after) = self.stage {
                        self.stage = Stage::Rewind(after);
                    }
                    return Ok(());
                }
                if pointer == td.data && completion == 13 {
                    td.short = Some(
                        td.requested
                            .checked_sub(residue as usize)
                            .ok_or(Error::Invalid)?,
                    );
                    return Ok(());
                }
                if pointer != td.ending {
                    return Err(Error::Conflict);
                }
                td.result = Some(if completion == 1 || completion == 13 {
                    Ok(td.short.unwrap_or(
                        td.requested
                            .checked_sub(residue as usize)
                            .ok_or(Error::Invalid)?,
                    ))
                } else {
                    Err(Error::Unavailable)
                });
            }
        }
        Ok(())
    }
}
impl Controller {
    /// 有限连接先同步停止意图；一次只推进一个物理动作，命令准入独立公平于忙等游标。
    pub fn usb_step(
        &mut self,
        now: u64,
        allow_new: bool,
        deadline: erhino_shared::time::Deadline,
    ) -> Result<(), Error> {
        let end = deadline
            .instant()
            .map_err(|_| Error::Invalid)?
            .ok_or(Error::Invalid)?;
        if self.phase != Phase::Ready {
            return Err(Error::NotReady);
        }
        if allow_new && let Some(index) = self.usb.iter().position(Option::is_none) {
            self.scan_port = self.scan_port % self.ports + 1;
            if self.port(self.scan_port)? & 1 != 0
                && !self.usb.iter().flatten().any(|d| d.port == self.scan_port)
            {
                self.next_connection = self
                    .next_connection
                    .checked_add(1)
                    .ok_or(Error::Exhausted)?;
                self.usb[index] = Some(Device::new(self.next_connection, self.scan_port, end));
                libkernel::debug!(
                    "USB port admitted: connection={}, port={}, PORTSC={:#x}",
                    self.next_connection,
                    self.scan_port,
                    self.port(self.scan_port)?
                );
            }
        }
        // 同一actor内临时取出物理owner，不跨await；错误也先恢复完整责任再交外层终局。
        let mut devices = core::mem::take(&mut self.usb);
        let result = (|| {
            for device in devices.iter_mut().flatten() {
                device.update_intent(self, now, allow_new, end)?;
            }
            let count = devices.len();
            let command = if self.pending_command.is_none() {
                (0..count)
                    .map(|offset| (self.command_cursor + offset) % count)
                    .find(|index| devices[*index].as_ref().is_some_and(Device::needs_command))
            } else {
                None
            };
            let next = command.or_else(|| {
                (0..count)
                    .map(|offset| (self.usb_cursor + offset) % count)
                    .find(|index| devices[*index].is_some())
            });
            if let Some(index) = next {
                let device = devices[index].as_mut().ok_or(Error::NotReady)?;
                let before = device.stage;
                device.step(self, now)?;
                self.usb_cursor = (index + 1) % count;
                if command.is_some() {
                    self.command_cursor = (index + 1) % count;
                }
                if before != device.stage {
                    libkernel::debug!(
                        "USB hardware transition: connection={}, port={}, {:?}->{:?}, PORTSC={:#x}",
                        device.connection,
                        device.port,
                        before,
                        device.stage,
                        self.port(device.port)?
                    );
                }
            }
            Ok(())
        })();
        self.usb = devices;
        result
    }
    pub fn usb_event(&mut self, event: Event) -> Result<(), Error> {
        if let Event::Port(port) = event {
            let value = self.port(port)?;
            // CSC可合并断开/重插；原连接已经被观察撤出后，不以当前CCS=1复活旧身份。
            if value & (1 << 17) != 0 {
                for device in self.usb.iter_mut().flatten().filter(|d| d.port == port) {
                    if device.stage != Stage::Reset {
                        device.disconnected = true;
                    }
                }
            }
            self.acknowledge_port(port)?;
            return Ok(());
        }
        let index = self
            .usb
            .iter()
            .position(|entry| {
                entry.as_ref().is_some_and(|device| match event {
                    Event::Command { pointer, .. } => device.command == Some(pointer),
                    Event::Transfer { slot, .. } => slot != 0 && device.slot == slot,
                    Event::Port(_) => false,
                })
            })
            .ok_or(Error::Conflict)?;
        let mut device = self.usb[index].take().ok_or(Error::NotReady)?;
        let result = device.event(self, event);
        self.usb[index] = Some(device);
        result
    }
    fn describe(&self, device: &Device) -> Result<Option<Interface>, Error> {
        let Some(keyboard) = device.keyboard else {
            return Ok(None);
        };
        Ok(Some(Interface {
            connection: device.connection,
            port: device.port,
            keyboard,
            ready: device.stage == Stage::Ready,
            present: self.connection_present(device.connection)?,
        }))
    }
    pub fn interfaces(&self) -> impl Iterator<Item = Result<Interface, Error>> + '_ {
        self.usb
            .iter()
            .flatten()
            .filter_map(|device| self.describe(device).transpose())
    }
    pub fn interface(&self, connection: u64) -> Result<Option<Interface>, Error> {
        self.usb
            .iter()
            .flatten()
            .find(|d| d.connection == connection)
            .map_or(Ok(None), |device| self.describe(device))
    }
    /// 连接寿命独立于本轮描述符/端点是否已经准备完成；重新Prepare不撤出登记身份。
    pub fn connection_present(&self, connection: u64) -> Result<bool, Error> {
        self.usb
            .iter()
            .flatten()
            .find(|d| d.connection == connection)
            .map_or(Ok(false), |d| {
                Ok(!d.disconnected && self.port(d.port)? & 1 != 0)
            })
    }
    pub fn usb_idle(&self) -> bool {
        self.usb.iter().flatten().all(|d| d.stage == Stage::Closed)
    }
    pub fn closed_connections(&self) -> impl Iterator<Item = u64> + '_ {
        self.usb
            .iter()
            .flatten()
            .filter(|d| d.stage == Stage::Closed && d.disconnected)
            .map(|d| d.connection)
    }
    pub fn usb_forget_disconnected(&mut self, connection: u64) -> Result<(), Error> {
        let index = self.index(connection)?;
        let device = self.usb[index].as_ref().ok_or(Error::Closed)?;
        if device.stage != Stage::Closed || !device.disconnected {
            return Err(Error::Busy);
        }
        self.usb[index] = None;
        Ok(())
    }
    fn index(&self, connection: u64) -> Result<usize, Error> {
        self.usb
            .iter()
            .position(|entry| entry.as_ref().is_some_and(|d| d.connection == connection))
            .ok_or(Error::Closed)
    }
    /// 只有原连接记录已完成端点/slot及逐页退休才返回true；缺记录不是停止证据。
    pub fn usb_stopped(&self, connection: u64) -> Result<bool, Error> {
        Ok(self.usb[self.index(connection)?]
            .as_ref()
            .ok_or(Error::Closed)?
            .stage
            == Stage::Closed)
    }
    pub fn usb_prepare(
        &mut self,
        connection: u64,
        deadline: erhino_shared::time::Deadline,
    ) -> Result<bool, Error> {
        let end = deadline
            .instant()
            .map_err(|_| Error::Invalid)?
            .ok_or(Error::Invalid)?;
        let index = self.index(connection)?;
        let device = self.usb[index].as_ref().ok_or(Error::Closed)?;
        if device.disconnected || self.port(device.port)? & 1 == 0 {
            return Err(Error::Closed);
        }
        if device.stage == Stage::Ready {
            return Ok(true);
        }
        if device.stage == Stage::Closed {
            let port = device.port;
            self.usb[index] = Some(Device::new(connection, port, end));
        }
        Ok(false)
    }
    pub fn usb_can_transfer(&self, connection: u64) -> Result<(), Error> {
        let device = self.usb[self.index(connection)?]
            .as_ref()
            .ok_or(Error::Closed)?;
        if device.disconnected || self.port(device.port)? & 1 == 0 {
            return Err(Error::Closed);
        }
        if device.stage != Stage::Ready || device.transfer.is_some() {
            return Err(Error::Busy);
        }
        Ok(())
    }
    pub fn usb_transfer(
        &mut self,
        connection: u64,
        request: crate::protocol::Transfer,
    ) -> Result<(), Error> {
        self.usb_can_transfer(connection)?;
        let index = self.index(connection)?;
        let mut device = self.usb[index].take().ok_or(Error::Closed)?;
        let result = (|| {
            let keyboard = device.keyboard.ok_or(Error::NotReady)?;
            match request {
                crate::protocol::Transfer::BootProtocol => {
                    device.control(self, Setup::boot_protocol(keyboard.interface))
                }
                crate::protocol::Transfer::InterruptIn => {
                    let data = device.page()?.address()?;
                    let ring = device.interrupt.as_mut().ok_or(Error::NotReady)?;
                    let ending = ring.push(Trb {
                        parameter: data,
                        status: 8,
                        control: (1 << 10) | (1 << 5) | (1 << 2),
                    })?;
                    device.transfer = Some(Transfer {
                        ending,
                        data: ending,
                        dci: keyboard.dci(),
                        requested: 8,
                        short: None,
                        result: None,
                    });
                    self.write(
                        self.doorbells + usize::from(device.slot) * 4,
                        u32::from(keyboard.dci()),
                    )?;
                    Ok(())
                }
            }
        })();
        self.usb[index] = Some(device);
        result
    }
    pub fn usb_result(&mut self, connection: u64) -> Result<Option<TransferResult>, Error> {
        let index = self.index(connection)?;
        let device = self.usb[index].as_ref().ok_or(Error::Closed)?;
        if !matches!(device.stage, Stage::Ready | Stage::Closed) {
            return Ok(None);
        }
        let Some(result) = device.transfer.as_ref().and_then(|td| td.result) else {
            return Ok(None);
        };
        let output = match result {
            Ok(count) => {
                if count > 8 {
                    return Err(Error::Invalid);
                }
                let mut bytes = [0; 8];
                device.copy(&mut bytes[..count])?;
                Ok((bytes, count as u8))
            }
            Err(error) => Err(error),
        };
        self.usb[index].as_mut().ok_or(Error::Closed)?.transfer = None;
        Ok(Some(output))
    }
    pub fn usb_cancel(
        &mut self,
        connection: u64,
        deadline: erhino_shared::time::Deadline,
    ) -> Result<(), Error> {
        let end = deadline
            .instant()
            .map_err(|_| Error::Invalid)?
            .ok_or(Error::Invalid)?;
        let index = self.index(connection)?;
        let device = self.usb[index].as_mut().ok_or(Error::Closed)?;
        if device.stage == Stage::Ready {
            if device
                .transfer
                .as_ref()
                .is_some_and(|td| td.result.is_none())
            {
                device.stage = Stage::StopEndpoint(AfterStop::Transfer);
                device.stop_end = Some(end);
            }
            return Ok(());
        }
        if matches!(
            device.stage,
            Stage::StopEndpoint(AfterStop::Transfer)
                | Stage::WaitStopEndpoint(AfterStop::Transfer)
                | Stage::WaitStoppedTransfer(AfterStop::Transfer)
                | Stage::Rewind(AfterStop::Transfer)
                | Stage::WaitRewind(AfterStop::Transfer)
        ) {
            return Ok(());
        }
        Err(Error::Closed)
    }
    pub fn usb_stop(
        &mut self,
        connection: u64,
        deadline: erhino_shared::time::Deadline,
    ) -> Result<(), Error> {
        let end = deadline
            .instant()
            .map_err(|_| Error::Invalid)?
            .ok_or(Error::Invalid)?;
        let index = self.index(connection)?;
        let device = self.usb[index].as_mut().ok_or(Error::Closed)?;
        device.stop_end.get_or_insert(end);
        device.stop();
        Ok(())
    }
}
