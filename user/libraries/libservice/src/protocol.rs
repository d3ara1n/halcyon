//! 服务目录 v2：声明与共享入口交付，不依赖文件节点或属性表达。
use erhino_shared::{message::PAYLOAD_MAX, object::Rights, time::Deadline};
use liboal::{
    Boundary,
    path::{validate_name, validate_path},
};
use librpc::bytes::{DecodeError, Reader, Writer};
pub const ID: u64 = 0x5352_5632_5245_4701;
pub const VERSION: u16 = 2;
pub const HEADER_LEN: usize = 32;
pub const BODY_MAX: usize = PAYLOAD_MAX - librpc::PREFIX_LEN - HEADER_LEN;
pub const DIRECTORY_GRANT_KIND: u64 = 0x5352_5632_4449_5201;
pub const AUTHORITY_GRANT_KIND: u64 = 0x5352_5632_4155_5401;

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
fn read_deadline(r: &mut Reader<'_>) -> Result<Deadline, Status> {
    let d = Deadline {
        kind: r.u32()?,
        reserved: r.u32()?,
        at_ns: r.u64()?,
    };
    d.instant().map_err(|_| Status::Invalid)?;
    Ok(d)
}
fn write_header(op: u16, status: Status, d: Deadline, len: usize, w: &mut Writer<'_>) {
    w.u16(VERSION);
    w.u16(op);
    w.u32(status as u32);
    w.u32(len as u32);
    w.u32(0);
    w.u32(d.kind);
    w.u32(d.reserved);
    w.u64(d.at_ns);
}
fn transport(r: &mut Reader<'_>) -> Result<Rights, Status> {
    let value = Rights::from_raw(r.u64()?);
    if !value.is_known() {
        return Err(Status::Invalid);
    }
    Ok(value)
}
pub fn valid_name(name: &str) -> bool {
    validate_name(name).is_ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Addressing {
    None = 0,
    Relative = 1,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProtocolDescriptor {
    pub id: u64,
    pub min_version: u16,
    pub max_version: u16,
    pub addressing: Addressing,
}
impl ProtocolDescriptor {
    pub const ENCODED_LEN: usize = 16;
    fn validate(self) -> Result<(), Status> {
        if self.min_version == 0 || self.min_version > self.max_version {
            return Err(Status::Invalid);
        }
        Ok(())
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, Status> {
        let v = Self {
            id: r.u64()?,
            min_version: r.u16()?,
            max_version: r.u16()?,
            addressing: match r.u32()? {
                0 => Addressing::None,
                1 => Addressing::Relative,
                _ => return Err(Status::Invalid),
            },
        };
        v.validate()?;
        Ok(v)
    }
    fn write(self, w: &mut Writer<'_>) {
        w.u64(self.id);
        w.u16(self.min_version);
        w.u16(self.max_version);
        w.u32(self.addressing as u32);
    }
}
#[derive(Debug, Clone, Copy)]
pub struct Descriptors<'a> {
    bytes: &'a [u8],
    count: u32,
}
impl<'a> Descriptors<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, Status> {
        if bytes.len() > BODY_MAX {
            return Err(Status::Invalid);
        }
        let mut r = Reader::new(bytes);
        let count = r.u32()?;
        if count == 0
            || usize::try_from(count)
                .ok()
                .and_then(|n| n.checked_mul(ProtocolDescriptor::ENCODED_LEN))
                != Some(r.remaining())
        {
            return Err(Status::Invalid);
        }
        for i in 0..count {
            let descriptor = ProtocolDescriptor::read(&mut r)?;
            let mut before =
                Reader::new(&bytes[4..4 + i as usize * ProtocolDescriptor::ENCODED_LEN]);
            for _ in 0..i {
                if ProtocolDescriptor::read(&mut before)?.id == descriptor.id {
                    return Err(Status::Invalid);
                }
            }
        }
        r.finish()?;
        Ok(Self { bytes, count })
    }
    pub fn encode(items: &[ProtocolDescriptor], out: &mut [u8]) -> Result<usize, Status> {
        let count = u32::try_from(items.len()).map_err(|_| Status::Invalid)?;
        let len = items
            .len()
            .checked_mul(ProtocolDescriptor::ENCODED_LEN)
            .and_then(|n| n.checked_add(4))
            .ok_or(Status::Invalid)?;
        if count == 0 || len > BODY_MAX {
            return Err(Status::Invalid);
        }
        for (i, item) in items.iter().enumerate() {
            item.validate()?;
            if items[..i].iter().any(|old| old.id == item.id) {
                return Err(Status::Invalid);
            }
        }
        if out.len() < len {
            return Err(Status::Resource);
        }
        let mut w = Writer::new(out);
        w.u32(count);
        for item in items {
            item.write(&mut w);
        }
        Ok(w.written())
    }
    pub fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }
    pub fn count(self) -> u32 {
        self.count
    }
    pub fn iter(self) -> DescriptorIter<'a> {
        DescriptorIter {
            reader: Reader::new(&self.bytes[4..]),
            left: self.count,
        }
    }
}
pub struct DescriptorIter<'a> {
    reader: Reader<'a>,
    left: u32,
}
impl Iterator for DescriptorIter<'_> {
    type Item = ProtocolDescriptor;
    fn next(&mut self) -> Option<Self::Item> {
        if self.left == 0 {
            return None;
        }
        self.left -= 1;
        Some(ProtocolDescriptor::read(&mut self.reader).expect("validated descriptor changed"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PublishRights(u32);
impl PublishRights {
    pub const CREATE: Self = Self(1);
    pub const REPLACE: Self = Self(2);
    pub const DELEGATE: Self = Self(4);
    pub const ALL: Self = Self(7);
    pub const fn from_raw(value: u32) -> Option<Self> {
        if value & !7 == 0 {
            Some(Self(value))
        } else {
            None
        }
    }
    pub const fn raw(self) -> u32 {
        self.0
    }
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}
impl core::ops::BitOr for PublishRights {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccessRights(u32);
impl AccessRights {
    pub const DESCRIBE: Self = Self(1);
    pub const ENUMERATE: Self = Self(2);
    pub const GET_ENTRY: Self = Self(4);
    pub const WATCH: Self = Self(8);
    pub const ALL: Self = Self(15);
    pub const fn from_raw(value: u32) -> Option<Self> {
        if value & !15 == 0 {
            Some(Self(value))
        } else {
            None
        }
    }
    pub const fn raw(self) -> u32 {
        self.0
    }
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}
impl core::ops::BitOr for AccessRights {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum RegisterMode {
    CreateOnly = 0,
    Replace = 1,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrationId {
    pub registry_incarnation: u64,
    pub serial: u64,
}
impl RegistrationId {
    pub fn validate(self) -> Result<(), Status> {
        if self.registry_incarnation == 0 || self.serial == 0 {
            return Err(Status::Invalid);
        }
        Ok(())
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, Status> {
        let v = Self {
            registry_incarnation: r.u64()?,
            serial: r.u64()?,
        };
        v.validate()?;
        Ok(v)
    }
    pub fn write(self, w: &mut Writer<'_>) {
        w.u64(self.registry_incarnation);
        w.u64(self.serial);
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum State {
    Active = 1,
    Withdrawn = 2,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Reason {
    None = 0,
    Explicit = 1,
    Replaced = 2,
    ControlClosed = 3,
    EndpointClosed = 4,
    ProviderStopping = 5,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegistrationInfo {
    pub id: RegistrationId,
    pub state: State,
    pub reason: Reason,
}
impl RegistrationInfo {
    pub const ENCODED_LEN: usize = 24;
    pub fn write(self, w: &mut Writer<'_>) {
        self.id.write(w);
        w.u32(self.state as u32);
        w.u32(self.reason as u32);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, Status> {
        let v = Self {
            id: RegistrationId::read(r)?,
            state: match r.u32()? {
                1 => State::Active,
                2 => State::Withdrawn,
                _ => return Err(Status::Invalid),
            },
            reason: match r.u32()? {
                0 => Reason::None,
                1 => Reason::Explicit,
                2 => Reason::Replaced,
                3 => Reason::ControlClosed,
                4 => Reason::EndpointClosed,
                5 => Reason::ProviderStopping,
                _ => return Err(Status::Invalid),
            },
        };
        v.validate()?;
        Ok(v)
    }
    fn validate(self) -> Result<(), Status> {
        self.id.validate()?;
        if (self.state == State::Active) != (self.reason == Reason::None) {
            return Err(Status::Invalid);
        }
        Ok(())
    }
}
pub const REGISTERED: u64 = 1;
pub const WITHDRAWN: u64 = 2;
pub const REPLACED: u64 = 4;
pub const INITIAL: u64 = 1 << 62;
pub const TERMINAL: u64 = 1 << 63;
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
    pub fn validate(self) -> Result<(), Status> {
        if self.sequence == 0
            || self.pending & !(REGISTERED | WITHDRAWN | REPLACED | INITIAL | TERMINAL) != 0
            || (self.state == WatchState::Active
                && (self.reason != WatchReason::Active
                    || self.sequence == u64::MAX
                    || self.pending & TERMINAL != 0))
            || (self.state == WatchState::Terminal && self.reason == WatchReason::Active)
        {
            return Err(Status::Invalid);
        }
        Ok(())
    }
    pub fn write(self, w: &mut Writer<'_>) {
        w.u64(self.sequence);
        w.u64(self.pending);
        w.u32(self.state as u32);
        w.u32(self.reason as u32);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, Status> {
        let v = Self {
            sequence: r.u64()?,
            pending: r.u64()?,
            state: match r.u32()? {
                1 => WatchState::Active,
                2 => WatchState::Terminal,
                _ => return Err(Status::Invalid),
            },
            reason: match r.u32()? {
                0 => WatchReason::Active,
                1 => WatchReason::TargetRetired,
                2 => WatchReason::ProviderStopping,
                3 => WatchReason::Cancelled,
                4 => WatchReason::Revoked,
                5 => WatchReason::SequenceExhausted,
                _ => return Err(Status::Invalid),
            },
        };
        v.validate()?;
        Ok(v)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Request<'a> {
    DelegateName {
        name: &'a str,
        publish_rights: PublishRights,
    },
    Register {
        name: &'a str,
        mode: RegisterMode,
        descriptors: Descriptors<'a>,
        export_transport: Rights,
    },
    GetEntry {
        path: &'a str,
    },
    Withdraw,
    QueryRegistration,
    WatchDirectory {
        directory_path: &'a str,
    },
    WatchName {
        directory_path: &'a str,
        name: &'a str,
    },
    QueryWatch,
    AckWatch {
        sequence: u64,
    },
    CancelWatch,
}
impl Request<'_> {
    pub const fn op(&self) -> u16 {
        match self {
            Self::DelegateName { .. } => 1,
            Self::Register { .. } => 2,
            Self::GetEntry { .. } => 3,
            Self::Withdraw => 4,
            Self::QueryRegistration => 5,
            Self::WatchDirectory { .. } => 6,
            Self::WatchName { .. } => 7,
            Self::QueryWatch => 8,
            Self::AckWatch { .. } => 9,
            Self::CancelWatch => 10,
        }
    }
    pub fn path(&self) -> Option<&str> {
        match self {
            Self::GetEntry { path } => Some(path),
            Self::WatchDirectory { directory_path } | Self::WatchName { directory_path, .. } => {
                Some(directory_path)
            }
            _ => None,
        }
    }
    pub fn body_len(&self) -> Result<usize, Status> {
        if let Some(path) = self.path() {
            validate_path(path).map_err(|_| Status::Invalid)?;
        }
        let len = match self {
            Self::DelegateName { name, .. } => {
                validate_name(name).map_err(|_| Status::Invalid)?;
                8usize.checked_add(name.len())
            }
            Self::Register {
                name,
                descriptors,
                export_transport,
                ..
            } => {
                validate_name(name).map_err(|_| Status::Invalid)?;
                if !export_transport.is_known() {
                    return Err(Status::Invalid);
                }
                20usize
                    .checked_add(name.len())
                    .and_then(|n| n.checked_add(descriptors.as_bytes().len()))
            }
            Self::GetEntry { path }
            | Self::WatchDirectory {
                directory_path: path,
            } => 4usize.checked_add(path.len()),
            Self::WatchName {
                directory_path,
                name,
            } => {
                validate_name(name).map_err(|_| Status::Invalid)?;
                8usize
                    .checked_add(directory_path.len())
                    .and_then(|n| n.checked_add(name.len()))
            }
            Self::AckWatch { .. } => Some(8),
            _ => Some(0),
        }
        .ok_or(Status::Invalid)?;
        if len > BODY_MAX {
            return Err(Status::Invalid);
        }
        Ok(len)
    }
    pub const fn slots(&self) -> u64 {
        if matches!(self, Self::Register { .. }) {
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
    if deadline.instant().map_err(|_| Status::Invalid)?.is_none()
        && matches!(request, Request::Register { .. })
    {
        return Err(Status::Invalid);
    }
    let len = request.body_len()?;
    if out.len() < HEADER_LEN + len {
        return Err(Status::Resource);
    }
    let mut w = Writer::new(out);
    write_header(request.op(), Status::Ok, deadline, len, &mut w);
    match request {
        Request::DelegateName {
            name,
            publish_rights,
        } => {
            w.bytes32(name.as_bytes());
            w.u32(publish_rights.raw());
        }
        Request::Register {
            name,
            mode,
            descriptors,
            export_transport,
        } => {
            w.bytes32(name.as_bytes());
            w.u32(*mode as u32);
            w.bytes32(descriptors.as_bytes());
            w.u64(export_transport.raw());
        }
        Request::GetEntry { path } => w.bytes32(path.as_bytes()),
        Request::WatchDirectory { directory_path } => w.bytes32(directory_path.as_bytes()),
        Request::WatchName {
            directory_path,
            name,
        } => {
            w.bytes32(directory_path.as_bytes());
            w.bytes32(name.as_bytes());
        }
        Request::AckWatch { sequence } => w.u64(*sequence),
        _ => {}
    }
    Ok(w.written())
}
pub fn decode_request(bytes: &[u8], slots: u64) -> Result<(Header, Request<'_>), Status> {
    let (h, body) = Header::decode(bytes)?;
    if h.status != 0 {
        return Err(Status::Invalid);
    }
    if h.version != VERSION {
        return Err(Status::Unsupported);
    }
    let mut r = Reader::new(body);
    let request = match h.op {
        1 => Request::DelegateName {
            name: r.text32()?,
            publish_rights: PublishRights::from_raw(r.u32()?).ok_or(Status::Invalid)?,
        },
        2 => {
            let name = r.text32()?;
            let mode = match r.u32()? {
                0 => RegisterMode::CreateOnly,
                1 => RegisterMode::Replace,
                _ => return Err(Status::Invalid),
            };
            let descriptors = Descriptors::decode(r.bytes32()?)?;
            let export_transport = transport(&mut r)?;
            Request::Register {
                name,
                mode,
                descriptors,
                export_transport,
            }
        }
        3 => Request::GetEntry { path: r.text32()? },
        4 => Request::Withdraw,
        5 => Request::QueryRegistration,
        6 => Request::WatchDirectory {
            directory_path: r.text32()?,
        },
        7 => Request::WatchName {
            directory_path: r.text32()?,
            name: r.text32()?,
        },
        8 => Request::QueryWatch,
        9 => Request::AckWatch { sequence: r.u64()? },
        10 => Request::CancelWatch,
        _ => return Err(Status::Unsupported),
    };
    r.finish()?;
    request.body_len()?;
    if slots != request.slots()
        || (h.op == 2 && h.deadline.instant().map_err(|_| Status::Invalid)?.is_none())
    {
        return Err(Status::Invalid);
    }
    Ok((h, request))
}
#[derive(Debug, Clone, Copy)]
pub enum Response<'a> {
    Empty,
    Registration(RegistrationInfo),
    Entry {
        id: RegistrationId,
        descriptors: Descriptors<'a>,
    },
    Watch(WatchInfo),
    Boundary(Boundary<'a>),
    Error(Status),
}
impl Response<'_> {
    fn len(&self, op: u16) -> Result<usize, Status> {
        let content = match self {
            Self::Error(status) if *status != Status::Ok => return Ok(0),
            Self::Empty if op == 1 => 0,
            Self::Registration(info) if matches!(op, 2 | 4 | 5) => {
                info.validate()?;
                24
            }
            Self::Entry { id, descriptors } if op == 3 => {
                id.validate()?;
                20usize
                    .checked_add(descriptors.as_bytes().len())
                    .ok_or(Status::Invalid)?
            }
            Self::Watch(info) if matches!(op, 6..=10) => {
                info.validate()?;
                24
            }
            Self::Boundary(boundary) if matches!(op, 3 | 6 | 7) => {
                boundary.encoded_len().map_err(|_| Status::Invalid)?
            }
            _ => return Err(Status::Invalid),
        };
        content
            .checked_add(if matches!(op, 3 | 6 | 7) { 4 } else { 0 })
            .ok_or(Status::Invalid)
    }
    pub fn slots(&self, op: u16) -> u64 {
        match self {
            Self::Error(_) => 0,
            Self::Boundary(b) => u64::from(b.capability_count() == 1),
            Self::Empty if op == 1 => 1,
            Self::Registration(_) if op == 2 => 1,
            Self::Entry { .. } => 1,
            Self::Watch(_) if matches!(op, 6 | 7) => 3,
            _ => 0,
        }
    }
}
pub fn encode_response(
    op: u16,
    response: &Response<'_>,
    deadline: Deadline,
    out: &mut [u8],
) -> Result<usize, Status> {
    deadline.instant().map_err(|_| Status::Invalid)?;
    let len = response.len(op)?;
    if len > BODY_MAX {
        return Err(Status::EntryTooLarge);
    }
    if out.len() < HEADER_LEN + len {
        return Err(Status::Resource);
    }
    let mut w = Writer::new(out);
    let status = if let Response::Error(status) = response {
        *status
    } else {
        Status::Ok
    };
    write_header(op, status, deadline, len, &mut w);
    if status != Status::Ok {
        return Ok(w.written());
    }
    if matches!(op, 3 | 6 | 7) {
        w.u32(match response {
            Response::Boundary(Boundary::Provider(_)) => 1,
            Response::Boundary(Boundary::Link(_)) => 2,
            _ => 0,
        });
    }
    match response {
        Response::Empty => {}
        Response::Registration(info) => info.write(&mut w),
        Response::Entry { id, descriptors } => {
            id.write(&mut w);
            w.bytes32(descriptors.as_bytes());
        }
        Response::Watch(info) => info.write(&mut w),
        Response::Boundary(b) => b.write(&mut w),
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
    if h.version != VERSION || h.op != request.op() || h.deadline != deadline {
        return Err(Status::Invalid);
    }
    let status = Status::from_raw(h.status).ok_or(Status::Invalid)?;
    if status != Status::Ok {
        if !body.is_empty() || slots != 0 {
            return Err(Status::Invalid);
        }
        return Ok(Response::Error(status));
    }
    let mut r = Reader::new(body);
    if matches!(h.op, 3 | 6 | 7) {
        let result = r.u32()?;
        if result != 0 {
            let boundary = Boundary::read(result, &mut r).map_err(|_| Status::Invalid)?;
            boundary
                .validate_for(request.path().ok_or(Status::Invalid)?)
                .map_err(|_| Status::Invalid)?;
            r.finish()?;
            let response = Response::Boundary(boundary);
            if slots != response.slots(h.op) {
                return Err(Status::Invalid);
            }
            return Ok(response);
        }
    }
    let response = match h.op {
        1 => Response::Empty,
        2 | 4 | 5 => Response::Registration(RegistrationInfo::read(&mut r)?),
        3 => Response::Entry {
            id: RegistrationId::read(&mut r)?,
            descriptors: Descriptors::decode(r.bytes32()?)?,
        },
        6..=10 => Response::Watch(WatchInfo::read(&mut r)?),
        _ => return Err(Status::Invalid),
    };
    r.finish()?;
    if response.slots(h.op) != slots {
        return Err(Status::Invalid);
    }
    Ok(response)
}

#[cfg(test)]
mod tests;
