//! 走路引擎：逐子树委托 Lookup、符号链接展开与 `..` 的客户端解释。
//!
//! 状态是单个逻辑位置列表 `logical`（前缀表条目以下的组件，含未确认
//! 尾部）加帧栈 `frames`（`frames[i].base` 分区 provider 区域：`logical
//! [base..]` 可从该帧 Handle 到达）。`..` 对逻辑列表词法回退、按需收缩
//! 帧栈，namespace 根处钳制；绝对 target 对前缀表重启整次解析，相对
//! target 原地替换链接分量；展开次数（40）、组件数、字节数与 Lookup
//! 步数受上限约束。终段策略随请求声明，提供者不解释 target。

use alloc::{string::String, vec::Vec};

use erhino_shared::object::Handle;
use libfal::{
    header::Status,
    lookup::ResolvePolicy,
    node::{NodeAttributes, NodeKind},
};

use crate::prefix::PrefixTable;
use crate::{BYTE_LIMIT, COMPONENT_LIMIT, SYMLINK_LIMIT};

/// Found 的节点元数据摘要（客户端所有权形态；value 尾由传输层交付调用方）。
#[derive(Debug, Clone, PartialEq)]
pub struct NodeSummary {
    pub kind: NodeKind,
    pub attributes: NodeAttributes,
    pub size: u64,
}

/// 单次 Lookup 的客户端视图结果（真实传输把线形映射到这里）。
#[derive(Debug, Clone, PartialEq)]
pub enum LookupOutcome {
    Found(NodeSummary),
    Delegate { dir: Handle, consumed: String, remaining: String },
    Link { consumed: String, target: String, remaining: String },
}

/// 走路传输抽象：host 测试注入 mock，真实实现走 librpc/libfal。
pub trait WalkTransport {
    fn lookup(
        &mut self,
        dir: Handle,
        policy: ResolvePolicy,
        path: &str,
    ) -> Result<LookupOutcome, Status>;
}

/// 解析错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    /// 路径不是绝对路径或含空段。
    IllegalPath,
    /// 无前缀条目命中（namespace 未覆盖）。
    NoProvider,
    /// 提供者返回的错误状态。
    Status(Status),
    /// 符号链接展开超限（40）。
    TooManyLinks,
    /// 组件数或字节预算耗尽。
    BudgetExceeded,
    /// 单次解析的 Lookup 步数超限（提供者退化或恶意循环）。
    StepLimit,
}

impl ResolveError {
    /// 归一化为 FAL 状态：op 层透传给调用方。
    pub fn status(self) -> Status {
        match self {
            Self::IllegalPath => Status::IllegalPath,
            Self::NoProvider => Status::NotFound,
            Self::Status(status) => status,
            Self::TooManyLinks => Status::TooManyLinks,
            Self::BudgetExceeded | Self::StepLimit => Status::IllegalArgument,
        }
    }
}

/// 解析终点：帧锚 Handle + 相对后缀与节点信息。
///
/// 后续操作（Enumerate/PropertyRead/…）以 `anchor` 为 Handle slot 1、
/// `rel` 为 body 内相对路径寻址；两个值来自最后一次成功的 Lookup。
#[derive(Debug, Clone, PartialEq)]
pub struct Position {
    pub anchor: Handle,
    pub rel: String,
    pub info: NodeSummary,
}

/// ResolveParent 的结果：父目录位置 + 终段名。
#[derive(Debug, Clone, PartialEq)]
pub struct ParentPosition {
    pub dir: Position,
    pub child: String,
}

/// 单次解析的 Lookup 步数上限。
const STEP_LIMIT: usize = 256;

struct Frame {
    dir: Handle,
    base: usize,
}

struct Budget {
    links: usize,
    components: usize,
    bytes: usize,
}

impl Budget {
    fn charge(&mut self, name: &str) -> Result<(), ResolveError> {
        self.components += 1;
        self.bytes += name.len();
        if self.components > COMPONENT_LIMIT || self.bytes > BYTE_LIMIT {
            return Err(ResolveError::BudgetExceeded);
        }
        Ok(())
    }
}

fn split_absolute(path: &str) -> Result<Vec<&str>, ResolveError> {
    if !path.starts_with('/') {
        return Err(ResolveError::IllegalPath);
    }
    let body = &path[1..];
    if body.is_empty() {
        return Ok(Vec::new());
    }
    let segments = body.split('/').collect::<Vec<_>>();
    if segments.iter().any(|s| s.is_empty()) {
        return Err(ResolveError::IllegalPath);
    }
    Ok(segments)
}

