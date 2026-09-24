//! ServiceRecord schema v1；一次属性快照同时携带实例与 endpoint。

use alloc::vec::Vec;
use erhino_shared::object::Rights;
use libfal::{
    authority::FalRights,
    bytes::{DecodeError, Reader},
    value::{
        ExportMode, ExportPolicy, Field, Protocol as ValueProtocol, Tag, Value, ValueError,
        validate,
    },
};

pub const SCHEMA: i64 = 1;
pub const FIELD_COUNT: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceRecord {
    pub instance: u64,
    pub protocol: u64,
    pub version: u32,
    pub generation: u64,
    pub endpoint_policy: ExportPolicy,
}

impl ServiceRecord {
    pub fn encode(self) -> Result<Vec<u8>, ValueError> {
        if self.instance == 0
            || self.protocol == 0
            || self.version == 0
            || self.generation == 0
            || !valid_endpoint_policy(self.endpoint_policy)
        {
            return Err(ValueError::Encoding);
        }
        let instance = self.instance.to_le_bytes();
        let protocol = self.protocol.to_le_bytes();
        let generation = self.generation.to_le_bytes();
        let fields = [
            Field {
                name: "schema",
                value: Value::Integer(SCHEMA),
            },
            Field {
                name: "instance",
                value: Value::Blob(&instance),
            },
            Field {
                name: "protocol",
                value: Value::Blob(&protocol),
            },
            Field {
                name: "version",
                value: Value::Integer(i64::from(self.version)),
            },
            Field {
                name: "generation",
                value: Value::Blob(&generation),
            },
            Field {
                name: "endpoint",
                value: Value::Handle {
                    slot: 0,
                    policy: self.endpoint_policy,
                },
            },
        ];
        let value = Value::Record(&fields);
        let len = value.encoded_len().ok_or(ValueError::Budget)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(len)
            .map_err(|_| ValueError::Allocation)?;
        bytes.resize(len, 0);
        let used = value.encode(&mut bytes)?;
        bytes.truncate(used);
        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DecodeError> {
        validate(bytes, 0, 1, bytes.len()).map_err(|_| DecodeError)?;
        let mut reader = Reader::new(bytes);
        if reader.u32()? != Tag::Record as u32
            || reader.u32()? != 0
            || reader.u32()? as usize != reader.remaining().saturating_sub(4)
            || reader.u32()? != 0
            || reader.u32()? as usize != FIELD_COUNT
            || reader.u32()? != 0
        {
            return Err(DecodeError);
        }
        let mut schema = None;
        let mut instance = None;
        let mut protocol = None;
        let mut version = None;
        let mut generation = None;
        let mut endpoint_policy = None;
        for _ in 0..FIELD_COUNT {
            let name_len = reader.u16()? as usize;
            if reader.u16()? != 0 {
                return Err(DecodeError);
            }
            let value_len = reader.u32()? as usize;
            let name = core::str::from_utf8(reader.bytes(name_len)?).map_err(|_| DecodeError)?;
            let value = reader.bytes(value_len)?;
            match name {
                "schema" if schema.is_none() => schema = Some(decode_integer(value)?),
                "instance" if instance.is_none() => instance = Some(decode_u64_blob(value)?),
                "protocol" if protocol.is_none() => protocol = Some(decode_u64_blob(value)?),
                "version" if version.is_none() => {
                    version = Some(u32::try_from(decode_integer(value)?).map_err(|_| DecodeError)?)
                }
                "generation" if generation.is_none() => generation = Some(decode_u64_blob(value)?),
                "endpoint" if endpoint_policy.is_none() => {
                    endpoint_policy = Some(decode_endpoint(value)?)
                }
                _ => return Err(DecodeError),
            }
        }
        reader.finish()?;
        let record = Self {
            instance: instance.ok_or(DecodeError)?,
            protocol: protocol.ok_or(DecodeError)?,
            version: version.ok_or(DecodeError)?,
            generation: generation.ok_or(DecodeError)?,
            endpoint_policy: endpoint_policy.ok_or(DecodeError)?,
        };
        if schema != Some(SCHEMA)
            || record.instance == 0
            || record.protocol == 0
            || record.version == 0
            || record.generation == 0
        {
            return Err(DecodeError);
        }
        Ok(record)
    }
}

fn decode_value_header<'a>(bytes: &'a [u8], expected: Tag) -> Result<Reader<'a>, DecodeError> {
    let mut reader = Reader::new(bytes);
    if reader.u32()? != expected as u32 || reader.u32()? != 0 {
        return Err(DecodeError);
    }
    let body_len = reader.u32()? as usize;
    if reader.u32()? != 0 || body_len != reader.remaining() {
        return Err(DecodeError);
    }
    Ok(reader)
}

