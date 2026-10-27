//! 设备管理与 Provision 的定宽 wire；capability 槽与正文分别校验。
use crate::{
    Error,
    binding::{Phase, Snapshot, Subject},
    identity::{BindingRef, DeviceId},
    registry::{Access, Scope},
    resource::{Entry, Limits},
};
use erhino_shared::{message::PAYLOAD_MAX, time::Deadline};
use librpc::bytes::{Reader, Writer};

pub const ID: u64 = 0x4445_5649_4345_0001;
pub const PROVISION_ID: u64 = 0x5052_4f56_4953_0001;
pub const BOOT_READY: u64 = 0x4445_5642_4f4f_0001;
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 32;
pub const BODY_MAX: usize = PAYLOAD_MAX - librpc::PREFIX_LEN - HEADER_LEN;
pub const DEVICE_FIXED_LEN: usize = 72;

pub fn error_code(error: Error) -> u32 {
    match error {
        Error::Permission => 1,
        Error::Invalid => 2,
        Error::Conflict => 3,
        Error::Quota => 4,
        Error::Resource => 5,
        Error::NotFound => 6,
        Error::StaleInstance => 7,
        Error::StaleEpoch => 8,
        Error::InvalidEpoch => 9,
        Error::Busy => 10,
        Error::NotReady => 11,
        Error::Closed => 12,
        Error::Unavailable => 13,
        Error::Unsupported => 14,
        Error::Exhausted => 15,
    }
}
pub fn decode_error(raw: u32) -> Result<Option<Error>, Error> {
    Ok(match raw {
        0 => None,
        1 => Some(Error::Permission),
        2 => Some(Error::Invalid),
        3 => Some(Error::Conflict),
        4 => Some(Error::Quota),
        5 => Some(Error::Resource),
        6 => Some(Error::NotFound),
        7 => Some(Error::StaleInstance),
        8 => Some(Error::StaleEpoch),
        9 => Some(Error::InvalidEpoch),
        10 => Some(Error::Busy),
        11 => Some(Error::NotReady),
        12 => Some(Error::Closed),
        13 => Some(Error::Unavailable),
        14 => Some(Error::Unsupported),
        15 => Some(Error::Exhausted),
        _ => return Err(Error::Invalid),
    })
}
#[derive(Clone, Copy, Debug)]
pub struct Header {
    pub op: u16,
    pub error: Option<Error>,
    pub deadline: Deadline,
}
impl Header {
    pub fn decode(bytes: &[u8]) -> Result<(Self, &[u8]), Error> {
        if bytes.len() > HEADER_LEN + BODY_MAX {
            return Err(Error::Invalid);
        }
        let mut r = Reader::new(bytes);
        if r.u16()? != VERSION {
            return Err(Error::Unsupported);
        }
        let op = r.u16()?;
        let error = decode_error(r.u32()?)?;
        let len = r.u32()? as usize;
        if r.u32()? != 0 {
            return Err(Error::Invalid);
        }
        let deadline = Deadline {
            kind: r.u32()?,
            reserved: r.u32()?,
            at_ns: r.u64()?,
        };
        deadline.instant().map_err(|_| Error::Invalid)?;
        let body = r.bytes(len)?;
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
    pub fn encode(self, body: &[u8], out: &mut [u8]) -> Result<usize, Error> {
        if body.len() > BODY_MAX || out.len() < HEADER_LEN + body.len() {
            return Err(Error::Invalid);
        }
        self.deadline.instant().map_err(|_| Error::Invalid)?;
        let mut w = Writer::new(out);
        w.u16(VERSION);
        w.u16(self.op);
        w.u32(self.error.map_or(0, error_code));
        w.u32(body.len() as u32);
        w.u32(0);
        w.u32(self.deadline.kind);
        w.u32(self.deadline.reserved);
        w.u64(self.deadline.at_ns);
        w.bytes(body);
        Ok(w.written())
    }
}

fn read_optional_id(r: &mut Reader<'_>) -> Result<Option<DeviceId>, Error> {
    let incarnation = r.u64()?;
    let serial = r.u64()?;
    if incarnation == 0 && serial == 0 {
        Ok(None)
    } else {
        DeviceId::new(incarnation, serial).map(Some)
    }
}
fn write_optional_id(id: Option<DeviceId>, w: &mut Writer<'_>) {
    match id {
        Some(id) => id.encode(w),
        None => {
            w.u64(0);
            w.u64(0);
        }
    }
}
fn name<'a>(r: &mut Reader<'a>) -> Result<&'a str, Error> {
    let value = r.text32()?;
    liboal::path::validate_name(value).map_err(|_| Error::Invalid)?;
    Ok(value)
}
fn add_text(fixed: usize, a: &[u8], b: &[u8]) -> Result<usize, Error> {
    fixed
        .checked_add(a.len())
        .and_then(|n| n.checked_add(b.len()))
        .filter(|n| *n <= BODY_MAX)
        .ok_or(Error::Invalid)
}

