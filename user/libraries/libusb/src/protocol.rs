//! 首批受限USB接口会话：每seq只做一份boot控制或8字节interrupt IN，结果保留到下一seq。
use erhino_shared::time::Deadline;
use libdevice::Error;
use librpc::bytes::{Reader, Writer};
pub const ID: u64 = u64::from_le_bytes(*b"USBINT01");
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 32;
pub const BODY_MAX: usize = erhino_shared::message::PAYLOAD_MAX - librpc::PREFIX_LEN - HEADER_LEN;
#[derive(Clone, Copy)]
pub struct Header {
    pub op: u16,
    pub error: Option<Error>,
    pub deadline: Deadline,
}
impl Header {
    pub fn encode(self, body: &[u8], out: &mut [u8]) -> Result<usize, Error> {
        if body.len() > BODY_MAX {
            return Err(Error::Invalid);
        }
        if out.len() < HEADER_LEN + body.len() {
            return Err(Error::Invalid);
        }
        self.deadline.instant().map_err(|_| Error::Invalid)?;
        let mut w = Writer::new(out);
        w.u16(VERSION);
        w.u16(self.op);
        w.u32(self.error.map_or(0, libdevice::protocol::error_code));
        w.u32(body.len() as u32);
        w.u32(0);
        w.u32(self.deadline.kind);
        w.u32(self.deadline.reserved);
        w.u64(self.deadline.at_ns);
        w.bytes(body);
        Ok(w.written())
    }
    pub fn decode(bytes: &[u8]) -> Result<(Self, &[u8]), Error> {
        let mut r = Reader::new(bytes);
        if r.u16()? != VERSION {
            return Err(Error::Unsupported);
        }
        let op = r.u16()?;
        let error = libdevice::protocol::decode_error(r.u32()?)?;
        let length = r.u32()? as usize;
        if r.u32()? != 0 || length > BODY_MAX {
            return Err(Error::Invalid);
        }
        let deadline = Deadline {
            kind: r.u32()?,
            reserved: r.u32()?,
            at_ns: r.u64()?,
        };
        deadline.instant().map_err(|_| Error::Invalid)?;
        let body = r.bytes(length)?;
        r.finish()?;
        Ok((
            Self {
                op,
                error,
                deadline,
            },
            body,
        ))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    BootProtocol,
    InterruptIn,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Transfer { sequence: u64, request: Transfer },
    Query { sequence: u64 },
    Cancel { sequence: u64 },
}
impl Command {
    pub fn sequence(self) -> u64 {
        match self {
            Self::Transfer { sequence, .. }
            | Self::Query { sequence }
            | Self::Cancel { sequence } => sequence,
        }
    }
    pub fn op(self) -> u16 {
        match self {
            Self::Transfer { .. } => 1,
            Self::Query { .. } => 2,
            Self::Cancel { .. } => 3,
        }
    }
    pub fn encode(self, deadline: Deadline, out: &mut [u8]) -> Result<usize, Error> {
        if self.sequence() == 0 {
            return Err(Error::Invalid);
        }
        let mut body = [0; 12];
        let mut w = Writer::new(&mut body);
        w.u64(self.sequence());
        if let Self::Transfer { request, .. } = self {
            w.u32(match request {
                Transfer::BootProtocol => 1,
                Transfer::InterruptIn => 2,
            });
        }
        let used = w.written();
        Header {
            op: self.op(),
            error: None,
            deadline,
        }
        .encode(&body[..used], out)
    }
    pub fn decode(bytes: &[u8], handles: usize) -> Result<(Header, Self), Error> {
        let (header, body) = Header::decode(bytes)?;
        if header.error.is_some() || handles != 0 {
            return Err(Error::Invalid);
        }
        let mut r = Reader::new(body);
        let sequence = r.u64()?;
        if sequence == 0 {
            return Err(Error::Invalid);
        }
        let command = match header.op {
            1 => Self::Transfer {
                sequence,
                request: match r.u32()? {
                    1 => Transfer::BootProtocol,
                    2 => Transfer::InterruptIn,
                    _ => return Err(Error::Unsupported),
                },
            },
            2 => Self::Query { sequence },
            3 => Self::Cancel { sequence },
            _ => return Err(Error::Unsupported),
        };
        r.finish()?;
        Ok((header, command))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    Pending,
    Complete { bytes: [u8; 8], count: u8 },
    Cancelled,
    Closed,
}
impl Progress {
    pub const BYTES: usize = 24;
    pub fn encode(self, sequence: u64, out: &mut [u8]) -> Result<usize, Error> {
        if sequence == 0 || out.len() < Self::BYTES {
            return Err(Error::Invalid);
        }
        let mut w = Writer::new(out);
        w.u64(sequence);
        let (phase, count, bytes) = match self {
            Self::Pending => (1, 0, [0; 8]),
            Self::Complete { bytes, count } if count <= 8 => (2, count, bytes),
            Self::Cancelled => (3, 0, [0; 8]),
            Self::Closed => (4, 0, [0; 8]),
            _ => return Err(Error::Invalid),
        };
        w.u32(phase);
        w.u32(u32::from(count));
        w.bytes(&bytes);
        Ok(w.written())
    }
    pub fn decode(bytes: &[u8], sequence: u64) -> Result<Self, Error> {
        let mut r = Reader::new(bytes);
        if r.u64()? != sequence || sequence == 0 {
            return Err(Error::Conflict);
        }
        let phase = r.u32()?;
        let count = r.u32()?;
        let mut data = [0; 8];
        data.copy_from_slice(r.bytes(8)?);
        r.finish()?;
        match (phase, count) {
            (1, 0) if data == [0; 8] => Ok(Self::Pending),
            (2, 0..=8) => Ok(Self::Complete {
                bytes: data,
                count: count as u8,
            }),
            (3, 0) if data == [0; 8] => Ok(Self::Cancelled),
            (4, 0) if data == [0; 8] => Ok(Self::Closed),
            _ => Err(Error::Invalid),
        }
    }
}
