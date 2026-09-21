//! 统一属性值编码；非递归验证所有嵌套值、名字及能力槽。

use crate::resource::FalResource;
use crate::{
    authority::FalRights,
    bytes::{Reader, Writer},
};
use alloc::sync::Arc;
use alloc::vec::Vec;
use erhino_shared::{
    call::SystemCallError,
    message::{MESSAGE_HANDLE_MAX, PAYLOAD_MAX},
    object::{HandleDescription, Rights},
};
use libsrv::budget::{Account, Charge};

pub const HEADER_LEN: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Tag {
    Integer = 1,
    Decimal = 2,
    String = 3,
    Blob = 4,
    Handle = 5,
    Array = 6,
    Record = 7,
}
impl Tag {
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            1 => Some(Self::Integer),
            2 => Some(Self::Decimal),
            3 => Some(Self::String),
            4 => Some(Self::Blob),
            5 => Some(Self::Handle),
            6 => Some(Self::Array),
            7 => Some(Self::Record),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Protocol {
    Opaque = 0,
    Directory = 1,
    Mailbox = 2,
    Notification = 3,
}
impl Protocol {
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Opaque),
            1 => Some(Self::Directory),
            2 => Some(Self::Mailbox),
            3 => Some(Self::Notification),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ExportMode {
    Repeatable = 0,
    Affine = 1,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExportPolicy {
    pub protocol: Protocol,
    pub mode: ExportMode,
    pub transport: Rights,
    pub fal_ceiling: FalRights,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandleField {
    pub slot: usize,
    pub slot_offset: usize,
    pub policy: ExportPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueError {
    Encoding,
    Budget,
    DuplicateField,
    HandleSlots,
    Allocation,
    Capability(SystemCallError),
    Affine,
    Unsupported,
}

pub trait Capability: Sized {
    fn description(&self) -> Result<HandleDescription, SystemCallError>;
    fn duplicate(&self, rights: Rights) -> Result<Self, SystemCallError>;
    fn close(self) -> Result<(), (Self, SystemCallError)>;
}

#[cfg(target_arch = "riscv64")]
impl Capability for rinlib::ipc::capability::Capability {
    fn description(&self) -> Result<HandleDescription, SystemCallError> {
        self.description()
    }
    fn duplicate(&self, rights: Rights) -> Result<Self, SystemCallError> {
        self.duplicate(rights)
    }
    fn close(self) -> Result<(), (Self, SystemCallError)> {
        self.close()
    }
}

pub struct Field<'a> {
    pub name: &'a str,
    pub value: Value<'a>,
}
pub enum Value<'a> {
    Integer(i64),
    Decimal(f64),
    Str(&'a str),
    Blob(&'a [u8]),
    Handle {
        slot: u16,
        policy: ExportPolicy,
    },
    Array {
        element: Tag,
        values: &'a [Value<'a>],
    },
    Record(&'a [Field<'a>]),
}

impl Value<'_> {
    pub fn tag(&self) -> Tag {
        match self {
            Self::Integer(_) => Tag::Integer,
            Self::Decimal(_) => Tag::Decimal,
            Self::Str(_) => Tag::String,
            Self::Blob(_) => Tag::Blob,
            Self::Handle { .. } => Tag::Handle,
            Self::Array { .. } => Tag::Array,
            Self::Record(_) => Tag::Record,
        }
    }
    pub fn encoded_len(&self) -> Option<usize> {
        let body = match self {
            Self::Integer(_) | Self::Decimal(_) => 8,
            Self::Str(value) => value.len(),
            Self::Blob(value) => value.len(),
            Self::Handle { .. } => 24,
            Self::Array { values, .. } => values
                .iter()
                .try_fold(8usize, |len, value| len.checked_add(value.encoded_len()?))?,
            Self::Record(fields) => fields.iter().try_fold(8usize, |len, field| {
                len.checked_add(8)?
                    .checked_add(field.name.len())?
                    .checked_add(field.value.encoded_len()?)
            })?,
        };
        HEADER_LEN.checked_add(body)
    }
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, ValueError> {
        let len = self
            .encoded_len()
            .filter(|len| *len <= PAYLOAD_MAX && *len <= out.len())
            .ok_or(ValueError::Budget)?;
        let mut writer = Writer::new(&mut out[..len]);
        self.write(&mut writer);
        Ok(len)
    }
    fn write(&self, writer: &mut Writer<'_>) {
        let len = self.encoded_len().expect("validated value length overflow");
        writer.u32(self.tag() as u32);
        writer.u32(match self {
            Self::Array { element, .. } => *element as u32,
            _ => 0,
        });
        writer.u32((len - HEADER_LEN) as u32);
        writer.u32(0);
        match self {
            Self::Integer(value) => writer.u64(*value as u64),
            Self::Decimal(value) => writer.u64(value.to_bits()),
            Self::Str(value) => writer.bytes(value.as_bytes()),
            Self::Blob(value) => writer.bytes(value),
            Self::Handle { slot, policy } => {
                writer.u16(*slot);
                writer.u16(policy.mode as u16);
                writer.u32(policy.protocol as u32);
                writer.u64(policy.transport.raw());
                writer.u32(policy.fal_ceiling.raw());
                writer.u32(0);
            }
            Self::Array { values, .. } => {
                writer.u32(values.len() as u32);
                writer.u32(0);
                for value in *values {
                    value.write(writer);
                }
            }
            Self::Record(fields) => {
                writer.u32(fields.len() as u32);
                writer.u32(0);
                for field in *fields {
                    writer.u16(field.name.len() as u16);
                    writer.u16(0);
                    writer.u32(
                        field
                            .value
                            .encoded_len()
                            .expect("validated field length overflow")
                            as u32,
                    );
                    writer.bytes(field.name.as_bytes());
                    field.value.write(writer);
                }
            }
        }
    }
}

struct Span {
    start: usize,
    end: usize,
    expected: Option<Tag>,
}

fn error<T>(result: crate::bytes::DecodeResult<T>) -> Result<T, ValueError> {
    result.map_err(|_| ValueError::Encoding)
}

/// slot_base 是本消息的业务起点；所有传入能力都必须恰好引用一次。
pub fn validate(
    bytes: &[u8],
    slot_base: usize,
    handle_count: usize,
    byte_budget: usize,
) -> Result<Vec<HandleField>, ValueError> {
    if bytes.len() > byte_budget || byte_budget > PAYLOAD_MAX || handle_count > MESSAGE_HANDLE_MAX {
        return Err(ValueError::Budget);
    }
    let maximum = bytes.len() / HEADER_LEN;
    if maximum == 0 {
        return Err(ValueError::Encoding);
    }
    let mut spans = Vec::new();
    let mut names = Vec::new();
    let mut handles = Vec::new();
    spans
        .try_reserve_exact(maximum)
        .map_err(|_| ValueError::Allocation)?;
    names
        .try_reserve_exact(maximum)
        .map_err(|_| ValueError::Allocation)?;
    handles
        .try_reserve_exact(handle_count)
        .map_err(|_| ValueError::Allocation)?;
    spans.push(Span {
        start: 0,
        end: bytes.len(),
        expected: None,
    });
    let mut visited = 0;
    let mut used = 0usize;
    while let Some(span) = spans.pop() {
        visited += 1;
        if visited > maximum {
            return Err(ValueError::Budget);
        }
        let mut reader = Reader::new(&bytes[span.start..span.end]);
        let tag = Tag::from_raw(error(reader.u32())?).ok_or(ValueError::Encoding)?;
        if span.expected.is_some_and(|expected| expected != tag) {
            return Err(ValueError::Encoding);
        }
        let aux = error(reader.u32())?;
        let length = error(reader.u32())? as usize;
        if error(reader.u32())? != 0 || length != reader.remaining() {
            return Err(ValueError::Encoding);
        }
        if tag != Tag::Array && aux != 0 {
            return Err(ValueError::Encoding);
        }
        match tag {
            Tag::Integer | Tag::Decimal => {
                if length != 8 {
                    return Err(ValueError::Encoding);
                }
                error(reader.u64())?;
            }
            Tag::String => {
                core::str::from_utf8(error(reader.bytes(length))?)
                    .map_err(|_| ValueError::Encoding)?;
            }
            Tag::Blob => {
                error(reader.bytes(length))?;
            }
            Tag::Handle => {
                if length != 24 {
                    return Err(ValueError::Encoding);
                }
                let offset = span.start + HEADER_LEN;
                let slot = error(reader.u16())? as usize;
                let mode = match error(reader.u16())? {
                    0 => ExportMode::Repeatable,
                    1 => ExportMode::Affine,
                    _ => return Err(ValueError::Encoding),
                };
                let protocol =
                    Protocol::from_raw(error(reader.u32())?).ok_or(ValueError::Encoding)?;
                let transport = Rights::from_raw(error(reader.u64())?);
                let fal_ceiling =
                    FalRights::from_raw(error(reader.u32())?).ok_or(ValueError::Encoding)?;
                if error(reader.u32())? != 0
                    || (protocol != Protocol::Directory && fal_ceiling != FalRights::NONE)
                {
                    return Err(ValueError::Encoding);
                }
                if !transport.is_known() || !transport.contains(Rights::TRANSIT) {
                    return Err(ValueError::Encoding);
                }
                let index = slot
                    .checked_sub(slot_base)
                    .filter(|index| *index < handle_count)
                    .ok_or(ValueError::HandleSlots)?;
                if used & (1 << index) != 0 {
                    return Err(ValueError::HandleSlots);
                }
                used |= 1 << index;
                handles.push(HandleField {
                    slot: index,
                    slot_offset: offset,
                    policy: ExportPolicy {
                        protocol,
                        mode,
                        transport,
                        fal_ceiling,
                    },
                });
            }
            Tag::Array | Tag::Record => {
                let count = error(reader.u32())? as usize;
                if error(reader.u32())? != 0
                    || count > maximum
                    || count > reader.remaining() / HEADER_LEN
                {
                    return Err(ValueError::Budget);
                }
                let expected = if tag == Tag::Array {
                    Some(Tag::from_raw(aux).ok_or(ValueError::Encoding)?)
                } else {
                    None
                };
                for _ in 0..count {
                    let field_length = if tag == Tag::Record {
                        let name_length = error(reader.u16())? as usize;
                        if error(reader.u16())? != 0 {
                            return Err(ValueError::Encoding);
                        }
                        let value_length = error(reader.u32())? as usize;
                        let name = core::str::from_utf8(error(reader.bytes(name_length))?)
                            .map_err(|_| ValueError::Encoding)?;
                        if name.is_empty() || name.bytes().any(|byte| byte == 0) {
                            return Err(ValueError::Encoding);
                        }
                        names.push((span.start, name));
                        value_length
                    } else {
                        if reader.remaining() < HEADER_LEN {
                            return Err(ValueError::Encoding);
                        }
                        let start = span.start + reader.consumed();
                        let mut header = Reader::new(&bytes[start..start + HEADER_LEN]);
                        error(header.u32())?;
                        error(header.u32())?;
                        HEADER_LEN
                            .checked_add(error(header.u32())? as usize)
                            .ok_or(ValueError::Encoding)?
                    };
                    let start = span.start + reader.consumed();
                    error(reader.bytes(field_length))?;
                    if spans.len() == maximum {
                        return Err(ValueError::Budget);
                    }
                    spans.push(Span {
                        start,
                        end: start + field_length,
                        expected,
                    });
                }
            }
        }
        error(reader.finish())?;
    }
    names.sort_unstable();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ValueError::DuplicateField);
    }
    if used != (1usize << handle_count) - 1 {
        return Err(ValueError::HandleSlots);
    }
    handles.sort_unstable_by_key(|field| field.slot);
    Ok(handles)
}

