//! 非 Lookup 操作的 body 编解码。
//!
//! 所有操作共享寻址前奏：`(policy, reserved, rel_path)`——`rel` 相对
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

impl OpAddress<'_> {
    pub fn encode(&self, out: &mut [u8]) -> DecodeResult<usize> {
        let mut writer = Writer::new(out);
        writer.u32(self.policy as u32);
        writer.u32(0);
        if !writer.sized_bytes(self.rel) {
            return Err(DecodeError);
        }
        Ok(writer.written())
    }

    pub fn decode(bytes: &[u8]) -> DecodeResult<(ResolvePolicy, &[u8])> {
        let mut reader = Reader::new(bytes);
        let policy = ResolvePolicy::from_u32(reader.u32()?).ok_or(DecodeError)?;
        let reserved = reader.u32()?;
        let rel = reader.sized_bytes()?;
        if reserved != 0 {
            return Err(DecodeError);
        }
        Ok((policy, rel))
    }
}

/// Create：创建目录/属性/流节点（初值随后续写）。
pub struct CreateRequest<'a> {
    pub address: OpAddress<'a>,
    pub kind: NodeKind,
    pub attributes: NodeAttributes,
}

impl CreateRequest<'_> {
    pub fn encode(&self, out: &mut [u8]) -> DecodeResult<usize> {
        let used = self.address.encode(out)?;
        let mut writer = Writer::new(&mut out[used..]);
        writer.u32(self.kind as u32);
        writer.u32(self.attributes.raw());
        Ok(used + writer.written())
    }

    pub fn decode(bytes: &[u8]) -> DecodeResult<(ResolvePolicy, &[u8], NodeKind, NodeAttributes)> {
        let (policy, rel) = OpAddress::decode(bytes)?;
        let rest = &bytes[8 + 2 + rel.len()..];
        let mut reader = Reader::new(rest);
        let kind = NodeKind::from_u32(reader.u32()?).ok_or(DecodeError)?;
        let attributes = NodeAttributes::from_raw(reader.u32()?);
        reader.finish()?;
        Ok((policy, rel, kind, attributes))
    }
}

/// CreateSymbolicLink：创建持久化路径文本。
pub struct CreateSymbolicLinkRequest<'a> {
    pub address: OpAddress<'a>,
    pub target: &'a [u8],
}

impl CreateSymbolicLinkRequest<'_> {
    pub fn encode(&self, out: &mut [u8]) -> DecodeResult<usize> {
        let used = self.address.encode(out)?;
        let mut writer = Writer::new(&mut out[used..]);
        if !writer.sized_bytes(self.target) {
            return Err(DecodeError);
        }
        Ok(used + writer.written())
    }

    pub fn decode(bytes: &[u8]) -> DecodeResult<(ResolvePolicy, &[u8], &[u8])> {
        let (policy, rel) = OpAddress::decode(bytes)?;
        let rest = &bytes[8 + 2 + rel.len()..];
        let mut reader = Reader::new(rest);
        let target = reader.sized_bytes()?;
        reader.finish()?;
        Ok((policy, rel, target))
    }
}

/// Delete：删除节点。
pub struct DeleteRequest<'a> {
    pub address: OpAddress<'a>,
}

impl DeleteRequest<'_> {
    pub fn encode(&self, out: &mut [u8]) -> DecodeResult<usize> {
        self.address.encode(out)
    }

    pub fn decode(bytes: &[u8]) -> DecodeResult<(ResolvePolicy, &[u8])> {
        OpAddress::decode(bytes)
    }
}

/// PropertyRead：读属性值（应答为属性值编码）。
pub struct PropertyReadRequest<'a> {
    pub address: OpAddress<'a>,
}

impl PropertyReadRequest<'_> {
    pub fn encode(&self, out: &mut [u8]) -> DecodeResult<usize> {
        self.address.encode(out)
    }

    pub fn decode(bytes: &[u8]) -> DecodeResult<(ResolvePolicy, &[u8])> {
        OpAddress::decode(bytes)
    }
}

/// ReadSymbolicLink：应答为 target 文本。
pub struct ReadSymbolicLinkRequest<'a> {
    pub address: OpAddress<'a>,
}

impl ReadSymbolicLinkRequest<'_> {
    pub fn encode(&self, out: &mut [u8]) -> DecodeResult<usize> {
        self.address.encode(out)
    }

    pub fn decode(bytes: &[u8]) -> DecodeResult<(ResolvePolicy, &[u8])> {
        OpAddress::decode(bytes)
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
        let used = request.encode(&mut buffer).unwrap();
        let (policy, rel, kind, attributes) = CreateRequest::decode(&buffer[..used]).unwrap();
        assert_eq!(policy, ResolvePolicy::FollowAll);
        assert_eq!(rel, b"a/b/new");
        assert_eq!(kind, NodeKind::Property);
        assert!(attributes.contains(NodeAttributes::READABLE | NodeAttributes::WRITEABLE));
    }

    #[test]
    fn symlink_create_roundtrip() {
        let mut buffer = [0u8; 64];
        let request = CreateSymbolicLinkRequest {
            address: OpAddress { policy: ResolvePolicy::FollowAll, rel: b"a/lnk" },
            target: b"../elsewhere",
        };
        let used = request.encode(&mut buffer).unwrap();
        let (policy, rel, target) = CreateSymbolicLinkRequest::decode(&buffer[..used]).unwrap();
        assert_eq!((policy, rel, target), (ResolvePolicy::FollowAll, &b"a/lnk"[..], &b"../elsewhere"[..]));
    }
}
