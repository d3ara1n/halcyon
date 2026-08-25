//! 内存文件系统：FAL 提供者的参考积木（单提供者、无委托、无 Handle 属性）。
//!
//! 树节点为目录/属性/流/符号链接四类；目录标记执行 X 可穿越（作为
//! 中间分量时）、R 可枚举、W 可增删，属性与流的标记约束读写。链接由
//! 边界返回客户端展开，本提供者从不解释 target。本积木用于验收线与
//! 宿主测试；具体文件系统的存储、缓存与身份模型不在其职责内
//! （见 fal.md「边界」）。

use alloc::{borrow::ToOwned, collections::BTreeMap, string::String, vec::Vec};

use crate::header::Status;
use crate::lookup::{NodeInfo, ResolvePolicy};
use crate::node::{validate_path, NodeAttributes, NodeKind};

#[derive(Debug, Clone)]
pub enum Node {
    Directory { attributes: NodeAttributes, children: BTreeMap<String, Node> },
    Property { attributes: NodeAttributes, value: Vec<u8> },
    Stream { attributes: NodeAttributes, data: Vec<u8> },
    SymbolicLink { target: String },
}

impl Node {
    pub fn kind(&self) -> NodeKind {
        match self {
            Self::Directory { .. } => NodeKind::Directory,
            Self::Property { .. } => NodeKind::Property,
            Self::Stream { .. } => NodeKind::Stream,
            Self::SymbolicLink { .. } => NodeKind::SymbolicLink,
        }
    }

    pub fn attributes(&self) -> NodeAttributes {
        match self {
            Self::Directory { attributes, .. }
            | Self::Property { attributes, .. }
            | Self::Stream { attributes, .. } => *attributes,
            Self::SymbolicLink { .. } => NodeAttributes::NONE,
        }
    }

    fn size(&self) -> u64 {
        match self {
            Self::Directory { children, .. } => children.len() as u64,
            Self::Property { value, .. } => value.len() as u64,
            Self::Stream { data, .. } => data.len() as u64,
            Self::SymbolicLink { target } => target.len() as u64,
        }
    }

    fn found(&self, include_target: bool) -> MemLookup {
        let target = match (self, include_target) {
            (Self::SymbolicLink { target }, true) => Some(target.clone()),
            _ => None,
        };
        MemLookup::Found { kind: self.kind(), attributes: self.attributes(), size: self.size(), target }
    }
}

fn children_of(node: &Node) -> Result<&BTreeMap<String, Node>, Status> {
    match node {
        Node::Directory { children, .. } => Ok(children),
        _ => Err(Status::NotADirectory),
    }
}

fn children_of_mut(node: &mut Node) -> Result<&mut BTreeMap<String, Node>, Status> {
    match node {
        Node::Directory { children, .. } => Ok(children),
        _ => Err(Status::NotADirectory),
    }
}