#[derive(Clone, Copy, Debug)]
pub enum Command<'a> {
    IssueSource {
        scope: Scope,
        access: Access,
    },
    SealSource,
    Register {
        seq: u64,
        parent: Option<DeviceId>,
        name: &'a str,
        identity: &'a [u8],
        limits: Limits,
        claimable: bool,
        stop_requirement: crate::binding::StopRequirement,
    },
    WithdrawPublication {
        seq: u64,
    },
    QueryDevice {
        id: DeviceId,
    },
    Claim {
        reference: BindingRef,
    },
    StopClaim {
        reference: BindingRef,
    },
    QueryBinding,
    IssueUse,
    GetResource {
        name: &'a str,
    },
    StopBinding,
    ReportQuiescent,
    WithdrawDevice,
    Enumerate {
        scope: Scope,
        after: u64,
        end: u64,
    },
    WatchScope {
        scope: Scope,
    },
    WatchBinding,
    QueryWatch,
    AckWatch {
        sequence: u64,
    },
    CancelWatch,
    IssueRegistrar,
    InstallFunction {
        name: &'a str,
        description: &'a [u8],
    },
    OpenFunction {
        reference: BindingRef,
        name: &'a str,
    },
    InstallActivation {
        protocol: u64,
    },
    GetActivation {
        reference: BindingRef,
    },
    QuerySourceStop,
    QueryUse,
}
impl Command<'_> {
    pub fn op(&self) -> u16 {
        match self {
            Self::IssueSource { .. } => 1,
            Self::SealSource => 2,
            Self::Register { .. } => 3,
            Self::WithdrawPublication { .. } => 4,
            Self::QueryDevice { .. } => 5,
            Self::Claim { .. } => 6,
            Self::StopClaim { .. } => 7,
            Self::QueryBinding => 8,
            Self::IssueUse => 9,
            Self::GetResource { .. } => 10,
            Self::StopBinding => 11,
            Self::ReportQuiescent => 12,
            Self::WithdrawDevice => 13,
            Self::Enumerate { .. } => 14,
            Self::WatchScope { .. } => 15,
            Self::WatchBinding => 16,
            Self::QueryWatch => 17,
            Self::AckWatch { .. } => 18,
            Self::CancelWatch => 19,
            Self::IssueRegistrar => 20,
            Self::InstallFunction { .. } => 21,
            Self::OpenFunction { .. } => 22,
            Self::InstallActivation { .. } => 23,
            Self::GetActivation { .. } => 24,
            Self::QuerySourceStop => 25,
            Self::QueryUse => 26,
        }
    }
    pub fn handles(&self) -> usize {
        usize::from(matches!(
            self,
            Self::Register { .. } | Self::InstallFunction { .. } | Self::InstallActivation { .. }
        ))
    }
    pub fn encode(&self, deadline: Deadline, out: &mut [u8]) -> Result<usize, Error> {
        let len = match self {
            Self::IssueSource { .. } => 20,
            Self::Register { name, identity, .. } => {
                // 登记后的完整观察回复也必须能交付，不能只让较短的 Register 入站通过。
                add_text(DEVICE_FIXED_LEN, name.as_bytes(), identity)?;
                add_text(48, name.as_bytes(), identity)?
            }
            Self::WithdrawPublication { .. } => 8,
            Self::QueryDevice { .. } => 16,
            Self::Claim { .. } | Self::StopClaim { .. } => 24,
            Self::GetResource { name } => add_text(4, name.as_bytes(), &[])?,
            Self::Enumerate { .. } => 32,
            Self::WatchScope { .. } => 16,
            Self::AckWatch { .. } => 8,
            Self::InstallFunction { name, description } => {
                add_text(16, name.as_bytes(), description)?;
                add_text(8, name.as_bytes(), description)?
            }
            Self::OpenFunction { name, .. } => add_text(28, name.as_bytes(), &[])?,
            Self::InstallActivation { .. } => 8,
            Self::GetActivation { .. } => 24,
            _ => 0,
        };
        if out.len() < HEADER_LEN + len {
            return Err(Error::Invalid);
        }
        let mut body = [0; BODY_MAX];
        let mut w = Writer::new(&mut body[..len]);
        match *self {
            Self::IssueSource { scope, access } => {
                write_optional_id(
                    match scope {
                        Scope::Root => None,
                        Scope::Subtree(id) => Some(id),
                    },
                    &mut w,
                );
                w.u32(access.raw());
            }
            Self::Register {
                seq,
                parent,
                name: n,
                identity,
                limits,
                claimable,
                stop_requirement,
            } => {
                if seq == 0 {
                    return Err(Error::Invalid);
                }
                liboal::path::validate_name(n).map_err(|_| Error::Invalid)?;
                w.u64(seq);
                write_optional_id(parent, &mut w);
                w.u32(limits.entries);
                w.u32(limits.text_bytes);
                w.u32(u32::from(claimable));
                w.u32(stop_requirement as u32);
                w.bytes32(n.as_bytes());
                w.bytes32(identity);
            }
            Self::WithdrawPublication { seq } => {
                if seq == 0 {
                    return Err(Error::Invalid);
                }
                w.u64(seq);
            }
            Self::QueryDevice { id } => id.encode(&mut w),
            Self::Claim { reference } | Self::StopClaim { reference } => reference.encode(&mut w),
            Self::GetResource { name: n } => {
                liboal::path::validate_name(n).map_err(|_| Error::Invalid)?;
                w.bytes32(n.as_bytes());
            }
            Self::Enumerate { scope, after, end } => {
                write_optional_id(
                    match scope {
                        Scope::Root => None,
                        Scope::Subtree(id) => Some(id),
                    },
                    &mut w,
                );
                w.u64(after);
                w.u64(end);
            }
            Self::WatchScope { scope } => {
                write_optional_id(
                    match scope {
                        Scope::Root => None,
                        Scope::Subtree(id) => Some(id),
                    },
                    &mut w,
                );
            }
            Self::AckWatch { sequence } => {
                if sequence == 0 {
                    return Err(Error::Invalid);
                }
                w.u64(sequence);
            }
            Self::InstallFunction { name, description } => {
                liboal::path::validate_name(name).map_err(|_| Error::Invalid)?;
                w.bytes32(name.as_bytes());
                w.bytes32(description);
            }
            Self::OpenFunction { reference, name } => {
                liboal::path::validate_name(name).map_err(|_| Error::Invalid)?;
                reference.encode(&mut w);
                w.bytes32(name.as_bytes());
            }
            Self::InstallActivation { protocol } => {
                if protocol == 0 {
                    return Err(Error::Invalid);
                }
                w.u64(protocol);
            }
            Self::GetActivation { reference } => reference.encode(&mut w),
            _ => {}
        }
        Header {
            op: self.op(),
            error: None,
            deadline,
        }
        .encode(&body[..len], out)
    }
}
impl<'a> Command<'a> {
    /// handle_count 不包含已由 RequestContext 取走的回复槽。
    pub fn decode(bytes: &'a [u8], handle_count: usize) -> Result<(Header, Self), Error> {
        let (header, body) = Header::decode(bytes)?;
        if header.error.is_some() {
            return Err(Error::Invalid);
        }
        let mut r = Reader::new(body);
        let command = match header.op {
            1 => Self::IssueSource {
                scope: read_optional_id(&mut r)?.map_or(Scope::Root, Scope::Subtree),
                access: Access::from_raw(r.u32()?)?,
            },
            2 => Self::SealSource,
            3 => {
                let seq = r.u64()?;
                if seq == 0 {
                    return Err(Error::Invalid);
                }
                let parent = read_optional_id(&mut r)?;
                let limits = Limits {
                    entries: r.u32()?,
                    text_bytes: r.u32()?,
                };
                let claimable = match r.u32()? {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::Invalid),
                };
                Self::Register {
                    seq,
                    parent,
                    limits,
                    claimable,
                    stop_requirement: crate::binding::StopRequirement::from_raw(r.u32()?)?,
                    name: name(&mut r)?,
                    identity: r.bytes32()?,
                }
            }
            4 => {
                let seq = r.u64()?;
                if seq == 0 {
                    return Err(Error::Invalid);
                }
                Self::WithdrawPublication { seq }
            }
            5 => Self::QueryDevice {
                id: DeviceId::decode(&mut r)?,
            },
            6 => Self::Claim {
                reference: BindingRef::decode(&mut r)?,
            },
            7 => Self::StopClaim {
                reference: BindingRef::decode(&mut r)?,
            },
            8 => Self::QueryBinding,
            9 => Self::IssueUse,
            10 => Self::GetResource {
                name: name(&mut r)?,
            },
            11 => Self::StopBinding,
            12 => Self::ReportQuiescent,
            13 => Self::WithdrawDevice,
            14 => Self::Enumerate {
                scope: read_optional_id(&mut r)?.map_or(Scope::Root, Scope::Subtree),
                after: r.u64()?,
                end: r.u64()?,
            },
            15 => Self::WatchScope {
                scope: read_optional_id(&mut r)?.map_or(Scope::Root, Scope::Subtree),
            },
            16 => Self::WatchBinding,
            17 => Self::QueryWatch,
            18 => {
                let sequence = r.u64()?;
                if sequence == 0 {
                    return Err(Error::Invalid);
                }
                Self::AckWatch { sequence }
            }
            19 => Self::CancelWatch,
            20 => Self::IssueRegistrar,
            21 => Self::InstallFunction {
                name: name(&mut r)?,
                description: r.bytes32()?,
            },
            22 => Self::OpenFunction {
                reference: BindingRef::decode(&mut r)?,
                name: name(&mut r)?,
            },
            23 => {
                let protocol = r.u64()?;
                if protocol == 0 {
                    return Err(Error::Invalid);
                }
                Self::InstallActivation { protocol }
            }
            24 => Self::GetActivation {
                reference: BindingRef::decode(&mut r)?,
            },
            25 => Self::QuerySourceStop,
            26 => Self::QueryUse,
            _ => return Err(Error::Unsupported),
        };
        r.finish()?;
        if handle_count != command.handles() {
            return Err(Error::Invalid);
        }
        if let Self::Register { name, identity, .. } = command {
            add_text(DEVICE_FIXED_LEN, name.as_bytes(), identity)?;
        }
        if let Self::InstallFunction { name, description } = command {
            add_text(16, name.as_bytes(), description)?;
        }
        Ok((header, command))
    }
}