pub struct StoredHandle<C> {
    pub owner: C,
    pub policy: ExportPolicy,
}
pub struct StoredValue<C> {
    pub bytes: Vec<u8>,
    pub handles: Vec<StoredHandle<C>>,
    _charge: Charge<FalResource>,
}

pub struct StoreFailure<C> {
    pub error: ValueError,
    pub handles: Vec<C>,
}

/// 从属性中取出的完整值；能力 owner 仍由调用方决定交付或恢复。
pub struct TakenValue<C> {
    pub bytes: Vec<u8>,
    pub handles: Vec<StoredHandle<C>>,
}

pub type ExportedValue<C> = (Vec<u8>, Vec<(C, Rights)>);

impl<C: Capability> StoredValue<C> {
    pub fn prepare(
        bytes: &[u8],
        owners: Vec<C>,
        slot_base: usize,
        byte_budget: usize,
        account: &Arc<Account<FalResource>>,
    ) -> Result<Self, StoreFailure<C>> {
        let prepare = (|| {
            let fields = validate(bytes, slot_base, owners.len(), byte_budget)?;
            for field in &fields {
                let description = owners[field.slot]
                    .description()
                    .map_err(ValueError::Capability)?;
                if !field.policy.transport.is_subset_of(description.rights) {
                    return Err(ValueError::Capability(SystemCallError::RightsDenied));
                }
                if field.policy.mode == ExportMode::Repeatable
                    && !description
                        .rights
                        .contains(Rights::DUPLICATE | Rights::TRANSIT)
                {
                    return Err(ValueError::Capability(SystemCallError::RightsDenied));
                }
                use erhino_shared::object::HandleRole;
                let valid = match field.policy.protocol {
                    Protocol::Opaque => true,
                    Protocol::Directory => {
                        description.role == HandleRole::MailboxSender as u32
                            && field
                                .policy
                                .transport
                                .contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
                    }
                    Protocol::Mailbox => {
                        description.role == HandleRole::MailboxSender as u32
                            || (description.role == HandleRole::MailboxSenderOnce as u32
                                && field.policy.mode == ExportMode::Affine)
                    }
                    Protocol::Notification => {
                        description.role == HandleRole::NotificationSignaler as u32
                    }
                };
                if !valid {
                    return Err(ValueError::Capability(SystemCallError::WrongObjectType));
                }
            }
            let allocation = bytes
                .len()
                .checked_add(owners.len() * core::mem::size_of::<StoredHandle<C>>())
                .ok_or(ValueError::Budget)?;
            let charge = account
                .acquire(FalResource::Bytes, allocation)
                .map_err(ValueError::Capability)?;
            let mut stored = Vec::new();
            stored
                .try_reserve_exact(bytes.len())
                .map_err(|_| ValueError::Allocation)?;
            stored.extend_from_slice(bytes);
            let mut handles = Vec::new();
            handles
                .try_reserve_exact(owners.len())
                .map_err(|_| ValueError::Allocation)?;
            for field in &fields {
                stored[field.slot_offset..field.slot_offset + 2]
                    .copy_from_slice(&(field.slot as u16).to_le_bytes());
            }
            Ok((fields, stored, handles, charge))
        })();
        let (fields, bytes, mut handles, charge) = match prepare {
            Ok(prepared) => prepared,
            Err(error) => {
                return Err(StoreFailure {
                    error,
                    handles: owners,
                });
            }
        };
        for (owner, field) in owners.into_iter().zip(fields) {
            handles.push(StoredHandle {
                owner,
                policy: field.policy,
            });
        }
        Ok(Self {
            bytes,
            handles,
            _charge: charge,
        })
    }

