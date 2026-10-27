//! 驱动激活 wire。观察只交状态/通知，不能经观察入口取得硬件或驱动控制。
use erhino_shared::{message::PAYLOAD_MAX, time::Deadline};
use libdevice::{Error, identity::BindingRef};
use librpc::bytes::{Reader, Writer};

pub const ID: u64 = u64::from_le_bytes(*b"DRVACT01");
pub const OBSERVATION_ID: u64 = u64::from_le_bytes(*b"DRVOBS01");
pub const BOOT_READY: u64 = u64::from_le_bytes(*b"DRVBOOT1");
pub const MANAGER_ID: u64 = u64::from_le_bytes(*b"DRVMGR01");
/// 非幂等的显式选择重置；成功不保证下一Claim可用，Unknown不得自动重发。
pub const MANAGER_RETRY: u16 = 3;

pub fn encode_manager_retry(
    device: libdevice::identity::DeviceId,
    deadline: Deadline,
    out: &mut [u8],
) -> Result<usize, Error> {
    let mut body = [0; libdevice::identity::DeviceId::ENCODED_LEN];
    device.encode(&mut Writer::new(&mut body));
    Header {
        op: MANAGER_RETRY,
        error: None,
        deadline,
    }
    .encode(&body, out)
}
pub const VERSION: u16 = 1;
pub const HEADER_LEN: usize = 32;
pub const BODY_MAX: usize = PAYLOAD_MAX - librpc::PREFIX_LEN - HEADER_LEN;