/// walk 核心结果：抵达终段，或中途/终段命中链接。
enum Walked<'a> {
    Reached(&'a Node),
    HitLink { node: &'a Node, at: usize },
}

/// 内存文件系统：以根目录起步，路径为服务内相对路径。
#[derive(Debug)]
pub struct MemFs {
    root: Node,
    /// 目录代数：任一修改使在途枚举 cursor 失效。
    generation: u64,
}

/// Lookup 的提供者侧结果（单提供者：Delegate 不出现）。
/// Found 携带节点元数据；SymbolicLink 终段（NoFollowFinal）含 target。
pub enum MemLookup {
    Found { kind: NodeKind, attributes: NodeAttributes, size: u64, target: Option<String> },
    Link { parent_rel: String, target: String, remaining: String },
}

/// 枚举页：项 + 续游标（0 = 完毕）。
pub struct MemPage {
    pub entries: Vec<(String, NodeKind)>,
    pub next_cursor: u64,
}

impl MemFs {
    pub fn new() -> Self {
        Self {
            root: Node::Directory {
                attributes: NodeAttributes::READABLE
                    | NodeAttributes::WRITEABLE
                    | NodeAttributes::EXECUTABLE,
                children: BTreeMap::new(),
            },
            generation: 0,
        }
    }

    /// 逐段行走：中间目录须具 X；命中链接即停（不解释）。
    fn walk<'a>(&'a self, rel: &[u8]) -> Result<Walked<'a>, Status> {
        let segments = split_rel(rel)?;
        let mut current = &self.root;
        for (index, segment) in segments.iter().enumerate() {
            let child = children_of(current)?.get(segment).ok_or(Status::NotFound)?;
            let final_component = index + 1 == segments.len();
            if let Node::SymbolicLink { .. } = child {
                return Ok(Walked::HitLink { node: child, at: index });
            }
            if final_component {
                return Ok(Walked::Reached(child));
            }
            if !child.attributes().contains(NodeAttributes::EXECUTABLE) {
                return Err(Status::NotAccessible);
            }
            current = child;
        }
        Ok(Walked::Reached(&self.root))
    }

    /// 可变行走：链接同样返回命中（写路径按节点类型分流）。
    fn walk_mut<'a>(&'a mut self, rel: &[u8]) -> Result<&'a mut Node, Status> {
        let segments = split_rel(rel)?;
        let mut current = &mut self.root;
        for segment in &segments[..segments.len().saturating_sub(1)] {
            let child = children_of_mut(current)?.get_mut(segment).ok_or(Status::NotFound)?;
            if matches!(child, Node::SymbolicLink { .. }) {
                return Err(Status::SymbolicLinkEncountered);
            }
            if !child.attributes().contains(NodeAttributes::EXECUTABLE) {
                return Err(Status::NotAccessible);
            }
            current = child;
        }
        match segments.last() {
            None => Ok(&mut self.root),
            Some(name) => {
                children_of_mut(current)?.get_mut(name).ok_or(Status::NotFound)
            }
        }
    }

    /// 节点查询：FollowAll 下终段链接返回边界，NoFollowFinal 返回节点。
    pub fn lookup(&self, policy: ResolvePolicy, rel: &[u8]) -> Result<MemLookup, Status> {
        match self.walk(rel)? {
            Walked::Reached(node) => Ok(node.found(false)),
            Walked::HitLink { node, at } => {
                if policy == ResolvePolicy::NoFollowFinal {
                    return Ok(node.found(true));
                }
                let segments = split_rel(rel)?;
                let Node::SymbolicLink { target } = node else {
                    unreachable!("HitLink carries a symbolic link");
                };
                Ok(MemLookup::Link {
                    parent_rel: segments[..at].join("/"),
                    target: target.clone(),
                    remaining: segments[at + 1..].join("/"),
                })
            }
        }
    }

    fn node_at(&self, policy: ResolvePolicy, rel: &[u8]) -> Result<&Node, Status> {
        match self.walk(rel)? {
            Walked::Reached(node) => Ok(node),
            Walked::HitLink { node, at } => {
                // NoFollowFinal 且链接为终段：按节点返回；否则交客户端展开。
                let segments = split_rel(rel)?;
                if policy == ResolvePolicy::NoFollowFinal && at + 1 == segments.len() {
                    Ok(node)
                } else {
                    Err(Status::SymbolicLinkEncountered)
                }
            }
        }
    }

    /// 创建节点（目录/属性/流）。
    pub fn create(
        &mut self,
        rel: &[u8],
        kind: NodeKind,
        attributes: NodeAttributes,
    ) -> Result<(), Status> {
        let parent_rel = parent_of(rel)?;
        let directory = self.walk_mut(parent_rel)?;
        let writable = directory.attributes().contains(NodeAttributes::WRITEABLE);
        let name = last_segment(rel)?;
        let children = children_of_mut(directory)?;
        if !writable {
            return Err(Status::NotAccessible);
        }
        if children.contains_key(name) {
            return Err(Status::Exists);
        }
        let node = match kind {
            NodeKind::Directory => Node::Directory { attributes, children: BTreeMap::new() },
            NodeKind::Property => Node::Property { attributes, value: Vec::new() },
            NodeKind::Stream => Node::Stream { attributes, data: Vec::new() },
            // 符号链接经 create_symlink 创建。
            NodeKind::SymbolicLink => return Err(Status::IllegalArgument),
        };
        children.insert(name.to_owned(), node);
        self.generation += 1;
        Ok(())
    }

    /// 创建符号链接（持久化路径文本，不解释）。
    pub fn link(&mut self, rel: &[u8], target: &[u8]) -> Result<(), Status> {
        let target = core::str::from_utf8(target).map_err(|_| Status::IllegalArgument)?;
        let parent_rel = parent_of(rel)?;
        let directory = self.walk_mut(parent_rel)?;
        let writable = directory.attributes().contains(NodeAttributes::WRITEABLE);
        let name = last_segment(rel)?;
        let children = children_of_mut(directory)?;
        if !writable {
            return Err(Status::NotAccessible);
        }
        if children.contains_key(name) {
            return Err(Status::Exists);
        }
        children.insert(name.to_owned(), Node::SymbolicLink { target: target.to_owned() });
        self.generation += 1;
        Ok(())
    }

    /// 删除节点（目录须为空）。
    pub fn delete(&mut self, rel: &[u8]) -> Result<(), Status> {
        let parent_rel = parent_of(rel)?;
        let directory = self.walk_mut(parent_rel)?;
        let writable = directory.attributes().contains(NodeAttributes::WRITEABLE);
        let name = last_segment(rel)?;
        let children = children_of_mut(directory)?;
        if !writable {
            return Err(Status::NotAccessible);
        }
        match children.get(name) {
            None => Err(Status::NotFound),
            Some(Node::Directory { children: inner, .. }) if !inner.is_empty() => {
                Err(Status::IllegalArgument)
            }
            Some(_) => {
                children.remove(name);
                self.generation += 1;
                Ok(())
            }
        }
    }

    /// 读属性值。
    pub fn property_read(&self, policy: ResolvePolicy, rel: &[u8]) -> Result<&[u8], Status> {
        match self.node_at(policy, rel)? {
            Node::Property { attributes, value } => {
                if !attributes.contains(NodeAttributes::READABLE) {
                    return Err(Status::NotAccessible);
                }
                Ok(value)
            }
            _ => Err(Status::HandleKindMismatch),
        }
    }

    /// 写属性值（整体替换）。
    pub fn property_write(
        &mut self,
        _policy: ResolvePolicy,
        rel: &[u8],
        value: &[u8],
    ) -> Result<(), Status> {
        match self.walk_mut(rel)? {
            Node::Property { attributes, value: stored } => {
                if !attributes.contains(NodeAttributes::WRITEABLE) {
                    return Err(Status::NotAccessible);
                }
                stored.clear();
                stored.extend_from_slice(value);
                self.generation += 1;
                Ok(())
            }
            Node::SymbolicLink { .. } => Err(Status::SymbolicLinkEncountered),
            _ => Err(Status::HandleKindMismatch),
        }
    }

    /// 偏移读。
    pub fn read_at(
        &self,
        policy: ResolvePolicy,
        rel: &[u8],
        offset: u64,
        len: u32,
    ) -> Result<&[u8], Status> {
        match self.node_at(policy, rel)? {
            Node::Stream { attributes, data } => {
                if !attributes.contains(NodeAttributes::READABLE) {
                    return Err(Status::NotAccessible);
                }
                let start = offset as usize;
                let end = start.saturating_add(len as usize);
                if end > data.len() {
                    return Err(Status::IllegalArgument);
                }
                Ok(&data[start..end])
            }
            _ => Err(Status::HandleKindMismatch),
        }
    }

    /// 偏移写（必要时以零扩展）。
    pub fn write_at(
        &mut self,
        policy: ResolvePolicy,
        rel: &[u8],
        offset: u64,
        bytes: &[u8],
    ) -> Result<u32, Status> {
        let _ = self.node_at(policy, rel)?; // 只读行走先行校验种类与权限。
        match self.walk_mut(rel)? {
            Node::Stream { attributes, data } => {
                if !attributes.contains(NodeAttributes::WRITEABLE) {
                    return Err(Status::NotAccessible);
                }
                let start = offset as usize;
                let end = start + bytes.len();
                if end > data.len() {
                    data.resize(end, 0);
                }
                data[start..end].copy_from_slice(bytes);
                Ok(bytes.len() as u32)
            }
            Node::SymbolicLink { .. } => Err(Status::SymbolicLinkEncountered),
            _ => Err(Status::HandleKindMismatch),
        }
    }

    /// 目录枚举：cursor 为 (generation, index) 的不透明编码；
    /// 代数不匹配即 CursorInvalid。
    pub fn enumerate(&self, rel: &[u8], cursor: u64, max_bytes: u32) -> Result<MemPage, Status> {
        let directory = match self.node_at(ResolvePolicy::FollowAll, rel)? {
            Node::Directory { attributes, children } => {
                if !attributes.contains(NodeAttributes::READABLE) {
                    return Err(Status::NotAccessible);
                }
                children
            }
            _ => return Err(Status::NotADirectory),
        };
        let generation = cursor >> 32;
        let index = (cursor & 0xFFFF_FFFF) as usize;
        if cursor != 0 && generation != self.generation {
            return Err(Status::CursorInvalid);
        }
        let mut entries = Vec::new();
        let mut used = 0usize;
        let mut position = 0usize;
        let mut next = 0u64;
        for (name, child) in directory {
            if position < index {
                position += 1;
                continue;
            }
            let cost = name.len() + 12;
            if used + cost > max_bytes as usize && !entries.is_empty() {
                next = (self.generation << 32) | position as u64;
                break;
            }
            used += cost;
            entries.push((name.clone(), child.kind()));
            position += 1;
        }
        Ok(MemPage { entries, next_cursor: next })
    }
}

