//! Drivers的只读身份/面貌；capability由装配交付，路径与PID均不是授权。
use crate::protocol::Phase;
use core::num::NonZeroU64;
use erhino_shared::proc::Pid;
use libdevice::{Error, identity::BindingRef};
use liboal::{
    ObjectInfo,
    info::{self, DIRECTORY, MarkerId, Tag},
    protocol::{BODY_MAX, Status},
};
use librpc::bytes::{Reader, Writer};

const NAMESPACE: u128 = 0x4841_4c43_594f_4e00_0044_5249_5645_5200;
pub const ROOT: MarkerId = MarkerId {
    namespace: NAMESPACE,
    local: 1,
};
pub const INSTANCE: MarkerId = MarkerId {
    namespace: NAMESPACE,
    local: 2,
};
pub const BINDINGS: MarkerId = MarkerId {
    namespace: NAMESPACE,
    local: 3,
};
pub const BINDING: MarkerId = MarkerId {
    namespace: NAMESPACE,
    local: 4,
};
pub const BINDINGS_NAME: &str = "bindings";
pub const NAME_BYTES: usize = 8 + (u64::BITS as usize / 4);
pub const CURSOR_BYTES: usize = 8 + 4 + 8 + 8;
const INSTANCE_FIXED_BYTES: usize = InstanceId::BYTES + 8 + 4 * 5;
const INSTANCE_INFO_FIXED_BYTES: usize = 8 + MarkerId::ENCODED_LEN * 2 + 4 + INSTANCE_FIXED_BYTES;
// 一项根页同时包含完整游标、名字和完整实例面貌；不是任意的key长度限额。
pub const MAX_IMPLEMENTATION_KEY: usize =
    BODY_MAX - 4 - 12 - CURSOR_BYTES - 8 - NAME_BYTES - INSTANCE_INFO_FIXED_BYTES;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstanceId {
    incarnation: NonZeroU64,
    serial: NonZeroU64,
}
impl InstanceId {
    pub const BYTES: usize = 16;
    pub fn new(incarnation: u64, serial: u64) -> Result<Self, Error> {
        Ok(Self {
            incarnation: NonZeroU64::new(incarnation).ok_or(Error::Invalid)?,
            serial: NonZeroU64::new(serial).ok_or(Error::Invalid)?,
        })
    }
    pub fn incarnation(self) -> u64 {
        self.incarnation.get()
    }
    pub fn serial(self) -> u64 {
        self.serial.get()
    }
    pub fn encode(self, w: &mut Writer<'_>) {
        w.u64(self.incarnation());
        w.u64(self.serial());
    }
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, Error> {
        Self::new(r.u64()?, r.u64()?)
    }
}
fn encode(
    marker: MarkerId,
    directory: bool,
    value: &[u8],
    out: &mut [u8],
) -> Result<usize, Status> {
    info::encode(
        if directory { &[DIRECTORY] } else { &[] },
        &[Tag { key: marker, value }],
        out,
    )
}
fn value(info: ObjectInfo<'_>, marker: MarkerId, directory: bool) -> Result<&[u8], Error> {
    if info.contains(DIRECTORY) != directory {
        return Err(Error::Invalid);
    }
    info.tags()
        .find(|tag| tag.key == marker)
        .map(|tag| tag.value)
        .ok_or(Error::Invalid)
}
pub fn encode_root(incarnation: u64, out: &mut [u8]) -> Result<usize, Status> {
    if incarnation == 0 {
        return Err(Status::Invalid);
    }
    encode(ROOT, true, &incarnation.to_le_bytes(), out)
}
pub fn decode_root(info: ObjectInfo<'_>) -> Result<u64, Error> {
    let mut r = Reader::new(value(info, ROOT, true)?);
    let incarnation = r.u64()?;
    r.finish()?;
    if incarnation == 0 {
        return Err(Error::Invalid);
    }
    Ok(incarnation)
}
pub fn encode_bindings(id: InstanceId, out: &mut [u8]) -> Result<usize, Status> {
    let mut bytes = [0; InstanceId::BYTES];
    id.encode(&mut Writer::new(&mut bytes));
    encode(BINDINGS, true, &bytes, out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstanceInfo<'a> {
    pub id: InstanceId,
    pub pid: Option<Pid>,
    pub implementation: &'a str,
    pub capacity: u32,
    /// 原Run中仍关联此实例的数量，包含尚在出生/停止收尾的成员。
    pub bindings: u32,
    pub accepting: bool,
    /// 原DONE观察或Collector已核实Dead；不表示硬件停止。
    pub cpu_done: bool,
    pub collected: bool,
    pub error: Option<Error>,
}
impl<'a> InstanceInfo<'a> {
    fn validate(self) -> Result<(), Error> {
        if self.pid == Some(0)
            || self.capacity == 0
            || self.bindings > self.capacity
            || self.implementation.is_empty()
            || self.implementation.len() > MAX_IMPLEMENTATION_KEY
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    pub fn encode(self, out: &mut [u8]) -> Result<usize, Status> {
        self.validate().map_err(|_| Status::Invalid)?;
        let mut bytes = [0; INSTANCE_FIXED_BYTES + MAX_IMPLEMENTATION_KEY];
        let mut w = Writer::new(&mut bytes);
        self.id.encode(&mut w);
        w.u64(self.pid.unwrap_or(0));
        w.u32(self.capacity);
        w.u32(self.bindings);
        w.u32(
            u32::from(self.accepting)
                | (u32::from(self.cpu_done) << 1)
                | (u32::from(self.collected) << 2),
        );
        w.u32(self.error.map_or(0, libdevice::protocol::error_code));
        w.bytes32(self.implementation.as_bytes());
        let used = w.written();
        encode(INSTANCE, true, &bytes[..used], out)
    }
    pub fn decode(info: ObjectInfo<'a>) -> Result<Self, Error> {
        let mut r = Reader::new(value(info, INSTANCE, true)?);
        let id = InstanceId::decode(&mut r)?;
        let pid = match r.u64()? {
            0 => None,
            pid => Some(pid),
        };
        let capacity = r.u32()?;
        let bindings = r.u32()?;
        let flags = r.u32()?;
        if flags & !7 != 0 {
            return Err(Error::Invalid);
        }
        let error = libdevice::protocol::decode_error(r.u32()?)?;
        let implementation = r.text32()?;
        r.finish()?;
        let info = Self {
            id,
            pid,
            implementation,
            capacity,
            bindings,
            accepting: flags & 1 != 0,
            cpu_done: flags & 2 != 0,
            collected: flags & 4 != 0,
            error,
        };
        info.validate()?;
        Ok(info)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingInfo {
    pub instance: InstanceId,
    pub reference: BindingRef,
    pub phase: Phase,
    pub released: bool,
}
impl BindingInfo {
    pub fn encode(self, out: &mut [u8]) -> Result<usize, Status> {
        let mut bytes = [0; InstanceId::BYTES + BindingRef::ENCODED_LEN + 8];
        let mut w = Writer::new(&mut bytes);
        self.instance.encode(&mut w);
        self.reference.encode(&mut w);
        w.u32(self.phase.code());
        w.u32(u32::from(self.released));
        encode(BINDING, false, &bytes, out)
    }
    pub fn decode(info: ObjectInfo<'_>) -> Result<Self, Error> {
        let mut r = Reader::new(value(info, BINDING, false)?);
        let instance = InstanceId::decode(&mut r)?;
        let reference = BindingRef::decode(&mut r)?;
        let phase = Phase::decode(r.u32()?)?;
        let released = match r.u32()? {
            0 => false,
            1 => true,
            _ => return Err(Error::Invalid),
        };
        r.finish()?;
        Ok(Self {
            instance,
            reference,
            phase,
            released,
        })
    }
}
fn name<'a>(prefix: &[u8; 8], serial: u64, out: &'a mut [u8; NAME_BYTES]) -> &'a str {
    out[..8].copy_from_slice(prefix);
    for i in 0..u64::BITS as usize / 4 {
        out[8 + i] = b"0123456789abcdef"
            [((serial >> (((u64::BITS as usize / 4 - 1) - i) * 4)) & 15) as usize];
    }
    // 所有输出字节来自两个固定ASCII前缀和十六进制字符表。
    core::str::from_utf8(out).expect("driver component lost ASCII validity")
}
pub fn instance_name(serial: u64, out: &mut [u8; NAME_BYTES]) -> &str {
    name(b"process-", serial, out)
}
pub fn binding_name(serial: u64, out: &mut [u8; NAME_BYTES]) -> &str {
    name(b"binding-", serial, out)
}
fn parse(name: &str, prefix: &str) -> Option<u64> {
    if name.len() != NAME_BYTES || !name.starts_with(prefix) {
        return None;
    }
    let digits = &name[prefix.len()..];
    if !digits
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    u64::from_str_radix(digits, 16).ok().filter(|&id| id != 0)
}
pub fn parse_instance_name(name: &str) -> Option<u64> {
    parse(name, "process-")
}
pub fn parse_binding_name(name: &str) -> Option<u64> {
    parse(name, "binding-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use erhino_shared::time::Deadline;
    use libdevice::identity::DeviceId;
    use liboal::protocol::{self as oal, DirectoryEntry, Page, Request, Response};

    // OAL实际运输是独立预算门：最长key、完整游标与单项摘要必须同时可交付。
    #[test]
    fn maximum_instance_summary_fits_real_oal_page() {
        let key = String::from_utf8(alloc::vec![b'x'; MAX_IMPLEMENTATION_KEY]).unwrap();
        let instance = InstanceInfo {
            id: InstanceId::new(7, 9).unwrap(),
            pid: Some(13),
            implementation: &key,
            capacity: 2,
            bindings: 2,
            accepting: true,
            cpu_done: false,
            collected: false,
            error: None,
        };
        let mut summary = [0; BODY_MAX];
        let length = instance.encode(&mut summary).unwrap();
        let summary = ObjectInfo::decode(&summary[..length]).unwrap();
        assert_eq!(InstanceInfo::decode(summary).unwrap(), instance);
        let mut name = [0; NAME_BYTES];
        let name = instance_name(instance.id.serial(), &mut name);
        let mut body = [0; BODY_MAX];
        let length = oal::encode_page(
            &[1; CURSOR_BYTES],
            false,
            &[DirectoryEntry {
                name,
                info: Some(summary),
            }],
            &mut body,
        )
        .unwrap();
        let page = Page::decode(&body[..length]).unwrap();
        let mut wire = [0; erhino_shared::message::PAYLOAD_MAX - librpc::PREFIX_LEN];
        let deadline = Deadline::at(123);
        let length = oal::encode_response(2, &Response::Page(page), deadline, &mut wire).unwrap();
        let command = Request::Enumerate {
            path: "",
            cursor: &[],
            max_entries: 1,
        };
        assert!(matches!(
            oal::decode_response(&wire[..length], &command, deadline, 0),
            Ok(Response::Page(_))
        ));
        let too_long = alloc::format!("{key}x");
        assert!(
            InstanceInfo {
                implementation: &too_long,
                ..instance
            }
            .encode(&mut body)
            .is_err()
        );
    }

    // 身份要保留incarnation与epoch，不能因同serial或观察到Terminal折叠新旧绑定。
    #[test]
    fn instance_and_binding_identity_remain_distinct() {
        assert_ne!(
            InstanceId::new(1, 2).unwrap(),
            InstanceId::new(2, 2).unwrap()
        );
        assert!(InstanceId::new(0, 1).is_err());
        let binding = BindingInfo {
            instance: InstanceId::new(7, 9).unwrap(),
            reference: BindingRef::new(DeviceId::new(5, 2).unwrap(), 3).unwrap(),
            phase: Phase::Terminal,
            released: false,
        };
        let mut bytes = [0; 128];
        let used = binding.encode(&mut bytes).unwrap();
        let decoded = BindingInfo::decode(ObjectInfo::decode(&bytes[..used]).unwrap()).unwrap();
        assert_eq!(decoded, binding);
        assert!(!decoded.released);
        assert_eq!(parse_instance_name("process-000000000000000f"), Some(15));
        assert_eq!(parse_instance_name("process-000000000000000F"), None);
        assert_eq!(parse_binding_name("binding-0000000000000000"), None);
    }
}
