//! FAL v2 正式操作与稳定位置编码；RPC slot 0 之外不设临时对象 anchor。

use crate::{
    authority::FalRights,
    bytes::{Reader, Writer},
    node::NodeKind,
};
use erhino_shared::{object::Rights, time::Deadline};

pub const ID: u64 = 0x4641_4c32;
pub const VERSION: u16 = 2;
pub const HEADER_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Op {
    Lookup = 1,
    Derive = 2,
    Enumerate = 3,
    Create = 4,
    Link = 5,
    Read = 6,
    Write = 7,
    Take = 8,
    Open = 9,
    ReadAt = 10,
    WriteAt = 11,
    Move = 12,
    Delete = 13,
    Watch = 14,
    Unwatch = 15,
}
impl Op {
    pub const fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            1 => Some(Self::Lookup),
            2 => Some(Self::Derive),
            3 => Some(Self::Enumerate),
            4 => Some(Self::Create),
            5 => Some(Self::Link),
            6 => Some(Self::Read),
            7 => Some(Self::Write),
            8 => Some(Self::Take),
            9 => Some(Self::Open),
            10 => Some(Self::ReadAt),
            11 => Some(Self::WriteAt),
            12 => Some(Self::Move),
            13 => Some(Self::Delete),
            14 => Some(Self::Watch),
            15 => Some(Self::Unwatch),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Status {
    Ok = 0,
    NotFound = 1,
    NotDirectory = 2,
    Permission = 3,
    Invalid = 4,
    Unsupported = 5,
    Exists = 6,
    NotEmpty = 7,
    GrantRevoked = 8,
    CrossDevice = 9,
    Conflict = 10,
    CursorInvalid = 11,
    Resource = 12,
    Quota = 13,
    Busy = 14,
    Cancelled = 15,
    Internal = 16,
}
impl Status {
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Ok),
            1 => Some(Self::NotFound),
            2 => Some(Self::NotDirectory),
            3 => Some(Self::Permission),
            4 => Some(Self::Invalid),
            5 => Some(Self::Unsupported),
            6 => Some(Self::Exists),
            7 => Some(Self::NotEmpty),
            8 => Some(Self::GrantRevoked),
            9 => Some(Self::CrossDevice),
            10 => Some(Self::Conflict),
            11 => Some(Self::CursorInvalid),
            12 => Some(Self::Resource),
            13 => Some(Self::Quota),
            14 => Some(Self::Busy),
            15 => Some(Self::Cancelled),
            16 => Some(Self::Internal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub op: Op,
    pub status: Status,
    pub body_len: usize,
    pub deadline: Deadline,
}
impl Header {
    pub fn decode(bytes: &[u8]) -> Result<(Self, &[u8]), crate::bytes::DecodeError> {
        let mut reader = Reader::new(bytes);
        if reader.u16()? != VERSION {
            return Err(crate::bytes::DecodeError);
        }
        let op = Op::from_raw(reader.u16()?).ok_or(crate::bytes::DecodeError)?;
        let status = Status::from_raw(reader.u32()?).ok_or(crate::bytes::DecodeError)?;
        let body_len = reader.u32()? as usize;
        if reader.u32()? != 0 {
            return Err(crate::bytes::DecodeError);
        }
        let deadline = Deadline {
            kind: reader.u32()?,
            reserved: reader.u32()?,
            at_ns: reader.u64()?,
        };
        deadline.instant().map_err(|_| crate::bytes::DecodeError)?;
        if body_len != reader.remaining() {
            return Err(crate::bytes::DecodeError);
        }
        let body = reader.bytes(body_len)?;
        Ok((
            Self {
                op,
                status,
                body_len,
                deadline,
            },
            body,
        ))
    }
    pub fn encode(self, out: &mut [u8]) {
        let mut writer = Writer::new(out);
        writer.u16(VERSION);
        writer.u16(self.op as u16);
        writer.u32(self.status as u32);
        writer.u32(self.body_len as u32);
        writer.u32(0);
        writer.u32(self.deadline.kind);
        writer.u32(self.deadline.reserved);
        writer.u64(self.deadline.at_ns);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expected {
    pub identity: u64,
    pub version: u64,
}
impl Expected {
    pub const NONE: Self = Self {
        identity: 0,
        version: 0,
    };
    pub fn read(reader: &mut Reader<'_>) -> Result<Self, crate::bytes::DecodeError> {
        let value = Self {
            identity: reader.u64()?,
            version: reader.u64()?,
        };
        if value.identity == 0 && value.version != 0 {
            return Err(crate::bytes::DecodeError);
        }
        Ok(value)
    }
    pub fn write(self, writer: &mut Writer<'_>) {
        writer.u64(self.identity);
        writer.u64(self.version);
    }
}

pub enum Request<'a> {
    Lookup {
        path: &'a str,
        no_follow_final: bool,
    },
    Derive {
        path: &'a str,
        rights: FalRights,
        transport: Rights,
        local_root: bool,
    },
    Enumerate {
        path: &'a str,
        version: u64,
        cursor: u64,
        limit: u32,
    },
    Create {
        name: &'a str,
        kind: NodeKind,
        rights: FalRights,
        value: &'a [u8],
    },
    Link {
        name: &'a str,
        target: &'a str,
    },
    Read {
        path: &'a str,
    },
    Write {
        path: &'a str,
        value: &'a [u8],
    },
    Take {
        path: &'a str,
    },
    Open {
        path: &'a str,
        write: bool,
        offset: u64,
        length: u64,
        bytes: u64,
        deadline: Deadline,
    },
    ReadAt {
        path: &'a str,
        offset: u64,
        count: u32,
    },
    WriteAt {
        path: &'a str,
        offset: u64,
        value: &'a [u8],
    },
    Move {
        name: &'a str,
        destination: &'a str,
        expected: Expected,
    },
    Delete {
        name: &'a str,
        expected: Expected,
    },
    Watch {
        path: &'a str,
        mask: u64,
    },
    Unwatch {
        subscription: u64,
    },
}
fn text<'a>(reader: &mut Reader<'a>) -> Result<&'a str, crate::bytes::DecodeError> {
    core::str::from_utf8(reader.sized_bytes()?).map_err(|_| crate::bytes::DecodeError)
}

