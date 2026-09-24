//! FAL2 基础操作协议；RPC slot 0 之外不设临时对象 anchor。

use crate::{
    authority::FalRights,
    bytes::{DecodeError, Reader, Writer},
    node::NodeKind,
};
use erhino_shared::time::Deadline;

mod stream;
pub use stream::{
    RNL2_PROTOCOL, StreamDirection, StreamInfo, StreamOffer, StreamReason, StreamState,
};

pub const ID: u64 = 0x4641_4c32;
pub const ROOT_GRANT_KIND: u64 = 0x4641_4c32_524f_4f54;
pub const PROVIDER_READY_KIND: u64 = 0x4641_4c32_5245_4144;
pub const PROVIDER_STOPPED_KIND: u64 = 0x4641_4c32_5354_4f50;
pub const VERSION: u16 = 2;
pub const HEADER_LEN: usize = 32;
pub const PROVIDER_REPORT_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResolvePolicy {
    FollowAll = 0,
    NoFollowFinal = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProviderReport {
    pub committed: u64,
    pub abandoned: u64,
    pub downstream_abandoned: u64,
}

impl ProviderReport {
    pub fn encode(self, out: &mut [u8]) -> Option<usize> {
        if out.len() < PROVIDER_REPORT_LEN {
            return None;
        }
        let mut writer = Writer::new(out);
        writer.reserve(PROVIDER_REPORT_LEN);
        writer.u16(VERSION);
        writer.u16(0);
        writer.u32(0);
        writer.u64(self.committed);
        writer.u64(self.abandoned);
        writer.u64(self.downstream_abandoned);
        Some(writer.written())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(bytes);
        if reader.u16()? != VERSION || reader.u16()? != 0 || reader.u32()? != 0 {
            return Err(DecodeError);
        }
        let report = Self {
            committed: reader.u64()?,
            abandoned: reader.u64()?,
            downstream_abandoned: reader.u64()?,
        };
        reader.finish()?;
        Ok(report)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Op {
    Lookup = 1,
    Create = 2,
    Read = 3,
    Write = 4,
    ReadAt = 5,
    WriteAt = 6,
    Delete = 7,
    Enumerate = 8,
    Link = 9,
    Derive = 10,
    Move = 11,
    Take = 12,
    Subscribe = 13,
    QuerySubscription = 14,
    Unsubscribe = 15,
    Open = 16,
    Start = 17,
    QueryStream = 18,
    FinishStream = 19,
    CancelStream = 20,
}

impl Op {
    pub const fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            1 => Some(Self::Lookup),
            2 => Some(Self::Create),
            3 => Some(Self::Read),
            4 => Some(Self::Write),
            5 => Some(Self::ReadAt),
            6 => Some(Self::WriteAt),
            7 => Some(Self::Delete),
            8 => Some(Self::Enumerate),
            9 => Some(Self::Link),
            10 => Some(Self::Derive),
            11 => Some(Self::Move),
            12 => Some(Self::Take),
            13 => Some(Self::Subscribe),
            14 => Some(Self::QuerySubscription),
            15 => Some(Self::Unsubscribe),
            16 => Some(Self::Open),
            17 => Some(Self::Start),
            18 => Some(Self::QueryStream),
            19 => Some(Self::FinishStream),
            20 => Some(Self::CancelStream),
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
    Conflict = 9,
    Resource = 10,
    Quota = 11,
    Busy = 12,
    Cancelled = 13,
    Internal = 14,
    CrossDevice = 15,
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
            9 => Some(Self::Conflict),
            10 => Some(Self::Resource),
            11 => Some(Self::Quota),
            12 => Some(Self::Busy),
            13 => Some(Self::Cancelled),
            14 => Some(Self::Internal),
            15 => Some(Self::CrossDevice),
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
    pub fn decode(bytes: &[u8]) -> Result<(Self, &[u8]), DecodeError> {
        let mut reader = Reader::new(bytes);
        if reader.u16()? != VERSION {
            return Err(DecodeError);
        }
        let op = Op::from_raw(reader.u16()?).ok_or(DecodeError)?;
        let status = Status::from_raw(reader.u32()?).ok_or(DecodeError)?;
        let body_len = reader.u32()? as usize;
        if reader.u32()? != 0 {
            return Err(DecodeError);
        }
        let deadline = Deadline {
            kind: reader.u32()?,
            reserved: reader.u32()?,
            at_ns: reader.u64()?,
        };
        deadline.instant().map_err(|_| DecodeError)?;
        if body_len != reader.remaining() {
            return Err(DecodeError);
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
        writer.reserve(HEADER_LEN);
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

    fn read(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let value = Self {
            identity: reader.u64()?,
            version: reader.u64()?,
        };
        if value.identity == 0 && value.version != 0 {
            return Err(DecodeError);
        }
        Ok(value)
    }

    fn write(self, writer: &mut Writer<'_>) {
        writer.u64(self.identity);
        writer.u64(self.version);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchMask(u64);

impl WatchMask {
    pub const NONE: Self = Self(0);
    pub const CREATE: Self = Self(1 << 0);
    pub const DELETE: Self = Self(1 << 1);
    pub const MODIFY: Self = Self(1 << 2);
    pub const RENAME: Self = Self(1 << 3);
    pub const TERMINATED: Self = Self(1 << 4);
    pub const EVENTS: Self = Self((1 << 4) - 1);
    pub const ALL: Self = Self((1 << 5) - 1);

    pub const fn from_raw(raw: u64) -> Option<Self> {
        if raw & !Self::ALL.0 == 0 {
            Some(Self(raw))
        } else {
            None
        }
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn intersects(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub const fn intersect(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }
}

impl core::ops::BitOr for WatchMask {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl core::ops::BitOrAssign for WatchMask {
    fn bitor_assign(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum WatchReason {
    Active = 0,
    NodeDeleted = 1,
    ProviderStopping = 2,
}

impl WatchReason {
    const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Active),
            1 => Some(Self::NodeDeleted),
            2 => Some(Self::ProviderStopping),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SubscriptionInfo {
    pub id: u64,
    pub generation: u64,
    pub effective_mask: WatchMask,
    pub pending: WatchMask,
    pub reason: WatchReason,
}

impl SubscriptionInfo {
    pub const ENCODED_LEN: usize = 40;

    fn write(self, writer: &mut Writer<'_>) {
        writer.u64(self.id);
        writer.u64(self.generation);
        writer.u64(self.effective_mask.raw());
        writer.u64(self.pending.raw());
        writer.u32(self.reason as u32);
        writer.u32(0);
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let info = Self {
            id: reader.u64()?,
            generation: reader.u64()?,
            effective_mask: WatchMask::from_raw(reader.u64()?).ok_or(DecodeError)?,
            pending: WatchMask::from_raw(reader.u64()?).ok_or(DecodeError)?,
            reason: WatchReason::from_raw(reader.u32()?).ok_or(DecodeError)?,
        };
        if info.id == 0
            || reader.u32()? != 0
            || !info.effective_mask.contains(WatchMask::TERMINATED)
            || !info.effective_mask.contains(info.pending)
            || (info.reason == WatchReason::Active
                && info.pending.intersects(WatchMask::TERMINATED))
        {
            return Err(DecodeError);
        }
        Ok(info)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request<'a> {
    Lookup {
        path: &'a str,
    },
    Create {
        name: &'a str,
        kind: NodeKind,
        rights: FalRights,
        value: &'a [u8],
    },
    Read {
        path: &'a str,
    },
    Write {
        path: &'a str,
        value: &'a [u8],
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
    Delete {
        name: &'a str,
        expected: Expected,
    },
    Enumerate {
        path: &'a str,
        cursor: u64,
        limit: u16,
    },
    Link {
        name: &'a str,
        target: &'a str,
        rights: FalRights,
    },
    Derive {
        path: &'a str,
        rights: FalRights,
    },
    Move {
        source_parent: &'a str,
        source_name: &'a str,
        destination_name: &'a str,
        expected: Expected,
    },
    Take {
        path: &'a str,
    },
    Subscribe {
        path: &'a str,
        mask: WatchMask,
    },
    QuerySubscription {
        id: u64,
    },
    Unsubscribe {
        id: u64,
    },
    Open {
        path: &'a str,
        expected_identity: Option<core::num::NonZeroU64>,
        direction: StreamDirection,
        offset: u64,
        length: Option<u64>,
        session_deadline: Deadline,
        stream_protocol: u32,
        tunnel_bytes: u32,
    },
    Start,
    QueryStream,
    FinishStream,
    CancelStream,
}

fn text<'a>(reader: &mut Reader<'a>) -> Result<&'a str, DecodeError> {
    core::str::from_utf8(reader.sized_bytes()?).map_err(|_| DecodeError)
}

fn text_len(value: &str) -> Option<usize> {
    (value.len() <= u16::MAX as usize).then_some(2 + value.len())
}

fn bytes_len(value: &[u8]) -> Option<usize> {
    (value.len() <= u16::MAX as usize).then_some(2 + value.len())
}

impl<'a> Request<'a> {
    pub const fn op(&self) -> Op {
        match self {
            Self::Lookup { .. } => Op::Lookup,
            Self::Create { .. } => Op::Create,
            Self::Read { .. } => Op::Read,
            Self::Write { .. } => Op::Write,
            Self::ReadAt { .. } => Op::ReadAt,
            Self::WriteAt { .. } => Op::WriteAt,
            Self::Delete { .. } => Op::Delete,
            Self::Enumerate { .. } => Op::Enumerate,
            Self::Link { .. } => Op::Link,
            Self::Derive { .. } => Op::Derive,
            Self::Move { .. } => Op::Move,
            Self::Take { .. } => Op::Take,
            Self::Subscribe { .. } => Op::Subscribe,
            Self::QuerySubscription { .. } => Op::QuerySubscription,
            Self::Unsubscribe { .. } => Op::Unsubscribe,
            Self::Open { .. } => Op::Open,
            Self::Start => Op::Start,
            Self::QueryStream => Op::QueryStream,
            Self::FinishStream => Op::FinishStream,
            Self::CancelStream => Op::CancelStream,
        }
    }

    pub fn encoded_len(&self) -> Option<usize> {
        match self {
            Self::Lookup { path } | Self::Read { path } => text_len(path),
            Self::Create { name, value, .. } => 8usize
                .checked_add(text_len(name)?)?
                .checked_add(bytes_len(value)?),
            Self::Write { path, value } => text_len(path)?.checked_add(bytes_len(value)?),
            Self::ReadAt { path, .. } => 16usize.checked_add(text_len(path)?),
            Self::WriteAt { path, value, .. } => 8usize
                .checked_add(text_len(path)?)?
                .checked_add(bytes_len(value)?),
            Self::Delete { name, .. } => 16usize.checked_add(text_len(name)?),
            Self::Enumerate { path, .. } => 12usize.checked_add(text_len(path)?),
            Self::Link { name, target, .. } => 4usize
                .checked_add(text_len(name)?)?
                .checked_add(text_len(target)?),
            Self::Derive { path, .. } => 4usize.checked_add(text_len(path)?),
            Self::Move {
                source_parent,
                source_name,
                destination_name,
                ..
            } => 16usize
                .checked_add(text_len(source_parent)?)?
                .checked_add(text_len(source_name)?)?
                .checked_add(text_len(destination_name)?),
            Self::Take { path } => text_len(path),
            Self::Subscribe { path, .. } => 8usize.checked_add(text_len(path)?),
            Self::QuerySubscription { .. } | Self::Unsubscribe { .. } => Some(8),
            Self::Open {
                path,
                offset,
                length,
                session_deadline,
                ..
            } => {
                session_deadline.instant().ok()??;
                if let Some(length) = length {
                    offset.checked_add(*length)?;
                }
                56usize.checked_add(text_len(path)?)
            }
            Self::Start | Self::QueryStream | Self::FinishStream | Self::CancelStream => Some(0),
        }
    }

    fn write(&self, writer: &mut Writer<'_>) {
        match self {
            Self::Lookup { path } | Self::Read { path } => {
                writer.sized_bytes(path.as_bytes());
            }
            Self::Create {
                name,
                kind,
                rights,
                value,
            } => {
                writer.u32(*kind as u32);
                writer.u32(rights.raw());
                writer.sized_bytes(name.as_bytes());
                writer.sized_bytes(value);
            }
            Self::Write { path, value } => {
                writer.sized_bytes(path.as_bytes());
                writer.sized_bytes(value);
            }
            Self::ReadAt {
                path,
                offset,
                count,
            } => {
                writer.u64(*offset);
                writer.u32(*count);
                writer.u32(0);
                writer.sized_bytes(path.as_bytes());
            }
            Self::WriteAt {
                path,
                offset,
                value,
            } => {
                writer.u64(*offset);
                writer.sized_bytes(path.as_bytes());
                writer.sized_bytes(value);
            }
            Self::Delete { name, expected } => {
                expected.write(writer);
                writer.sized_bytes(name.as_bytes());
            }
            Self::Enumerate {
                path,
                cursor,
                limit,
            } => {
                writer.u64(*cursor);
                writer.u16(*limit);
                writer.u16(0);
                writer.sized_bytes(path.as_bytes());
            }
            Self::Link {
                name,
                target,
                rights,
            } => {
                writer.u32(rights.raw());
                writer.sized_bytes(name.as_bytes());
                writer.sized_bytes(target.as_bytes());
            }
            Self::Derive { path, rights } => {
                writer.u32(rights.raw());
                writer.sized_bytes(path.as_bytes());
            }
            Self::Move {
                source_parent,
                source_name,
                destination_name,
                expected,
            } => {
                expected.write(writer);
                writer.sized_bytes(source_parent.as_bytes());
                writer.sized_bytes(source_name.as_bytes());
                writer.sized_bytes(destination_name.as_bytes());
            }
            Self::Take { path } => writer.sized_bytes(path.as_bytes()),
            Self::Subscribe { path, mask } => {
                writer.u64(mask.raw());
                writer.sized_bytes(path.as_bytes());
            }
            Self::QuerySubscription { id } | Self::Unsubscribe { id } => writer.u64(*id),
            Self::Open {
                path,
                expected_identity,
                direction,
                offset,
                length,
                session_deadline,
                stream_protocol,
                tunnel_bytes,
            } => {
                writer.u32(*direction as u32);
                writer.u32(u32::from(length.is_some()));
                writer.u64(*offset);
                writer.u64(length.unwrap_or(0));
                writer.u32(session_deadline.kind);
                writer.u32(session_deadline.reserved);
                writer.u64(session_deadline.at_ns);
                writer.u32(*stream_protocol);
                writer.u32(*tunnel_bytes);
                writer.u64(expected_identity.map_or(0, core::num::NonZeroU64::get));
                writer.sized_bytes(path.as_bytes());
            }
            Self::Start | Self::QueryStream | Self::FinishStream | Self::CancelStream => {}
        }
    }

    pub fn decode(op: Op, body: &'a [u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(body);
        let request = match op {
            Op::Lookup => Self::Lookup {
                path: text(&mut reader)?,
            },
            Op::Create => Self::Create {
                kind: NodeKind::from_u32(reader.u32()?).ok_or(DecodeError)?,
                rights: FalRights::from_raw(reader.u32()?).ok_or(DecodeError)?,
                name: text(&mut reader)?,
                value: reader.sized_bytes()?,
            },
            Op::Read => Self::Read {
                path: text(&mut reader)?,
            },
            Op::Write => Self::Write {
                path: text(&mut reader)?,
                value: reader.sized_bytes()?,
            },
            Op::ReadAt => {
                let offset = reader.u64()?;
                let count = reader.u32()?;
                if reader.u32()? != 0 {
                    return Err(DecodeError);
                }
                Self::ReadAt {
                    path: text(&mut reader)?,
                    offset,
                    count,
                }
            }
            Op::WriteAt => Self::WriteAt {
                offset: reader.u64()?,
                path: text(&mut reader)?,
                value: reader.sized_bytes()?,
            },
            Op::Delete => Self::Delete {
                expected: Expected::read(&mut reader)?,
                name: text(&mut reader)?,
            },
            Op::Enumerate => {
                let cursor = reader.u64()?;
                let limit = reader.u16()?;
                if reader.u16()? != 0 || limit == 0 {
                    return Err(DecodeError);
                }
                Self::Enumerate {
                    path: text(&mut reader)?,
                    cursor,
                    limit,
                }
            }
            Op::Link => Self::Link {
                rights: FalRights::from_raw(reader.u32()?).ok_or(DecodeError)?,
                name: text(&mut reader)?,
                target: text(&mut reader)?,
            },
            Op::Derive => Self::Derive {
                rights: FalRights::from_raw(reader.u32()?).ok_or(DecodeError)?,
                path: text(&mut reader)?,
            },
            Op::Move => Self::Move {
                expected: Expected::read(&mut reader)?,
                source_parent: text(&mut reader)?,
                source_name: text(&mut reader)?,
                destination_name: text(&mut reader)?,
            },
            Op::Take => Self::Take {
                path: text(&mut reader)?,
            },
            Op::Subscribe => {
                let mask = WatchMask::from_raw(reader.u64()?).ok_or(DecodeError)?;
                if mask.is_empty() || mask.intersects(WatchMask::TERMINATED) {
                    return Err(DecodeError);
                }
                Self::Subscribe {
                    path: text(&mut reader)?,
                    mask,
                }
            }
            Op::QuerySubscription => {
                let id = reader.u64()?;
                if id == 0 {
                    return Err(DecodeError);
                }
                Self::QuerySubscription { id }
            }
            Op::Unsubscribe => {
                let id = reader.u64()?;
                if id == 0 {
                    return Err(DecodeError);
                }
                Self::Unsubscribe { id }
            }
            Op::Open => {
                let direction = StreamDirection::from_raw(reader.u32()?).ok_or(DecodeError)?;
                let flags = reader.u32()?;
                if flags & !1 != 0 {
                    return Err(DecodeError);
                }
                let offset = reader.u64()?;
                let raw_length = reader.u64()?;
                if flags == 0 && raw_length != 0 {
                    return Err(DecodeError);
                }
                let length = (flags == 1).then_some(raw_length);
                if let Some(length) = length {
                    offset.checked_add(length).ok_or(DecodeError)?;
                }
                let session_deadline = Deadline {
                    kind: reader.u32()?,
                    reserved: reader.u32()?,
                    at_ns: reader.u64()?,
                };
                if session_deadline
                    .instant()
                    .map_err(|_| DecodeError)?
                    .is_none()
                {
                    return Err(DecodeError);
                }
                Self::Open {
                    direction,
                    offset,
                    length,
                    session_deadline,
                    stream_protocol: reader.u32()?,
                    tunnel_bytes: reader.u32()?,
                    expected_identity: core::num::NonZeroU64::new(reader.u64()?),
                    path: text(&mut reader)?,
                }
            }
            Op::Start => Self::Start,
            Op::QueryStream => Self::QueryStream,
            Op::FinishStream => Self::FinishStream,
            Op::CancelStream => Self::CancelStream,
        };
        reader.finish()?;
        Ok(request)
    }
}

pub fn encode_request(request: &Request<'_>, deadline: Deadline, out: &mut [u8]) -> Option<usize> {
    let body_len = request.encoded_len()?;
    let total = HEADER_LEN.checked_add(body_len)?;
    if total > out.len() || body_len > u32::MAX as usize {
        return None;
    }
    Header {
        op: request.op(),
        status: Status::Ok,
        body_len,
        deadline,
    }
    .encode(&mut out[..HEADER_LEN]);
    let mut writer = Writer::new(&mut out[HEADER_LEN..total]);
    request.write(&mut writer);
    Some(total)
}

pub fn decode_request(bytes: &[u8]) -> Result<(Header, Request<'_>), DecodeError> {
    let (header, body) = Header::decode(bytes)?;
    if header.status != Status::Ok {
        return Err(DecodeError);
    }
    Ok((header, Request::decode(header.op, body)?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NodeInfo {
    pub identity: u64,
    pub version: u64,
    pub kind: NodeKind,
    pub rights: FalRights,
    pub size: u64,
}

impl NodeInfo {
    pub const ENCODED_LEN: usize = 32;

    fn write(self, writer: &mut Writer<'_>) {
        writer.u64(self.identity);
        writer.u64(self.version);
        writer.u32(self.kind as u32);
        writer.u32(self.rights.raw());
        writer.u64(self.size);
    }

    fn read(reader: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            identity: reader.u64()?,
            version: reader.u64()?,
            kind: NodeKind::from_u32(reader.u32()?).ok_or(DecodeError)?,
            rights: FalRights::from_raw(reader.u32()?).ok_or(DecodeError)?,
            size: reader.u64()?,
        })
    }
}

const LOOKUP_FOUND: u32 = 0;
const LOOKUP_DELEGATE: u32 = 1;
const LOOKUP_LINK: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryEntry<'a> {
    pub name: &'a str,
    pub info: NodeInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Enumeration<'a> {
    pub next: u64,
    pub count: u16,
    /// `[name_len:u16, name_bytes, NodeInfo]*`.
    pub entries: &'a [u8],
}

impl<'a> Enumeration<'a> {
    pub fn iter(self) -> EntryIter<'a> {
        EntryIter {
            reader: Reader::new(self.entries),
            remaining: self.count,
        }
    }
}

pub struct EntryIter<'a> {
    reader: Reader<'a>,
    remaining: u16,
}

impl<'a> Iterator for EntryIter<'a> {
    type Item = Result<DirectoryEntry<'a>, DecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        Some((|| {
            let name = text(&mut self.reader)?;
            let info = NodeInfo::read(&mut self.reader)?;
            if self.remaining == 0 {
                self.reader.finish()?;
            }
            Ok(DirectoryEntry { name, info })
        })())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response<'a> {
    Node(NodeInfo),
    Delegate {
        node: NodeInfo,
        consumed: &'a str,
        remaining: &'a str,
    },
    LinkBoundary {
        node: NodeInfo,
        consumed: &'a str,
        target: &'a str,
        remaining: &'a str,
    },
    Entries(Enumeration<'a>),
    Value(&'a [u8]),
    Written(u32),
    Empty,
    Subscription(SubscriptionInfo),
    StreamOffer(StreamOffer),
    StreamInfo(StreamInfo),
}

impl<'a> Response<'a> {
    fn encoded_len(&self, op: Op) -> Option<usize> {
        match self {
            Self::Node(_) if op == Op::Lookup => Some(4 + NodeInfo::ENCODED_LEN),
            Self::Node(_) => Some(NodeInfo::ENCODED_LEN),
            Self::Delegate {
                consumed,
                remaining,
                ..
            } => 4usize
                .checked_add(NodeInfo::ENCODED_LEN)?
                .checked_add(text_len(consumed)?)?
                .checked_add(text_len(remaining)?),
            Self::LinkBoundary {
                consumed,
                target,
                remaining,
                ..
            } => 4usize
                .checked_add(NodeInfo::ENCODED_LEN)?
                .checked_add(text_len(consumed)?)?
                .checked_add(text_len(target)?)?
                .checked_add(text_len(remaining)?),
            Self::Entries(entries) => {
                for entry in entries.iter() {
                    entry.ok()?;
                }
                12usize.checked_add(entries.entries.len())
            }
            Self::Value(value) => bytes_len(value),
            Self::Written(_) => Some(4),
            Self::Empty => Some(0),
            Self::Subscription(_) => Some(SubscriptionInfo::ENCODED_LEN),
            Self::StreamOffer(_) if op == Op::Open => Some(StreamOffer::ENCODED_LEN),
            Self::StreamInfo(_)
                if matches!(
                    op,
                    Op::Start | Op::QueryStream | Op::FinishStream | Op::CancelStream
                ) =>
            {
                Some(StreamInfo::ENCODED_LEN)
            }
            _ => None,
        }
    }

    fn write(&self, op: Op, writer: &mut Writer<'_>) {
        match self {
            Self::Node(info) => {
                if op == Op::Lookup {
                    writer.u32(LOOKUP_FOUND);
                }
                info.write(writer);
            }
            Self::Delegate {
                node,
                consumed,
                remaining,
            } => {
                writer.u32(LOOKUP_DELEGATE);
                node.write(writer);
                writer.sized_bytes(consumed.as_bytes());
                writer.sized_bytes(remaining.as_bytes());
            }
            Self::LinkBoundary {
                node,
                consumed,
                target,
                remaining,
            } => {
                writer.u32(LOOKUP_LINK);
                node.write(writer);
                writer.sized_bytes(consumed.as_bytes());
                writer.sized_bytes(target.as_bytes());
                writer.sized_bytes(remaining.as_bytes());
            }
            Self::Entries(entries) => {
                writer.u64(entries.next);
                writer.u16(entries.count);
                writer.u16(0);
                writer.bytes(entries.entries);
            }
            Self::Value(value) => writer.sized_bytes(value),
            Self::Written(count) => writer.u32(*count),
            Self::Empty => {}
            Self::Subscription(info) => info.write(writer),
            Self::StreamOffer(offer) => offer.write(writer),
            Self::StreamInfo(info) => info.write(writer),
        }
    }

    pub fn decode(op: Op, body: &'a [u8]) -> Result<Self, DecodeError> {
        let mut reader = Reader::new(body);
        let response = match op {
            Op::Lookup => match reader.u32()? {
                LOOKUP_FOUND => Self::Node(NodeInfo::read(&mut reader)?),
                LOOKUP_DELEGATE => Self::Delegate {
                    node: NodeInfo::read(&mut reader)?,
                    consumed: text(&mut reader)?,
                    remaining: text(&mut reader)?,
                },
                LOOKUP_LINK => Self::LinkBoundary {
                    node: NodeInfo::read(&mut reader)?,
                    consumed: text(&mut reader)?,
                    target: text(&mut reader)?,
                    remaining: text(&mut reader)?,
                },
                _ => return Err(DecodeError),
            },
            Op::Create | Op::Link | Op::Derive => Self::Node(NodeInfo::read(&mut reader)?),
            Op::Enumerate => {
                let next = reader.u64()?;
                let count = reader.u16()?;
                if reader.u16()? != 0 {
                    return Err(DecodeError);
                }
                let entries = reader.bytes(reader.remaining())?;
                if count == 0 && !entries.is_empty() {
                    return Err(DecodeError);
                }
                let enumeration = Enumeration {
                    next,
                    count,
                    entries,
                };
                for entry in enumeration.iter() {
                    entry?;
                }
                Self::Entries(enumeration)
            }
            Op::Read | Op::ReadAt => Self::Value(reader.sized_bytes()?),
            Op::WriteAt => Self::Written(reader.u32()?),
            Op::Write | Op::Delete | Op::Move | Op::Unsubscribe => Self::Empty,
            Op::Take => Self::Value(reader.sized_bytes()?),
            Op::Subscribe | Op::QuerySubscription => {
                Self::Subscription(SubscriptionInfo::read(&mut reader)?)
            }
            Op::Open => Self::StreamOffer(StreamOffer::read(&mut reader)?),
            Op::Start | Op::QueryStream | Op::FinishStream | Op::CancelStream => {
                Self::StreamInfo(StreamInfo::read(&mut reader)?)
            }
        };
        reader.finish()?;
        Ok(response)
    }

    pub const fn capability_count(&self, op: Op) -> Option<usize> {
        match (op, self) {
            (
                Op::Lookup,
                Self::Delegate {
                    node: _,
                    consumed: _,
                    remaining: _,
                },
            )
            | (Op::Derive, Self::Node(_)) => Some(1),
            (Op::Lookup, Self::Node(_))
            | (
                Op::Lookup,
                Self::LinkBoundary {
                    node: _,
                    consumed: _,
                    target: _,
                    remaining: _,
                },
            )
            | (Op::Create | Op::Link, Self::Node(_))
            | (Op::Enumerate, Self::Entries(_))
            | (Op::Read | Op::ReadAt, Self::Value(_))
            | (Op::Take, Self::Value(_))
            | (Op::WriteAt, Self::Written(_))
            | (Op::Write | Op::Delete | Op::Move | Op::Unsubscribe, Self::Empty)
            | (Op::Subscribe | Op::QuerySubscription, Self::Subscription(_)) => Some(0),
            (Op::Open, Self::StreamOffer(_)) => Some(2),
            (
                Op::Start | Op::QueryStream | Op::FinishStream | Op::CancelStream,
                Self::StreamInfo(_),
            ) => Some(0),
            _ => None,
        }
    }
}

pub fn encode_response(
    op: Op,
    status: Status,
    deadline: Deadline,
    response: &Response<'_>,
    out: &mut [u8],
) -> Option<usize> {
    let body_len = if status == Status::Ok {
        response.capability_count(op)?;
        response.encoded_len(op)?
    } else {
        if !matches!(response, Response::Empty) {
            return None;
        }
        0
    };
    let total = HEADER_LEN.checked_add(body_len)?;
    if total > out.len() || body_len > u32::MAX as usize {
        return None;
    }
    Header {
        op,
        status,
        body_len,
        deadline,
    }
    .encode(&mut out[..HEADER_LEN]);
    let mut writer = Writer::new(&mut out[HEADER_LEN..total]);
    response.write(op, &mut writer);
    Some(total)
}

pub fn decode_response(bytes: &[u8]) -> Result<(Header, Response<'_>), DecodeError> {
    let (header, body) = Header::decode(bytes)?;
    let response = if header.status == Status::Ok {
        Response::decode(header.op, body)?
    } else {
        if !body.is_empty() {
            return Err(DecodeError);
        }
        Response::Empty
    };
    Ok((header, response))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_report_roundtrip_is_strict() {
        let report = ProviderReport {
            committed: 17,
            abandoned: 3,
            downstream_abandoned: 2,
        };
        let mut bytes = [0; PROVIDER_REPORT_LEN + 1];
        let used = report.encode(&mut bytes).unwrap();
        assert_eq!(used, PROVIDER_REPORT_LEN);
        assert_eq!(ProviderReport::decode(&bytes[..used]).unwrap(), report);
        assert!(ProviderReport::decode(&bytes[..used - 1]).is_err());
        assert!(ProviderReport::decode(&bytes[..used + 1]).is_err());
        bytes[2] = 1;
        assert!(ProviderReport::decode(&bytes[..used]).is_err());
    }

    #[test]
    fn request_roundtrip_for_published_operations() {
        let requests = [
            Request::Lookup { path: "a/b" },
            Request::Create {
                name: "child",
                kind: NodeKind::Property,
                rights: FalRights::READ_PROPERTY | FalRights::WRITE_PROPERTY,
                value: b"value",
            },
            Request::Read { path: "child" },
            Request::Write {
                path: "child",
                value: b"next",
            },
            Request::ReadAt {
                path: "stream",
                offset: 3,
                count: 7,
            },
            Request::WriteAt {
                path: "stream",
                offset: 5,
                value: b"bytes",
            },
            Request::Delete {
                name: "child",
                expected: Expected::NONE,
            },
            Request::Enumerate {
                path: "",
                cursor: 0,
                limit: 16,
            },
            Request::Link {
                name: "alias",
                target: "child",
                rights: FalRights::TRAVERSE,
            },
            Request::Derive {
                path: "child",
                rights: FalRights::TRAVERSE | FalRights::ENUMERATE,
            },
            Request::Move {
                source_parent: "source",
                source_name: "old",
                destination_name: "new",
                expected: Expected {
                    identity: 7,
                    version: 3,
                },
            },
            Request::Take { path: "affine" },
            Request::Subscribe {
                path: "watched",
                mask: WatchMask::CREATE | WatchMask::MODIFY,
            },
            Request::QuerySubscription { id: 17 },
            Request::Unsubscribe { id: 17 },
        ];
        let mut buffer = [0; 512];
        for request in requests {
            let used = encode_request(&request, Deadline::at(99), &mut buffer).unwrap();
            let (header, decoded) = decode_request(&buffer[..used]).unwrap();
            assert_eq!(header.op, request.op());
            assert_eq!(decoded, request);
        }
    }

    #[test]
    fn subscription_response_roundtrip_and_masks_are_strict() {
        let info = SubscriptionInfo {
            id: 17,
            generation: 9,
            effective_mask: WatchMask::CREATE | WatchMask::TERMINATED,
            pending: WatchMask::CREATE,
            reason: WatchReason::Active,
        };
        let mut buffer = [0; 128];
        let used = encode_response(
            Op::Subscribe,
            Status::Ok,
            Deadline::INFINITE,
            &Response::Subscription(info),
            &mut buffer,
        )
        .unwrap();
        let (_, response) = decode_response(&buffer[..used]).unwrap();
        assert_eq!(response, Response::Subscription(info));
        buffer[HEADER_LEN + 24..HEADER_LEN + 32]
            .copy_from_slice(&WatchMask::RENAME.raw().to_le_bytes());
        assert!(decode_response(&buffer[..used]).is_err());

        let invalid = Request::Subscribe {
            path: "",
            mask: WatchMask::TERMINATED,
        };
        let used = encode_request(&invalid, Deadline::INFINITE, &mut buffer).unwrap();
        assert!(decode_request(&buffer[..used]).is_err());
        assert!(WatchMask::from_raw(1 << 63).is_none());
    }

    #[test]
    fn response_roundtrip_and_error_body_rule() {
        let info = NodeInfo {
            identity: 7,
            version: 3,
            kind: NodeKind::Stream,
            rights: FalRights::READ_STREAM,
            size: 64,
        };
        let mut buffer = [0; 128];
        let used = encode_response(
            Op::Lookup,
            Status::Ok,
            Deadline::INFINITE,
            &Response::Node(info),
            &mut buffer,
        )
        .unwrap();
        let (_, response) = decode_response(&buffer[..used]).unwrap();
        assert_eq!(response, Response::Node(info));
        assert!(
            encode_response(
                Op::Lookup,
                Status::NotFound,
                Deadline::INFINITE,
                &Response::Node(info),
                &mut buffer,
            )
            .is_none()
        );
    }

    #[test]
    fn delegate_and_enumeration_responses_roundtrip() {
        let info = NodeInfo {
            identity: 11,
            version: 2,
            kind: NodeKind::Directory,
            rights: FalRights::TRAVERSE,
            size: 0,
        };
        let entry_bytes = [
            5, 0, b'a', b'l', b'i', b'a', b's', 11, 0, 0, 0, 0, 0, 0, 0, // identity
            2, 0, 0, 0, 0, 0, 0, 0, // version
            1, 0, 0, 0, // kind
            1, 0, 0, 0, // rights
            0, 0, 0, 0, 0, 0, 0, 0, // size
        ];
        let mut buffer = [0; 256];
        let used = encode_response(
            Op::Lookup,
            Status::Ok,
            Deadline::INFINITE,
            &Response::Delegate {
                node: info,
                consumed: "mount",
                remaining: "leaf",
            },
            &mut buffer,
        )
        .unwrap();
        let (_, response) = decode_response(&buffer[..used]).unwrap();
        assert_eq!(
            response,
            Response::Delegate {
                node: info,
                consumed: "mount",
                remaining: "leaf",
            }
        );

        let used = encode_response(
            Op::Enumerate,
            Status::Ok,
            Deadline::INFINITE,
            &Response::Entries(Enumeration {
                next: 7,
                count: 1,
                entries: &entry_bytes,
            }),
            &mut buffer,
        )
        .unwrap();
        let (_, Response::Entries(entries)) = decode_response(&buffer[..used]).unwrap() else {
            panic!("enumeration response decoded as the wrong variant");
        };
        assert_eq!(entries.next, 7);
        assert_eq!(entries.count, 1);
        assert_eq!(
            entries.iter().next().unwrap().unwrap(),
            DirectoryEntry {
                name: "alias",
                info,
            }
        );
    }

    #[test]
    fn capability_layout_and_enumeration_tail_are_strict() {
        let info = NodeInfo {
            identity: 1,
            version: 1,
            kind: NodeKind::Directory,
            rights: FalRights::TRAVERSE,
            size: 0,
        };
        assert_eq!(Response::Node(info).capability_count(Op::Derive), Some(1));
        assert_eq!(Response::Node(info).capability_count(Op::Create), Some(0));
        assert_eq!(Response::Empty.capability_count(Op::Move), Some(0));
        assert_eq!(
            Response::Delegate {
                node: info,
                consumed: "a",
                remaining: "b",
            }
            .capability_count(Op::Lookup),
            Some(1)
        );

        let mut buffer = [0; 128];
        let malformed = [1, 0, b'x'];
        assert!(
            encode_response(
                Op::Enumerate,
                Status::Ok,
                Deadline::INFINITE,
                &Response::Entries(Enumeration {
                    next: 0,
                    count: 1,
                    entries: &malformed,
                }),
                &mut buffer,
            )
            .is_none()
        );
    }

    #[test]
    fn request_rejects_nonzero_status_and_reserved_fields() {
        let mut buffer = [0; 64];
        let used = encode_request(
            &Request::Lookup { path: "" },
            Deadline::INFINITE,
            &mut buffer,
        )
        .unwrap();
        buffer[4..8].copy_from_slice(&(Status::Invalid as u32).to_le_bytes());
        assert!(decode_request(&buffer[..used]).is_err());
        buffer[4..8].copy_from_slice(&(Status::Ok as u32).to_le_bytes());
        buffer[12] = 1;
        assert!(decode_request(&buffer[..used]).is_err());
    }
}