/// 父路径：去掉终段后的相对路径（根下为空）。
fn parent_of(rel: &[u8]) -> Result<&[u8], Status> {
    if split_rel(rel)?.is_empty() {
        return Err(Status::IllegalPath);
    }
    match rel.iter().rposition(|&b| b == b'/') {
        Some(position) => Ok(&rel[..position]),
        None => Ok(&[]),
    }
}

fn last_segment(rel: &[u8]) -> Result<&str, Status> {
    let text = core::str::from_utf8(rel).map_err(|_| Status::IllegalPath)?;
    text.rsplit('/').next().ok_or(Status::IllegalPath)
}

/// 拆分服务内相对路径：空路径 = 根；拒绝空段、`.`、`..` 与绝对形式。
fn split_rel(rel: &[u8]) -> Result<Vec<String>, Status> {
    if !validate_path(rel) {
        return Err(Status::IllegalPath);
    }
    if rel.is_empty() {
        return Ok(Vec::new());
    }
    Ok(core::str::from_utf8(rel)
        .map_err(|_| Status::IllegalPath)?
        .split('/')
        .map(str::to_owned)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rw() -> NodeAttributes {
        NodeAttributes::READABLE | NodeAttributes::WRITEABLE | NodeAttributes::EXECUTABLE
    }

    #[test]
    fn create_lookup_delete_cycle() {
        let mut fs = MemFs::new();
        fs.create(b"hello", NodeKind::Directory, rw()).unwrap();
        fs.create(b"hello/world", NodeKind::Property, rw()).unwrap();

        let found = fs.lookup(ResolvePolicy::FollowAll, b"hello/world").unwrap();
        assert!(matches!(&found, MemLookup::Found { kind, .. } if *kind == NodeKind::Property));

        assert!(matches!(fs.create(b"hello", NodeKind::Directory, rw()), Err(Status::Exists)));
        fs.delete(b"hello/world").unwrap();
        fs.delete(b"hello").unwrap();
        assert!(matches!(fs.lookup(ResolvePolicy::FollowAll, b"hello"), Err(Status::NotFound)));
    }

    #[test]
    fn root_lookup_and_info() {
        let fs = MemFs::new();
        let found = fs.lookup(ResolvePolicy::FollowAll, b"").unwrap();
        assert!(matches!(&found, MemLookup::Found { kind, .. } if *kind == NodeKind::Directory));
    }

    #[test]
    fn symlink_boundary_not_interpreted() {
        let mut fs = MemFs::new();
        fs.create(b"target", NodeKind::Property, rw()).unwrap();
        fs.link(b"lnk", b"target").unwrap();

        match fs.lookup(ResolvePolicy::FollowAll, b"lnk").unwrap() {
            MemLookup::Link { parent_rel, target, remaining } => {
                assert_eq!((parent_rel.as_str(), target.as_str(), remaining.as_str()), ("", "target", ""));
            }
            _ => panic!("expected link boundary"),
        }
        // NoFollowFinal 终段链接：Found 含 target。
        let found = fs.lookup(ResolvePolicy::NoFollowFinal, b"lnk").unwrap();
        assert!(matches!(&found,
            MemLookup::Found { kind: NodeKind::SymbolicLink, target: Some(t), .. } if t == "target"));
        assert!(matches!(
            fs.property_read(ResolvePolicy::FollowAll, b"lnk"),
            Err(Status::SymbolicLinkEncountered)
        ));
    }

    #[test]
    fn read_at_and_write_at() {
        let mut fs = MemFs::new();
        fs.create(b"bin", NodeKind::Stream, rw()).unwrap();
        assert_eq!(fs.write_at(ResolvePolicy::FollowAll, b"bin", 4, &[1, 2, 3]).unwrap(), 3);
        assert_eq!(
            fs.read_at(ResolvePolicy::FollowAll, b"bin", 0, 7).unwrap(),
            &[0, 0, 0, 0, 1, 2, 3]
        );
        assert!(matches!(
            fs.read_at(ResolvePolicy::FollowAll, b"bin", 6, 4),
            Err(Status::IllegalArgument)
        ));
    }

    #[test]
    fn property_write_and_read() {
        let mut fs = MemFs::new();
        fs.create(b"answer", NodeKind::Property, rw()).unwrap();
        fs.property_write(ResolvePolicy::FollowAll, b"answer", &[42]).unwrap();
        assert_eq!(fs.property_read(ResolvePolicy::FollowAll, b"answer").unwrap(), &[42]);
    }

    #[test]
    fn enumerate_pages_with_generation_cursor() {
        let mut fs = MemFs::new();
        for name in ["a", "b", "c", "d"] {
            fs.create(name.as_bytes(), NodeKind::Property, rw()).unwrap();
        }
        let first = fs.enumerate(b"", 0, 30).unwrap();
        assert_eq!(first.entries.len(), 2);
        fs.create(b"e", NodeKind::Property, rw()).unwrap();
        assert!(matches!(fs.enumerate(b"", first.next_cursor, 30), Err(Status::CursorInvalid)));
    }

    #[test]
    fn execute_gate_on_traversal() {
        let mut fs = MemFs::new();
        fs.create(
            b"closed",
            NodeKind::Directory,
            NodeAttributes::READABLE | NodeAttributes::WRITEABLE,
        )
        .unwrap();
        fs.create(b"closed/inner", NodeKind::Property, rw()).unwrap();
        assert!(matches!(
            fs.lookup(ResolvePolicy::FollowAll, b"closed/inner"),
            Err(Status::NotAccessible)
        ));
    }
}
