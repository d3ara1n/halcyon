//! 偏移读写（ReadAt/WriteAt）body 编解码：随机访问的控制面 kind。

use crate::bytes::{DecodeResult, Reader, Writer};
use crate::op::OpAddress;

/// ReadAt：请求偏移与长度；应答为 sized bytes（受消息上限约束）。
pub struct ReadAtRequest<'a> {
    pub address: OpAddress<'a>,
    pub offset: u64,
    pub len: u32,
}

impl ReadAtRequest<'_> {
    pub fn encode(&self, out: &mut [u8]) -> DecodeResult<usize> {
        let used = self.address.encode(out)?;
        let mut writer = Writer::new(&mut out[used..]);
        writer.u64(self.offset);
        writer.u32(self.len);
        writer.u32(0);
        Ok(used + writer.written())
    }

    /// 解码：返回（策略、rel、offset、len）。
    pub fn decode(bytes: &[u8]) -> DecodeResult<(crate::lookup::ResolvePolicy, &[u8], u64, u32)> {
        let (policy, rel) = OpAddress::decode(bytes)?;
        let rest = &bytes[10 + rel.len()..];
        let mut reader = Reader::new(rest);
        let offset = reader.u64()?;
        let len = reader.u32()?;
        let reserved = reader.u32()?;
        reader.finish()?;
        if reserved != 0 {
            return Err(crate::bytes::DecodeError);
        }
        Ok((policy, rel, offset, len))
    }
}

/// WriteAt：请求偏移与字节段。
pub struct WriteAtRequest<'a> {
    pub address: OpAddress<'a>,
    pub offset: u64,
    pub bytes: &'a [u8],
}

impl WriteAtRequest<'_> {
    pub fn encode(&self, out: &mut [u8]) -> DecodeResult<usize> {
        let used = self.address.encode(out)?;
        let mut writer = Writer::new(&mut out[used..]);
        writer.u64(self.offset);
        if !writer.sized_bytes(self.bytes) {
            return Err(crate::bytes::DecodeError);
        }
        Ok(used + writer.written())
    }

    /// 解码：返回（策略、rel、offset、bytes）；应答为写入字节数（u32）。
    pub fn decode(bytes: &[u8]) -> DecodeResult<(crate::lookup::ResolvePolicy, &[u8], u64, &[u8])> {
        let (policy, rel) = OpAddress::decode(bytes)?;
        let rest = &bytes[10 + rel.len()..];
        let mut reader = Reader::new(rest);
        let offset = reader.u64()?;
        let data = reader.sized_bytes()?;
        reader.finish()?;
        Ok((policy, rel, offset, data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lookup::ResolvePolicy;

    #[test]
    fn read_at_roundtrip() {
        let mut buffer = [0u8; 64];
        let request = ReadAtRequest {
            address: OpAddress { policy: ResolvePolicy::FollowAll, rel: b"bin/init" },
            offset: 8,
            len: 32,
        };
        let used = request.encode(&mut buffer).unwrap();
        assert_eq!(
            ReadAtRequest::decode(&buffer[..used]).unwrap(),
            (ResolvePolicy::FollowAll, &b"bin/init"[..], 8, 32)
        );
    }

    #[test]
    fn write_at_roundtrip() {
        let mut buffer = [0u8; 64];
        let request = WriteAtRequest {
            address: OpAddress { policy: ResolvePolicy::FollowAll, rel: b"out" },
            offset: 16,
            bytes: &[1, 2, 3, 4],
        };
        let used = request.encode(&mut buffer).unwrap();
        assert_eq!(
            WriteAtRequest::decode(&buffer[..used]).unwrap(),
            (ResolvePolicy::FollowAll, &b"out"[..], 16, &[1, 2, 3, 4][..])
        );
    }
}
