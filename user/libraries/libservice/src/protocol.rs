//! 服务注册控制协议 v1；payload 位于 RpcPrefix 之后。

use erhino_shared::{object::Rights, time::Deadline};
use libfal::{
    PATH_MAX,
    authority::FalRights,
    bytes::{DecodeError, Reader, Writer},
    value::{ExportMode, ExportPolicy, Protocol as ValueProtocol},
};

pub const ID: u64 = 0x5352_5631_5245_4701;
pub const VERSION: u16 = 1;
pub const DIRECTORY_GRANT_KIND: u64 = 0x5352_5631_4449_5201;
pub const AUTHORITY_GRANT_KIND: u64 = 0x5352_5631_4155_5401;
pub const HEADER_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum Op {
    DelegateName = 1,
    Register = 2,
    QueryName = 3,
    Withdraw = 4,
    PublishReady = 5,
    BeginDrain = 6,
    Query = 7,
}

impl Op {
    pub const fn from_raw(raw: u16) -> Option<Self> {
        match raw {
            1 => Some(Self::DelegateName),
            2 => Some(Self::Register),
            3 => Some(Self::QueryName),
            4 => Some(Self::Withdraw),
            5 => Some(Self::PublishReady),
            6 => Some(Self::BeginDrain),
            7 => Some(Self::Query),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Status {
    Ok = 0,
    NotFound = 1,
    Permission = 2,
    Invalid = 3,
    Exists = 4,
    Conflict = 5,
    Busy = 6,
    Expired = 7,
    Resource = 8,
    Quota = 9,
    Cancelled = 10,
    Internal = 11,
}

impl Status {
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Ok),
            1 => Some(Self::NotFound),
            2 => Some(Self::Permission),
            3 => Some(Self::Invalid),
            4 => Some(Self::Exists),
            5 => Some(Self::Conflict),
            6 => Some(Self::Busy),
            7 => Some(Self::Expired),
            8 => Some(Self::Resource),
            9 => Some(Self::Quota),
            10 => Some(Self::Cancelled),
            11 => Some(Self::Internal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Scope {
    Root = 1,
    ExactName = 2,
}

impl Scope {
    const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            1 => Some(Self::Root),
            2 => Some(Self::ExactName),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum State {
    Starting = 1,
    Ready = 2,
    Draining = 3,
    Terminal = 4,
}

impl State {
    const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            1 => Some(Self::Starting),
            2 => Some(Self::Ready),
            3 => Some(Self::Draining),
            4 => Some(Self::Terminal),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum TerminalReason {
    None = 0,
    Withdrawn = 1,
    ControlClosed = 2,
    EndpointClosed = 3,
    EstablishExpired = 4,
    ProviderStopping = 5,
}

impl TerminalReason {
    const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::None),
            1 => Some(Self::Withdrawn),
            2 => Some(Self::ControlClosed),
            3 => Some(Self::EndpointClosed),
            4 => Some(Self::EstablishExpired),
            5 => Some(Self::ProviderStopping),
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
        let deadline = read_deadline(&mut reader)?;
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
        write_deadline(&mut writer, self.deadline);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthorityInfo {
    pub identity: u64,
    pub scope: Scope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceInfo {
    pub instance: u64,
    pub generation: u64,
    pub state: State,
    pub reason: TerminalReason,
    pub protocol: u64,
    pub version: u32,
    pub establish_deadline: Deadline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request<'a> {
    DelegateName {
        name: &'a str,
    },
    Register {
        name: &'a str,
        protocol: u64,
        version: u32,
        policy: ExportPolicy,
        establish_deadline: Deadline,
    },
    QueryName {
        name: &'a str,
    },
    Withdraw {
        name: &'a str,
        expected_instance: u64,
        expected_generation: u64,
    },
    PublishReady,
    BeginDrain,
    Query,
}

impl Request<'_> {
    pub const fn op(&self) -> Op {
        match self {
            Self::DelegateName { .. } => Op::DelegateName,
            Self::Register { .. } => Op::Register,
            Self::QueryName { .. } => Op::QueryName,
            Self::Withdraw { .. } => Op::Withdraw,
            Self::PublishReady => Op::PublishReady,
            Self::BeginDrain => Op::BeginDrain,
            Self::Query => Op::Query,
        }
    }

    pub fn encoded_len(&self) -> Option<usize> {
        match self {
            Self::DelegateName { name } | Self::QueryName { name } => {
                2usize.checked_add(name.len())
            }
            Self::Register { name, .. } => 2usize
                .checked_add(name.len())?
                .checked_add(8 + 4 + 4 + 2 + 2 + 8 + 4 + 4 + 16),
            Self::Withdraw { name, .. } => 2usize.checked_add(name.len())?.checked_add(16),
            Self::PublishReady | Self::BeginDrain | Self::Query => Some(0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response {
    Empty,
    Authority(AuthorityInfo),
    Instance(InstanceInfo),
}

impl Response {
    pub const fn encoded_len(self) -> usize {
        match self {
            Self::Empty => 0,
            Self::Authority(_) => 16,
            Self::Instance(_) => 56,
        }
    }
}

pub fn encode_request(request: &Request<'_>, deadline: Deadline, out: &mut [u8]) -> Option<usize> {
    let body_len = request.encoded_len()?;
    let total = HEADER_LEN.checked_add(body_len)?;
    if total > out.len() || !request_valid(request) {
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
    match request {
        Request::DelegateName { name } | Request::QueryName { name } => {
            writer.sized_bytes(name.as_bytes());
        }
        Request::Register {
            name,
            protocol,
            version,
            policy,
            establish_deadline,
        } => {
            writer.sized_bytes(name.as_bytes());
            writer.u64(*protocol);
            writer.u32(*version);
            writer.u32(policy.protocol as u32);
            writer.u16(policy.mode as u16);
            writer.u16(0);
            writer.u64(policy.transport.raw());
            writer.u32(policy.fal_ceiling.raw());
            writer.u32(0);
            write_deadline(&mut writer, *establish_deadline);
        }
        Request::Withdraw {
            name,
            expected_instance,
            expected_generation,
        } => {
            writer.sized_bytes(name.as_bytes());
            writer.u64(*expected_instance);
            writer.u64(*expected_generation);
        }
        Request::PublishReady | Request::BeginDrain | Request::Query => {}
    }
    Some(total)
}

pub fn decode_request(bytes: &[u8]) -> Result<(Header, Request<'_>), DecodeError> {
    let (header, body) = Header::decode(bytes)?;
    if header.status != Status::Ok {
        return Err(DecodeError);
    }
    let mut reader = Reader::new(body);
    let request = match header.op {
        Op::DelegateName => Request::DelegateName {
            name: read_name(&mut reader)?,
        },
        Op::Register => {
            let name = read_name(&mut reader)?;
            let protocol = reader.u64()?;
            let version = reader.u32()?;
            let value_protocol = ValueProtocol::from_raw(reader.u32()?).ok_or(DecodeError)?;
            let mode = match reader.u16()? {
                0 => ExportMode::Repeatable,
                1 => ExportMode::Affine,
                _ => return Err(DecodeError),
            };
            if reader.u16()? != 0 {
                return Err(DecodeError);
            }
            let transport = Rights::from_raw(reader.u64()?);
            let fal_ceiling = FalRights::from_raw(reader.u32()?).ok_or(DecodeError)?;
            if reader.u32()? != 0 {
                return Err(DecodeError);
            }
            let establish_deadline = read_deadline(&mut reader)?;
            Request::Register {
                name,
                protocol,
                version,
                policy: ExportPolicy {
                    protocol: value_protocol,
                    mode,
                    transport,
                    fal_ceiling,
                },
                establish_deadline,
            }
        }
        Op::QueryName => Request::QueryName {
            name: read_name(&mut reader)?,
        },
        Op::Withdraw => Request::Withdraw {
            name: read_name(&mut reader)?,
            expected_instance: reader.u64()?,
            expected_generation: reader.u64()?,
        },
        Op::PublishReady => Request::PublishReady,
        Op::BeginDrain => Request::BeginDrain,
        Op::Query => Request::Query,
    };
    reader.finish()?;
    if !request_valid(&request) {
        return Err(DecodeError);
    }
    Ok((header, request))
}

pub fn encode_response(
    op: Op,
    status: Status,
    deadline: Deadline,
    response: Response,
    out: &mut [u8],
) -> Option<usize> {
    let body_len = response.encoded_len();
    if status != Status::Ok && body_len != 0 {
        return None;
    }
    let total = HEADER_LEN.checked_add(body_len)?;
    if total > out.len() {
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
    match response {
        Response::Empty => {}
        Response::Authority(info) => {
            writer.u64(info.identity);
            writer.u32(info.scope as u32);
            writer.u32(0);
        }
        Response::Instance(info) => write_instance(&mut writer, info),
    }
    Some(total)
}

pub fn decode_response(bytes: &[u8]) -> Result<(Header, Response), DecodeError> {
    let (header, body) = Header::decode(bytes)?;
    if header.status != Status::Ok {
        if !body.is_empty() {
            return Err(DecodeError);
        }
        return Ok((header, Response::Empty));
    }
    let mut reader = Reader::new(body);
    let response = match header.op {
        Op::DelegateName => Response::Authority(AuthorityInfo {
            identity: reader.u64()?,
            scope: Scope::from_raw(reader.u32()?).ok_or(DecodeError)?,
        }),
        Op::Register | Op::QueryName | Op::PublishReady | Op::BeginDrain | Op::Query => {
            Response::Instance(read_instance(&mut reader)?)
        }
        Op::Withdraw => Response::Empty,
    };
    if matches!(response, Response::Authority(_)) && reader.u32()? != 0 {
        return Err(DecodeError);
    }
    reader.finish()?;
    Ok((header, response))
}

pub const fn request_capability_count(op: Op) -> usize {
    match op {
        Op::Register => 1,
        _ => 0,
    }
}

pub fn response_capability_count(op: Op, status: Status) -> usize {
    if status != Status::Ok {
        return 0;
    }
    match op {
        Op::DelegateName | Op::Register => 1,
        _ => 0,
    }
}

pub(crate) fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= PATH_MAX
        && !name
            .bytes()
            .any(|byte| byte == 0 || b"/*?\\".contains(&byte))
}

fn request_valid(request: &Request<'_>) -> bool {
    match request {
        Request::DelegateName { name }
        | Request::QueryName { name }
        | Request::Withdraw { name, .. } => valid_name(name),
        Request::Register {
            name,
            protocol,
            version,
            policy,
            establish_deadline,
        } => {
            valid_name(name)
                && *protocol != 0
                && *version != 0
                && policy.mode == ExportMode::Repeatable
                && policy.transport.is_known()
                && policy
                    .transport
                    .contains(Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT)
                && match policy.protocol {
                    ValueProtocol::Directory => policy.fal_ceiling.contains(FalRights::TRAVERSE),
                    ValueProtocol::Mailbox => policy.fal_ceiling == FalRights::NONE,
                    ValueProtocol::Opaque | ValueProtocol::Notification => false,
                }
                && establish_deadline.instant().ok().flatten().is_some()
        }
        Request::PublishReady | Request::BeginDrain | Request::Query => true,
    }
}

fn read_name<'a>(reader: &mut Reader<'a>) -> Result<&'a str, DecodeError> {
    let bytes = reader.sized_bytes()?;
    let name = core::str::from_utf8(bytes).map_err(|_| DecodeError)?;
    valid_name(name).then_some(name).ok_or(DecodeError)
}

fn read_deadline(reader: &mut Reader<'_>) -> Result<Deadline, DecodeError> {
    let deadline = Deadline {
        kind: reader.u32()?,
        reserved: reader.u32()?,
        at_ns: reader.u64()?,
    };
    deadline.instant().map_err(|_| DecodeError)?;
    Ok(deadline)
}

fn write_deadline(writer: &mut Writer<'_>, deadline: Deadline) {
    writer.u32(deadline.kind);
    writer.u32(deadline.reserved);
    writer.u64(deadline.at_ns);
}

fn write_instance(writer: &mut Writer<'_>, info: InstanceInfo) {
    writer.u64(info.instance);
    writer.u64(info.generation);
    writer.u32(info.state as u32);
    writer.u32(info.reason as u32);
    writer.u64(info.protocol);
    writer.u32(info.version);
    writer.u32(0);
    write_deadline(writer, info.establish_deadline);
}

fn read_instance(reader: &mut Reader<'_>) -> Result<InstanceInfo, DecodeError> {
    let instance = reader.u64()?;
    let generation = reader.u64()?;
    let state = State::from_raw(reader.u32()?).ok_or(DecodeError)?;
    let reason = TerminalReason::from_raw(reader.u32()?).ok_or(DecodeError)?;
    let protocol = reader.u64()?;
    let version = reader.u32()?;
    if instance == 0
        || generation == 0
        || protocol == 0
        || version == 0
        || reader.u32()? != 0
        || matches!(state, State::Starting | State::Ready) && reason != TerminalReason::None
        || matches!(state, State::Draining | State::Terminal) && reason == TerminalReason::None
    {
        return Err(DecodeError);
    }
    let establish_deadline = read_deadline(reader)?;
    Ok(InstanceInfo {
        instance,
        generation,
        state,
        reason,
        protocol,
        version,
        establish_deadline,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory_policy() -> ExportPolicy {
        ExportPolicy {
            protocol: ValueProtocol::Directory,
            mode: ExportMode::Repeatable,
            transport: Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
            fal_ceiling: FalRights::TRAVERSE | FalRights::ENUMERATE,
        }
    }

    #[test]
    fn all_requests_roundtrip_with_strict_header_and_tail() {
        let deadline = Deadline::at(99);
        let requests = [
            Request::DelegateName { name: "fs.second" },
            Request::Register {
                name: "fs.second",
                protocol: libfal::protocol::ID,
                version: u32::from(libfal::protocol::VERSION),
                policy: directory_policy(),
                establish_deadline: Deadline::at(120),
            },
            Request::QueryName { name: "fs.second" },
            Request::Withdraw {
                name: "fs.second",
                expected_instance: 7,
                expected_generation: 3,
            },
            Request::PublishReady,
            Request::BeginDrain,
            Request::Query,
        ];
        for request in requests {
            let mut bytes = [0; 256];
            let used = encode_request(&request, deadline, &mut bytes).unwrap();
            assert_eq!(decode_request(&bytes[..used]).unwrap().1, request);
            assert!(decode_request(&bytes[..used - 1]).is_err());
            bytes[12] = 1;
            assert!(decode_request(&bytes[..used]).is_err());
        }
    }

    #[test]
    fn responses_roundtrip_and_failure_has_no_body() {
        let info = InstanceInfo {
            instance: 7,
            generation: 3,
            state: State::Ready,
            reason: TerminalReason::None,
            protocol: libfal::protocol::ID,
            version: u32::from(libfal::protocol::VERSION),
            establish_deadline: Deadline::at(120),
        };
        let mut bytes = [0; 128];
        let used = encode_response(
            Op::PublishReady,
            Status::Ok,
            Deadline::at(99),
            Response::Instance(info),
            &mut bytes,
        )
        .unwrap();
        assert_eq!(
            decode_response(&bytes[..used]).unwrap().1,
            Response::Instance(info)
        );
        assert!(
            encode_response(
                Op::PublishReady,
                Status::Conflict,
                Deadline::at(99),
                Response::Instance(info),
                &mut bytes,
            )
            .is_none()
        );
        for (state, reason) in [
            (State::Starting, TerminalReason::None),
            (State::Ready, TerminalReason::None),
            (State::Draining, TerminalReason::Withdrawn),
            (State::Terminal, TerminalReason::ControlClosed),
        ] {
            let state_info = InstanceInfo {
                state,
                reason,
                ..info
            };
            let used = encode_response(
                Op::Query,
                Status::Ok,
                Deadline::at(99),
                Response::Instance(state_info),
                &mut bytes,
            )
            .unwrap();
            assert_eq!(
                decode_response(&bytes[..used]).unwrap().1,
                Response::Instance(state_info)
            );
        }
    }

    #[test]
    fn invalid_names_and_endpoint_policies_are_rejected() {
        let mut bytes = [0; 256];
        assert!(
            encode_request(
                &Request::DelegateName { name: "a/b" },
                Deadline::INFINITE,
                &mut bytes,
            )
            .is_none()
        );
        let mut policy = directory_policy();
        policy.mode = ExportMode::Affine;
        assert!(
            encode_request(
                &Request::Register {
                    name: "service",
                    protocol: 1,
                    version: 1,
                    policy,
                    establish_deadline: Deadline::at(1),
                },
                Deadline::INFINITE,
                &mut bytes,
            )
            .is_none()
        );
    }
}
