//! Provider 路由管理协议。
//!
//! 管理 endpoint 与普通 FAL grant 分离；绑定消息只由持有独立管理 sender
//! 的装配者发送，并携带目标 provider 的母 DirectoryGrant。

use crate::{
    authority::FalRights,
    bytes::{DecodeError, Reader, Writer},
    node::validate_path,
};

/// Provider 路由管理 RPC 协议标识。
pub const ID: u64 = 0x4641_4c32_5254_4501;
pub const VERSION: u16 = 1;
const HEADER_LEN: usize = 12;
pub const RESPONSE_LEN: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Status {
    Ok = 0,
    Invalid = 1,
    Permission = 2,
    Busy = 3,
    Internal = 4,
}

impl Status {
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Ok),
            1 => Some(Self::Invalid),
            2 => Some(Self::Permission),
            3 => Some(Self::Busy),
            4 => Some(Self::Internal),
            _ => None,
        }
    }
}

pub fn encode_status(status: Status, out: &mut [u8]) -> Option<usize> {
    if out.len() < RESPONSE_LEN {
        return None;
    }
    out[..RESPONSE_LEN].copy_from_slice(&(status as u32).to_le_bytes());
    Some(RESPONSE_LEN)
}

pub fn decode_status(bytes: &[u8]) -> Result<Status, DecodeError> {
    if bytes.len() != RESPONSE_LEN {
        return Err(DecodeError);
    }
    Status::from_u32(u32::from_le_bytes(bytes.try_into().unwrap())).ok_or(DecodeError)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bind<'a> {
    pub name: &'a str,
    pub rights: FalRights,
}

impl Bind<'_> {
    pub fn encoded_len(&self) -> Option<usize> {
        let name_len = u16::try_from(self.name.len()).ok()?;
        if name_len == 0 || self.name.contains('/') || !validate_path(self.name.as_bytes()) {
            return None;
        }
        HEADER_LEN.checked_add(name_len as usize)
    }

    pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
        let total = self.encoded_len()?;
        if out.len() < total {
            return None;
        }
        let mut writer = Writer::new(out);
        writer.reserve(total);
        writer.u16(VERSION);
        writer.u16(0);
        writer.u32(self.rights.raw());
        writer.u16(self.name.len() as u16);
        writer.u16(0);
        writer.bytes(self.name.as_bytes());
        Some(writer.written())
    }

    pub fn decode(bytes: &[u8]) -> Result<Bind<'_>, DecodeError> {
        let mut reader = Reader::new(bytes);
        if reader.u16()? != VERSION || reader.u16()? != 0 {
            return Err(DecodeError);
        }
        let rights = FalRights::from_raw(reader.u32()?).ok_or(DecodeError)?;
        let name_len = reader.u16()? as usize;
        if reader.u16()? != 0 {
            return Err(DecodeError);
        }
        let name = core::str::from_utf8(reader.bytes(name_len)?).map_err(|_| DecodeError)?;
        if reader.finish().is_err()
            || name.is_empty()
            || name.contains('/')
            || !validate_path(name.as_bytes())
        {
            return Err(DecodeError);
        }
        Ok(Bind { name, rights })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_roundtrip_and_strict_tail() {
        let bind = Bind {
            name: "remote",
            rights: FalRights::READ_PROPERTY | FalRights::TRAVERSE,
        };
        let mut bytes = [0; 64];
        let used = bind.encode(&mut bytes).unwrap();
        assert_eq!(Bind::decode(&bytes[..used]).unwrap(), bind);
        assert!(Bind::decode(&bytes[..used - 1]).is_err());
        bytes[used] = 1;
        assert!(Bind::decode(&bytes[..used + 1]).is_err());
    }

    #[test]
    fn status_roundtrip_is_strict() {
        let mut bytes = [0; RESPONSE_LEN];
        assert_eq!(encode_status(Status::Busy, &mut bytes), Some(RESPONSE_LEN));
        assert_eq!(decode_status(&bytes), Ok(Status::Busy));
        assert!(decode_status(&bytes[..3]).is_err());
        bytes.copy_from_slice(&99u32.to_le_bytes());
        assert!(decode_status(&bytes).is_err());
    }

    #[test]
    fn bind_rejects_non_component_names() {
        let mut bytes = [0; 64];
        for name in ["", ".", "..", "a/b"] {
            assert!(
                Bind {
                    name,
                    rights: FalRights::ALL,
                }
                .encode(&mut bytes)
                .is_none()
            );
        }
    }
}
