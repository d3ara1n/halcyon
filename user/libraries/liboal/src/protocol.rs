//! OAL 自己的发现协议头和正文；不封装领域操作。

use crate::{
    Boundary, ObjectInfo,
    path::{validate_name, validate_path},
};
use erhino_shared::{
    message::{MESSAGE_HANDLE_MAX, PAYLOAD_MAX},
    time::Deadline,
};
use librpc::bytes::{DecodeError, Reader, Writer};

pub const ID: u64 = 0x4f41_4c31;
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 32;
pub const BODY_MAX: usize = PAYLOAD_MAX - librpc::PREFIX_LEN - HEADER_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Op {
    Lookup = 1,
    Enumerate = 2,
}
impl Op {
    pub const fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            1 => Some(Self::Lookup),
            2 => Some(Self::Enumerate),
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

/// 原始 op/version/status 保留到角色及操作识别后；未知 op 可回显 Unsupported。
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
        let mut reader = Reader::new(bytes);
        let version = reader.u16()?;
        let op = reader.u16()?;
        let status = reader.u32()?;
        let body_len = reader.u32()?;
        if reader.u32()? != 0 {
            return Err(Status::Invalid);
        }
        let deadline = Deadline {
            kind: reader.u32()?,
            reserved: reader.u32()?,
            at_ns: reader.u64()?,
        };
        deadline.instant().map_err(|_| Status::Invalid)?;
        let body = reader.bytes(usize::try_from(body_len).map_err(|_| Status::Invalid)?)?;
        reader.finish()?;
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

fn write_header(
    op: u16,
    status: Status,
    deadline: Deadline,
    body_len: usize,
    writer: &mut Writer<'_>,
) {
    writer.u16(VERSION);
    writer.u16(op);
    writer.u32(status as u32);
    writer.u32(body_len as u32);
    writer.u32(0);
    // shared 拥有 Deadline 的字段与合法性；此处只使用既有 16 字节表示。
    writer.u32(deadline.kind);
    writer.u32(deadline.reserved);
    writer.u64(deadline.at_ns);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request<'a> {
    Lookup {
        path: &'a str,
    },
    Enumerate {
        path: &'a str,
        cursor: &'a [u8],
        max_entries: u32,
    },
}
impl Request<'_> {
    pub const fn op(&self) -> Op {
        match self {
            Self::Lookup { .. } => Op::Lookup,
            Self::Enumerate { .. } => Op::Enumerate,
        }
    }
    pub fn path(&self) -> &str {
        match self {
            Self::Lookup { path } | Self::Enumerate { path, .. } => path,
        }
    }
    fn validate(&self) -> Result<(), Status> {
        validate_path(self.path())?;
        if let Self::Enumerate {
            path,
            cursor,
            max_entries,
        } = self
            && (*max_entries == 0 || (!cursor.is_empty() && !path.is_empty()))
        {
            return Err(Status::Invalid);
        }
        Ok(())
    }
    pub fn encoded_len(&self) -> Result<usize, Status> {
        self.validate()?;
        let body_len = match self {
            Self::Lookup { path } => 4usize.checked_add(path.len()),
            Self::Enumerate { path, cursor, .. } => 12usize
                .checked_add(path.len())
                .and_then(|n| n.checked_add(cursor.len())),
        }
        .ok_or(Status::Invalid)?;
        if body_len > BODY_MAX {
            return Err(Status::Invalid);
        }
        Ok(HEADER_LEN + body_len)
    }
}

pub fn encode_request(
    request: &Request<'_>,
    deadline: Deadline,
    out: &mut [u8],
) -> Result<usize, Status> {
    deadline.instant().map_err(|_| Status::Invalid)?;
    let len = request.encoded_len()?;
    if out.len() < len {
        return Err(Status::Resource);
    }
    let mut writer = Writer::new(out);
    write_header(
        request.op() as u16,
        Status::Ok,
        deadline,
        len - HEADER_LEN,
        &mut writer,
    );
    writer.bytes32(request.path().as_bytes());
    if let Request::Enumerate {
        cursor,
        max_entries,
        ..
    } = request
    {
        writer.bytes32(cursor);
        writer.u32(*max_entries);
    }
    Ok(writer.written())
}