/// 解析绝对路径至终点。`policy` 为 FollowAll / NoFollowFinal；
/// ResolveParent 用 [`resolve_parent`]。
pub fn resolve(
    transport: &mut impl WalkTransport,
    table: &PrefixTable,
    path: &str,
    policy: ResolvePolicy,
) -> Result<Position, ResolveError> {
    debug_assert!(matches!(
        policy,
        ResolvePolicy::FollowAll | ResolvePolicy::NoFollowFinal
    ));
    let mut budget = Budget { links: SYMLINK_LIMIT, components: 0, bytes: 0 };
    if !path.starts_with('/') {
        return Err(ResolveError::IllegalPath);
    }
    let mut steps = 0usize;
    let mut current = String::from(path);

    'restart: loop {
        let (entry, suffix) = table.match_path(&current).ok_or(ResolveError::NoProvider)?;
        let mut frames = Vec::new();
        frames.push(Frame { dir: entry.directory, base: 0 });
        let mut logical: Vec<String> = Vec::new();
        for segment in split_absolute_suffix(suffix)? {
            budget.charge(segment)?;
            logical.push(String::from(segment));
        }

        loop {
            normalize(&mut logical, &mut frames);
            let frame = frames.last().expect("frames never empty");
            let rel = logical[frame.base..].join("/");
            steps += 1;
            if steps > STEP_LIMIT {
                return Err(ResolveError::StepLimit);
            }

            if rel.is_empty() {
                // 位置即帧锚本身：空路径查询其节点信息。
                let info = transport.lookup(frame.dir, policy, "").map_err(ResolveError::Status)?;
                return match info {
                    LookupOutcome::Found(info) => Ok(Position {
                        anchor: frame.dir,
                        rel: String::new(),
                        info,
                    }),
                    // 空路径不产生边界：提供者不得对根返回 Delegate/Link。
                    _ => Err(ResolveError::Status(Status::Internal)),
                };
            }

            let outcome = transport
                .lookup(frame.dir, policy, &rel)
                .map_err(ResolveError::Status)?;
            match outcome {
                LookupOutcome::Found(info) => {
                    return Ok(Position { anchor: frame.dir, rel, info });
                }
                LookupOutcome::Delegate { dir, consumed, remaining } => {
                    let consumed_count = consume_prefix(&mut logical, frame.base, &consumed, &remaining)?;
                    frames.push(Frame { dir, base: frame.base + consumed_count });
                }
                LookupOutcome::Link { consumed, target, remaining } => {
                    if budget.links == 0 {
                        return Err(ResolveError::TooManyLinks);
                    }
                    budget.links -= 1;
                    let consumed_count =
                        consume_prefix(&mut logical, frame.base, &consumed, &remaining)?;
                    let link_at = frame.base + consumed_count;
                    let after: Vec<String> = logical.split_off(link_at + 1);
                    logical.pop(); // 链接分量本身被 target 替换

                    if target.starts_with('/') {
                        let mut full = String::from(target.as_str());
                        for component in &after {
                            full.push('/');
                            full.push_str(component);
                        }
                        current = full;
                        continue 'restart;
                    }
                    for segment in target.split('/') {
                        if segment.is_empty() {
                            return Err(ResolveError::IllegalPath);
                        }
                        budget.charge(segment)?;
                        logical.push(String::from(segment));
                    }
                    logical.extend(after);
                }
            }
        }
    }
}

/// 解析至终段父目录（create/delete/rename 语义）：父路径 FollowAll，
/// 返回父位置与终段名。
pub fn resolve_parent(
    transport: &mut impl WalkTransport,
    table: &PrefixTable,
    path: &str,
) -> Result<ParentPosition, ResolveError> {
    let mut segments = split_absolute(path)?;
    let child = segments.pop().ok_or(ResolveError::IllegalPath)?;
    if child == "." || child == ".." {
        return Err(ResolveError::IllegalPath);
    }
    let mut parent = String::from("/");
    parent.push_str(&segments.join("/"));
    let dir = resolve(transport, table, &parent, ResolvePolicy::FollowAll)?;
    Ok(ParentPosition { dir, child: String::from(child) })
}

fn split_absolute_suffix(suffix: &str) -> Result<Vec<&str>, ResolveError> {
    if suffix.is_empty() {
        return Ok(Vec::new());
    }
    let segments = suffix.split('/').collect::<Vec<_>>();
    // 空段非法；`.` 与 `..` 合法——由词法归一在走路前处理。
    if segments.iter().any(|s| s.is_empty()) {
        return Err(ResolveError::IllegalPath);
    }
    Ok(segments)
}