pub fn encode_scan(scan: crate::registry::Scan, out: &mut [u8]) -> Result<usize, Error> {
    if out.len() < 32 {
        return Err(Error::Invalid);
    }
    let mut w = Writer::new(out);
    w.u64(scan.end);
    w.u64(scan.next);
    write_optional_id(scan.device, &mut w);
    Ok(w.written())
}

pub fn encode_stop_claim(
    result: crate::binding::Stop,
    reference: BindingRef,
    out: &mut [u8],
) -> Result<usize, Error> {
    if out.len() < 4 + BindingRef::ENCODED_LEN {
        return Err(Error::Invalid);
    }
    match result {
        crate::binding::Stop::Closed => {
            out[..4].copy_from_slice(&0u32.to_le_bytes());
            reference.encode(&mut Writer::new(&mut out[4..]));
            Ok(4 + BindingRef::ENCODED_LEN)
        }
        crate::binding::Stop::Binding(snapshot) => {
            if snapshot.reference != reference || snapshot.phase != Phase::Stopping {
                return Err(Error::Invalid);
            }
            out[..4].copy_from_slice(&1u32.to_le_bytes());
            encode_snapshot(snapshot, &mut out[4..]).map(|used| used + 4)
        }
    }
}
pub fn decode_stop_claim(
    bytes: &[u8],
    reference: BindingRef,
    handles: usize,
) -> Result<crate::binding::Stop, Error> {
    let (header, body) = Header::decode(bytes)?;
    if header.op != 7 || handles != 0 {
        return Err(Error::Invalid);
    }
    if let Some(error) = header.error {
        if !body.is_empty() {
            return Err(Error::Invalid);
        }
        return Err(error);
    }
    let mut reader = Reader::new(body);
    let result = match reader.u32()? {
        0 => {
            if BindingRef::decode(&mut reader)? != reference {
                return Err(Error::StaleEpoch);
            }
            crate::binding::Stop::Closed
        }
        1 => {
            let snapshot = decode_snapshot(reader.bytes(reader.remaining())?)?;
            if snapshot.reference != reference {
                return Err(Error::StaleEpoch);
            }
            if snapshot.phase != Phase::Stopping {
                return Err(Error::Invalid);
            }
            crate::binding::Stop::Binding(snapshot)
        }
        _ => return Err(Error::Invalid),
    };
    reader.finish()?;
    Ok(result)
}
pub fn encode_source_stop(status: crate::registry::SourceStop) -> [u8; 4] {
    (match status {
        crate::registry::SourceStop::Open => 0u32,
        crate::registry::SourceStop::Stopping => 1,
        crate::registry::SourceStop::Closed => 2,
    })
    .to_le_bytes()
}
pub fn decode_source_stop(
    bytes: &[u8],
    handles: usize,
) -> Result<crate::registry::SourceStop, Error> {
    let (header, body) = Header::decode(bytes)?;
    if header.op != 25 || handles != 0 {
        return Err(Error::Invalid);
    }
    if let Some(error) = header.error {
        if !body.is_empty() {
            return Err(Error::Invalid);
        }
        return Err(error);
    }
    let mut reader = Reader::new(body);
    let status = match reader.u32()? {
        0 => crate::registry::SourceStop::Open,
        1 => crate::registry::SourceStop::Stopping,
        2 => crate::registry::SourceStop::Closed,
        _ => return Err(Error::Invalid),
    };
    reader.finish()?;
    Ok(status)
}