pub fn decode_request(bytes: &[u8]) -> Result<(Header, Request<'_>), Status> {
    let (header, body) = Header::decode(bytes)?;
    if header.status != 0 {
        return Err(Status::Invalid);
    }
    if header.version != VERSION {
        return Err(Status::Unsupported);
    }
    let op = Op::from_raw(header.op).ok_or(Status::Unsupported)?;
    let mut reader = Reader::new(body);
    let path = reader.text32()?;
    let request = match op {
        Op::Lookup => Request::Lookup { path },
        Op::Enumerate => Request::Enumerate {
            path,
            cursor: reader.bytes32()?,
            max_entries: reader.u32()?,
        },
    };
    reader.finish()?;
    request.validate()?;
    Ok((header, request))
}

/// RPC 已验证槽 0 的回复权；OAL 的发现请求不接受任何业务输入能力。
pub fn decode_received_request(
    bytes: &[u8],
    occupied_slots: u64,
) -> Result<(Header, Request<'_>), Status> {
    if occupied_slots != 1 {
        return Err(Status::Invalid);
    }
    decode_request(bytes)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectoryEntry<'a> {
    pub name: &'a str,
    /// 缺省摘要是边界项，不是虚构的远端 ObjectInfo。
    pub info: Option<ObjectInfo<'a>>,
}
impl<'a> DirectoryEntry<'a> {
    fn read_validated(reader: &mut Reader<'a>) -> Self {
        // 此入口只由私有的已验证 Page 迭代器调用，不重复完整验证 ObjectInfo。
        let name = reader.text32().expect("validated entry name");
        let bytes = reader.bytes32().expect("validated entry info");
        Self {
            name,
            info: if bytes.is_empty() {
                None
            } else {
                Some(ObjectInfo::view_validated(bytes))
            },
        }
    }

    fn read(reader: &mut Reader<'a>) -> Result<Self, Status> {
        let name = reader.text32()?;
        validate_name(name)?;
        let info = reader.bytes32()?;
        Ok(Self {
            name,
            info: if info.is_empty() {
                None
            } else {
                Some(ObjectInfo::decode(info)?)
            },
        })
    }
}

/// 已验证的整页编码；不按远端 count 预分配，不为分页持有 provider 会话。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Page<'a> {
    bytes: &'a [u8],
    next_cursor: &'a [u8],
    done: bool,
    count: u32,
    entries_start: usize,
}

#[cfg(target_arch = "riscv64")]
pub(crate) struct PageLayout {
    cursor_len: usize,
    done: bool,
    count: u32,
    entries_start: usize,
}

impl<'a> Page<'a> {
    #[cfg(target_arch = "riscv64")]
    pub(crate) fn layout(self) -> PageLayout {
        PageLayout {
            cursor_len: self.next_cursor.len(),
            done: self.done,
            count: self.count,
            entries_start: self.entries_start,
        }
    }

    /// 同一个私有 payload owner 保存布局，无自引用或重新完整解码。
    #[cfg(target_arch = "riscv64")]
    pub(crate) fn from_layout(bytes: &'a [u8], layout: &PageLayout) -> Self {
        Self {
            bytes,
            next_cursor: &bytes[4..4 + layout.cursor_len],
            done: layout.done,
            count: layout.count,
            entries_start: layout.entries_start,
        }
    }

