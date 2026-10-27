//! 首批 bus 0 ECAM 与 memory BAR 操作。调用者持有本总线独占的配置访问权。
use crate::host::HostInfo;
use erhino_shared::{call::SystemCallError, object::Rights};
use libkernel::{
    mm::Placement,
    mmio::{MmioRegion, MmioUse},
};

pub const COMMAND_MEMORY: u16 = 1 << 1;
pub const COMMAND_MASTER: u16 = 1 << 2;
pub const COMMAND_INTX_DISABLE: u16 = 1 << 10;
pub const XHCI_IDENTITY: &[u8] = b"pci:0c0330";
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Address {
    pub slot: u8,
    pub function: u8,
}
impl Address {
    pub fn offset(self, register: usize) -> Result<usize, SystemCallError> {
        if self.slot >= 32 || self.function >= 8 || register >= 4096 {
            return Err(SystemCallError::IllegalArgument);
        }
        Ok((usize::from(self.slot) << 15) | (usize::from(self.function) << 12) | register)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Function {
    pub address: Address,
    pub vendor: u16,
    pub device: u16,
    pub class: u32,
    pub revision: u8,
    pub header: u8,
    pub interrupt_pin: u8,
}
impl Function {
    pub fn identity(&self) -> [u8; 10] {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut identity = *b"pci:000000";
        for index in 0..6 {
            identity[4 + index] = HEX[((self.class >> ((5 - index) * 4)) & 15) as usize];
        }
        identity
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bar {
    pub register: u8,
    pub flags: u32,
    pub bytes: u64,
    pub wide: bool,
}
impl Bar {
    pub fn decode(
        register: u8,
        original: u32,
        mask_low: u32,
        mask_high: u32,
    ) -> Result<Option<Self>, SystemCallError> {
        if register >= 6 || original & 1 != 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let wide = match (original >> 1) & 3 {
            0 => false,
            2 if register < 5 => true,
            _ => return Err(SystemCallError::IllegalArgument),
        };
        let mask = if wide {
            (u64::from(mask_high) << 32) | u64::from(mask_low & !15)
        } else {
            u64::from(mask_low & !15)
        };
        if mask == 0 {
            return Ok(None);
        }
        let bytes = if wide {
            (!mask).wrapping_add(1)
        } else {
            u64::from((!(mask as u32)).wrapping_add(1))
        };
        if bytes == 0 || !bytes.is_power_of_two() {
            return Err(SystemCallError::IllegalArgument);
        }
        Ok(Some(Self {
            register,
            flags: original & 15,
            bytes,
            wide,
        }))
    }
    pub fn placement(self, info: HostInfo, after: u64) -> Result<(u64, u64), SystemCallError> {
        // 完整页是资源隔离粒度，小 BAR 也独占整页，不把同页寄存器发给不同绑定。
        let alignment = self
            .bytes
            .max(erhino_shared::proc::PROCESS_PAGE_SIZE as u64);
        let bus = after
            .max(info.window_bus)
            .checked_next_multiple_of(alignment)
            .ok_or(SystemCallError::IllegalArgument)?;
        let end = bus
            .checked_add(alignment)
            .ok_or(SystemCallError::IllegalArgument)?;
        if end
            > info
                .window_bus
                .checked_add(info.window_bytes)
                .ok_or(SystemCallError::IllegalArgument)?
            || (!self.wide && end > 1u64 << 32)
        {
            return Err(SystemCallError::ReachLimit);
        }
        let cpu = info
            .window_cpu
            .checked_add(bus - info.window_bus)
            .ok_or(SystemCallError::IllegalArgument)?;
        if !cpu.is_multiple_of(erhino_shared::proc::PROCESS_PAGE_SIZE as u64) {
            return Err(SystemCallError::IllegalArgument);
        }
        Ok((bus, cpu))
    }
}

pub struct Ecam {
    region: MmioRegion<MmioUse>,
}
impl Ecam {
    pub fn map(usage: MmioUse, info: HostInfo) -> Result<Self, (MmioUse, SystemCallError)> {
        if info.first_bus != 0 {
            return Err((usage, SystemCallError::IllegalArgument));
        }
        usage
            .map_owned(0, 1 << 20, true, Placement::Anywhere)
            .map(|region| Self { region })
    }
    /// # Safety
    /// 本轮尚未报告消费者停止，上级排除撤销；不存在并行配置访问，寄存器读取语义成立。
    pub unsafe fn function(&self, address: Address) -> Result<Option<Function>, SystemCallError> {
        // SAFETY: 调用者排除本轮撤销/配置竞争，以下仅访问标准配置头。
        unsafe {
            let identity = self.region.read_u32(address.offset(0)?)?;
            if identity & 0xffff == 0xffff {
                return Ok(None);
            }
            let class_revision = self.region.read_u32(address.offset(8)?)?;
            Ok(Some(Function {
                address,
                vendor: identity as u16,
                device: (identity >> 16) as u16,
                class: class_revision >> 8,
                revision: class_revision as u8,
                header: self.region.read_u8(address.offset(14)?)?,
                interrupt_pin: self.region.read_u8(address.offset(0x3d)?)?,
            }))
        }
    }
    /// # Safety
    /// 同 function；调用者拥有实际配置写 authority，并保持相应供应/硬件停止责任。
    pub unsafe fn command(
        &self,
        address: Address,
        set: u16,
        clear: u16,
    ) -> Result<(), SystemCallError> {
        io_fence();
        // SAFETY: 标准 COMMAND 为 16 位；不以 32 位写入影响相邻 W1C status。
        unsafe {
            let command = self.region.read_u16(address.offset(4)?)?;
            self.region
                .write_u16(address.offset(4)?, (command | set) & !clear)?;
        }
        io_fence();
        Ok(())
    }
    /// # Safety
    /// 本 function 尚无激活使用者；准备 owner 保管探测后的地址/解码效果，失败仍负责清理。
    pub unsafe fn probe_bar(
        &self,
        address: Address,
        register: u8,
    ) -> Result<Option<Bar>, SystemCallError> {
        if register >= 6 {
            return Err(SystemCallError::IllegalArgument);
        }
        // SAFETY: 调用方独占配置及本轮寿命；先关闭 MEMORY/IO/BME，探测不启动 DMA。
        unsafe {
            self.command(
                address,
                COMMAND_INTX_DISABLE,
                COMMAND_MEMORY | COMMAND_MASTER | 1,
            )?;
            let offset = address.offset(0x10 + usize::from(register) * 4)?;
            let original = self.region.read_u32(offset)?;
            if original & 1 != 0 {
                return Err(SystemCallError::IllegalArgument);
            }
            let wide = (original >> 1) & 3 == 2;
            if wide && register == 5 {
                return Err(SystemCallError::IllegalArgument);
            }
            let high = if wide {
                self.region.read_u32(offset + 4)?
            } else {
                0
            };
            self.region.write_u32(offset, u32::MAX)?;
            if wide {
                self.region.write_u32(offset + 4, u32::MAX)?;
            }
            io_fence();
            let mask_low = self.region.read_u32(offset)?;
            let mask_high = if wide {
                self.region.read_u32(offset + 4)?
            } else {
                0
            };
            self.region.write_u32(offset, original)?;
            if wide {
                self.region.write_u32(offset + 4, high)?;
            }
            io_fence();
            Bar::decode(register, original, mask_low, mask_high)
        }
    }
    /// # Safety
    /// BAR/窗口已由真实供应 owner 排他保留；本 function 仍无激活使用者且 BME 关闭。
    pub unsafe fn install_bar(
        &self,
        address: Address,
        bar: Bar,
        bus: u64,
    ) -> Result<(), SystemCallError> {
        if !bus.is_multiple_of(bar.bytes) || (!bar.wide && bus > u32::MAX as u64) {
            return Err(SystemCallError::IllegalArgument);
        }
        // SAFETY: 调用者持本轮配置 authority，地址已经受授权窗口及排他性核验。
        unsafe {
            let offset = address.offset(0x10 + usize::from(bar.register) * 4)?;
            self.region.write_u32(offset, bus as u32 | bar.flags)?;
            if bar.wide {
                self.region.write_u32(offset + 4, (bus >> 32) as u32)?;
            }
            io_fence();
            self.command(address, COMMAND_MEMORY, COMMAND_MASTER)?;
        }
        Ok(())
    }
    /// # Safety
    /// 所有本轮配置访问已终止且父控制尚未撤销/复用该 VA；解除不是硬件停止证明。
    pub unsafe fn unmap(self) -> Result<(), (Self, SystemCallError)> {
        // SAFETY: 由调用者证明映射寿命和停止顺序。
        unsafe { self.region.unmap() }.map_err(|(region, error)| (Self { region }, error))
    }
}
fn io_fence() {
    #[cfg(target_arch = "riscv64")]
    // SAFETY: FENCE 不访问 Rust 内存；排序本 hart 的设备及内存访问，不证明设备完成。
    unsafe {
        core::arch::asm!("fence iorw,iorw", options(nostack, preserves_flags));
    }
    #[cfg(not(target_arch = "riscv64"))]
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

pub const CONFIG_USE_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw(),
);

#[cfg(test)]
mod tests {
    use super::*;
    // 固定 qemu-xhci BAR0 是 64 位、0x4000 字节；全 1 探测的掩码来自固定 QEMU 源码。
    #[test]
    fn xhci_bar_mask_and_bus_cpu_translation() {
        let bar = Bar::decode(0, 4, 0xffff_c004, 0xffff_ffff)
            .unwrap()
            .unwrap();
        assert_eq!(bar.bytes, 0x4000);
        assert!(bar.wide);
        let info = HostInfo {
            first_bus: 0,
            last_bus: 255,
            ecam_cpu: 0x3000_0000,
            ecam_bytes: 0x1000_0000,
            window_bus: 0x4000_0000,
            window_cpu: 0x8000_0000,
            window_bytes: 0x4000_0000,
            intx: [32; 16],
            targets: 1,
        };
        assert_eq!(
            bar.placement(info, 0x4000_1000),
            Ok((0x4000_4000, 0x8000_4000))
        );
        assert_eq!(Bar::decode(1, 0, 0, 0), Ok(None));
        assert!(Bar::decode(5, 4, 0xffff_c004, 0xffff_ffff).is_err());
    }
}