/// Watch 错误回复没有正文或能力；成功创建与已有控制回复分别校验两槽和零槽。
pub fn decode_watch(bytes: &[u8], op: u16, handles: usize) -> Result<crate::watch::Info, Error> {
    if !matches!(op, 15..=19) {
        return Err(Error::Invalid);
    }
    let (header, body) = Header::decode(bytes)?;
    if header.op != op {
        return Err(Error::Invalid);
    }
    if let Some(error) = header.error {
        if !body.is_empty() || handles != 0 {
            return Err(Error::Invalid);
        }
        return Err(error);
    }
    if handles != if matches!(op, 15 | 16) { 2 } else { 0 } {
        return Err(Error::Invalid);
    }
    crate::watch::Info::decode(body)
}
pub fn decode_scan(
    bytes: &[u8],
    after: u64,
    expected_end: u64,
) -> Result<crate::registry::Scan, Error> {
    let mut r = Reader::new(bytes);
    let end = r.u64()?;
    let next = r.u64()?;
    let device = read_optional_id(&mut r)?;
    r.finish()?;
    if (expected_end != 0 && end != expected_end)
        || next > end
        || after > next
        || device.is_some_and(|id| id.serial() != next || next <= after)
        || (device.is_none() && next != end)
    {
        return Err(Error::Invalid);
    }
    Ok(crate::registry::Scan { end, next, device })
}

