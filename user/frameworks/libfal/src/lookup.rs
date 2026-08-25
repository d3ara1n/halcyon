//! 节点查询（Lookup）：走路协议的三值应答。
//!
//! 请求携带相对 Handle slot 1 目录的剩余路径后缀；提供者在自有子树内
//! 尽量行进，抵达子树边界（Delegate）、遭遇符号链接
//! （SymbolicLinkBoundary）或终点（Found）时停止。应答 Handle 布局：
//! Delegate 的子目录 Handle 与 SymbolicLinkBoundary 的父目录 Handle
//! 都在应答 slot 1 以 Handle move 交付。

use crate::{
    bytes::{DecodeError, DecodeResult, Reader, Writer},
    node::{NodeAttributes, NodeKind},
};

/// 终段策略：由请求声明，见 fal.md「走路」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ResolvePolicy {
    /// 全部分量跟随符号链接。
    FollowAll = 0,
    /// 终段不跟随（对符号链接本身操作）。
    NoFollowFinal = 1,
    /// 解析至父（终段的父目录，供 create/unlink/rename）。
    ResolveParent = 2,
}

impl ResolvePolicy {
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::FollowAll),
            1 => Some(Self::NoFollowFinal),
            2 => Some(Self::ResolveParent),
            _ => None,
        }
    }
}

/// Lookup 请求 body：策略 + 保留区 + 路径后缀字节。
pub struct LookupRequest<'a> {
    pub policy: ResolvePolicy,
    pub path: &'a [u8],
}

impl LookupRequest<'_> {
    pub fn encode(&self, out: &mut [u8]) -> DecodeResult<usize> {
        let mut writer = Writer::new(out);
        writer.u32(self.policy as u32);
        writer.u32(0);
        if !writer.bytes(self.path) {
            return Err(DecodeError);
        }
        Ok(writer.written())
    }

    pub fn decode(bytes: &[u8]) -> DecodeResult<(ResolvePolicy, &[u8])> {
        let mut reader = Reader::new(bytes);
        let policy = ResolvePolicy::from_u32(reader.u32()?).ok_or(DecodeError)?;
        let reserved = reader.u32()?;
        let path = reader.bytes(reader.remaining())?;
        if reserved != 0 {
            return Err(DecodeError);
        }
        Ok((policy, path))
    }
}

/// Found 应答 body：节点类型、标记、尺寸与可选自描述尾。
///
/// 尾随槽位（sized bytes）按 kind 判别：今天是 SymbolicLink 的
/// target 文本；无尾的 kind 到 size 截止。多态收敛在判别式上，
/// 新节点类型的自描述走同一槽位。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeInfo<'a> {
    pub kind: NodeKind,
    pub attributes: NodeAttributes,
    /// 语义随 kind：Directory = 子项数，其余 = 字节数。
    pub size: u64,
    /// SymbolicLink 的 target 文本；其余 kind 为空。
    pub value: &'a [u8],
}

impl NodeInfo<'_> {
    pub fn encode(&self, out: &mut [u8]) -> DecodeResult<usize> {
        let mut writer = Writer::new(out);
        writer.u32(self.kind as u32);
        writer.u32(self.attributes.raw());
        writer.u64(self.size);
        writer.u32(0);
        if self.kind == NodeKind::SymbolicLink && !writer.sized_bytes(self.value) {
            return Err(DecodeError);
        }
        Ok(writer.written())
    }

    pub fn decode(bytes: &[u8]) -> DecodeResult<(NodeKind, NodeAttributes, u64, &[u8])> {
        let mut reader = Reader::new(bytes);
        let kind = NodeKind::from_u32(reader.u32()?).ok_or(DecodeError)?;
        let attributes = NodeAttributes::from_raw(reader.u32()?);
        let size = reader.u64()?;
        let reserved = reader.u32()?;
        if reserved != 0 {
            return Err(DecodeError);
        }
        let value = if kind == NodeKind::SymbolicLink { reader.sized_bytes()? } else { &[] };
        reader.finish()?;
        Ok((kind, attributes, size, value))
    }
}

/// 跨界信息体：已消费前缀与剩余后缀（Delegate 与
/// SymbolicLinkBoundary 共用；相对 target 的续走起点在应答 slot 1）。
pub struct Boundary<'a> {
    pub consumed: &'a [u8],
    pub remaining: &'a [u8],
}

