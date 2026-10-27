//! 键盘状态Query/Watch/Ack；每个Watch有独立确认游标，通知不代表无损事件流。
use crate::Snapshot;
use erhino_shared::time::Deadline;
use libdevice::Error;
use librpc::bytes::{Reader, Writer};
pub const ID: u64 = u64::from_le_bytes(*b"HIDKEY01");
pub const HEADER_LEN: usize = libdevice::protocol::HEADER_LEN;
// 首批领域使用同一版本化RPC信封；op及正文由本协议解释。
pub use libdevice::protocol::Header;
#[derive(Clone, Copy, Debug)]
pub enum Command {
    Watch,
    Query,
    Ack(u64),
}
impl Command {
    pub fn op(self) -> u16 {
        match self {
            Self::Watch => 1,
            Self::Query => 2,
            Self::Ack(_) => 3,
        }
    }
    pub fn encode(self, deadline: Deadline, out: &mut [u8]) -> Result<usize, Error> {
        let mut body = [0; 8];
        let length = if let Self::Ack(seq) = self {
            if seq == 0 {
                return Err(Error::Invalid);
            }
            body.copy_from_slice(&seq.to_le_bytes());
            8
        } else {
            0
        };
        Header {
            op: self.op(),
            error: None,
            deadline,
        }
        .encode(&body[..length], out)
    }
    pub fn decode(bytes: &[u8], handles: usize) -> Result<(Header, Self), Error> {
        let (header, body) = Header::decode(bytes)?;
        if header.error.is_some() || handles != 0 {
            return Err(Error::Invalid);
        }
        let mut reader = Reader::new(body);
        let command = match header.op {
            1 => Self::Watch,
            2 => Self::Query,
            3 => {
                let seq = reader.u64()?;
                if seq == 0 {
                    return Err(Error::Invalid);
                }
                Self::Ack(seq)
            }
            _ => return Err(Error::Unsupported),
        };
        reader.finish()?;
        Ok((header, command))
    }
}
impl Snapshot {
    pub const BYTES: usize = 24;
    pub fn encode(self, pending: bool, out: &mut [u8]) -> Result<usize, Error> {
        if self.sequence == 0 || out.len() < Self::BYTES {
            return Err(Error::Invalid);
        }
        let mut w = Writer::new(out);
        w.u64(self.sequence);
        w.u32(u32::from(self.valid) | (u32::from(self.connected) << 1) | (u32::from(pending) << 2));
        w.u8(self.modifiers);
        w.bytes(&self.keys);
        w.bytes(&[0; 5]);
        Ok(w.written())
    }
    pub fn decode(bytes: &[u8]) -> Result<(Self, bool), Error> {
        let mut r = Reader::new(bytes);
        let sequence = r.u64()?;
        let flags = r.u32()?;
        let modifiers = r.u8()?;
        let mut keys = [0; 6];
        keys.copy_from_slice(r.bytes(6)?);
        if sequence == 0 || flags & !7 != 0 || r.bytes(5)? != [0; 5] || flags & 3 == 1 {
            return Err(Error::Invalid);
        }
        r.finish()?;
        let mut last = 0;
        let mut zero = false;
        for key in keys {
            if key == 0 {
                zero = true;
            } else if zero || key <= last || key <= 3 {
                return Err(Error::Invalid);
            }
            last = key;
        }
        Ok((
            Self {
                sequence,
                modifiers,
                keys,
                valid: flags & 1 != 0,
                connected: flags & 2 != 0,
            },
            flags & 4 != 0,
        ))
    }
}