    pub fn retire_step(&mut self, budget: usize) -> Result<bool, SystemCallError> {
        for _ in 0..budget {
            let Some(handle) = self.handles.pop() else {
                self.bytes.clear();
                return Ok(true);
            };
            match handle.owner.close() {
                Ok(()) => (),
                Err((owner, error)) => {
                    self.handles.push(StoredHandle {
                        owner,
                        policy: handle.policy,
                    });
                    return Err(error);
                }
            }
        }
        Ok(self.handles.is_empty())
    }

    pub fn has_affine(&self) -> bool {
        self.handles
            .iter()
            .any(|handle| handle.policy.mode == ExportMode::Affine)
    }

    /// 复制可重复导出的能力，并把编码中的槽位重写为回复槽位。
    ///
    /// DirectoryGrant 不允许走普通 Duplicate；它必须由 provider 的
    /// Derive 路径产生新的授权。affine 值只能走显式 Take。
    pub fn duplicate_for_reply(&self) -> Result<ExportedValue<C>, ValueError> {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(self.bytes.len())
            .map_err(|_| ValueError::Allocation)?;
        bytes.extend_from_slice(&self.bytes);
        let fields = validate(&bytes, 0, self.handles.len(), bytes.len())
            .map_err(|_| ValueError::Encoding)?;
        let mut owners = Vec::new();
        owners
            .try_reserve_exact(self.handles.len())
            .map_err(|_| ValueError::Allocation)?;
        for (slot, (handle, field)) in self.handles.iter().zip(fields).enumerate() {
            if handle.policy.mode == ExportMode::Affine {
                return Err(ValueError::Affine);
            }
            if handle.policy.protocol == Protocol::Directory {
                return Err(ValueError::Unsupported);
            }
            let owner = handle
                .owner
                .duplicate(handle.policy.transport)
                .map_err(ValueError::Capability)?;
            bytes[field.slot_offset..field.slot_offset + 2]
                .copy_from_slice(&(slot as u16).to_le_bytes());
            owners.push((owner, handle.policy.transport));
        }
        Ok((bytes, owners))
    }