impl Boundary<'_> {
    pub fn encode(&self, out: &mut [u8]) -> DecodeResult<usize> {
        let mut writer = Writer::new(out);
        if !writer.sized_bytes(self.consumed) || !writer.sized_bytes(self.remaining) {
            return Err(DecodeError);
        }
        Ok(writer.written())
    }

    pub fn decode(bytes: &[u8]) -> DecodeResult<(&[u8], &[u8])> {
        let mut reader = Reader::new(bytes);
        let consumed = reader.sized_bytes()?;
        let remaining = reader.sized_bytes()?;
        reader.finish()?;
        Ok((consumed, remaining))
    }
}

/// 符号链接边界 body：在 [`Boundary`] 之前附 target 文本
/// （`consumed` + `target` + `remaining` 各自 u16 前缀）。
pub struct LinkBoundary<'a> {
    pub consumed: &'a [u8],
    pub target: &'a [u8],
    pub remaining: &'a [u8],
}

impl LinkBoundary<'_> {
    pub fn encode(&self, out: &mut [u8]) -> DecodeResult<usize> {
        let mut writer = Writer::new(out);
        if !writer.sized_bytes(self.consumed)
            || !writer.sized_bytes(self.target)
            || !writer.sized_bytes(self.remaining)
        {
            return Err(DecodeError);
        }
        Ok(writer.written())
    }

    pub fn decode(bytes: &[u8]) -> DecodeResult<(&[u8], &[u8], &[u8])> {
        let mut reader = Reader::new(bytes);
        let consumed = reader.sized_bytes()?;
        let target = reader.sized_bytes()?;
        let remaining = reader.sized_bytes()?;
        reader.finish()?;
        Ok((consumed, target, remaining))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrip() {
        let mut buffer = [0u8; 64];
        let request = LookupRequest { policy: ResolvePolicy::NoFollowFinal, path: b"a/b/c" };
        let used = request.encode(&mut buffer).unwrap();
        let (policy, path) = LookupRequest::decode(&buffer[..used]).unwrap();
        assert_eq!(policy, ResolvePolicy::NoFollowFinal);
        assert_eq!(path, b"a/b/c");
    }

    #[test]
    fn node_info_roundtrip() {
        let mut buffer = [0u8; 32];
        let info = NodeInfo {
            kind: NodeKind::Stream,
            attributes: NodeAttributes::READABLE | NodeAttributes::EXECUTABLE,
            size: 8192,
            value: &[],
        };
        let used = info.encode(&mut buffer).unwrap();
        let (kind, attributes, size, value) = NodeInfo::decode(&buffer[..used]).unwrap();
        assert_eq!(kind, NodeKind::Stream);
        assert!(attributes.contains(NodeAttributes::READABLE | NodeAttributes::EXECUTABLE));
        assert_eq!(size, 8192);
        assert_eq!(value, &[][..]);
    }

    #[test]
    fn node_info_link_carries_target() {
        let mut buffer = [0u8; 32];
        let info = NodeInfo {
            kind: NodeKind::SymbolicLink,
            attributes: NodeAttributes::NONE,
            size: 6,
            value: b"target",
        };
        let used = info.encode(&mut buffer).unwrap();
        let (kind, _, size, value) = NodeInfo::decode(&buffer[..used]).unwrap();
        assert_eq!(kind, NodeKind::SymbolicLink);
        assert_eq!(size, 6);
        assert_eq!(value, b"target");
    }

    #[test]
    fn boundary_and_link_boundary_roundtrip() {
        let mut buffer = [0u8; 64];
        let boundary = Boundary { consumed: b"a/b", remaining: b"c/d" };
        let used = boundary.encode(&mut buffer).unwrap();
        assert_eq!(Boundary::decode(&buffer[..used]).unwrap(), (b"a/b".as_slice(), b"c/d".as_slice()));

        let link = LinkBoundary { consumed: b"a", target: b"../x", remaining: b"c" };
        let used = link.encode(&mut buffer).unwrap();
        assert_eq!(
            LinkBoundary::decode(&buffer[..used]).unwrap(),
            (b"a".as_slice(), b"../x".as_slice(), b"c".as_slice())
        );
    }
}