    pub fn decode(bytes: &'a [u8]) -> Result<Self, Status> {
        if bytes.len() > BODY_MAX - 4 {
            return Err(Status::Invalid);
        }
        let mut reader = Reader::new(bytes);
        let next_cursor = reader.bytes32()?;
        let done = match reader.u32()? {
            0 => false,
            1 => true,
            _ => return Err(Status::Invalid),
        };
        let count = reader.u32()?;
        let entries_start = reader.consumed();
        validate_page(next_cursor, done, count)?;
        let minimum = usize::try_from(count)
            .map_err(|_| Status::Invalid)?
            .checked_mul(9)
            .ok_or(Status::Invalid)?;
        if minimum > reader.remaining() {
            return Err(Status::Invalid);
        }
        for _ in 0..count {
            DirectoryEntry::read(&mut reader)?;
        }
        reader.finish()?;
        Ok(Self {
            bytes,
            next_cursor,
            done,
            count,
            entries_start,
        })
    }
    pub fn as_bytes(self) -> &'a [u8] {
        self.bytes
    }
    pub fn next_cursor(self) -> &'a [u8] {
        self.next_cursor
    }
    pub fn done(self) -> bool {
        self.done
    }
    pub fn count(self) -> u32 {
        self.count
    }
    pub fn entries(self) -> Entries<'a> {
        Entries {
            reader: Reader::new(&self.bytes[self.entries_start..]),
            remaining: self.count,
        }
    }
}

pub struct Entries<'a> {
    reader: Reader<'a>,
    remaining: u32,
}
impl<'a> Iterator for Entries<'a> {
    type Item = DirectoryEntry<'a>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.remaining -= 1;
        // Page 私有构造及不可变报文确保所有项已验证；不会接触新输入。
        Some(DirectoryEntry::read_validated(&mut self.reader))
    }
}

fn validate_page(cursor: &[u8], done: bool, count: u32) -> Result<(), Status> {
    if (done && !cursor.is_empty()) || (!done && (cursor.is_empty() || count == 0)) {
        return Err(Status::Invalid);
    }
    if 12usize.checked_add(cursor.len()).ok_or(Status::Invalid)? > BODY_MAX {
        return Err(Status::Invalid);
    }
    Ok(())
}