fn decode_integer(bytes: &[u8]) -> Result<i64, DecodeError> {
    let mut reader = decode_value_header(bytes, Tag::Integer)?;
    let value = reader.u64()? as i64;
    reader.finish()?;
    Ok(value)
}

fn decode_u64_blob(bytes: &[u8]) -> Result<u64, DecodeError> {
    let mut reader = decode_value_header(bytes, Tag::Blob)?;
    let value = reader.u64()?;
    reader.finish()?;
    Ok(value)
}

fn decode_endpoint(bytes: &[u8]) -> Result<ExportPolicy, DecodeError> {
    let mut reader = decode_value_header(bytes, Tag::Handle)?;
    if reader.u16()? != 0 {
        return Err(DecodeError);
    }
    let mode = match reader.u16()? {
        0 => ExportMode::Repeatable,
        1 => ExportMode::Affine,
        _ => return Err(DecodeError),
    };
    let protocol = ValueProtocol::from_raw(reader.u32()?).ok_or(DecodeError)?;
    let transport = Rights::from_raw(reader.u64()?);
    let fal_ceiling = FalRights::from_raw(reader.u32()?).ok_or(DecodeError)?;
    if reader.u32()? != 0 {
        return Err(DecodeError);
    }
    reader.finish()?;
    let policy = ExportPolicy {
        protocol,
        mode,
        transport,
        fal_ceiling,
    };
    valid_endpoint_policy(policy)
        .then_some(policy)
        .ok_or(DecodeError)
}

fn valid_endpoint_policy(policy: ExportPolicy) -> bool {
    policy.mode == ExportMode::Repeatable
        && policy.transport.is_known()
        && policy
            .transport
            .contains(Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT)
        && match policy.protocol {
            ValueProtocol::Directory => policy.fal_ceiling.contains(FalRights::TRAVERSE),
            ValueProtocol::Mailbox => policy.fal_ceiling == FalRights::NONE,
            ValueProtocol::Opaque | ValueProtocol::Notification => false,
        }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> ServiceRecord {
        ServiceRecord {
            instance: u64::MAX - 3,
            protocol: 0x4641_4c32,
            version: 2,
            generation: u64::MAX - 7,
            endpoint_policy: ExportPolicy {
                protocol: ValueProtocol::Directory,
                mode: ExportMode::Repeatable,
                transport: Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
                fal_ceiling: FalRights::TRAVERSE | FalRights::ENUMERATE | FalRights::READ_PROPERTY,
            },
        }
    }

    #[test]
    fn service_record_roundtrip_preserves_unsigned_identities() {
        let record = record();
        let bytes = record.encode().unwrap();
        assert_eq!(ServiceRecord::decode(&bytes).unwrap(), record);
    }

    #[test]
    fn service_record_rejects_unknown_duplicate_and_missing_fields() {
        let record = record();
        let bytes = record.encode().unwrap();
        let needle = b"schema";
        let offset = bytes
            .windows(needle.len())
            .position(|window| window == needle)
            .unwrap();

        let mut unknown = bytes.clone();
        unknown[offset..offset + needle.len()].copy_from_slice(b"unknow");
        assert!(ServiceRecord::decode(&unknown).is_err());

        let mut duplicate = bytes.clone();
        let instance = b"instance";
        let instance_offset = duplicate
            .windows(instance.len())
            .position(|window| window == instance)
            .unwrap();
        duplicate[instance_offset..instance_offset + instance.len()].copy_from_slice(b"protocol");
        assert!(ServiceRecord::decode(&duplicate).is_err());

        assert!(ServiceRecord::decode(&bytes[..bytes.len() - 1]).is_err());
    }
}
