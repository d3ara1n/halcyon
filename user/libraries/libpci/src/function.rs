//! PCI function 的本轮供应 metadata 和受限 BME 会话；不开放任意配置写。
use crate::config::Address;
use libdevice::Error;
pub const RESOURCES: [&str; 4] = ["registers", "interrupt", "dma", "pci-function"];
pub const ID: u64 = u64::from_le_bytes(*b"PCIFUNC1");
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FunctionInfo {
    pub address: Address,
    pub vendor: u16,
    pub device: u16,
    pub registers_cpu: u64,
    pub registers_bytes: u64,
    pub interrupt: u32,
    pub target: u32,
}
impl FunctionInfo {
    pub const BYTES: usize = 40;
    pub fn encode(self) -> [u8; Self::BYTES] {
        let mut out = [0; Self::BYTES];
        out[..4].copy_from_slice(&1u32.to_le_bytes());
        out[4] = self.address.slot;
        out[5] = self.address.function;
        out[8..10].copy_from_slice(&self.vendor.to_le_bytes());
        out[10..12].copy_from_slice(&self.device.to_le_bytes());
        out[16..24].copy_from_slice(&self.registers_cpu.to_le_bytes());
        out[24..32].copy_from_slice(&self.registers_bytes.to_le_bytes());
        out[32..36].copy_from_slice(&self.interrupt.to_le_bytes());
        out[36..40].copy_from_slice(&self.target.to_le_bytes());
        out
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != Self::BYTES
            || bytes[..4] != 1u32.to_le_bytes()
            || bytes[6..8] != [0; 2]
            || bytes[12..16] != [0; 4]
        {
            return Err(Error::Invalid);
        }
        let info = Self {
            address: Address {
                slot: bytes[4],
                function: bytes[5],
            },
            vendor: u16::from_le_bytes(bytes[8..10].try_into().map_err(|_| Error::Invalid)?),
            device: u16::from_le_bytes(bytes[10..12].try_into().map_err(|_| Error::Invalid)?),
            registers_cpu: u64::from_le_bytes(
                bytes[16..24].try_into().map_err(|_| Error::Invalid)?,
            ),
            registers_bytes: u64::from_le_bytes(
                bytes[24..32].try_into().map_err(|_| Error::Invalid)?,
            ),
            interrupt: u32::from_le_bytes(bytes[32..36].try_into().map_err(|_| Error::Invalid)?),
            target: u32::from_le_bytes(bytes[36..40].try_into().map_err(|_| Error::Invalid)?),
        };
        if info.address.slot >= 32
            || info.address.function >= 8
            || info.registers_bytes == 0
            || info
                .registers_cpu
                .checked_add(info.registers_bytes)
                .is_none()
            || info.interrupt == 0
            || info.target >= 64
        {
            return Err(Error::Invalid);
        }
        Ok(info)
    }
}
