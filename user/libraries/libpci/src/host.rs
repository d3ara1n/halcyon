//! PCI host 的冻结资源描述；整数按小端编码，接收者还须核实际 control 几何。
use libdevice::Error;

pub const HOST_IDENTITY: &[u8] = b"platform:pci-host-ecam-generic";
pub const HOST_NAME: &str = "pci";
pub const HOST_RESOURCES: [&str; 4] = ["ecam", "mmio-window", "irq-control", "dma-control"];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostInfo {
    pub first_bus: u8,
    pub last_bus: u8,
    pub ecam_cpu: u64,
    pub ecam_bytes: u64,
    pub window_bus: u64,
    pub window_cpu: u64,
    pub window_bytes: u64,
    /// DT interrupt-map 经 mask 后的四个 slot × 四个 pin；索引 slot*4+pin-1。
    pub intx: [u32; 16],
    pub targets: u64,
}

impl HostInfo {
    pub const BYTES: usize = 120;
    pub fn encode(self) -> [u8; Self::BYTES] {
        let mut bytes = [0; Self::BYTES];
        bytes[..4].copy_from_slice(&1u32.to_le_bytes());
        bytes[4] = self.first_bus;
        bytes[5] = self.last_bus;
        for (index, value) in [
            self.ecam_cpu,
            self.ecam_bytes,
            self.window_bus,
            self.window_cpu,
            self.window_bytes,
            self.targets,
        ]
        .into_iter()
        .enumerate()
        {
            let at = 8 + index * 8;
            bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
        }
        for (index, source) in self.intx.into_iter().enumerate() {
            let at = 56 + index * 4;
            bytes[at..at + 4].copy_from_slice(&source.to_le_bytes());
        }
        bytes
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != Self::BYTES || bytes[..4] != 1u32.to_le_bytes() || bytes[6..8] != [0; 2] {
            return Err(Error::Invalid);
        }
        let number = |at| {
            u64::from_le_bytes(
                bytes[at..at + 8]
                    .try_into()
                    .expect("host metadata integer width changed"),
            )
        };
        let info = Self {
            first_bus: bytes[4],
            last_bus: bytes[5],
            ecam_cpu: number(8),
            ecam_bytes: number(16),
            window_bus: number(24),
            window_cpu: number(32),
            window_bytes: number(40),
            targets: number(48),
            intx: core::array::from_fn(|index| {
                u32::from_le_bytes(
                    bytes[56 + index * 4..60 + index * 4]
                        .try_into()
                        .expect("host metadata IRQ width changed"),
                )
            }),
        };
        if info.first_bus > info.last_bus
            || info.targets == 0
            || info.window_bytes == 0
            || !info.ecam_cpu.is_multiple_of(1 << 20)
            || info.ecam_bytes < (u64::from(info.last_bus) - u64::from(info.first_bus) + 1) << 20
            || info.ecam_cpu.checked_add(info.ecam_bytes).is_none()
            || info.window_cpu.checked_add(info.window_bytes).is_none()
            || info.window_bus.checked_add(info.window_bytes).is_none()
            || info.intx.contains(&0)
        {
            return Err(Error::Invalid);
        }
        Ok(info)
    }
    pub fn irq(self, slot: u8, pin: u8) -> Result<u32, Error> {
        if slot >= 32 || !(1..=4).contains(&pin) {
            return Err(Error::Invalid);
        }
        Ok(self.intx[usize::from(slot & 3) * 4 + usize::from(pin - 1)])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 预期来自 ECAM 每 bus 1 MiB 及完整 0..255 范围，不从当前解析器生成。
    #[test]
    fn full_bus_range_and_masked_intx_routes() {
        let mut bytes = [0; HostInfo::BYTES];
        bytes[..4].copy_from_slice(&1u32.to_le_bytes());
        bytes[5] = 255;
        for (at, value) in [
            (8, 0x3000_0000u64),
            (16, 0x1000_0000),
            (24, 0x4000_0000),
            (32, 0x4000_0000),
            (40, 0x4000_0000),
            (48, 1),
        ] {
            bytes[at..at + 8].copy_from_slice(&value.to_le_bytes());
        }
        for (index, source) in [
            32u32, 33, 34, 35, 33, 34, 35, 32, 34, 35, 32, 33, 35, 32, 33, 34,
        ]
        .into_iter()
        .enumerate()
        {
            bytes[56 + index * 4..60 + index * 4].copy_from_slice(&source.to_le_bytes());
        }
        let info = HostInfo::decode(&bytes).unwrap();
        assert_eq!(info.irq(0, 1), Ok(32));
        assert_eq!(info.irq(5, 4), Ok(32));
        assert_eq!(info.irq(1, 0), Err(Error::Invalid));
        bytes[16..24].copy_from_slice(&0x0ff0_0000u64.to_le_bytes());
        assert_eq!(HostInfo::decode(&bytes), Err(Error::Invalid));
    }
}
