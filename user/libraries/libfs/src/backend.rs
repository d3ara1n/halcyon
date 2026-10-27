//! 单一内存文件 backing：目录成员、纯值、稀疏数据和弱父关系均只有一份真值。

use crate::{
    authority::{AccessSnapshot, FsRights},
    data::{Data, PreparedLength, PreparedWrite},
    management::ManagementSnapshot,
    markers,
    node::NodeKind,
    protocol::{BODY_MAX, Status},
    resource::FsResource,
    store::{NodeId, NodeRef, NodeStore, Payload, PreparedNode, RetireContext, RetireProgress},
    value::{Scalar, StoredProperty},
    watch::WatchTable,
};
use alloc::{rc::Rc, string::String, sync::Arc};
use erhino_shared::{call::SystemCallError, object::Rights};
use libbudget::{AccountView, Charge};
use libkernel::ipc::message::MailboxSender;
use liboal::{Boundary, LinkBoundary, ObjectInfo, ProviderBoundary, Tag};
use librpc::bytes::{Reader, Writer};
use metadata_admission::{Counter, Permit};
use ordered_table::{OrderedTable, PreparedEntry};

fn system(error: SystemCallError) -> Status {
    match error {
        SystemCallError::QuotaExceeded => Status::Quota,
        SystemCallError::OutOfMemory => Status::Resource,
        SystemCallError::ObjectClosed => Status::Revoked,
        SystemCallError::ObjectBusy => Status::Busy,
        _ => Status::Resource,
    }
}
fn text(value: &str) -> Result<String, Status> {
    let mut s = String::new();
    s.try_reserve_exact(value.len())
        .map_err(|_| Status::Resource)?;
    s.push_str(value);
    Ok(s)
}

/// 所有引入/改名路径共用的 D-F2 准入；max_info 是种类所有允许形状的上界。
pub fn admit_member(name: &str, max_info: usize) -> Result<(), Status> {
    liboal::path::validate_name(name).map_err(|_| Status::Invalid)?;
    let required = name
        .len()
        .checked_mul(2)
        .and_then(|n| n.checked_add(44))
        .and_then(|n| n.checked_add(max_info))
        .ok_or(Status::EntryTooLarge)?;
    if required > BODY_MAX {
        return Err(Status::EntryTooLarge);
    }
    Ok(())
}

/// 请求快照只保留稳定记录的业务 pin；能力与费用始终留在 backing。
#[derive(Clone)]
pub struct Binding {
    reference: NodeRef,
    max_rights: FsRights,
    output_transport: Rights,
}
struct BindingRecord {
    issuer: Option<MailboxSender>,
    shared: Option<MailboxSender>,
    max_rights: FsRights,
    output_transport: Rights,
}
impl Binding {
    pub fn id(&self) -> NodeId {
        self.reference.id()
    }
    pub fn max_rights(&self) -> FsRights {
        self.max_rights
    }
    pub fn output_transport(&self) -> Rights {
        self.output_transport
    }
}

enum MemberKind {
    Local(NodeId),
    Binding(NodeId),
}
struct Member {
    kind: MemberKind,
    slot: Option<Permit>,
    _charge: Charge,
}
struct Directory {
    members: OrderedTable<Member, (), String>,
}
struct Link {
    target: String,
    _charge: Charge,
}
enum Body {
    Directory(Directory),
    Property(StoredProperty),
    Stream(Data),
    Link(Link),
}
struct MemoryNode {
    body: Body,
    ceiling: FsRights,
    version: u64,
    parent: Option<NodeId>,
}
impl MemoryNode {
    fn kind(&self) -> NodeKind {
        match self.body {
            Body::Directory(_) => NodeKind::Directory,
            Body::Property(_) => NodeKind::Property,
            Body::Stream(_) => NodeKind::Stream,
            Body::Link(_) => NodeKind::SymbolicLink,
        }
    }
}
enum BackingRecord {
    File(MemoryNode),
    Binding(BindingRecord),
}
impl Payload for BackingRecord {
    fn retire(
        &mut self,
        ctx: &mut RetireContext<'_, Self>,
        budget: usize,
    ) -> Result<RetireProgress, SystemCallError> {
        match self {
            Self::File(node) => node.retire(ctx, budget),
            Self::Binding(binding) => {
                let owner = if binding.issuer.is_some() {
                    &mut binding.issuer
                } else {
                    &mut binding.shared
                };
                let work_done = if let Some(sender) = owner.take() {
                    // 两个均为非映射叶 owner；每预算项关闭一个，不需失败轮转。
                    drop(sender);
                    1
                } else {
                    0
                };
                Ok(RetireProgress {
                    work_done,
                    done: binding.issuer.is_none() && binding.shared.is_none(),
                })
            }
        }
    }
}
impl MemoryNode {
    fn retire(
        &mut self,
        ctx: &mut RetireContext<'_, BackingRecord>,
        budget: usize,
    ) -> Result<RetireProgress, SystemCallError> {
        let mut work = 0;
        match &mut self.body {
            Body::Directory(dir) => {
                while work < budget {
                    let Some(entry) = dir.members.pop_first() else {
                        break;
                    };
                    let (_, entry) = entry.into_parts();
                    match entry.kind {
                        MemberKind::Local(id) => ctx.unlink(id),
                        MemberKind::Binding(id) => ctx.unlink(id),
                    }
                    work += 1;
                }
                Ok(RetireProgress {
                    work_done: work,
                    done: dir.members.is_empty(),
                })
            }
            Body::Stream(data) => {
                let work_done = data.retire_step(budget);
                Ok(RetireProgress {
                    work_done,
                    done: data.retired(),
                })
            }
            Body::Property(_) | Body::Link(_) => Ok(RetireProgress {
                work_done: 0,
                done: true,
            }),
        }
    }
}

pub enum Resolved {
    Local(NodeRef),
    Provider {
        consumed_bytes: u32,
        remaining: String,
        binding: Binding,
        parent: NodeRef,
    },
    Link {
        parent_bytes: u32,
        target: String,
        remaining: String,
    },
}
impl Resolved {
    pub fn boundary(&self) -> Option<Boundary<'_>> {
        match self {
            Self::Local(_) => None,
            Self::Provider {
                consumed_bytes,
                remaining,
                ..
            } => Some(Boundary::Provider(ProviderBoundary {
                consumed_bytes: *consumed_bytes,
                remaining,
            })),
            Self::Link {
                parent_bytes,
                target,
                remaining,
            } => Some(Boundary::Link(LinkBoundary {
                parent_bytes: *parent_bytes,
                target,
                remaining,
            })),
        }
    }
}
pub enum Discovery<'a> {
    Found(ObjectInfo<'a>),
    Boundary(Resolved),
}
pub enum Enumeration<'a> {
    Page(liboal::protocol::Page<'a>),
    Boundary(Resolved),
}