pub fn encode_device(
    info: &crate::registry::DeviceInfo<'_>,
    out: &mut [u8],
) -> Result<usize, Error> {
    let len = add_text(DEVICE_FIXED_LEN, info.name.as_bytes(), info.identity)?;
    liboal::path::validate_name(info.name).map_err(|_| Error::Invalid)?;
    let (phase, subject) = match info.binding {
        None => (0, 0),
        Some(binding)
            if binding.reference.device() == info.id
                && Some(binding.reference.epoch()) == info.epoch =>
        {
            let phase = match binding.phase {
                Phase::Preparing => 1,
                Phase::ResourcesReady => 2,
                Phase::Stopping => 3,
                Phase::Released => return Err(Error::Invalid),
            };
            (phase, binding.subject.context())
        }
        Some(_) => return Err(Error::Invalid),
    };
    let mut body = [0; BODY_MAX];
    let mut w = Writer::new(&mut body[..len]);
    info.id.encode(&mut w);
    write_optional_id(info.parent, &mut w);
    w.u32(info.limits.entries);
    w.u32(info.limits.text_bytes);
    w.u32(u32::from(info.present));
    w.u64(info.epoch.unwrap_or(0));
    w.u32(phase);
    w.u64(subject);
    w.bytes32(info.name.as_bytes());
    w.bytes32(info.identity);
    Header {
        op: 5,
        error: None,
        deadline: Deadline::INFINITE,
    }
    .encode(&body[..len], out)
}

