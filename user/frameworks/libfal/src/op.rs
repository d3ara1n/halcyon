//! 非 Lookup 操作的 body 编解码。
//!
//! 所有操作共享寻址前奏：`(policy, reserved, sized rel)`——`rel` 相对
//! Handle slot 1 的帧锚目录（来自客户端走路引擎的 `Position`）。提供者
//! 在 op 内部行走时遭遇符号链接返回 [`Status::SymbolicLinkEncountered`]，
//! 客户端展开后重试（与走路共享展开上限）。

use crate::bytes::{DecodeError, DecodeResult, Reader, Writer};
use crate::lookup::ResolvePolicy;
use crate::node::{NodeAttributes, NodeKind};

/// 寻址前奏：终段策略 + 保留区 + 相对路径。
pub struct OpAddress<'a> {
    pub policy: ResolvePolicy,
    pub rel: &'a [u8],
}

/// 寻址前奏的固定字节数：policy u32 + reserved u32 + sized rel 头。
pub const ADDRESS_HEADER_LEN: usize = 8 + 2;

impl OpAddress<'_> {
    /// 线长：policy u32 + reserved u32 + sized rel（u16 前缀）。
    pub fn encoded_len(&self) -> usize {
        ADDRESS_HEADER_LEN + self.rel.len()
    }

    pub fn encode(&self, out: &mut [u8]) -> usize {
        let mut writer = Writer::new(out);
        writer.reserve(self.encoded_len());
        writer.u32(self.policy as u32);
        writer.u32(0);
        writer.sized_bytes(self.rel);
        writer.written()
    }

    /// 解码寻址前奏：返回（策略、rel、前奏消费的字节数）。
    /// 调用方以消费长度定位后续参数，禁止各处重复偏移算术。
    pub fn decode(bytes: &[u8]) -> DecodeResult<(ResolvePolicy, &[u8], usize)> {
        let mut reader = Reader::new(bytes);
        let policy = ResolvePolicy::from_u32(reader.u32()?).ok_or(DecodeError)?;
        let reserved = reader.u32()?;
        let rel = reader.sized_bytes()?;
        if reserved != 0 {
            return Err(DecodeError);
        }
        Ok((policy, rel, reader.consumed()))
    }
}

/// Create：创建目录/属性/流节点（初值随后续写）。
pub struct CreateRequest<'a> {
    pub address: OpAddress<'a>,
    pub kind: NodeKind,
    pub attributes: NodeAttributes,
}

impl CreateRequest<'_> {
    /// 线长：寻址前奏 + kind u32 + attributes u32。
    pub fn encoded_len(&self) -> usize {
        self.address.encoded_len() + 8
    }

    pub fn encode(&self, out: &mut [u8]) -> usize {
        let used = self.address.encode(out);
        let mut writer = Writer::new(&mut out[used..]);
        writer.reserve(8);
        writer.u32(self.kind as u32);
        writer.u32(self.attributes.raw());
        used + writer.written()
    }

    pub fn decode(bytes: &[u8]) -> DecodeResult<(ResolvePolicy, &[u8], NodeKind, NodeAttributes)> {
        let (policy, rel, used) = OpAddress::decode(bytes)?;
        let mut reader = Reader::new(&bytes[used..]);
        let kind = NodeKind::from_u32(reader.u32()?).ok_or(DecodeError)?;
        let attributes = NodeAttributes::from_raw(reader.u32()?);
        reader.finish()?;
        Ok((policy, rel, kind, attributes))
    }
}

/// Link：创建符号链接（持久化路径文本）。
pub struct LinkRequest<'a> {
    pub address: OpAddress<'a>,
    pub target: &'a [u8],
}

/// Write：写属性整值（整体替换）。
pub struct WriteRequest<'a> {
    pub address: OpAddress<'a>,
    pub value: &'a [u8],
}

impl WriteRequest<'_> {
    /// 线长：寻址前奏 + sized value。
    pub fn encoded_len(&self) -> usize {
        self.address.encoded_len() + 2 + self.value.len()
    }

    pub fn encode(&self, out: &mut [u8]) -> usize {
        let used = self.address.encode(out);
        let mut writer = Writer::new(&mut out[used..]);
        writer.reserve(2 + self.value.len());
        writer.sized_bytes(self.value);
        used + writer.written()
    }

    /// 解码：返回（策略、rel、value）。
    pub fn decode(bytes: &[u8]) -> DecodeResult<(ResolvePolicy, &[u8], &[u8])> {
        let (policy, rel, used) = OpAddress::decode(bytes)?;
        let mut reader = Reader::new(&bytes[used..]);
        let value = reader.sized_bytes()?;
        reader.finish()?;
        Ok((policy, rel, value))
    }
}

impl LinkRequest<'_> {
    /// 线长：寻址前奏 + sized target。
    pub fn encoded_len(&self) -> usize {
        self.address.encoded_len() + 2 + self.target.len()
    }

    pub fn encode(&self, out: &mut [u8]) -> usize {
        let used = self.address.encode(out);
        let mut writer = Writer::new(&mut out[used..]);
        writer.reserve(2 + self.target.len());
        writer.sized_bytes(self.target);
        used + writer.written()
    }

    pub fn decode(bytes: &[u8]) -> DecodeResult<(ResolvePolicy, &[u8], &[u8])> {
        let (policy, rel, used) = OpAddress::decode(bytes)?;
        let mut reader = Reader::new(&bytes[used..]);
        let target = reader.sized_bytes()?;
        reader.finish()?;
        Ok((policy, rel, target))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_roundtrip() {
        let mut buffer = [0u8; 64];
        let request = CreateRequest {
            address: OpAddress { policy: ResolvePolicy::FollowAll, rel: b"a/b/new" },
            kind: NodeKind::Property,
            attributes: NodeAttributes::READABLE | NodeAttributes::WRITEABLE,
        };
        let used = request.encode(&mut buffer);
        let (policy, rel, kind, attributes) = CreateRequest::decode(&buffer[..used]).unwrap();
        assert_eq!(policy, ResolvePolicy::FollowAll);
        assert_eq!(rel, b"a/b/new");
        assert_eq!(kind, NodeKind::Property);
        assert!(attributes.contains(NodeAttributes::READABLE | NodeAttributes::WRITEABLE));
    }

    #[test]
    fn link_roundtrip() {
        let mut buffer = [0u8; 64];
        let request = LinkRequest {
            address: OpAddress { policy: ResolvePolicy::FollowAll, rel: b"a/lnk" },
            target: b"../elsewhere",
        };
        let used = request.encode(&mut buffer);
        let (policy, rel, target) = LinkRequest::decode(&buffer[..used]).unwrap();
        assert_eq!((policy, rel, target), (ResolvePolicy::FollowAll, &b"a/lnk"[..], &b"../elsewhere"[..]));
    }
}