/// backing 的实际退休活动；存活记录与可立即推进的队列是不同事实。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetirementState {
    Ready,
    Waiting,
    Empty,
}
pub struct MemoryBackend {
    nodes: NodeStore<BackingRecord>,
    pub(crate) watches: WatchTable,
    root: Option<NodeRef>,
    members: Arc<Counter>,
    limit: usize,
    watch_after: Option<u64>,
    watch_scanning: bool,
    watch_dirty: bool,
    watch_retry: bool,
    maintenance_phase: u8,
    trim_after: Option<NodeId>,
    trim_scanning: bool,
    trim_dirty: bool,
    trim_retry: bool,
    data_changed: bool,
}
pub struct PreparedMutation {
    intent: Intent,
}
impl PreparedMutation {
    /// CreateOpen 在名字可见前保管本次新建实例，不能按名字挑另一个文件。
    pub fn created_target(&self) -> Option<NodeRef> {
        match &self.intent {
            Intent::Create { node, .. } => Some(node.reference().clone()),
            _ => None,
        }
    }
}
enum Intent {
    Create {
        access: AccessSnapshot,
        parent: NodeRef,
        node: PreparedNode<BackingRecord>,
        entry: PreparedEntry<Member, (), String>,
    },
    Bind {
        access: ManagementSnapshot,
        parent: NodeRef,
        version: u64,
        ceiling: FsRights,
        previous: Option<NodeId>,
        replace: bool,
        binding: Binding,
        entry: PreparedEntry<Member, (), String>,
    },
    Unbind {
        access: ManagementSnapshot,
        parent: NodeRef,
        version: u64,
        ceiling: FsRights,
        name: String,
        binding: Binding,
    },
    Remove {
        access: AccessSnapshot,
        parent: NodeRef,
        version: u64,
        name: String,
        target: NodeRef,
    },
    Rename {
        source: AccessSnapshot,
        destination: AccessSnapshot,
        from: NodeRef,
        to: NodeRef,
        from_version: u64,
        to_version: u64,
        old_name: String,
        target: NodeRef,
        replaced: Option<NodeRef>,
        entry: PreparedEntry<Member, (), String>,
    },
    Set {
        access: AccessSnapshot,
        target: NodeRef,
        version: u64,
        value: StoredProperty,
    },
    Write {
        access: AccessSnapshot,
        target: NodeRef,
        version: u64,
        write: PreparedWrite,
    },
    Length {
        access: AccessSnapshot,
        target: NodeRef,
        version: u64,
        length: PreparedLength,
    },
    Noop {
        source: AccessSnapshot,
        destination: AccessSnapshot,
        parent: NodeRef,
        version: u64,
    },
}
pub struct CommitFailure {
    pub status: Status,
    pub mutation: PreparedMutation,
}

