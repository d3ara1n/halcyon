//! 设备管理者的用户态平台委托；服务槽合同独立于内核初始槽。
use crate::Error;

pub const BOOTSTRAP: usize = 0;
pub const POOL: usize = 1;
pub const DESCRIPTION: usize = 2;
pub const ROOT_BASE: usize = 3;
pub const BYTES: usize = 16;
const MAGIC: u64 = u64::from_le_bytes(*b"DEVPLATF");
const VERSION: u32 = 1;

/// 首批全记录同序委托：第 i 条描述记录绑定本服务 ROOT_BASE+i 槽。
/// 不支持任意重排或部分委托；原内核 handle_index 不参与服务寻址。
pub struct Delegation {
    count: usize,
}
impl Delegation {
    pub fn encode(count: usize) -> Result<[u8; BYTES], Error> {
        let count = u32::try_from(count).map_err(|_| Error::Invalid)?;
        let mut bytes = [0; BYTES];
        bytes[..8].copy_from_slice(&MAGIC.to_le_bytes());
        bytes[8..12].copy_from_slice(&VERSION.to_le_bytes());
        bytes[12..16].copy_from_slice(&count.to_le_bytes());
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8], grants: usize) -> Result<Self, Error> {
        if bytes.len() != BYTES
            || bytes[..8] != MAGIC.to_le_bytes()
            || bytes[8..12] != VERSION.to_le_bytes()
        {
            return Err(Error::Invalid);
        }
        let count =
            u32::from_le_bytes(bytes[12..16].try_into().map_err(|_| Error::Invalid)?) as usize;
        if count.checked_add(ROOT_BASE) != Some(grants) {
            return Err(Error::Invalid);
        }
        Ok(Self { count })
    }
    pub fn count(&self) -> usize {
        self.count
    }
    pub fn grant(&self, record: usize) -> Option<usize> {
        if record >= self.count {
            None
        } else {
            ROOT_BASE.checked_add(record)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // 独立服务合同：描述在 2，五条记录在 3..8；多余或缺失 grant 均拒绝。
    #[test]
    fn delegated_slots_are_exact() {
        let bytes = Delegation::encode(5).unwrap();
        let config = Delegation::decode(&bytes, 8).unwrap();
        assert_eq!(config.grant(0), Some(3));
        assert_eq!(config.grant(4), Some(7));
        assert_eq!(config.grant(5), None);
        assert!(Delegation::decode(&bytes, 7).is_err());
        assert!(Delegation::decode(&bytes, 9).is_err());
        assert!(Delegation::decode(&bytes[..15], 8).is_err());
        let empty = Delegation::decode(&Delegation::encode(0).unwrap(), 3).unwrap();
        assert_eq!(empty.grant(0), None);
    }
}