/// `.`/`..` 的词法处理：`..` 弹出逻辑组件并收缩越界帧，根处钳制。
fn normalize(logical: &mut Vec<String>, frames: &mut Vec<Frame>) {
    let mut work: Vec<String> = Vec::with_capacity(logical.len());
    let mut changed = false;
    for component in logical.drain(..) {
        match component.as_str() {
            "." => changed = true,
            ".." => {
                changed = true;
                if work.pop().is_some() {
                    while frames.len() > 1 && frames.last().unwrap().base > work.len() {
                        frames.pop();
                    }
                }
            }
            name => work.push(String::from(name)),
        }
    }
    if changed {
        while frames.len() > 1 && frames.last().unwrap().base > work.len() {
            frames.pop();
        }
    }
    *logical = work;
}

/// 验证 consumed/remaining 与逻辑列表吻合，返回 consumed 组件数。
/// 逻辑列表本身不变（帧分区推进由调用方完成）。
fn consume_prefix(
    logical: &[String],
    base: usize,
    consumed: &str,
    remaining: &str,
) -> Result<usize, ResolveError> {
    let consumed_segments: Vec<&str> =
        if consumed.is_empty() { Vec::new() } else { consumed.split('/').collect() };
    let remaining_segments: Vec<&str> =
        if remaining.is_empty() { Vec::new() } else { remaining.split('/').collect() };
    let total = consumed_segments.len() + remaining_segments.len();
    if logical.len() < base + total {
        return Err(ResolveError::IllegalPath);
    }
    for (offset, segment) in consumed_segments.iter().enumerate() {
        if logical[base + offset] != *segment {
            return Err(ResolveError::IllegalPath);
        }
    }
    for (offset, segment) in remaining_segments.iter().enumerate() {
        if logical[base + consumed_segments.len() + offset] != *segment {
            return Err(ResolveError::IllegalPath);
        }
    }
    Ok(consumed_segments.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use libfal::lookup::ResolvePolicy::{FollowAll, NoFollowFinal};
    use libfal::node::{NodeAttributes, NodeKind};

    fn handle(raw: u64) -> Handle {
        Handle::from_raw(raw)
    }

    fn info(kind: NodeKind) -> NodeSummary {
        NodeSummary { kind, attributes: NodeAttributes::NONE, size: 0 }
    }

    /// mock 提供者图（grant "/" → A=1）：
    /// A: a/{b/{deep, sub→委托 B}, link→"b/deep", abst→"/a/b", loop→"loop"}
    /// B（3）: sub 根下有 x。
    struct MockTransport {
        last_policy: Option<ResolvePolicy>,
    }

    impl MockTransport {
        fn new() -> Self {
            Self { last_policy: None }
        }
    }

    impl WalkTransport for MockTransport {
        fn lookup(
            &mut self,
            dir: Handle,
            policy: ResolvePolicy,
            path: &str,
        ) -> Result<LookupOutcome, Status> {
            self.last_policy = Some(policy);
            let found = |kind| Ok(LookupOutcome::Found(info(kind)));
            match (dir.raw(), path) {
                (1, "") => found(NodeKind::Directory),
                (1, "a") => found(NodeKind::Directory),
                (1, "a/b") | (1, "a/b/deep") if path == "a/b/deep" => {
                    found(NodeKind::Property)
                }
                (1, "a/b") => found(NodeKind::Directory),
                // 委托边界：consumed 含边界分量 sub，dir2 为其 Handle。
                (1, "a/b/sub") => Ok(LookupOutcome::Delegate {
                    dir: handle(3),
                    consumed: String::from("a/b/sub"),
                    remaining: String::new(),
                }),
                (1, "a/b/sub/x") | (1, "a/b/sub/extra") => Ok(LookupOutcome::Delegate {
                    dir: handle(3),
                    consumed: String::from("a/b/sub"),
                    remaining: String::from(if path == "a/b/sub/x" { "x" } else { "extra" }),
                }),
                (1, "a/link") => match policy {
                    ResolvePolicy::NoFollowFinal => found(NodeKind::SymbolicLink),
                    _ => Ok(LookupOutcome::Link {
                        consumed: String::from("a"),
                        target: String::from("b/deep"),
                        remaining: String::new(),
                    }),
                },
                (1, "a/abst") => match policy {
                    ResolvePolicy::NoFollowFinal => found(NodeKind::SymbolicLink),
                    _ => Ok(LookupOutcome::Link {
                        consumed: String::from("a"),
                        target: String::from("/a/b"),
                        remaining: String::new(),
                    }),
                },
                (1, "a/loop") => Ok(LookupOutcome::Link {
                    consumed: String::from("a"),
                    target: String::from("loop"),
                    remaining: String::new(),
                }),
                (3, "") => found(NodeKind::Directory),
                (3, "x") | (3, "extra") => found(NodeKind::Stream),
                _ => Err(Status::NotFound),
            }
        }
    }

    fn table() -> PrefixTable {
        let mut table = PrefixTable::new();
        table.mount("/", handle(1)).unwrap();
        table
    }

    #[test]
    fn plain_lookup_and_root() {
        let mut mock = MockTransport::new();
        let position = resolve(&mut mock, &table(), "/", FollowAll).unwrap();
        assert_eq!(position.info.kind, NodeKind::Directory);
        assert_eq!(position.rel, "");

        let position = resolve(&mut mock, &table(), "/a/b/deep", FollowAll).unwrap();
        assert_eq!(position.info.kind, NodeKind::Property);
        assert_eq!(position.anchor, handle(1));
        assert_eq!(position.rel, "a/b/deep");
    }

    #[test]
    fn policy_is_passed_through() {
        let mut mock = MockTransport::new();
        resolve(&mut mock, &table(), "/a/b", NoFollowFinal).unwrap();
        assert_eq!(mock.last_policy, Some(NoFollowFinal));
    }

    #[test]
    fn relative_and_absolute_symlink_expansion() {
        let mut mock = MockTransport::new();
        let position = resolve(&mut mock, &table(), "/a/link", FollowAll).unwrap();
        assert_eq!(position.info.kind, NodeKind::Property);
        assert_eq!(position.rel, "a/b/deep");

        let position = resolve(&mut mock, &table(), "/a/abst", FollowAll).unwrap();
        assert_eq!(position.rel, "a/b");
    }

    #[test]
    fn no_follow_final_returns_link_node() {
        let mut mock = MockTransport::new();
        let position = resolve(&mut mock, &table(), "/a/link", NoFollowFinal).unwrap();
        assert_eq!(position.info.kind, NodeKind::SymbolicLink);
        assert_eq!(position.rel, "a/link");
    }

    #[test]
    fn symlink_loop_hits_limit() {
        let mut mock = MockTransport::new();
        assert_eq!(
            resolve(&mut mock, &table(), "/a/loop", FollowAll).unwrap_err(),
            ResolveError::TooManyLinks
        );
    }

    #[test]
    fn delegation_crosses_provider() {
        let mut mock = MockTransport::new();
        let position = resolve(&mut mock, &table(), "/a/b/sub/x", FollowAll).unwrap();
        assert_eq!(position.info.kind, NodeKind::Stream);
        assert_eq!(position.anchor, handle(3));
        assert_eq!(position.rel, "x");

        // 委托边界 + 剩余后缀。
        let position = resolve(&mut mock, &table(), "/a/b/sub/extra", FollowAll).unwrap();
        assert_eq!(position.anchor, handle(3));
        assert_eq!(position.rel, "extra");
    }

    #[test]
    fn dotdot_crosses_delegation_boundary() {
        let mut mock = MockTransport::new();
        // 词法归一发生在走路前：sub/.. 抵消，A 内直达 deep。
        let position = resolve(&mut mock, &table(), "/a/b/sub/../deep", FollowAll).unwrap();
        assert_eq!(position.info.kind, NodeKind::Property);
        assert_eq!(position.rel, "a/b/deep");
        assert_eq!(position.anchor, handle(1));
    }

    #[test]
    fn dotdot_clamps_at_namespace_root() {
        let mut mock = MockTransport::new();
        let position = resolve(&mut mock, &table(), "/../../a/b/deep", FollowAll).unwrap();
        assert_eq!(position.rel, "a/b/deep");
    }

    #[test]
    fn resolve_parent_splits_final_component() {
        let mut mock = MockTransport::new();
        let parent = resolve_parent(&mut mock, &table(), "/a/b/new").unwrap();
        assert_eq!(parent.dir.rel, "a/b");
        assert_eq!(parent.dir.info.kind, NodeKind::Directory);
        assert_eq!(parent.child, "new");

        // 根下创建：父位置即根锚。
        let parent = resolve_parent(&mut mock, &table(), "/new").unwrap();
        assert_eq!(parent.dir.rel, "");
        assert_eq!(parent.child, "new");
    }

    #[test]
    fn rejects_relative_and_empty_segments() {
        let mut mock = MockTransport::new();
        assert_eq!(
            resolve(&mut mock, &table(), "a/b", FollowAll).unwrap_err(),
            ResolveError::IllegalPath
        );
        assert_eq!(
            resolve(&mut mock, &table(), "/a//b", FollowAll).unwrap_err(),
            ResolveError::IllegalPath
        );
    }
}