impl MemoryBackend {
    pub fn new(
        account: &AccountView<FsResource>,
        limit: usize,
        watch_limit: usize,
        wake: Rc<dyn libexecution::wake::Wake>,
    ) -> Result<Self, Status> {
        Self::with_bindings(account, limit, 0, watch_limit, wake)
    }
    pub fn with_bindings(
        account: &AccountView<FsResource>,
        limit: usize,
        binding_limit: usize,
        watch_limit: usize,
        wake: Rc<dyn libexecution::wake::Wake>,
    ) -> Result<Self, Status> {
        if limit == 0 {
            return Err(Status::Invalid);
        }
        let watches = WatchTable::new(watch_limit)?;
        let members = Arc::try_new(Counter::new(limit)).map_err(|_| Status::Resource)?;
        let root = MemoryNode {
            body: Body::Directory(Directory {
                members: OrderedTable::new(limit),
            }),
            ceiling: FsRights::ALL,
            version: 1,
            parent: None,
        };
        let (nodes, root) = NodeStore::new(
            BackingRecord::File(root),
            account,
            limit.checked_add(binding_limit).ok_or(Status::Resource)?,
            wake,
        )
        .map_err(|failure| system(failure.error))?;
        Ok(Self {
            nodes,
            watches,
            root: Some(root),
            members,
            limit,
            watch_after: None,
            watch_scanning: false,
            watch_dirty: false,
            watch_retry: false,
            maintenance_phase: 0,
            trim_after: None,
            trim_scanning: false,
            trim_dirty: false,
            trim_retry: false,
            data_changed: false,
        })
    }
    pub fn root(&self) -> Option<&NodeRef> {
        self.root.as_ref()
    }
    fn node(&self, reference: &NodeRef) -> Option<&MemoryNode> {
        match self.nodes.get(reference)? {
            BackingRecord::File(node) => Some(node),
            BackingRecord::Binding(_) => None,
        }
    }
    fn node_mut(&mut self, reference: &NodeRef) -> Option<&mut MemoryNode> {
        match self.nodes.get_mut(reference)? {
            BackingRecord::File(node) => Some(node),
            BackingRecord::Binding(_) => None,
        }
    }
    fn binding(&self, id: NodeId) -> Result<Binding, Status> {
        let reference = self.nodes.pin_linked(id).ok_or(Status::Revoked)?;
        let BackingRecord::Binding(record) = self.nodes.get(&reference).ok_or(Status::Revoked)?
        else {
            return Err(Status::TypeMismatch);
        };
        let max_rights = record.max_rights;
        let output_transport = record.output_transport;
        Ok(Binding {
            reference,
            max_rights,
            output_transport,
        })
    }
    pub fn binding_active(&self, binding: &Binding) -> bool {
        !self.nodes.is_sealed() && self.nodes.state(binding.id()) == crate::store::NodeState::Linked
    }
    #[cfg(any(target_arch = "riscv64", test))]
    pub(crate) fn check_binding_boundary(
        &self,
        access: &AccessSnapshot,
        binding: &Binding,
        parent: &NodeRef,
    ) -> Result<(), Status> {
        self.check_directory_scope(access, parent)?;
        if !self.binding_active(binding) {
            return Err(Status::Revoked);
        }
        Ok(())
    }
    /// 管理根只定义本地作用域，普通 FsRights 不授予管理操作。
    pub fn check_management_scope(
        &self,
        access: &ManagementSnapshot,
        selected: &NodeRef,
    ) -> Result<(), Status> {
        if self.nodes.is_sealed() || !access.is_active() {
            return Err(Status::Revoked);
        }
        self.directory(selected)?;
        let mut current = selected.clone();
        for _ in 0..self.limit {
            if current.id() == access.root().id() {
                return Ok(());
            }
            let parent = self
                .node(&current)
                .and_then(|node| node.parent)
                .ok_or(Status::Permission)?;
            current = self.nodes.pin_live(parent).ok_or(Status::Permission)?;
        }
        Err(Status::Permission)
    }
    /// 仅走真实本地目录；管理请求从不沿 Binding 或链接转发。
    pub fn management_parent<'a>(
        &self,
        access: &ManagementSnapshot,
        path: &'a str,
    ) -> Result<(NodeRef, &'a str), Status> {
        liboal::path::validate_path(path).map_err(|_| Status::Invalid)?;
        if path.is_empty() {
            return Err(Status::Invalid);
        }
        let (prefix, name) = path.rsplit_once('/').unwrap_or(("", path));
        admit_member(name, 0)?;
        let mut parent = access.root().clone();
        self.check_management_scope(access, &parent)?;
        if !prefix.is_empty() {
            for component in prefix.split('/') {
                let member = self
                    .directory(&parent)?
                    .members
                    .get_by(component)
                    .ok_or(Status::NotFound)?;
                let id = match member.kind {
                    MemberKind::Local(id) => id,
                    MemberKind::Binding(_) => return Err(Status::CrossFilesystem),
                };
                parent = self.nodes.pin_linked(id).ok_or(Status::NotFound)?;
                if self
                    .node(&parent)
                    .is_some_and(|node| node.kind() == NodeKind::SymbolicLink)
                {
                    return Err(Status::TypeMismatch);
                }
                self.directory(&parent)?;
            }
        }
        self.check_management_scope(access, &parent)?;
        Ok((parent, name))
    }
    /// 在接管能力/发下游前安装不可见记录，所有取消共用正常退休责任。
    pub fn reserve_binding(
        &mut self,
        max_rights: FsRights,
        output_transport: Rights,
        account: &AccountView<FsResource>,
    ) -> Result<Binding, Status> {
        if !output_transport.is_known() {
            return Err(Status::Invalid);
        }
        let prepared = self
            .nodes
            .prepare_charged(
                BackingRecord::Binding(BindingRecord {
                    issuer: None,
                    shared: None,
                    max_rights,
                    output_transport,
                }),
                account,
                FsResource::Binding,
            )
            .map_err(|failure| system(failure.error))?;
        let reference = self.nodes.commit(prepared);
        Ok(Binding {
            reference,
            max_rights,
            output_transport,
        })
    }
    /// 即使 provider 已封口，已安装准备仍可收容迟到的新 owner 并正常退休。
    pub fn adopt_binding_sender(
        &mut self,
        binding: &Binding,
        sender: MailboxSender,
        shared: bool,
    ) -> Result<(), (MailboxSender, Status)> {
        let Some(BackingRecord::Binding(record)) = self.nodes.get_mut(&binding.reference) else {
            return Err((sender, Status::Revoked));
        };
        let slot = if shared {
            &mut record.shared
        } else {
            &mut record.issuer
        };
        if slot.is_some() {
            return Err((sender, Status::Conflict));
        }
        *slot = Some(sender);
        Ok(())
    }
    pub fn binding_sender(
        &self,
        binding: &Binding,
        shared: bool,
    ) -> Result<&MailboxSender, Status> {
        let Some(BackingRecord::Binding(record)) = self.nodes.get(&binding.reference) else {
            return Err(Status::Revoked);
        };
        if shared {
            record.shared.as_ref()
        } else {
            record.issuer.as_ref()
        }
        .ok_or(Status::Busy)
    }
    pub fn contains(&self, node: &NodeRef) -> bool {
        self.node(node).is_some()
    }
    pub fn ceiling(&self, node: &NodeRef) -> Result<FsRights, Status> {
        self.node(node)
            .map(|node| node.ceiling)
            .ok_or(Status::NotFound)
    }
    /// 对已选择目录按当前弱父链复核授权根范围；不因名字变化重新选择目标。
    pub(crate) fn check_directory_scope(
        &self,
        access: &AccessSnapshot,
        selected: &NodeRef,
    ) -> Result<(), Status> {
        if !access.is_active() {
            return Err(Status::Revoked);
        }
        self.directory(selected)?;
        let mut current = selected.clone();
        for _ in 0..self.limit {
            if current.id() == access.root().id() {
                return Ok(());
            }
            let parent = self
                .node(&current)
                .and_then(|node| node.parent)
                .ok_or(Status::Permission)?;
            current = self.nodes.pin_live(parent).ok_or(Status::Permission)?;
            self.check(access, &current, FsRights::TRAVERSE)?;
        }
        Err(Status::Permission)
    }
    pub fn check_directory_issue(
        &self,
        access: &AccessSnapshot,
        selected: &NodeRef,
        policy: crate::authority::Issuance,
    ) -> Result<(), Status> {
        self.check_directory_scope(access, selected)?;
        policy.validate_derived(access, self.ceiling(selected)?)
    }
    pub(crate) fn check(
        &self,
        access: &AccessSnapshot,
        node: &NodeRef,
        required: FsRights,
    ) -> Result<(), Status> {
        if !access.is_active() {
            return Err(Status::Revoked);
        }
        let node = self.node(node).ok_or(Status::NotFound)?;
        if !access.rights().intersect(node.ceiling).contains(required) {
            return Err(Status::Permission);
        }
        Ok(())
    }
    fn directory(&self, node: &NodeRef) -> Result<&Directory, Status> {
        match &self.node(node).ok_or(Status::NotFound)?.body {
            Body::Directory(dir) => Ok(dir),
            _ => Err(Status::NotDirectory),
        }
    }
    fn directory_mut(&mut self, node: &NodeRef) -> &mut Directory {
        // 只有本次独占提交在确认版本/种类后调用，中间没有借出 backing 或执行外部代码。
        match &mut self
            .node_mut(node)
            .expect("validated directory missing")
            .body
        {
            Body::Directory(dir) => dir,
            _ => unreachable!("validated directory kind changed"),
        }
    }
    fn version(&self, node: &NodeRef) -> Result<u64, Status> {
        let version = self.node(node).ok_or(Status::NotFound)?.version;
        version.checked_add(1).ok_or(Status::Resource)?;
        Ok(version)
    }
    fn bump(&mut self, node: &NodeRef) {
        // prepare/最终复核已确认 version 可增，独占提交期间没有其它事务。
        let node = self.node_mut(node).expect("validated node missing");
        node.version = node
            .version
            .checked_add(1)
            .expect("validated version exhausted");
    }

    /// OAL 和各 fs 操作都直接使用这一次实际遍历；末项跟随行为由领域操作确定。
    pub fn resolve(
        &self,
        access: &AccessSnapshot,
        path: &str,
        follow_final: bool,
    ) -> Result<Resolved, Status> {
        liboal::path::validate_path(path).map_err(|_| Status::Invalid)?;
        if path.len() > BODY_MAX {
            return Err(Status::Invalid);
        }
        if !access.is_active() {
            return Err(Status::Revoked);
        }
        if !self.contains(access.root()) {
            return Err(Status::CrossFilesystem);
        }
        let mut current = access.root().clone();
        if path.is_empty() {
            return Ok(Resolved::Local(current));
        }
        let mut offset = 0usize;
        let mut parts = path.split('/').peekable();
        while let Some(name) = parts.next() {
            self.check(access, &current, FsRights::TRAVERSE)?;
            let dir = self.directory(&current)?;
            let member = dir.members.get_by(name).ok_or(Status::NotFound)?;
            let end = offset.checked_add(name.len()).ok_or(Status::Invalid)?;
            let more = parts.peek().is_some();
            let remaining = if more { &path[end + 1..] } else { "" };
            match &member.kind {
                MemberKind::Binding(id) => {
                    return Ok(Resolved::Provider {
                        consumed_bytes: u32::try_from(end).map_err(|_| Status::Invalid)?,
                        remaining: text(remaining)?,
                        binding: self.binding(*id)?,
                        parent: current.clone(),
                    });
                }
                MemberKind::Local(id) => {
                    current = self.nodes.pin_live(*id).ok_or(Status::NotFound)?;
                }
            }
            if let Body::Link(target) = &self.node(&current).ok_or(Status::NotFound)?.body
                && (more || follow_final)
            {
                return Ok(Resolved::Link {
                    parent_bytes: u32::try_from(offset.saturating_sub(1))
                        .map_err(|_| Status::Invalid)?,
                    target: text(&target.target)?,
                    remaining: text(remaining)?,
                });
            }
            offset = end + 1;
        }
        Ok(Resolved::Local(current))
    }
    pub fn local_directory(
        &self,
        access: &AccessSnapshot,
        path: &str,
        required: FsRights,
    ) -> Result<Resolved, Status> {
        let result = self.resolve(access, path, true)?;
        if let Resolved::Local(node) = &result {
            self.directory(node)?;
            self.check(access, node, required)?;
        }
        Ok(result)
    }
    fn parent<'a>(
        &self,
        access: &AccessSnapshot,
        path: &'a str,
        required: FsRights,
    ) -> Result<(Resolved, &'a str), Status> {
        if path.is_empty() {
            return Err(Status::Permission);
        }
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        Ok((self.local_directory(access, parent, required)?, name))
    }
    pub fn lookup<'a>(
        &self,
        access: &AccessSnapshot,
        path: &str,
        out: &'a mut [u8],
    ) -> Result<Discovery<'a>, Status> {
        if !access
            .rights()
            .contains(FsRights::TRAVERSE | FsRights::DESCRIBE)
        {
            return Err(Status::Permission);
        }
        match self.resolve(access, path, false)? {
            Resolved::Local(node) => {
                self.check(access, &node, FsRights::DESCRIBE)?;
                let len = self.info(&node, out)?;
                let info = ObjectInfo::decode(&out[..len]).map_err(|_| Status::Internal)?;
                Ok(Discovery::Found(info))
            }
            boundary => Ok(Discovery::Boundary(boundary)),
        }
    }
    fn info(&self, node: &NodeRef, out: &mut [u8]) -> Result<usize, Status> {
        let node = self.node(node).ok_or(Status::NotFound)?;
        let mut traits = [liboal::DIRECTORY; 4];
        let mut count = 1usize;
        let mut tags = [Tag {
            key: markers::BYTE_LENGTH,
            value: &[],
        }; 1];
        let mut tag_count = 0;
        let length;
        match &node.body {
            Body::Directory(_) => {}
            Body::Link(_) => traits[0] = markers::SYMBOLIC_LINK,
            Body::Property(value) => {
                let value = value.value();
                traits[0] = markers::REGULAR_FILE;
                traits[1] = markers::PROPERTY;
                traits[2] = match value.scalar() {
                    Scalar::Bool => markers::BOOLEAN,
                    Scalar::Signed64 | Scalar::Unsigned64 => markers::INTEGER,
                    Scalar::Float64 => markers::FLOAT,
                    Scalar::String => markers::STRING,
                    Scalar::Blob => markers::BLOB,
                };
                count = 3;
                if value.is_array() {
                    traits[3] = markers::ARRAY;
                    count = 4;
                }
            }
            Body::Stream(data) => {
                traits[0] = markers::REGULAR_FILE;
                traits[1] = markers::STREAM;
                count = 2;
                length = data.len().to_le_bytes();
                tags[0].value = &length;
                tag_count = 1;
            }
        }
        liboal::info::encode(&traits[..count], &tags[..tag_count], out).map_err(|error| match error
        {
            liboal::protocol::Status::SummaryTooLarge => Status::SummaryTooLarge,
            _ => Status::Resource,
        })
    }

    fn cursor_directory(&self, access: &AccessSnapshot, serial: u64) -> Result<NodeRef, Status> {
        if !access.is_active() {
            return Err(Status::Revoked);
        }
        if !access
            .rights()
            .contains(FsRights::TRAVERSE | FsRights::ENUMERATE)
        {
            return Err(Status::Permission);
        }
        let id = NodeId::from_raw(serial).ok_or(Status::CursorInvalid)?;
        let selected = self.nodes.pin_live(id).ok_or(Status::CursorInvalid)?;
        self.directory(&selected)
            .map_err(|_| Status::CursorInvalid)?;
        self.check(access, &selected, FsRights::ENUMERATE)?;
        self.check(access, &selected, FsRights::TRAVERSE)
            .and_then(|()| self.check_directory_scope(access, &selected))
            .map_err(|_| Status::CursorInvalid)?;
        Ok(selected)
    }
    pub fn enumerate<'a>(
        &self,
        access: &AccessSnapshot,
        incarnation: u64,
        path: &str,
        cursor: &[u8],
        max_entries: u32,
        out: &'a mut [u8],
    ) -> Result<Enumeration<'a>, Status> {
        if !access.is_active() {
            return Err(Status::Revoked);
        }
        if !access
            .rights()
            .contains(FsRights::TRAVERSE | FsRights::ENUMERATE)
        {
            return Err(Status::Permission);
        }
        if incarnation == 0 || max_entries == 0 || (!cursor.is_empty() && !path.is_empty()) {
            return Err(Status::Invalid);
        }
        let (parent, after) = if cursor.is_empty() {
            match self.local_directory(access, path, FsRights::TRAVERSE | FsRights::ENUMERATE)? {
                Resolved::Local(node) => (node, ""),
                boundary => return Ok(Enumeration::Boundary(boundary)),
            }
        } else {
            let mut r = Reader::new(cursor);
            if r.u64().map_err(|_| Status::CursorInvalid)? != incarnation {
                return Err(Status::CursorInvalid);
            }
            let serial = r.u64().map_err(|_| Status::CursorInvalid)?;
            let after = r.text32().map_err(|_| Status::CursorInvalid)?;
            liboal::path::validate_name(after).map_err(|_| Status::CursorInvalid)?;
            r.finish().map_err(|_| Status::CursorInvalid)?;
            (self.cursor_directory(access, serial)?, after)
        };
        let dir = self.directory(&parent)?;
        // 预备整页固定 ABI 上界，页内只输出完整项；不按未验证 count 分配。
        let mut entries = [0u8; BODY_MAX];
        let mut w = Writer::new(&mut entries);
        let mut count = 0u32;
        let mut last = after;
        let mut more = false;
        while let Some((name, member)) =
            dir.members
                .next_after(if last.is_empty() { None } else { Some(last) })
        {
            let mut info = [0u8; NodeKind::Property.max_info_len()];
            let info_len = match member.kind {
                MemberKind::Local(id) => {
                    let node = self.nodes.pin_live(id).ok_or(Status::NotFound)?;
                    self.info(&node, &mut info)?
                }
                MemberKind::Binding(_) => 0,
            };
            let item_len = 8usize
                .checked_add(name.len())
                .and_then(|n| n.checked_add(info_len))
                .ok_or(Status::EntryTooLarge)?;
            let cursor_len = 20usize
                .checked_add(name.len())
                .ok_or(Status::EntryTooLarge)?;
            let full = 16usize
                .checked_add(cursor_len)
                .and_then(|n| n.checked_add(w.written()))
                .and_then(|n| n.checked_add(item_len))
                .ok_or(Status::EntryTooLarge)?;
            if count == max_entries || full > BODY_MAX {
                more = true;
                break;
            }
            w.bytes32(name.as_bytes());
            w.bytes32(&info[..info_len]);
            count += 1;
            last = name;
        }
        if more && count == 0 {
            return Err(Status::EntryTooLarge);
        }
        let entries_len = w.written();
        let mut opaque = [0u8; BODY_MAX];
        let cursor_len = if more {
            let mut w = Writer::new(&mut opaque);
            w.u64(incarnation);
            w.u64(parent.id().raw());
            w.bytes32(last.as_bytes());
            w.written()
        } else {
            0
        };
        let len = 12usize
            .checked_add(cursor_len)
            .and_then(|n| n.checked_add(entries_len))
            .ok_or(Status::EntryTooLarge)?;
        if len + 4 > BODY_MAX || out.len() < len {
            return Err(Status::EntryTooLarge);
        }
        let mut w = Writer::new(out);
        w.bytes32(&opaque[..cursor_len]);
        w.u32(u32::from(!more));
        w.u32(count);
        w.bytes(&entries[..entries_len]);
        let used = w.written();
        Ok(Enumeration::Page(
            liboal::protocol::Page::decode(&out[..used]).map_err(|_| Status::Internal)?,
        ))
    }

    fn prepare_member(
        &self,
        parent: &NodeRef,
        name: &str,
        kind: MemberKind,
        slot: Option<Permit>,
        account: &AccountView<FsResource>,
    ) -> Result<PreparedEntry<Member, (), String>, Status> {
        let key = text(name)?;
        let charge = account
            .acquire(
                FsResource::Bytes,
                key.capacity()
                    .checked_add(PreparedEntry::<Member, (), String>::allocation_bytes())
                    .ok_or(Status::Resource)?,
            )
            .map_err(system)?;
        let value = Member {
            kind,
            slot,
            _charge: charge,
        };
        self.directory(parent)?
            .members
            .prepare_insert_candidate(key, value, &libbudget::AllocatorBacked)
            .map_err(|error| match error {
                ordered_table::InsertError::Limit(_) => Status::Quota,
                ordered_table::InsertError::Storage(error, _) => {
                    system(libbudget::allocator_error(error))
                }
            })
    }
    pub fn prepare_create(
        &self,
        access: &AccessSnapshot,
        parent: NodeRef,
        name: &str,
        kind: NodeKind,
        ceiling: FsRights,
        initial: &[u8],
    ) -> Result<PreparedMutation, Status> {
        admit_member(name, kind.max_info_len())?;
        self.check(access, &parent, FsRights::CREATE)?;
        let dir = self.directory(&parent)?;
        if dir.members.get_by(name).is_some() {
            return Err(Status::Exists);
        }
        self.version(&parent)?;
        let slot = Counter::try_acquire(&self.members).map_err(|_| Status::Quota)?;
        let body = match kind {
            NodeKind::Directory => {
                if !initial.is_empty() {
                    return Err(Status::Invalid);
                }
                Body::Directory(Directory {
                    members: OrderedTable::new(self.limit),
                })
            }
            NodeKind::Property => {
                Body::Property(StoredProperty::prepare(initial, access.account())?)
            }
            NodeKind::Stream => {
                if !initial.is_empty() {
                    return Err(Status::Invalid);
                }
                Body::Stream(Data::new(access.account()).map_err(system)?)
            }
            NodeKind::SymbolicLink => {
                let target = core::str::from_utf8(initial).map_err(|_| Status::Invalid)?;
                liboal::path::validate_target(target).map_err(|_| Status::Invalid)?;
                let target = text(target)?;
                let charge = access
                    .account()
                    .acquire(FsResource::Bytes, target.capacity())
                    .map_err(system)?;
                Body::Link(Link {
                    target,
                    _charge: charge,
                })
            }
        };
        let node = self
            .nodes
            .prepare(
                BackingRecord::File(MemoryNode {
                    body,
                    ceiling,
                    version: 1,
                    parent: if kind == NodeKind::Directory {
                        Some(parent.id())
                    } else {
                        None
                    },
                }),
                access.account(),
            )
            .map_err(|failure| system(failure.error))?;
        let entry = self.prepare_member(
            &parent,
            name,
            MemberKind::Local(node.id()),
            Some(slot),
            access.account(),
        )?;
        Ok(PreparedMutation {
            intent: Intent::Create {
                access: access.clone(),
                parent,
                node,
                entry,
            },
        })
    }
    pub fn prepare_binding(
        &self,
        access: &ManagementSnapshot,
        parent: NodeRef,
        name: &str,
        binding: Binding,
        replace: bool,
    ) -> Result<PreparedMutation, Status> {
        admit_member(name, 0)?;
        self.check_management_scope(access, &parent)?;
        if self.nodes.state(binding.id()) != crate::store::NodeState::Unbound {
            return Err(Status::Conflict);
        }
        if !access.max_rights().contains(binding.max_rights())
            || !binding
                .output_transport()
                .is_subset_of(access.output_transport())
        {
            return Err(Status::Permission);
        }
        let dir = self.directory(&parent)?;
        if let Some(old) = dir.members.get_by(name) {
            if !replace {
                return Err(Status::Exists);
            }
            if matches!(old.kind, MemberKind::Local(_)) {
                return Err(Status::TypeMismatch);
            }
        }
        let version = self.version(&parent)?;
        let previous = match dir.members.get_by(name).map(|member| &member.kind) {
            Some(MemberKind::Binding(id)) => Some(*id),
            _ => None,
        };
        let ceiling = self.ceiling(&parent)?;
        let slot = if dir.members.get_by(name).is_some() {
            None
        } else {
            Some(Counter::try_acquire(&self.members).map_err(|_| Status::Quota)?)
        };
        let entry = self.prepare_member(
            &parent,
            name,
            MemberKind::Binding(binding.id()),
            slot,
            access.account(),
        )?;
        Ok(PreparedMutation {
            intent: Intent::Bind {
                access: access.clone(),
                parent,
                version,
                ceiling,
                previous,
                replace,
                binding,
                entry,
            },
        })
    }
    pub fn prepare_unbind(
        &self,
        access: &ManagementSnapshot,
        parent: NodeRef,
        name: &str,
    ) -> Result<PreparedMutation, Status> {
        admit_member(name, 0)?;
        self.check_management_scope(access, &parent)?;
        let member = self
            .directory(&parent)?
            .members
            .get_by(name)
            .ok_or(Status::NotFound)?;
        let MemberKind::Binding(id) = member.kind else {
            return Err(Status::TypeMismatch);
        };
        Ok(PreparedMutation {
            intent: Intent::Unbind {
                access: access.clone(),
                version: self.version(&parent)?,
                ceiling: self.ceiling(&parent)?,
                parent,
                name: text(name)?,
                binding: self.binding(id)?,
            },
        })
    }
    fn validate_binding_prepared(&self, binding: &Binding) -> Result<(), Status> {
        if self.nodes.state(binding.id()) != crate::store::NodeState::Unbound {
            return Err(Status::Conflict);
        }
        let Some(BackingRecord::Binding(record)) = self.nodes.get(&binding.reference) else {
            return Err(Status::Revoked);
        };
        if record.issuer.is_none() || record.shared.is_none() {
            return Err(Status::Busy);
        }
        Ok(())
    }
    pub fn prepare_remove(
        &self,
        access: &AccessSnapshot,
        parent: NodeRef,
        name: &str,
    ) -> Result<PreparedMutation, Status> {
        self.check(access, &parent, FsRights::REMOVE)?;
        let member = self
            .directory(&parent)?
            .members
            .get_by(name)
            .ok_or(Status::NotFound)?;
        let MemberKind::Local(id) = member.kind else {
            return Err(Status::Busy);
        };
        let target = self.nodes.pin_live(id).ok_or(Status::NotFound)?;
        if let Body::Directory(dir) = &self.node(&target).ok_or(Status::NotFound)?.body
            && !dir.members.is_empty()
        {
            return Err(Status::NotEmpty);
        }
        Ok(PreparedMutation {
            intent: Intent::Remove {
                access: access.clone(),
                version: self.version(&parent)?,
                parent,
                name: text(name)?,
                target,
            },
        })
    }
    pub fn prepare_set(
        &self,
        access: &AccessSnapshot,
        target: NodeRef,
        bytes: &[u8],
    ) -> Result<PreparedMutation, Status> {
        self.check(access, &target, FsRights::WRITE_PROPERTY)?;
        if !matches!(
            self.node(&target).ok_or(Status::NotFound)?.body,
            Body::Property(_)
        ) {
            return Err(Status::TypeMismatch);
        }
        Ok(PreparedMutation {
            intent: Intent::Set {
                access: access.clone(),
                version: self.version(&target)?,
                target,
                value: StoredProperty::prepare(bytes, access.account())?,
            },
        })
    }
    pub fn stream_length(&self, node: &NodeRef) -> Result<u64, Status> {
        match &self.node(node).ok_or(Status::NotFound)?.body {
            Body::Stream(data) => Ok(data.len()),
            _ => Err(Status::TypeMismatch),
        }
    }
    pub fn stream_is_trimming(&self, node: &NodeRef) -> Result<bool, Status> {
        match &self.node(node).ok_or(Status::NotFound)?.body {
            Body::Stream(data) => Ok(data.is_trimming()),
            _ => Err(Status::TypeMismatch),
        }
    }
    pub fn prepare_length(
        &self,
        access: &AccessSnapshot,
        target: NodeRef,
        length: u64,
    ) -> Result<PreparedMutation, Status> {
        self.check(access, &target, FsRights::WRITE_STREAM)?;
        let Body::Stream(data) = &self.node(&target).ok_or(Status::NotFound)?.body else {
            return Err(Status::TypeMismatch);
        };
        Ok(PreparedMutation {
            intent: Intent::Length {
                access: access.clone(),
                version: self.version(&target)?,
                length: data.prepare_length(length).map_err(system)?,
                target,
            },
        })
    }
    pub fn prepare_write(
        &self,
        access: &AccessSnapshot,
        target: NodeRef,
        offset: u64,
        bytes: &[u8],
    ) -> Result<PreparedMutation, Status> {
        self.check(access, &target, FsRights::WRITE_STREAM)?;
        let Body::Stream(data) = &self.node(&target).ok_or(Status::NotFound)?.body else {
            return Err(Status::TypeMismatch);
        };
        Ok(PreparedMutation {
            intent: Intent::Write {
                access: access.clone(),
                version: self.version(&target)?,
                write: data
                    .prepare_write(offset, bytes, access.account())
                    .map_err(system)?,
                target,
            },
        })
    }
    pub fn prepare_rename(
        &self,
        source: &AccessSnapshot,
        from: NodeRef,
        name: &str,
        destination: &AccessSnapshot,
        new_name: &str,
        replace: bool,
    ) -> Result<PreparedMutation, Status> {
        let to = destination.root().clone();
        if !self.contains(destination.root()) || !self.contains(&to) {
            return Err(Status::CrossFilesystem);
        }
        self.check(source, &from, FsRights::REMOVE)?;
        self.check(destination, &to, FsRights::CREATE)?;
        let member = self
            .directory(&from)?
            .members
            .get_by(name)
            .ok_or(Status::NotFound)?;
        let MemberKind::Local(id) = member.kind else {
            return Err(Status::Busy);
        };
        let target = self.nodes.pin_live(id).ok_or(Status::NotFound)?;
        let kind = self.node(&target).ok_or(Status::NotFound)?.kind();
        admit_member(new_name, kind.max_info_len())?;
        if from.id() == to.id() && name == new_name {
            return Ok(PreparedMutation {
                intent: Intent::Noop {
                    source: source.clone(),
                    destination: destination.clone(),
                    parent: from.clone(),
                    version: self.version(&from)?,
                },
            });
        }
        let replaced = if let Some(member) = self.directory(&to)?.members.get_by(new_name) {
            if !replace {
                return Err(Status::Exists);
            }
            self.check(destination, &to, FsRights::REMOVE)?;
            let MemberKind::Local(id) = member.kind else {
                return Err(Status::Busy);
            };
            let old = self.nodes.pin_live(id).ok_or(Status::NotFound)?;
            let node = self.node(&old).ok_or(Status::NotFound)?;
            if (node.kind() == NodeKind::Directory) != (kind == NodeKind::Directory) {
                return Err(Status::TypeMismatch);
            }
            if let Body::Directory(dir) = &node.body
                && !dir.members.is_empty()
            {
                return Err(Status::NotEmpty);
            }
            Some(old)
        } else {
            None
        };
        if kind == NodeKind::Directory {
            self.validate_parent_chain(&target, &to)?;
        }
        let entry = if from.id() == to.id() {
            let key = text(new_name)?;
            let charge = destination
                .account()
                .acquire(
                    FsResource::Bytes,
                    key.capacity()
                        .checked_add(PreparedEntry::<Member, (), String>::allocation_bytes())
                        .ok_or(Status::Resource)?,
                )
                .map_err(system)?;
            PreparedEntry::try_new(
                key,
                Member {
                    kind: MemberKind::Local(id),
                    slot: None,
                    _charge: charge,
                },
                &libbudget::AllocatorBacked,
            )
            .map_err(|error| match error {
                ordered_table::InsertError::Limit(_) => Status::Quota,
                ordered_table::InsertError::Storage(error, _) => {
                    system(libbudget::allocator_error(error))
                }
            })?
        } else {
            self.prepare_member(
                &to,
                new_name,
                MemberKind::Local(id),
                None,
                destination.account(),
            )?
        };
        Ok(PreparedMutation {
            intent: Intent::Rename {
                source: source.clone(),
                destination: destination.clone(),
                from_version: self.version(&from)?,
                to_version: self.version(&to)?,
                from,
                to,
                old_name: text(name)?,
                target,
                replaced,
                entry,
            },
        })
    }
    fn validate_parent_chain(&self, target: &NodeRef, to: &NodeRef) -> Result<(), Status> {
        let mut current = to.clone();
        for _ in 0..self.limit {
            if current.id() == target.id() {
                return Err(Status::Invalid);
            }
            let node = self.node(&current).ok_or(Status::Conflict)?;
            let Some(parent) = node.parent else {
                return Ok(());
            };
            current = self.nodes.pin_live(parent).ok_or(Status::Conflict)?;
        }
        Err(Status::Invalid)
    }
    fn validate_empty(&self, target: &NodeRef) -> Result<(), Status> {
        if let Body::Directory(dir) = &self.node(target).ok_or(Status::Conflict)?.body
            && !dir.members.is_empty()
        {
            return Err(Status::NotEmpty);
        }
        Ok(())
    }
    fn validates(&self, intent: &Intent) -> Result<(), Status> {
        if self.nodes.is_sealed() {
            return Err(Status::Revoked);
        }
        let valid = |access: &AccessSnapshot, node: &NodeRef, version: u64| -> Result<(), Status> {
            if !access.is_active() {
                return Err(Status::Revoked);
            }
            if self.node(node).is_none_or(|node| node.version != version) {
                return Err(Status::Conflict);
            }
            Ok(())
        };
        match intent {
            Intent::Create {
                access,
                parent,
                entry,
                ..
            } => {
                self.check_directory_scope(access, parent)?;
                self.check(access, parent, FsRights::CREATE)?;
                // 创建只要求本名字仍空闲；其他成员变更不使预付准备失效。
                if self
                    .directory(parent)?
                    .members
                    .get_by(entry.key_ref().as_str())
                    .is_some()
                {
                    return Err(Status::Conflict);
                }
                self.version(parent)?;
                Ok(())
            }
            Intent::Bind {
                access,
                parent,
                version,
                binding,
                ceiling,
                previous,
                entry,
                ..
            } => {
                self.check_management_scope(access, parent)?;
                if self.version(parent)? != *version || self.ceiling(parent)? != *ceiling {
                    return Err(Status::Conflict);
                }
                let current = match self
                    .directory(parent)?
                    .members
                    .get_by(entry.key_ref().as_str())
                    .map(|m| &m.kind)
                {
                    None => None,
                    Some(MemberKind::Binding(id)) => Some(*id),
                    Some(MemberKind::Local(_)) => return Err(Status::Conflict),
                };
                if current != *previous {
                    return Err(Status::Conflict);
                }
                self.validate_binding_prepared(binding)
            }
            Intent::Unbind {
                access,
                parent,
                version,
                ceiling,
                name,
                binding,
            } => {
                self.check_management_scope(access, parent)?;
                if self.version(parent)? != *version || self.ceiling(parent)? != *ceiling {
                    return Err(Status::Conflict);
                }
                if !matches!(self.directory(parent)?.members.get_by(name.as_str()).map(|m| &m.kind),
                    Some(MemberKind::Binding(id)) if *id == binding.id())
                {
                    return Err(Status::Conflict);
                }
                Ok(())
            }
            Intent::Remove {
                access,
                parent,
                version,
                target,
                ..
            } => {
                valid(access, parent, *version)?;
                self.validate_empty(target)
            }
            Intent::Set {
                access,
                target,
                version,
                ..
            }
            | Intent::Write {
                access,
                target,
                version,
                ..
            } => valid(access, target, *version),
            Intent::Length {
                access,
                target,
                version,
                length,
            } => {
                valid(access, target, *version)?;
                let Body::Stream(data) = &self.node(target).ok_or(Status::Conflict)?.body else {
                    return Err(Status::TypeMismatch);
                };
                if !data.validates_length(length) {
                    return Err(Status::Conflict);
                }
                Ok(())
            }
            Intent::Rename {
                source,
                destination,
                from,
                to,
                from_version,
                to_version,
                target,
                replaced,
                ..
            } => {
                valid(source, from, *from_version)?;
                valid(destination, to, *to_version)?;
                if let Some(replaced) = replaced {
                    self.validate_empty(replaced)?;
                }
                if self.node(target).ok_or(Status::Conflict)?.kind() == NodeKind::Directory {
                    self.validate_parent_chain(target, to)?;
                }
                Ok(())
            }
            Intent::Noop {
                source,
                destination,
                parent,
                version,
            } => {
                valid(source, parent, *version)?;
                if destination.is_active() {
                    Ok(())
                } else {
                    Err(Status::Revoked)
                }
            }
        }
    }
    #[expect(
        clippy::result_large_err,
        reason = "复核失败返还完整未提交准备，错误路径不分配"
    )]
    pub fn commit(&mut self, mutation: PreparedMutation) -> Result<Option<NodeRef>, CommitFailure> {
        if let Err(status) = self.validates(&mutation.intent) {
            return Err(CommitFailure { status, mutation });
        }
        // 此段仅消费预建 Node/AVL entry 和独占 permit；所有版本及权限已复核，无分配。
        let reference = match mutation.intent {
            Intent::Create {
                parent,
                node,
                entry,
                ..
            } => {
                let child = self.nodes.commit(node);
                self.nodes.link(&child);
                self.directory_mut(&parent).members.insert_prepared(entry);
                self.bump(&parent);
                self.changed(parent.id(), crate::protocol::WATCH_CREATE);
                Some(child)
            }
            Intent::Bind {
                parent,
                replace,
                binding,
                mut entry,
                ..
            } => {
                if replace
                    && let Some(old) = self
                        .directory_mut(&parent)
                        .members
                        .remove_by(entry.key_ref().as_str())
                {
                    let old = old.into_value();
                    entry.value_mut().slot = old.slot;
                    if let MemberKind::Binding(id) = old.kind {
                        self.nodes.unlink_id(id);
                    }
                }
                self.nodes.link(&binding.reference);
                self.directory_mut(&parent).members.insert_prepared(entry);
                self.bump(&parent);
                self.changed(parent.id(), crate::protocol::WATCH_CREATE);
                None
            }
            Intent::Remove {
                parent,
                name,
                target,
                ..
            } => {
                let _removed = self.directory_mut(&parent).members.remove_by(name.as_str());
                if let Some(node) = self.node_mut(&target) {
                    node.parent = None;
                }
                self.nodes.unlink(&target);
                self.bump(&parent);
                self.changed(parent.id(), crate::protocol::WATCH_DELETE);
                self.changed(target.id(), crate::protocol::WATCH_UNBOUND);
                Some(target)
            }
            Intent::Unbind {
                parent,
                name,
                binding,
                ..
            } => {
                let _removed = self.directory_mut(&parent).members.remove_by(name.as_str());
                self.nodes.unlink(&binding.reference);
                self.bump(&parent);
                self.changed(parent.id(), crate::protocol::WATCH_DELETE);
                None
            }
            Intent::Rename {
                from,
                to,
                old_name,
                target,
                replaced,
                mut entry,
                ..
            } => {
                let old = self
                    .directory_mut(&from)
                    .members
                    .remove_by(old_name.as_str())
                    .expect("validated rename source missing")
                    .into_value();
                entry.value_mut().slot = old.slot;
                if let Some(replaced) = replaced {
                    let _old = self
                        .directory_mut(&to)
                        .members
                        .remove_by(entry.key_ref().as_str());
                    if let Some(node) = self.node_mut(&replaced) {
                        node.parent = None;
                    }
                    self.nodes.unlink(&replaced);
                    self.changed(replaced.id(), crate::protocol::WATCH_UNBOUND);
                }
                self.directory_mut(&to).members.insert_prepared(entry);
                if let Some(node) = self.node_mut(&target)
                    && node.kind() == NodeKind::Directory
                {
                    node.parent = Some(to.id());
                }
                self.bump(&from);
                if from.id() != to.id() {
                    self.bump(&to);
                    self.changed(to.id(), crate::protocol::WATCH_RENAME);
                }
                self.changed(from.id(), crate::protocol::WATCH_RENAME);
                Some(target)
            }
            Intent::Set { target, value, .. } => {
                let node = self.node_mut(&target).expect("validated property missing");
                node.body = Body::Property(value);
                self.bump(&target);
                self.changed(target.id(), crate::protocol::WATCH_MODIFY);
                Some(target)
            }
            Intent::Length { target, length, .. } => {
                let node = self.node_mut(&target).expect("validated stream missing");
                let Body::Stream(data) = &mut node.body else {
                    unreachable!("validated stream kind changed");
                };
                data.commit_length(length);
                let trimming = data.is_trimming();
                self.trim_dirty |= trimming;
                self.data_changed = true;
                self.nodes.wake_retirement();
                self.bump(&target);
                self.changed(target.id(), crate::protocol::WATCH_MODIFY);
                None
            }
            Intent::Write { target, write, .. } => {
                let node = self.node_mut(&target).expect("validated stream missing");
                let Body::Stream(data) = &mut node.body else {
                    unreachable!("validated stream kind changed")
                };
                data.commit(write);
                self.bump(&target);
                self.changed(target.id(), crate::protocol::WATCH_MODIFY);
                Some(target)
            }
            Intent::Noop { .. } => None,
        };
        Ok(reference)
    }
    pub fn property(
        &self,
        access: &AccessSnapshot,
        node: &NodeRef,
    ) -> Result<crate::value::Property<'_>, Status> {
        self.check(access, node, FsRights::READ_PROPERTY)?;
        match &self.node(node).ok_or(Status::NotFound)?.body {
            Body::Property(value) => Ok(value.value()),
            _ => Err(Status::TypeMismatch),
        }
    }
    pub fn read_at(
        &self,
        access: &AccessSnapshot,
        node: &NodeRef,
        offset: u64,
        out: &mut [u8],
    ) -> Result<usize, Status> {
        self.check(access, node, FsRights::READ_STREAM)?;
        match &self.node(node).ok_or(Status::NotFound)?.body {
            Body::Stream(data) => Ok(data.read(offset, out)),
            _ => Err(Status::TypeMismatch),
        }
    }
    pub fn read_link(&self, access: &AccessSnapshot, node: &NodeRef) -> Result<&str, Status> {
        self.check(access, node, FsRights::READ_PROPERTY)?;
        match &self.node(node).ok_or(Status::NotFound)?.body {
            Body::Link(target) => Ok(&target.target),
            _ => Err(Status::TypeMismatch),
        }
    }
    pub fn parent_for_operation<'a>(
        &self,
        access: &AccessSnapshot,
        path: &'a str,
        required: FsRights,
    ) -> Result<(Resolved, &'a str), Status> {
        self.parent(access, path, required)
    }
    pub fn seal(&mut self) {
        self.watches.seal();
        self.schedule_watches();
        self.nodes.seal();
        self.root = None;
    }
    pub fn retire_step(&mut self, budget: usize) -> Result<RetireProgress, SystemCallError> {
        self.nodes.retire_step(budget)
    }
    fn changed(&mut self, target: NodeId, bits: u64) {
        if self.watches.is_empty() {
            return;
        }
        self.watches.changed(target, bits);
        self.schedule_watches();
    }
    pub(crate) fn schedule_watches(&mut self) {
        self.watch_dirty = !self.watches.is_empty();
        self.nodes.wake_retirement();
    }
    /// 唤醒可能来自最后业务 pin 消散；弱目标状态在同一轮维护重新核对。
    pub fn begin_maintenance(&mut self) {
        self.watch_dirty = !self.watches.is_empty();
    }
    pub fn take_data_change(&mut self) -> bool {
        core::mem::take(&mut self.data_changed)
    }
    fn has_trim_work(&self) -> bool {
        self.trim_scanning || self.trim_dirty || self.trim_retry
    }
    /// 每单位最多选一条实际记录并删除一个块；不保活已经进入整体退休的节点。
    fn trim_step(&mut self) {
        if !self.trim_scanning {
            if !self.trim_dirty && !self.trim_retry {
                return;
            }
            self.trim_scanning = true;
            self.trim_dirty = false;
            self.trim_retry = false;
            self.trim_after = None;
        }
        let Some(id) = self.nodes.next_record_id(self.trim_after) else {
            self.trim_scanning = false;
            return;
        };
        self.trim_after = Some(id);
        let Some(BackingRecord::File(node)) = self.nodes.live_payload_mut(id) else {
            return;
        };
        let Body::Stream(data) = &mut node.body else {
            return;
        };
        let pending = data.is_trimming();
        data.trim_step(1);
        self.trim_retry |= data.is_trimming();
        if pending && !data.is_trimming() {
            self.data_changed = true;
            self.nodes.wake_retirement();
        }
    }
    /// 退休、Trimming和订阅轮转各占一个工作单元，持续任一类工作不能饿死其它类。
    pub fn maintenance_step(&mut self, budget: usize) -> Result<RetireProgress, SystemCallError> {
        if budget == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let mut done = 0;
        while done < budget {
            if !self.nodes.has_retire_work()
                && !self.has_trim_work()
                && !self.watch_scanning
                && !self.watch_dirty
                && !self.watch_retry
            {
                break;
            }
            done += 1;
            self.maintenance_phase = (self.maintenance_phase + 1) % 3;
            if self.maintenance_phase == 0 {
                if self.nodes.has_retire_work() {
                    self.nodes.retire_step(1)?;
                }
                continue;
            }
            if self.maintenance_phase == 1 {
                self.trim_step();
                continue;
            }
            if !self.watch_scanning {
                if !self.watch_dirty && !self.watch_retry {
                    continue;
                }
                self.watch_dirty = false;
                self.watch_retry = false;
                self.watch_scanning = true;
                self.watch_after = None;
            }
            if let Some(context) = self.watches.next_context(self.watch_after) {
                self.watch_after = Some(context);
                if let Some(target) = self.watches.target(context) {
                    self.watches.refresh(context, self.nodes.state(target));
                }
                if self.watches.signal(context).is_err() {
                    self.watch_retry = true;
                }
            } else {
                self.watch_scanning = false;
            }
        }
        Ok(RetireProgress {
            work_done: done,
            done: self.is_empty(),
        })
    }
    pub fn maintenance_step_at(
        &mut self,
        now: u64,
        budget: usize,
    ) -> Result<RetireProgress, SystemCallError> {
        self.nodes.observe_time(now)?;
        self.maintenance_step(budget)
    }
    pub fn retirement_deadline(&self) -> Option<u64> {
        self.nodes.retirement_deadline()
    }
    pub fn take_retire_failure(&mut self) -> Option<(NodeId, SystemCallError)> {
        self.nodes.take_retire_failure()
    }
    pub fn has_retire_work(&self) -> bool {
        self.nodes.has_retire_work()
    }
    pub fn retirement_state(&self) -> RetirementState {
        if self.has_retire_work()
            || self.has_trim_work()
            || self.watch_scanning
            || self.watch_dirty
            || self.watch_retry
        {
            RetirementState::Ready
        } else if self.is_empty() {
            RetirementState::Empty
        } else {
            RetirementState::Waiting
        }
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty() && self.watches.is_empty()
    }
    #[expect(
        clippy::result_large_err,
        reason = "未完成退休时返还唯一 backing owner，错误路径不分配"
    )]
    pub fn close(self) -> Result<(), Self> {
        if self.nodes.is_sealed() && self.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }
}

#[cfg(test)]
mod tests;