impl<'a> Request<'a> {
    pub fn decode(op: Op, body: &'a [u8]) -> Result<Self, crate::bytes::DecodeError> {
        let mut reader = Reader::new(body);
        let request = match op {
            Op::Lookup => {
                let flags = reader.u32()?;
                if flags & !1 != 0 {
                    return Err(crate::bytes::DecodeError);
                }
                Self::Lookup {
                    path: text(&mut reader)?,
                    no_follow_final: flags != 0,
                }
            }
            Op::Derive => {
                let rights = FalRights::from_raw(reader.u32()?).ok_or(crate::bytes::DecodeError)?;
                let flags = reader.u32()?;
                if flags & !1 != 0 {
                    return Err(crate::bytes::DecodeError);
                }
                let transport = Rights::from_raw(reader.u64()?);
                if !transport.is_known() {
                    return Err(crate::bytes::DecodeError);
                }
                Self::Derive {
                    path: text(&mut reader)?,
                    rights,
                    transport,
                    local_root: flags != 0,
                }
            }
            Op::Enumerate => Self::Enumerate {
                version: reader.u64()?,
                cursor: reader.u64()?,
                limit: reader.u32()?,
                path: text(&mut reader)?,
            },
            Op::Create => {
                let kind = NodeKind::from_u32(reader.u32()?).ok_or(crate::bytes::DecodeError)?;
                let rights = FalRights::from_raw(reader.u32()?).ok_or(crate::bytes::DecodeError)?;
                Self::Create {
                    name: text(&mut reader)?,
                    kind,
                    rights,
                    value: reader.sized_bytes()?,
                }
            }
            Op::Link => Self::Link {
                name: text(&mut reader)?,
                target: text(&mut reader)?,
            },
            Op::Read => Self::Read {
                path: text(&mut reader)?,
            },
            Op::Write => Self::Write {
                path: text(&mut reader)?,
                value: reader.sized_bytes()?,
            },
            Op::Take => Self::Take {
                path: text(&mut reader)?,
            },
            Op::Open => {
                let flags = reader.u32()?;
                if flags & !1 != 0 {
                    return Err(crate::bytes::DecodeError);
                }
                let offset = reader.u64()?;
                let length = reader.u64()?;
                let bytes = reader.u64()?;
                let deadline = Deadline {
                    kind: reader.u32()?,
                    reserved: reader.u32()?,
                    at_ns: reader.u64()?,
                };
                deadline.instant().map_err(|_| crate::bytes::DecodeError)?;
                offset
                    .checked_add(length)
                    .ok_or(crate::bytes::DecodeError)?;
                Self::Open {
                    path: text(&mut reader)?,
                    write: flags != 0,
                    offset,
                    length,
                    bytes,
                    deadline,
                }
            }
            Op::ReadAt => Self::ReadAt {
                offset: reader.u64()?,
                count: reader.u32()?,
                path: text(&mut reader)?,
            },
            Op::WriteAt => Self::WriteAt {
                offset: reader.u64()?,
                path: text(&mut reader)?,
                value: reader.sized_bytes()?,
            },
            Op::Move => Self::Move {
                expected: Expected::read(&mut reader)?,
                name: text(&mut reader)?,
                destination: text(&mut reader)?,
            },
            Op::Delete => Self::Delete {
                expected: Expected::read(&mut reader)?,
                name: text(&mut reader)?,
            },
            Op::Watch => Self::Watch {
                mask: reader.u64()?,
                path: text(&mut reader)?,
            },
            Op::Unwatch => Self::Unwatch {
                subscription: reader.u64()?,
            },
        };
        reader.finish()?;
        Ok(request)
    }
    pub fn extra_handles(&self) -> Option<usize> {
        match self {
            Self::Move { .. } | Self::Watch { .. } => Some(1),
            Self::Create {
                kind: NodeKind::Property,
                ..
            }
            | Self::Write { .. } => None,
            _ => Some(0),
        }
    }
}