pub fn decode_device(
    bytes: &[u8],
    expected: DeviceId,
    handle_count: usize,
) -> Result<crate::registry::DeviceInfo<'_>, Error> {
    let (header, body) = Header::decode(bytes)?;
    if header.op != 5 || handle_count != 0 {
        return Err(Error::Invalid);
    }
    if let Some(error) = header.error {
        if !body.is_empty() {
            return Err(Error::Invalid);
        }
        return Err(error);
    }
    let mut r = Reader::new(body);
    let id = DeviceId::decode(&mut r)?;
    if id != expected {
        return Err(Error::StaleInstance);
    }
    let parent = read_optional_id(&mut r)?;
    let limits = Limits {
        entries: r.u32()?,
        text_bytes: r.u32()?,
    };
    let present = match r.u32()? {
        0 => false,
        1 => true,
        _ => return Err(Error::Invalid),
    };
    let epoch = r.u64()?;
    let phase = r.u32()?;
    let subject = r.u64()?;
    let binding = match (phase, subject) {
        (0, 0) => None,
        (1..=3, 1..) => Some(Snapshot {
            reference: BindingRef::new(id, epoch)?,
            subject: Subject::from_authenticated_context(subject)?,
            phase: match phase {
                1 => Phase::Preparing,
                2 => Phase::ResourcesReady,
                _ => Phase::Stopping,
            },
        }),
        _ => return Err(Error::Invalid),
    };
    let name = name(&mut r)?;
    let identity = r.bytes32()?;
    r.finish()?;
    Ok(crate::registry::DeviceInfo {
        id,
        parent,
        limits,
        present,
        epoch: (epoch != 0).then_some(epoch),
        binding,
        name,
        identity,
    })
}