    /// 线性化 affine Take：存储值立即变为空 Blob，原始值与 owner 交给操作任务。
    /// 原有额度仍由这个 StoredValue 持有，避免在提交前重新申请资源。
    pub fn take(&mut self, mut empty: Vec<u8>) -> Result<TakenValue<C>, ValueError> {
        if !self.has_affine() {
            return Err(ValueError::Affine);
        }
        if empty.capacity() < HEADER_LEN {
            return Err(ValueError::Allocation);
        }
        empty.extend_from_slice(&[Tag::Blob as u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let bytes = core::mem::replace(&mut self.bytes, empty);
        let handles = core::mem::take(&mut self.handles);
        Ok(TakenValue { bytes, handles })
    }

    pub fn restore(&mut self, taken: TakenValue<C>) {
        self.bytes = taken.bytes;
        self.handles = taken.handles;
    }

    pub fn commit_take(&mut self) {
        assert!(
            self.handles.is_empty(),
            "committed take retained capability owners"
        );
        self._charge.shrink_to(self.bytes.len());
    }
}

/// 将属性值中的业务槽位从请求布局重写为回复布局。
pub fn rebase_slots(
    bytes: &[u8],
    handle_count: usize,
    from_base: usize,
    to_base: usize,
) -> Result<Vec<u8>, ValueError> {
    let fields = validate(bytes, from_base, handle_count, bytes.len())?;
    let mut rebased = bytes.to_vec();
    for (slot, field) in fields.into_iter().enumerate() {
        let target = slot
            .checked_add(to_base)
            .filter(|slot| *slot <= u16::MAX as usize)
            .ok_or(ValueError::HandleSlots)?;
        rebased[field.slot_offset..field.slot_offset + 2]
            .copy_from_slice(&(target as u16).to_le_bytes());
    }
    Ok(rebased)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::sync::Arc;
    use erhino_shared::object::{HandleDescription, HandleRole};
    use libsrv::budget::{Budget, Taxonomy};

    #[derive(Clone)]
    struct TestCapability {
        id: u64,
        role: HandleRole,
        rights: Rights,
    }

    impl Capability for TestCapability {
        fn description(&self) -> Result<HandleDescription, SystemCallError> {
            Ok(HandleDescription {
                object_id: self.id,
                related_object_id: 1,
                kind: 0,
                role: self.role as u32,
                rights: self.rights,
                badge: 0,
                reserved: 0,
            })
        }

        fn duplicate(&self, rights: Rights) -> Result<Self, SystemCallError> {
            if !rights.is_subset_of(self.rights) {
                return Err(SystemCallError::RightsDenied);
            }
            Ok(Self {
                id: self.id,
                role: self.role,
                rights,
            })
        }

        fn close(self) -> Result<(), (Self, SystemCallError)> {
            Ok(())
        }
    }

    fn account() -> Arc<Account<FalResource>> {
        let mut limits = [0; FalResource::COUNT];
        limits[FalResource::Bytes.slot()] = 4096;
        Budget::new(&limits, 1).unwrap().account(&limits).unwrap()
    }

    fn handle_value(mode: ExportMode) -> Vec<u8> {
        let value = Value::Handle {
            slot: 1,
            policy: ExportPolicy {
                protocol: Protocol::Mailbox,
                mode,
                transport: Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
                fal_ceiling: FalRights::NONE,
            },
        };
        let mut bytes = vec![0; value.encoded_len().unwrap()];
        let used = value.encode(&mut bytes).unwrap();
        bytes.truncate(used);
        bytes
    }

    fn capability() -> TestCapability {
        TestCapability {
            id: 7,
            role: HandleRole::MailboxSender,
            rights: Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        }
    }

    #[test]
    fn repeatable_export_duplicates_and_rebases_reply_slot() {
        let stored = StoredValue::prepare(
            &handle_value(ExportMode::Repeatable),
            vec![capability()],
            1,
            PAYLOAD_MAX,
            &account(),
        )
        .unwrap_or_else(|_| panic!("repeatable value preparation failed"));
        let (bytes, handles) = stored.duplicate_for_reply().unwrap();
        assert_eq!(handles.len(), 1);
        assert_eq!(handles[0].0.id, 7);
        assert_eq!(validate(&bytes, 0, 1, PAYLOAD_MAX).unwrap().len(), 1);
    }

    #[test]
    fn affine_take_can_be_restored_without_losing_owner() {
        let mut stored = StoredValue::prepare(
            &handle_value(ExportMode::Affine),
            vec![capability()],
            1,
            PAYLOAD_MAX,
            &account(),
        )
        .unwrap_or_else(|_| panic!("affine value preparation failed"));
        let mut empty = Vec::new();
        empty.try_reserve_exact(HEADER_LEN).unwrap();
        let taken = stored.take(empty).unwrap();
        assert_eq!(taken.handles.len(), 1);
        assert!(stored.handles.is_empty());
        assert!(validate(&stored.bytes, 0, 0, PAYLOAD_MAX).is_ok());
        stored.restore(taken);
        assert!(stored.has_affine());
        assert_eq!(validate(&stored.bytes, 0, 1, PAYLOAD_MAX).unwrap().len(), 1);
    }

    #[test]
    fn record_roundtrip_rejects_duplicate_field_names() {
        let fields = [
            Field {
                name: "instance",
                value: Value::Integer(17),
            },
            Field {
                name: "protocol",
                value: Value::Str("fal"),
            },
        ];
        let value = Value::Record(&fields);
        let mut bytes = vec![0; value.encoded_len().unwrap()];
        let used = value.encode(&mut bytes).unwrap();
        bytes.truncate(used);
        assert!(validate(&bytes, 0, 0, PAYLOAD_MAX).is_ok());

        let duplicate = [
            Field {
                name: "endpoint",
                value: Value::Integer(1),
            },
            Field {
                name: "endpoint",
                value: Value::Integer(2),
            },
        ];
        let value = Value::Record(&duplicate);
        let mut bytes = vec![0; value.encoded_len().unwrap()];
        let used = value.encode(&mut bytes).unwrap();
        bytes.truncate(used);
        assert_eq!(
            validate(&bytes, 0, 0, PAYLOAD_MAX),
            Err(ValueError::DuplicateField)
        );
    }
}
