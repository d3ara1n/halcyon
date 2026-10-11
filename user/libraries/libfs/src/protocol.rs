//! fs v1 线上协议；直接以本次路径操作 backing，没有发现或身份握手前置。

use crate::{authority::FsRights, node::NodeKind, value::Property};
use erhino_shared::{message::PAYLOAD_MAX, object::Rights, time::Deadline};
use liboal::{
    Boundary,
    path::{validate_name, validate_path, validate_target},
};
use librpc::bytes::{DecodeError, Reader, Writer};

pub const ID: u64 = 0x4653_3031;
pub const ROOT_GRANT_KIND: u64 = 0x4653_3031_524f_4f54;
pub const PROVIDER_READY_KIND: u64 = 0x4653_3031_5244_5901;
pub const PROVIDER_STOPPED_KIND: u64 = 0x4653_3031_5354_5001;
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 32;
pub const BODY_MAX: usize = PAYLOAD_MAX - librpc::PREFIX_LEN - HEADER_LEN;

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
    Revoked = 8,
    Conflict = 9,
    Resource = 10,
    Quota = 11,
    Busy = 12,
    Cancelled = 13,
    Internal = 14,
    CrossFilesystem = 15,
    TypeMismatch = 16,
    Range = 17,
    Expired = 18,
    CursorInvalid = 19,
    SummaryTooLarge = 20,
    EntryTooLarge = 21,
    ValueTooLarge = 22,
    Unavailable = 23,
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
            8 => Some(Self::Revoked),
            9 => Some(Self::Conflict),
            10 => Some(Self::Resource),
            11 => Some(Self::Quota),
            12 => Some(Self::Busy),
            13 => Some(Self::Cancelled),
            14 => Some(Self::Internal),
            15 => Some(Self::CrossFilesystem),
            16 => Some(Self::TypeMismatch),
            17 => Some(Self::Range),
            18 => Some(Self::Expired),
            19 => Some(Self::CursorInvalid),
            20 => Some(Self::SummaryTooLarge),
            21 => Some(Self::EntryTooLarge),
            22 => Some(Self::ValueTooLarge),
            23 => Some(Self::Unavailable),
            _ => None,
        }
    }
}
impl From<DecodeError> for Status {
    fn from(_: DecodeError) -> Self {
        Self::Invalid
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Op {
    Get = 1,
    Set = 2,
    ReadAt = 3,
    WriteAt = 4,
    Open = 5,
    CreateOpen = 6,
    Create = 7,
    Remove = 8,
    Rename = 9,
    ReadLink = 10,
    SetLength = 11,
    Sync = 12,
    DeriveDirectory = 13,
    Watch = 14,
    Start = 15,
    QueryStream = 16,
    Finish = 17,
    Cancel = 18,
    QueryWatch = 19,
    AckWatch = 20,
    CancelWatch = 21,
}
impl Op {
    pub const fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            1 => Some(Self::Get),
            2 => Some(Self::Set),
            3 => Some(Self::ReadAt),
            4 => Some(Self::WriteAt),
            5 => Some(Self::Open),
            6 => Some(Self::CreateOpen),
            7 => Some(Self::Create),
            8 => Some(Self::Remove),
            9 => Some(Self::Rename),
            10 => Some(Self::ReadLink),
            11 => Some(Self::SetLength),
            12 => Some(Self::Sync),
            13 => Some(Self::DeriveDirectory),
            14 => Some(Self::Watch),
            15 => Some(Self::Start),
            16 => Some(Self::QueryStream),
            17 => Some(Self::Finish),
            18 => Some(Self::Cancel),
            19 => Some(Self::QueryWatch),
            20 => Some(Self::AckWatch),
            21 => Some(Self::CancelWatch),
            _ => None,
        }
    }
    pub const fn has_path(self) -> bool {
        (self as u16) <= 14
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub version: u16,
    pub op: u16,
    pub status: u32,
    pub body_len: u32,
    pub deadline: Deadline,
}
impl Header {
    pub fn decode(bytes: &[u8]) -> Result<(Self, &[u8]), Status> {
        if bytes.len() > HEADER_LEN + BODY_MAX {
            return Err(Status::Invalid);
        }
        let mut r = Reader::new(bytes);
        let version = r.u16()?;
        let op = r.u16()?;
        let status = r.u32()?;
        let body_len = r.u32()?;
        if r.u32()? != 0 {
            return Err(Status::Invalid);
        }
        let deadline = read_deadline(&mut r)?;
        let body = r.bytes(usize::try_from(body_len).map_err(|_| Status::Invalid)?)?;
        r.finish()?;
        Ok((
            Self {
                version,
                op,
                status,
                body_len,
                deadline,
            },
            body,
        ))
    }
}
pub(crate) fn read_deadline(r: &mut Reader<'_>) -> Result<Deadline, Status> {
    let d = Deadline {
        kind: r.u32()?,
        reserved: r.u32()?,
        at_ns: r.u64()?,
    };
    d.instant().map_err(|_| Status::Invalid)?;
    Ok(d)
}
pub(crate) fn write_deadline(d: Deadline, w: &mut Writer<'_>) {
    w.u32(d.kind);
    w.u32(d.reserved);
    w.u64(d.at_ns);
}
pub(crate) fn write_header(
    op: u16,
    status: Status,
    deadline: Deadline,
    len: usize,
    w: &mut Writer<'_>,
) {
    w.u16(VERSION);
    w.u16(op);
    w.u32(status as u32);
    w.u32(len as u32);
    w.u32(0);
    write_deadline(deadline, w);
}
fn rights(r: &mut Reader<'_>) -> Result<Rights, Status> {
    let value = Rights::from_raw(r.u64()?);
    if !value.is_known() {
        return Err(Status::Invalid);
    }
    Ok(value)
}
fn fs_rights(r: &mut Reader<'_>) -> Result<FsRights, Status> {
    FsRights::from_raw(r.u32()?).ok_or(Status::Invalid)
}
fn boolean(r: &mut Reader<'_>) -> Result<bool, Status> {
    match r.u32()? {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(Status::Invalid),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Direction {
    Read = 1,
    Write = 2,
}
impl Direction {
    fn read(r: &mut Reader<'_>) -> Result<Self, Status> {
        match r.u32()? {
            1 => Ok(Self::Read),
            2 => Ok(Self::Write),
            _ => Err(Status::Invalid),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenOptions {
    pub direction: Direction,
    pub offset: u64,
    pub length: Option<u64>,
    pub session_deadline: Deadline,
    pub tunnel_bytes: u32,
}
impl OpenOptions {
    fn validate(self, create: bool) -> Result<(), Status> {
        self.session_deadline
            .instant()
            .map_err(|_| Status::Invalid)?;
        if let Some(length) = self.length {
            self.offset.checked_add(length).ok_or(Status::Range)?;
        }
        if self.direction == Direction::Write && self.length.is_some() {
            return Err(Status::Invalid);
        }
        if create
            && (self.direction != Direction::Write || self.offset != 0 || self.length.is_some())
        {
            return Err(Status::Invalid);
        }
        Ok(())
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, Status> {
        let direction = Direction::read(r)?;
        let offset = r.u64()?;
        let present = boolean(r)?;
        let len = r.u64()?;
        if !present && len != 0 {
            return Err(Status::Invalid);
        }
        Ok(Self {
            direction,
            offset,
            length: present.then_some(len),
            session_deadline: read_deadline(r)?,
            tunnel_bytes: r.u32()?,
        })
    }
    fn write(self, w: &mut Writer<'_>) {
        w.u32(self.direction as u32);
        w.u64(self.offset);
        w.u32(u32::from(self.length.is_some()));
        w.u64(self.length.unwrap_or(0));
        write_deadline(self.session_deadline, w);
        w.u32(self.tunnel_bytes);
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Request<'a> {
    Get {
        path: &'a str,
    },
    Set {
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
        data: &'a [u8],
    },
    Open {
        path: &'a str,
        options: OpenOptions,
    },
    CreateOpen {
        path: &'a str,
        options: OpenOptions,
        rights: FsRights,
    },
    Create {
        path: &'a str,
        kind: NodeKind,
        rights: FsRights,
        initial: &'a [u8],
    },
    Remove {
        path: &'a str,
    },
    Rename {
        source: &'a str,
        destination_name: &'a str,
        replace: bool,
    },
    ReadLink {
        path: &'a str,
    },
    SetLength {
        path: &'a str,
        length: u64,
    },
    Sync {
        path: &'a str,
    },
    DeriveDirectory {
        path: &'a str,
        rights: FsRights,
        sender_transport: Rights,
        output_transport: Rights,
    },
    Watch {
        path: &'a str,
        mask: u64,
    },
    Start,
    QueryStream,
    Finish,
    Cancel,
    QueryWatch,
    AckWatch {
        sequence: u64,
    },
    CancelWatch,
}
impl Request<'_> {
    pub const fn op(&self) -> Op {
        match self {
            Self::Get { .. } => Op::Get,
            Self::Set { .. } => Op::Set,
            Self::ReadAt { .. } => Op::ReadAt,
            Self::WriteAt { .. } => Op::WriteAt,
            Self::Open { .. } => Op::Open,
            Self::CreateOpen { .. } => Op::CreateOpen,
            Self::Create { .. } => Op::Create,
            Self::Remove { .. } => Op::Remove,
            Self::Rename { .. } => Op::Rename,
            Self::ReadLink { .. } => Op::ReadLink,
            Self::SetLength { .. } => Op::SetLength,
            Self::Sync { .. } => Op::Sync,
            Self::DeriveDirectory { .. } => Op::DeriveDirectory,
            Self::Watch { .. } => Op::Watch,
            Self::Start => Op::Start,
            Self::QueryStream => Op::QueryStream,
            Self::Finish => Op::Finish,
            Self::Cancel => Op::Cancel,
            Self::QueryWatch => Op::QueryWatch,
            Self::AckWatch { .. } => Op::AckWatch,
            Self::CancelWatch => Op::CancelWatch,
        }
    }
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::Get { path }
            | Self::Set { path, .. }
            | Self::ReadAt { path, .. }
            | Self::WriteAt { path, .. }
            | Self::Open { path, .. }
            | Self::CreateOpen { path, .. }
            | Self::Create { path, .. }
            | Self::Remove { path }
            | Self::ReadLink { path }
            | Self::SetLength { path, .. }
            | Self::Sync { path }
            | Self::DeriveDirectory { path, .. }
            | Self::Watch { path, .. } => Some(path),
            Self::Rename { source, .. } => Some(source),
            _ => None,
        }
    }
    fn validate(&self) -> Result<(), Status> {
        if let Some(path) = self.path() {
            validate_path(path).map_err(|_| Status::Invalid)?;
        }
        match self {
            Self::Set { value, .. } => {
                Property::decode(value)?;
            }
            Self::ReadAt { offset, count, .. } => {
                offset.checked_add(u64::from(*count)).ok_or(Status::Range)?;
            }
            Self::WriteAt { offset, data, .. } => {
                offset
                    .checked_add(u64::try_from(data.len()).map_err(|_| Status::Range)?)
                    .ok_or(Status::Range)?;
            }
            Self::Open { options, .. } => options.validate(false)?,
            Self::CreateOpen { options, .. } => options.validate(true)?,
            Self::Create { kind, initial, .. } => match kind {
                NodeKind::Directory | NodeKind::Stream => {
                    if !initial.is_empty() {
                        return Err(Status::Invalid);
                    }
                }
                NodeKind::Property => {
                    Property::decode(initial)?;
                }
                NodeKind::SymbolicLink => {
                    validate_target(core::str::from_utf8(initial).map_err(|_| Status::Invalid)?)
                        .map_err(|_| Status::Invalid)?
                }
            },
            Self::Rename {
                destination_name, ..
            } => validate_name(destination_name).map_err(|_| Status::Invalid)?,
            Self::DeriveDirectory {
                sender_transport,
                output_transport,
                ..
            } => {
                if !sender_transport.is_known() || !output_transport.is_known() {
                    return Err(Status::Invalid);
                }
            }
            Self::Watch { mask, .. } if mask & !WATCH_BUSINESS != 0 => return Err(Status::Invalid),
            _ => {}
        }
        Ok(())
    }
    pub fn body_len(&self) -> Result<usize, Status> {
        self.validate()?;
        let base = self.path().map_or(0, |p| 4 + p.len());
        let extra = match self {
            Self::Set { value, .. } => 4usize.checked_add(value.len()),
            Self::ReadAt { .. } => Some(12),
            Self::WriteAt { data, .. } => 12usize.checked_add(data.len()),
            Self::Open { .. } => Some(44),
            Self::CreateOpen { .. } => Some(48),
            Self::Create { initial, .. } => 12usize.checked_add(initial.len()),
            Self::Rename {
                destination_name, ..
            } => 8usize.checked_add(destination_name.len()),
            Self::SetLength { .. } | Self::AckWatch { .. } | Self::Watch { .. } => Some(8),
            Self::DeriveDirectory { .. } => Some(20),
            _ => Some(0),
        }
        .ok_or(Status::Invalid)?;
        let len = base.checked_add(extra).ok_or(Status::Invalid)?;
        if len > BODY_MAX {
            return Err(Status::Invalid);
        }
        Ok(len)
    }
    pub const fn request_slots(&self) -> u64 {
        if matches!(self, Self::Rename { .. }) {
            3
        } else {
            1
        }
    }
}
pub fn encode_request(
    request: &Request<'_>,
    deadline: Deadline,
    out: &mut [u8],
) -> Result<usize, Status> {
    deadline.instant().map_err(|_| Status::Invalid)?;
    let len = request.body_len()?;
    if out.len() < HEADER_LEN + len {
        return Err(Status::Resource);
    }
    let mut w = Writer::new(out);
    write_header(request.op() as u16, Status::Ok, deadline, len, &mut w);
    if let Some(path) = request.path() {
        w.bytes32(path.as_bytes());
    }
    match request {
        Request::Set { value, .. } => w.bytes32(value),
        Request::ReadAt { offset, count, .. } => {
            w.u64(*offset);
            w.u32(*count);
        }
        Request::WriteAt { offset, data, .. } => {
            w.u64(*offset);
            w.bytes32(data);
        }
        Request::Open { options, .. } => options.write(&mut w),
        Request::CreateOpen {
            options, rights, ..
        } => {
            options.write(&mut w);
            w.u32(rights.raw());
        }
        Request::Create {
            kind,
            rights,
            initial,
            ..
        } => {
            w.u32(*kind as u32);
            w.u32(rights.raw());
            w.bytes32(initial);
        }
        Request::Rename {
            destination_name,
            replace,
            ..
        } => {
            w.bytes32(destination_name.as_bytes());
            w.u32(u32::from(*replace));
        }
        Request::SetLength { length, .. } => w.u64(*length),
        Request::DeriveDirectory {
            rights,
            sender_transport,
            output_transport,
            ..
        } => {
            w.u32(rights.raw());
            w.u64(sender_transport.raw());
            w.u64(output_transport.raw());
        }
        Request::Watch { mask, .. } => w.u64(*mask),
        Request::AckWatch { sequence } => w.u64(*sequence),
        _ => {}
    }
    Ok(w.written())
}
pub fn decode_request(bytes: &[u8], slots: u64) -> Result<(Header, Request<'_>), Status> {
    let (header, body) = Header::decode(bytes)?;
    if header.status != 0 {
        return Err(Status::Invalid);
    }
    if header.version != VERSION {
        return Err(Status::Unsupported);
    }
    let op = Op::from_raw(header.op).ok_or(Status::Unsupported)?;
    let mut r = Reader::new(body);
    let path = if op.has_path() { r.text32()? } else { "" };
    let request = match op {
        Op::Get => Request::Get { path },
        Op::Set => Request::Set {
            path,
            value: r.bytes32()?,
        },
        Op::ReadAt => Request::ReadAt {
            path,
            offset: r.u64()?,
            count: r.u32()?,
        },
        Op::WriteAt => Request::WriteAt {
            path,
            offset: r.u64()?,
            data: r.bytes32()?,
        },
        Op::Open => Request::Open {
            path,
            options: OpenOptions::read(&mut r)?,
        },
        Op::CreateOpen => Request::CreateOpen {
            path,
            options: OpenOptions::read(&mut r)?,
            rights: fs_rights(&mut r)?,
        },
        Op::Create => Request::Create {
            path,
            kind: NodeKind::from_raw(r.u32()?).ok_or(Status::Invalid)?,
            rights: fs_rights(&mut r)?,
            initial: r.bytes32()?,
        },
        Op::Remove => Request::Remove { path },
        Op::Rename => Request::Rename {
            source: path,
            destination_name: r.text32()?,
            replace: boolean(&mut r)?,
        },
        Op::ReadLink => Request::ReadLink { path },
        Op::SetLength => Request::SetLength {
            path,
            length: r.u64()?,
        },
        Op::Sync => Request::Sync { path },
        Op::DeriveDirectory => Request::DeriveDirectory {
            path,
            rights: fs_rights(&mut r)?,
            sender_transport: rights(&mut r)?,
            output_transport: rights(&mut r)?,
        },
        Op::Watch => Request::Watch {
            path,
            mask: r.u64()?,
        },
        Op::Start => Request::Start,
        Op::QueryStream => Request::QueryStream,
        Op::Finish => Request::Finish,
        Op::Cancel => Request::Cancel,
        Op::QueryWatch => Request::QueryWatch,
        Op::AckWatch => Request::AckWatch { sequence: r.u64()? },
        Op::CancelWatch => Request::CancelWatch,
    };
    r.finish()?;
    request.validate()?;
    if slots != request.request_slots() {
        return Err(Status::Invalid);
    }
    Ok((header, request))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum IoEnd {
    Continue = 0,
    Eof = 1,
    Error = 2,
}
#[derive(Debug, Clone, Copy)]
pub struct IoResult<'a> {
    pub count: u64,
    pub end: IoEnd,
    pub cause: Status,
    pub data: &'a [u8],
}
impl IoResult<'_> {
    fn validate(&self, read: bool) -> Result<(), Status> {
        if (read && self.count != self.data.len() as u64)
            || (!read && !self.data.is_empty())
            || ((self.end == IoEnd::Error) != (self.cause != Status::Ok))
        {
            return Err(Status::Invalid);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamOffer {
    pub session_id: u64,
    pub direction: Direction,
    pub tunnel_bytes: u32,
    pub start: u64,
    pub read_end: u64,
    pub offer_deadline: Deadline,
}
impl StreamOffer {
    pub const ENCODED_LEN: usize = 48;
    fn validate(self) -> Result<(), Status> {
        if self.session_id == 0
            || self.tunnel_bytes == 0
            || self
                .offer_deadline
                .instant()
                .map_err(|_| Status::Invalid)?
                .is_none()
            || (self.direction == Direction::Read && self.read_end < self.start)
            || (self.direction == Direction::Write && self.read_end != 0)
        {
            return Err(Status::Invalid);
        }
        Ok(())
    }
    fn write(self, w: &mut Writer<'_>) {
        w.u64(self.session_id);
        w.u32(self.direction as u32);
        w.u32(self.tunnel_bytes);
        w.u64(self.start);
        w.u64(self.read_end);
        write_deadline(self.offer_deadline, w);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, Status> {
        let v = Self {
            session_id: r.u64()?,
            direction: Direction::read(r)?,
            tunnel_bytes: r.u32()?,
            start: r.u64()?,
            read_end: r.u64()?,
            offer_deadline: read_deadline(r)?,
        };
        v.validate()?;
        Ok(v)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum StreamState {
    Offered = 1,
    Active = 2,
    Terminal = 3,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum StreamReason {
    None = 0,
    Completed = 1,
    Cancelled = 2,
    Expired = 3,
    PeerClosed = 4,
    Backend = 5,
    ProviderStopped = 6,
    Revoked = 7,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    pub session_id: u64,
    pub state: StreamState,
    pub outcome: Status,
    pub reason: StreamReason,
    pub accepted: u64,
    pub transported: u64,
}
impl StreamInfo {
    pub const ENCODED_LEN: usize = 40;
    fn validate(self) -> Result<(), Status> {
        if self.session_id == 0 {
            return Err(Status::Invalid);
        }
        if self.state != StreamState::Terminal {
            if self.outcome != Status::Ok || self.reason != StreamReason::None {
                return Err(Status::Invalid);
            }
        } else {
            let required = match self.reason {
                StreamReason::None => return Err(Status::Invalid),
                StreamReason::Completed => Status::Ok,
                StreamReason::Cancelled => Status::Cancelled,
                StreamReason::Expired => Status::Expired,
                StreamReason::PeerClosed | StreamReason::ProviderStopped => Status::Unavailable,
                StreamReason::Revoked => Status::Revoked,
                StreamReason::Backend => {
                    if self.outcome == Status::Ok {
                        return Err(Status::Invalid);
                    }
                    self.outcome
                }
            };
            if self.outcome != required {
                return Err(Status::Invalid);
            }
        }
        Ok(())
    }
    fn write(self, w: &mut Writer<'_>) {
        w.u64(self.session_id);
        w.u32(self.state as u32);
        w.u32(self.outcome as u32);
        w.u32(self.reason as u32);
        w.u32(0);
        w.u64(self.accepted);
        w.u64(self.transported);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, Status> {
        let session_id = r.u64()?;
        let state = match r.u32()? {
            1 => StreamState::Offered,
            2 => StreamState::Active,
            3 => StreamState::Terminal,
            _ => return Err(Status::Invalid),
        };
        let outcome = Status::from_raw(r.u32()?).ok_or(Status::Invalid)?;
        let reason = match r.u32()? {
            0 => StreamReason::None,
            1 => StreamReason::Completed,
            2 => StreamReason::Cancelled,
            3 => StreamReason::Expired,
            4 => StreamReason::PeerClosed,
            5 => StreamReason::Backend,
            6 => StreamReason::ProviderStopped,
            7 => StreamReason::Revoked,
            _ => return Err(Status::Invalid),
        };
        if r.u32()? != 0 {
            return Err(Status::Invalid);
        }
        let v = Self {
            session_id,
            state,
            outcome,
            reason,
            accepted: r.u64()?,
            transported: r.u64()?,
        };
        v.validate()?;
        Ok(v)
    }
}
pub const WATCH_CREATE: u64 = 1 << 0;
pub const WATCH_DELETE: u64 = 1 << 1;
pub const WATCH_RENAME: u64 = 1 << 2;
pub const WATCH_MODIFY: u64 = 1 << 3;
pub const WATCH_UNBOUND: u64 = 1 << 4;
pub const WATCH_BUSINESS: u64 = (1 << 5) - 1;
pub const WATCH_INITIAL: u64 = 1 << 62;
pub const WATCH_TERMINAL: u64 = 1 << 63;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum WatchState {
    Active = 1,
    Terminal = 2,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum WatchReason {
    Active = 0,
    TargetRetired = 1,
    ProviderStopping = 2,
    Cancelled = 3,
    Revoked = 4,
    SequenceExhausted = 5,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchInfo {
    pub sequence: u64,
    pub pending: u64,
    pub state: WatchState,
    pub reason: WatchReason,
}
impl WatchInfo {
    pub const ENCODED_LEN: usize = 24;
    fn validate(self) -> Result<(), Status> {
        if self.sequence == 0
            || self.pending & !(WATCH_BUSINESS | WATCH_INITIAL | WATCH_TERMINAL) != 0
            || (self.state == WatchState::Active
                && (self.reason != WatchReason::Active
                    || self.sequence == u64::MAX
                    || self.pending & WATCH_TERMINAL != 0))
            || (self.state == WatchState::Terminal && self.reason == WatchReason::Active)
        {
            return Err(Status::Invalid);
        }
        Ok(())
    }
    fn write(self, w: &mut Writer<'_>) {
        w.u64(self.sequence);
        w.u64(self.pending);
        w.u32(self.state as u32);
        w.u32(self.reason as u32);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, Status> {
        let sequence = r.u64()?;
        let pending = r.u64()?;
        let state = match r.u32()? {
            1 => WatchState::Active,
            2 => WatchState::Terminal,
            _ => return Err(Status::Invalid),
        };
        let reason = match r.u32()? {
            0 => WatchReason::Active,
            1 => WatchReason::TargetRetired,
            2 => WatchReason::ProviderStopping,
            3 => WatchReason::Cancelled,
            4 => WatchReason::Revoked,
            5 => WatchReason::SequenceExhausted,
            _ => return Err(Status::Invalid),
        };
        let v = Self {
            sequence,
            pending,
            state,
            reason,
        };
        v.validate()?;
        Ok(v)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Response<'a> {
    Empty,
    Value(Property<'a>),
    Io(IoResult<'a>),
    Offer(StreamOffer),
    Stream(StreamInfo),
    Watch(WatchInfo),
    LinkTarget(&'a str),
    Boundary(Boundary<'a>),
    Error(Status),
}
impl Response<'_> {
    pub fn capability_count(&self, op: Op) -> usize {
        match self {
            Self::Boundary(value) => value.capability_count(),
            Self::Offer(_) => 2,
            Self::Watch(_) if op == Op::Watch => 2,
            Self::Empty if op == Op::DeriveDirectory => 1,
            _ => 0,
        }
    }
    fn content_len(&self, op: Op) -> Result<usize, Status> {
        Ok(match self {
            Self::Empty
                if matches!(
                    op,
                    Op::Set
                        | Op::Create
                        | Op::Remove
                        | Op::Rename
                        | Op::SetLength
                        | Op::Sync
                        | Op::DeriveDirectory
                ) =>
            {
                0
            }
            Self::Value(value) if op == Op::Get => 4 + value.as_bytes().len(),
            Self::Io(io) if matches!(op, Op::ReadAt | Op::WriteAt) => {
                io.validate(op == Op::ReadAt)?;
                20usize.checked_add(io.data.len()).ok_or(Status::Invalid)?
            }
            Self::Offer(v) if matches!(op, Op::Open | Op::CreateOpen) => {
                v.validate()?;
                48
            }
            Self::Stream(v)
                if matches!(op, Op::Start | Op::QueryStream | Op::Finish | Op::Cancel) =>
            {
                v.validate()?;
                40
            }
            Self::Watch(v)
                if matches!(
                    op,
                    Op::Watch | Op::QueryWatch | Op::AckWatch | Op::CancelWatch
                ) =>
            {
                v.validate()?;
                24
            }
            Self::LinkTarget(target) if op == Op::ReadLink => {
                validate_target(target).map_err(|_| Status::Invalid)?;
                4usize.checked_add(target.len()).ok_or(Status::Invalid)?
            }
            Self::Boundary(value) if op.has_path() => {
                value.encoded_len().map_err(|_| Status::Invalid)?
            }
            _ => return Err(Status::Invalid),
        })
    }
}
pub fn encode_response(
    op: u16,
    response: &Response<'_>,
    deadline: Deadline,
    out: &mut [u8],
) -> Result<usize, Status> {
    deadline.instant().map_err(|_| Status::Invalid)?;
    let (status, len) = match response {
        Response::Error(status) if *status != Status::Ok => (*status, 0),
        _ => {
            let known = Op::from_raw(op).ok_or(Status::Invalid)?;
            let len = response
                .content_len(known)?
                .checked_add(usize::from(known.has_path()) * 4)
                .ok_or(Status::Invalid)?;
            (Status::Ok, len)
        }
    };
    if len > BODY_MAX {
        return Err(Status::Invalid);
    }
    if out.len() < HEADER_LEN + len {
        return Err(Status::Resource);
    }
    let mut w = Writer::new(out);
    write_header(op, status, deadline, len, &mut w);
    if status != Status::Ok {
        return Ok(w.written());
    }
    let known = Op::from_raw(op).ok_or(Status::Invalid)?;
    if known.has_path() {
        w.u32(match response {
            Response::Boundary(Boundary::Provider(_)) => 1,
            Response::Boundary(Boundary::Link(_)) => 2,
            _ => 0,
        });
    }
    match response {
        Response::Empty => {}
        Response::Value(value) => w.bytes32(value.as_bytes()),
        Response::Io(io) => {
            w.u64(io.count);
            w.u32(io.end as u32);
            w.u32(io.cause as u32);
            w.bytes32(io.data);
        }
        Response::Offer(v) => v.write(&mut w),
        Response::Stream(v) => v.write(&mut w),
        Response::Watch(v) => v.write(&mut w),
        Response::LinkTarget(v) => w.bytes32(v.as_bytes()),
        Response::Boundary(v) => v.write(&mut w),
        Response::Error(_) => return Err(Status::Invalid),
    }
    Ok(w.written())
}
pub fn decode_response<'a>(
    bytes: &'a [u8],
    request: &Request<'_>,
    deadline: Deadline,
    slots: u64,
) -> Result<Response<'a>, Status> {
    let (h, body) = Header::decode(bytes)?;
    if h.version != VERSION || h.op != request.op() as u16 || h.deadline != deadline {
        return Err(Status::Invalid);
    }
    let status = Status::from_raw(h.status).ok_or(Status::Invalid)?;
    if status != Status::Ok {
        if !body.is_empty() || slots != 0 {
            return Err(Status::Invalid);
        }
        return Ok(Response::Error(status));
    }
    let op = request.op();
    let mut r = Reader::new(body);
    if op.has_path() {
        let result = r.u32()?;
        if result != 0 {
            let boundary = Boundary::read(result, &mut r).map_err(|_| Status::Invalid)?;
            boundary
                .validate_for(request.path().ok_or(Status::Invalid)?)
                .map_err(|_| Status::Invalid)?;
            r.finish()?;
            if slots != u64::from(matches!(boundary, Boundary::Provider(_))) {
                return Err(Status::Invalid);
            }
            return Ok(Response::Boundary(boundary));
        }
    }
    let response = match op {
        Op::Get => Response::Value(Property::decode(r.bytes32()?)?),
        Op::ReadAt | Op::WriteAt => {
            let count = r.u64()?;
            let end = match r.u32()? {
                0 => IoEnd::Continue,
                1 => IoEnd::Eof,
                2 => IoEnd::Error,
                _ => return Err(Status::Invalid),
            };
            let cause = Status::from_raw(r.u32()?).ok_or(Status::Invalid)?;
            let io = IoResult {
                count,
                end,
                cause,
                data: r.bytes32()?,
            };
            io.validate(op == Op::ReadAt)?;
            let max = match request {
                Request::ReadAt { count, .. } => u64::from(*count),
                Request::WriteAt { data, .. } => {
                    u64::try_from(data.len()).map_err(|_| Status::Invalid)?
                }
                _ => return Err(Status::Invalid),
            };
            if count > max || (max > 0 && count == 0 && end == IoEnd::Continue) {
                return Err(Status::Invalid);
            }
            Response::Io(io)
        }
        Op::Open | Op::CreateOpen => {
            let offer = StreamOffer::read(&mut r)?;
            let options = match request {
                Request::Open { options, .. } | Request::CreateOpen { options, .. } => options,
                _ => return Err(Status::Invalid),
            };
            if offer.direction != options.direction || offer.start != options.offset {
                return Err(Status::Invalid);
            }
            if offer.direction == Direction::Read
                && let Some(length) = options.length
            {
                let end = options.offset.checked_add(length).ok_or(Status::Invalid)?;
                if offer.read_end > end {
                    return Err(Status::Invalid);
                }
            }
            let offer_at = offer
                .offer_deadline
                .instant()
                .map_err(|_| Status::Invalid)?
                .ok_or(Status::Invalid)?;
            for deadline in [deadline, options.session_deadline] {
                if let Some(at) = deadline.instant().map_err(|_| Status::Invalid)?
                    && offer_at > at
                {
                    return Err(Status::Invalid);
                }
            }
            Response::Offer(offer)
        }
        Op::Start | Op::QueryStream | Op::Finish | Op::Cancel => {
            Response::Stream(StreamInfo::read(&mut r)?)
        }
        Op::Watch | Op::QueryWatch | Op::AckWatch | Op::CancelWatch => {
            Response::Watch(WatchInfo::read(&mut r)?)
        }
        Op::ReadLink => {
            let target = r.text32()?;
            validate_target(target).map_err(|_| Status::Invalid)?;
            Response::LinkTarget(target)
        }
        _ => Response::Empty,
    };
    r.finish()?;
    let count = response.capability_count(op);
    let expected = (1u64 << count) - 1;
    if slots != expected {
        return Err(Status::Invalid);
    }
    Ok(response)
}

#[cfg(test)]
mod tests;