#[derive(Clone, Copy)]
pub struct Header {
    pub op: u16,
    pub error: Option<Error>,
    pub deadline: Deadline,
}
impl Header {
    pub fn encode(self, body: &[u8], out: &mut [u8]) -> Result<usize, Error> {
        if body.len() > BODY_MAX || out.len() < HEADER_LEN + body.len() {
            return Err(Error::Invalid);
        }
        self.deadline.instant().map_err(|_| Error::Invalid)?;
        let mut writer = Writer::new(out);
        writer.u16(VERSION);
        writer.u16(self.op);
        writer.u32(self.error.map_or(0, libdevice::protocol::error_code));
        writer.u32(body.len() as u32);
        writer.u32(0);
        writer.u32(self.deadline.kind);
        writer.u32(self.deadline.reserved);
        writer.u64(self.deadline.at_ns);
        writer.bytes(body);
        Ok(writer.written())
    }
    pub fn decode(bytes: &[u8]) -> Result<(Self, &[u8]), Error> {
        let mut reader = Reader::new(bytes);
        if reader.u16()? != VERSION {
            return Err(Error::Unsupported);
        }
        let op = reader.u16()?;
        let error = libdevice::protocol::decode_error(reader.u32()?)?;
        let length = reader.u32()? as usize;
        if reader.u32()? != 0 || length > BODY_MAX {
            return Err(Error::Invalid);
        }
        let deadline = Deadline {
            kind: reader.u32()?,
            reserved: reader.u32()?,
            at_ns: reader.u64()?,
        };
        deadline.instant().map_err(|_| Error::Invalid)?;
        let body = reader.bytes(length)?;
        reader.finish()?;
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
#[derive(Clone, Copy, Debug)]
pub enum Command<'a> {
    Activate {
        reference: BindingRef,
        identity: &'a [u8],
        registrar: bool,
    },
    /// 始终随附原 BindingUse，Stop 先于 Activate 到达时也能认证并观察原轮。
    Stop { reference: BindingRef },
}
impl Command<'_> {
    pub fn op(self) -> u16 {
        match self {
            Self::Activate { .. } => 1,
            Self::Stop { .. } => 2,
        }
    }
    pub fn reference(self) -> BindingRef {
        match self {
            Self::Activate { reference, .. } | Self::Stop { reference } => reference,
        }
    }
    pub fn handles(self) -> usize {
        match self {
            Self::Activate {
                registrar: true, ..
            } => 2,
            _ => 1,
        }
    }
    pub fn encode(self, deadline: Deadline, out: &mut [u8]) -> Result<usize, Error> {
        let mut body = [0; BODY_MAX];
        let mut writer = Writer::new(&mut body);
        self.reference().encode(&mut writer);
        if let Self::Activate {
            identity,
            registrar,
            ..
        } = self
        {
            if identity.is_empty() || identity.len() > BODY_MAX - BindingRef::ENCODED_LEN - 8 {
                return Err(Error::Invalid);
            }
            writer.u32(u32::from(registrar));
            writer.bytes32(identity);
        }
        let used = writer.written();
        Header {
            op: self.op(),
            error: None,
            deadline,
        }
        .encode(&body[..used], out)
    }
    pub fn decode(bytes: &[u8], handles: usize) -> Result<(Header, Command<'_>), Error> {
        let (header, body) = Header::decode(bytes)?;
        if header.error.is_some() {
            return Err(Error::Invalid);
        }
        let mut reader = Reader::new(body);
        let reference = BindingRef::decode(&mut reader)?;
        let command = match header.op {
            1 => {
                let registrar = match reader.u32()? {
                    0 => false,
                    1 => true,
                    _ => return Err(Error::Invalid),
                };
                let identity = reader.bytes32()?;
                if identity.is_empty() {
                    return Err(Error::Invalid);
                }
                Command::Activate {
                    reference,
                    identity,
                    registrar,
                }
            }
            2 => Command::Stop { reference },
            _ => return Err(Error::Unsupported),
        };
        reader.finish()?;
        if handles != command.handles() {
            return Err(Error::Invalid);
        }
        Ok((header, command))
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Starting,
    Ready,
    Stopping,
    Terminal,
}
impl Phase {
    pub fn code(self) -> u32 {
        match self {
            Self::Starting => 1,
            Self::Ready => 2,
            Self::Stopping => 3,
            Self::Terminal => 4,
        }
    }
    pub fn decode(code: u32) -> Result<Self, Error> {
        match code {
            1 => Ok(Self::Starting),
            2 => Ok(Self::Ready),
            3 => Ok(Self::Stopping),
            4 => Ok(Self::Terminal),
            _ => Err(Error::Invalid),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub reference: BindingRef,
    pub sequence: u64,
    pub pending: bool,
    pub phase: Phase,
    pub error: Option<Error>,
    /// 仅实际可信装配允许转交该事实；Terminal 本身不代表硬件静止。
    pub consumer_stopped: bool,
}
impl Snapshot {
    pub const BYTES: usize = BindingRef::ENCODED_LEN + 24;
    pub fn encode(self, out: &mut [u8]) -> Result<usize, Error> {
        if out.len() < Self::BYTES || self.sequence == 0 {
            return Err(Error::Invalid);
        }
        let mut writer = Writer::new(out);
        self.reference.encode(&mut writer);
        writer.u64(self.sequence);
        writer.u32(self.phase.code());
        writer.u32(self.error.map_or(0, libdevice::protocol::error_code));
        writer.u32(u32::from(self.pending) | (u32::from(self.consumer_stopped) << 1));
        writer.u32(0);
        Ok(writer.written())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut reader = Reader::new(bytes);
        let reference = BindingRef::decode(&mut reader)?;
        let sequence = reader.u64()?;
        let phase = Phase::decode(reader.u32()?)?;
        let error = libdevice::protocol::decode_error(reader.u32()?)?;
        let flags = reader.u32()?;
        if sequence == 0 || flags & !3 != 0 || reader.u32()? != 0 {
            return Err(Error::Invalid);
        }
        reader.finish()?;
        if flags & 2 != 0 && !matches!(phase, Phase::Stopping | Phase::Terminal) {
            return Err(Error::Invalid);
        }
        Ok(Self {
            reference,
            sequence,
            pending: flags & 1 != 0,
            phase,
            error,
            consumer_stopped: flags & 2 != 0,
        })
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Observe {
    Watch,
    Query,
    Ack(u64),
}

/// Activate/Stop 的同一回执：状态、具名功能描述，能力另占观察入口及 Ready 功能根槽。
pub fn encode_receipt(
    snapshot: Snapshot,
    name: &str,
    description: &[u8],
    out: &mut [u8],
) -> Result<usize, Error> {
    if snapshot.phase == Phase::Ready {
        liboal::path::validate_name(name).map_err(|_| Error::Invalid)?;
    } else if !name.is_empty() || !description.is_empty() {
        return Err(Error::Invalid);
    }
    let length = Snapshot::BYTES
        .checked_add(8)
        .and_then(|n| n.checked_add(name.len()))
        .and_then(|n| n.checked_add(description.len()))
        .ok_or(Error::Invalid)?;
    if length > out.len() || length > BODY_MAX {
        return Err(Error::Invalid);
    }
    snapshot.encode(&mut out[..Snapshot::BYTES])?;
    let mut writer = Writer::new(&mut out[Snapshot::BYTES..]);
    writer.bytes32(name.as_bytes());
    writer.bytes32(description);
    Ok(length)
}
impl Observe {
    pub fn op(self) -> u16 {
        match self {
            Self::Watch => 1,
            Self::Query => 2,
            Self::Ack(_) => 3,
        }
    }
    pub fn encode(self, deadline: Deadline, out: &mut [u8]) -> Result<usize, Error> {
        let body = match self {
            Self::Ack(sequence) => Some(sequence.to_le_bytes()),
            _ => None,
        };
        Header {
            op: self.op(),
            error: None,
            deadline,
        }
        .encode(body.as_ref().map_or(&[], |bytes| bytes.as_slice()), out)
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
            3 => Self::Ack(reader.u64()?),
            _ => return Err(Error::Unsupported),
        };
        reader.finish()?;
        Ok((header, command))
    }
}