pub fn encode_snapshot(snapshot: Snapshot, out: &mut [u8]) -> Result<usize, Error> {
    if out.len() < 36 {
        return Err(Error::Invalid);
    }
    let mut w = Writer::new(out);
    snapshot.reference.encode(&mut w);
    w.u64(snapshot.subject.context());
    w.u32(match snapshot.phase {
        Phase::Preparing => 1,
        Phase::ResourcesReady => 2,
        Phase::Stopping => 3,
        Phase::Released => 4,
    });
    Ok(w.written())
}
pub fn decode_snapshot(bytes: &[u8]) -> Result<Snapshot, Error> {
    let mut r = Reader::new(bytes);
    let reference = BindingRef::decode(&mut r)?;
    let subject = Subject::from_authenticated_context(r.u64()?)?;
    let phase = match r.u32()? {
        1 => Phase::Preparing,
        2 => Phase::ResourcesReady,
        3 => Phase::Stopping,
        4 => Phase::Released,
        _ => return Err(Error::Invalid),
    };
    r.finish()?;
    Ok(Snapshot {
        reference,
        subject,
        phase,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UseInfo {
    pub binding: Snapshot,
    pub stop_requirement: crate::binding::StopRequirement,
    pub access: crate::binding::UseAccess,
}
impl UseInfo {
    pub const BYTES: usize = 44;
    pub fn encode(self, out: &mut [u8]) -> Result<usize, Error> {
        if out.len() < Self::BYTES {
            return Err(Error::Invalid);
        }
        encode_snapshot(self.binding, &mut out[..36])?;
        let mut w = Writer::new(&mut out[36..Self::BYTES]);
        w.u32(self.stop_requirement as u32);
        w.u32(self.access.raw());
        Ok(Self::BYTES)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader::new(bytes);
        let binding = decode_snapshot(r.bytes(36)?)?;
        let stop_requirement = crate::binding::StopRequirement::from_raw(r.u32()?)?;
        let access = crate::binding::UseAccess::from_raw(r.u32()?)?;
        r.finish()?;
        Ok(Self {
            binding,
            stop_requirement,
            access,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProvisionCommand {
    Prepare(u64),
    Get {
        epoch: u64,
        index: u32,
    },
    Stop {
        epoch: u64,
        consumer_quiescent: bool,
    },
    Query(u64),
}
impl ProvisionCommand {
    pub fn op(self) -> u16 {
        match self {
            Self::Prepare(_) => 1,
            Self::Get { .. } => 2,
            Self::Stop { .. } => 3,
            Self::Query(_) => 4,
        }
    }
    pub fn epoch(self) -> u64 {
        match self {
            Self::Prepare(e) | Self::Query(e) => e,
            Self::Get { epoch, .. } | Self::Stop { epoch, .. } => epoch,
        }
    }
    pub fn encode(self, deadline: Deadline, out: &mut [u8]) -> Result<usize, Error> {
        if self.epoch() == 0 {
            return Err(Error::InvalidEpoch);
        }
        let mut body = [0; 12];
        let mut w = Writer::new(&mut body);
        w.u64(self.epoch());
        if let Self::Get { index, .. } = self {
            w.u32(index);
        }
        if let Self::Stop {
            consumer_quiescent, ..
        } = self
        {
            w.u32(u32::from(consumer_quiescent));
        }
        let len = w.written();
        Header {
            op: self.op(),
            error: None,
            deadline,
        }
        .encode(&body[..len], out)
    }
    pub fn decode(bytes: &[u8], handle_count: usize) -> Result<(Header, Self), Error> {
        let (header, body) = Header::decode(bytes)?;
        if header.error.is_some() || handle_count != usize::from(header.op == 1) {
            return Err(Error::Invalid);
        }
        let mut r = Reader::new(body);
        let epoch = r.u64()?;
        if epoch == 0 {
            return Err(Error::InvalidEpoch);
        }
        let command = match header.op {
            1 => Self::Prepare(epoch),
            2 => Self::Get {
                epoch,
                index: r.u32()?,
            },
            3 => Self::Stop {
                epoch,
                consumer_quiescent: match r.u32()? {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::Invalid),
                },
            },
            4 => Self::Query(epoch),
            _ => return Err(Error::Unsupported),
        };
        r.finish()?;
        Ok((header, command))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Progress {
    Preparing,
    Prepared(u32),
    Stopping,
    Closed,
    Rejected(Error),
    Failed(Error),
}
impl Progress {
    fn validate(self, command: ProvisionCommand) -> Result<Self, Error> {
        if command.epoch() == 0 {
            return Err(Error::InvalidEpoch);
        }
        match (command, self) {
            (ProvisionCommand::Get { .. }, _) => Err(Error::Invalid),
            (ProvisionCommand::Prepare(_) | ProvisionCommand::Query(_), _) => Ok(self),
            (_, Self::Stopping | Self::Closed | Self::Failed(_)) => Ok(self),
            _ => Err(Error::Invalid),
        }
    }
    pub fn encode(self, command: ProvisionCommand, out: &mut [u8]) -> Result<usize, Error> {
        self.validate(command)?;
        let mut body = [0; 16];
        let mut w = Writer::new(&mut body);
        w.u64(command.epoch());
        let (phase, value) = match self {
            Self::Preparing => (1, 0),
            Self::Prepared(n) => (2, n),
            Self::Stopping => (3, 0),
            Self::Closed => (4, 0),
            Self::Rejected(e) => (5, error_code(e)),
            Self::Failed(e) => (6, error_code(e)),
        };
        w.u32(phase);
        w.u32(value);
        Header {
            op: command.op(),
            error: None,
            deadline: Deadline::INFINITE,
        }
        .encode(&body, out)
    }
    pub fn decode(
        bytes: &[u8],
        command: ProvisionCommand,
        handle_count: usize,
    ) -> Result<Self, Error> {
        let (header, body) = Header::decode(bytes)?;
        if header.op != command.op() || matches!(command, ProvisionCommand::Get { .. }) {
            return Err(Error::Invalid);
        }
        if let Some(error) = header.error {
            if !body.is_empty() || handle_count != 0 {
                return Err(Error::Invalid);
            }
            return Err(error);
        }
        if header.op != command.op()
            || handle_count != 0
            || matches!(command, ProvisionCommand::Get { .. })
        {
            return Err(Error::Invalid);
        }
        let mut r = Reader::new(body);
        if r.u64()? != command.epoch() {
            return Err(Error::StaleEpoch);
        }
        let phase = r.u32()?;
        let value = r.u32()?;
        r.finish()?;
        let progress = match (phase, value) {
            (1, 0) => Ok(Self::Preparing),
            (2, n) => Ok(Self::Prepared(n)),
            (3, 0) => Ok(Self::Stopping),
            (4, 0) => Ok(Self::Closed),
            (5, n) => Ok(Self::Rejected(decode_error(n)?.ok_or(Error::Invalid)?)),
            (6, n) => Ok(Self::Failed(decode_error(n)?.ok_or(Error::Invalid)?)),
            _ => Err(Error::Invalid),
        }?;
        progress.validate(command)
    }
}
/// Get 回复把轮次放在资源表项之前；Claim 收集端按实际较大的回复头预留。
pub const RESOURCE_HEADER_LEN: usize = HEADER_LEN + 8;
pub fn decode_resource(
    bytes: &[u8],
    epoch: u64,
    index: u32,
    handle_count: usize,
) -> Result<Entry<'_>, Error> {
    let (header, body) = Header::decode(bytes)?;
    if header.op != 2 {
        return Err(Error::Invalid);
    }
    if let Some(error) = header.error {
        if !body.is_empty() || handle_count != 0 {
            return Err(Error::Invalid);
        }
        return Err(error);
    }
    let mut r = Reader::new(body);
    if r.u64()? != epoch {
        return Err(Error::StaleEpoch);
    }
    let entry = Entry::decode(r.bytes(r.remaining())?, RESOURCE_HEADER_LEN, handle_count)?;
    if entry.index != index {
        return Err(Error::Conflict);
    }
    Ok(entry)
}