/// 编码整项、整页；容量不足返回 EntryTooLarge，不输出截断项或无进展非终页。
pub fn encode_page(
    cursor: &[u8],
    done: bool,
    entries: &[DirectoryEntry<'_>],
    out: &mut [u8],
) -> Result<usize, Status> {
    let count = u32::try_from(entries.len()).map_err(|_| Status::EntryTooLarge)?;
    validate_page(cursor, done, count)?;
    let mut len = 12usize
        .checked_add(cursor.len())
        .ok_or(Status::EntryTooLarge)?;
    for entry in entries {
        validate_name(entry.name)?;
        len = len
            .checked_add(8)
            .and_then(|n| n.checked_add(entry.name.len()))
            .and_then(|n| n.checked_add(entry.info.map_or(0, |info| info.as_bytes().len())))
            .ok_or(Status::EntryTooLarge)?;
    }
    if len > BODY_MAX - 4 {
        return Err(Status::EntryTooLarge);
    }
    if out.len() < len {
        return Err(Status::Resource);
    }
    let mut writer = Writer::new(out);
    writer.bytes32(cursor);
    writer.u32(u32::from(done));
    writer.u32(count);
    for entry in entries {
        writer.bytes32(entry.name.as_bytes());
        writer.bytes32(entry.info.map_or(&[], ObjectInfo::as_bytes));
    }
    Ok(writer.written())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response<'a> {
    Found(ObjectInfo<'a>),
    Page(Page<'a>),
    Boundary(Boundary<'a>),
    Error(Status),
}
impl Response<'_> {
    pub const fn capability_count(&self) -> usize {
        match self {
            Self::Boundary(value) => value.capability_count(),
            _ => 0,
        }
    }
    fn body_len(&self, op: u16) -> Result<usize, Status> {
        let len = match self {
            Self::Found(info) if op == Op::Lookup as u16 => info.as_bytes().len(),
            Self::Page(page) if op == Op::Enumerate as u16 => page.as_bytes().len(),
            Self::Boundary(value) if Op::from_raw(op).is_some() => value.encoded_len()?,
            Self::Error(status) if *status != Status::Ok => return Ok(0),
            _ => return Err(Status::Invalid),
        };
        let body = len.checked_add(4).ok_or(Status::Invalid)?;
        if body > BODY_MAX {
            return Err(Status::Invalid);
        }
        Ok(body)
    }
}

pub fn encode_response(
    op: u16,
    response: &Response<'_>,
    deadline: Deadline,
    out: &mut [u8],
) -> Result<usize, Status> {
    deadline.instant().map_err(|_| Status::Invalid)?;
    let body_len = response.body_len(op)?;
    if out.len() < HEADER_LEN + body_len {
        return Err(Status::Resource);
    }
    let mut writer = Writer::new(out);
    let status = if let Response::Error(status) = response {
        *status
    } else {
        Status::Ok
    };
    write_header(op, status, deadline, body_len, &mut writer);
    match response {
        Response::Found(info) => {
            writer.u32(0);
            writer.bytes(info.as_bytes());
        }
        Response::Page(page) => {
            writer.u32(0);
            writer.bytes(page.as_bytes());
        }
        Response::Boundary(boundary) => {
            writer.u32(if matches!(boundary, Boundary::Provider(_)) {
                1
            } else {
                2
            });
            boundary.write(&mut writer);
        }
        Response::Error(_) => {}
    }
    Ok(writer.written())
}

/// occupied_slots 是实际收到的非空能力槽位图；先验证角色/数量再交 typed owner。
/// RpcPrefix 的 protocol/txid/Response 身份由 librpc 在调用本方法前验证。
pub fn decode_response<'a>(
    bytes: &'a [u8],
    request: &Request<'_>,
    deadline: Deadline,
    occupied_slots: u64,
) -> Result<Response<'a>, Status> {
    request.validate()?;
    let (header, body) = Header::decode(bytes)?;
    if header.version != VERSION || header.op != request.op() as u16 || header.deadline != deadline
    {
        return Err(Status::Invalid);
    }
    let status = Status::from_raw(header.status).ok_or(Status::Invalid)?;
    // 位图包含业务槽而不是 RPC 回复权；不存在的高位同样拒绝。
    if occupied_slots >> MESSAGE_HANDLE_MAX != 0 {
        return Err(Status::Invalid);
    }
    if status != Status::Ok {
        if !body.is_empty() || occupied_slots != 0 {
            return Err(Status::Invalid);
        }
        return Ok(Response::Error(status));
    }
    let mut reader = Reader::new(body);
    let result = reader.u32()?;
    if occupied_slots != u64::from(result == 1) {
        return Err(Status::Invalid);
    }
    let response = match result {
        0 => match request {
            Request::Lookup { .. } => {
                Response::Found(ObjectInfo::decode(reader.bytes(reader.remaining())?)?)
            }
            Request::Enumerate {
                cursor,
                max_entries,
                ..
            } => {
                let page = Page::decode(reader.bytes(reader.remaining())?)?;
                if page.count > *max_entries || (!page.done && page.next_cursor == *cursor) {
                    return Err(Status::Invalid);
                }
                Response::Page(page)
            }
        },
        1 | 2 => {
            if let Request::Enumerate { cursor, .. } = request
                && !cursor.is_empty()
            {
                return Err(Status::Invalid);
            }
            let boundary = Boundary::read(result, &mut reader)?;
            boundary.validate_for(request.path())?;
            // Lookup 末项返回面貌；LinkBoundary 只接续尚未消费的后缀。
            if matches!(request, Request::Lookup { .. })
                && matches!(boundary, Boundary::Link(value) if value.remaining.is_empty())
            {
                return Err(Status::Invalid);
            }
            Response::Boundary(boundary)
        }
        _ => return Err(Status::Invalid),
    };
    reader.finish()?;
    Ok(response)
}

#[cfg(test)]
mod tests;
