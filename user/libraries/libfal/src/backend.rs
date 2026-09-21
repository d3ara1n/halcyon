//! 内存后端的稳定名字事务；节点、名字、属性 owner 与流数据分别拥有资源。

use crate::resource::FalResource;
use crate::{
    authority::{AccessSnapshot, FalRights},
    data::{Data, PreparedWrite},
    node::NodeKind,
    store::{NodeId, NodeRef, NodeStore, Payload, PreparedNode, RetireContext, RetireProgress},
    value::{Capability, StoredValue, TakenValue},
};
use alloc::{string::String, sync::Arc};
use erhino_shared::call::SystemCallError;
use libbudget::{AccountView, Charge};
use metadata_admission::{Counter, Permit};
use ordered_table::{OrderedTable, PreparedEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendError {
    NotFound,
    NotDirectory,
    Permission,
    Exists,
    NotEmpty,
    Conflict,
    InvalidName,
    Cycle,
    Busy,
    Closed,
    Resource(SystemCallError),
}
impl From<SystemCallError> for BackendError {
    fn from(error: SystemCallError) -> Self {
        Self::Resource(error)
    }
}

struct Entry {
    node: NodeId,
    slot: Option<Permit>,
    _charge: Charge,
}
pub struct Directory {
    entries: OrderedTable<Entry, String>,
}
pub enum Body<C> {
    Directory(Directory),
    Property(StoredValue<C>),
    Stream(Data),
    Link(String),
}
pub struct MemoryNode<C> {
    body: Body<C>,
    rights: FalRights,
    version: u64,
    parent: Option<NodeId>,
    take_reserved: bool,
}

impl<C> MemoryNode<C> {
    pub fn kind(&self) -> NodeKind {
        match self.body {
            Body::Directory(_) => NodeKind::Directory,
            Body::Property(_) => NodeKind::Property,
            Body::Stream(_) => NodeKind::Stream,
            Body::Link(_) => NodeKind::SymbolicLink,
        }
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn rights(&self) -> FalRights {
        self.rights
    }
    pub fn body(&self) -> &Body<C> {
        &self.body
    }
    pub fn take_reserved(&self) -> bool {
        self.take_reserved
    }
}

impl<C: Capability> Payload for MemoryNode<C> {
    fn retire(
        &mut self,
        context: &mut RetireContext<'_, Self>,
        budget: usize,
    ) -> Result<RetireProgress, SystemCallError> {
        match &mut self.body {
            Body::Directory(directory) => {
                let mut work_done = 0;
                while work_done < budget {
                    let Some((_, entry)) = directory.entries.pop_first() else {
                        break;
                    };
                    context.unlink(entry.node);
                    work_done += 1;
                }
                Ok(RetireProgress {
                    work_done,
                    done: directory.entries.is_empty(),
                })
            }
            Body::Property(value) => {
                let before = value.handles.len();
                let done = value.retire_step(budget)?;
                Ok(RetireProgress {
                    work_done: before - value.handles.len(),
                    done,
                })
            }
            Body::Stream(data) => {
                let work_done = data.retire_step(budget);
                Ok(RetireProgress {
                    work_done,
                    done: data.retired(),
                })
            }
            Body::Link(target) => {
                target.clear();
                Ok(RetireProgress {
                    work_done: 0,
                    done: true,
                })
            }
        }
    }
}

pub struct Position {
    parent: NodeRef,
    name: String,
    expected: Option<(NodeId, u64)>,
}
impl Position {
    pub fn parent(&self) -> &NodeRef {
        &self.parent
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}

pub struct MemoryBackend<C> {
    nodes: NodeStore<MemoryNode<C>>,
    root: Option<NodeRef>,
    entry_slots: Arc<Counter>,
    limit: usize,
    epoch: u64,
}

pub struct PreparedMutation<C> {
    epoch: u64,
    next_epoch: u64,
    intent: Intent<C>,
}

pub struct PreparedTake<C> {
    target: NodeRef,
    version: u64,
    next_version: u64,
    value: Option<TakenValue<C>>,
}

enum Intent<C> {
    Move {
        source: NodeRef,
        destination: NodeRef,
        target: NodeRef,
        source_version: u64,
        destination_version: u64,
        target_version: u64,
        target_next: u64,
        source_next: u64,
        destination_next: u64,
        source_name: String,
        entry: PreparedEntry<Entry, String>,
        ancestor: Option<NodeRef>,
        checked: bool,
    },
    Create {
        parent: NodeRef,
        version: u64,
        next_version: u64,
        node: PreparedNode<MemoryNode<C>>,
        entry: PreparedEntry<Entry, String>,
    },
    Delete {
        parent: NodeRef,
        version: u64,
        next_version: u64,
        target: NodeRef,
        target_version: u64,
        target_next: u64,
        name: String,
    },
    Write {
        target: NodeRef,
        version: u64,
        next_version: u64,
        data: PreparedWrite,
    },
    Property {
        target: NodeRef,
        version: u64,
        next_version: u64,
        value: StoredValue<C>,
    },
}

pub enum CommitResult<C> {
    Created(NodeRef),
    Deleted(NodeRef),
    Written,
    Moved(NodeRef),
    PropertyReplaced(StoredValue<C>),
}
pub struct CommitFailure<C> {
    pub error: BackendError,
    pub mutation: PreparedMutation<C>,
}
pub struct PropertyFailure<C> {
    pub error: BackendError,
    pub value: StoredValue<C>,
}
pub struct CreateFailure<C> {
    pub error: BackendError,
    pub body: Body<C>,
}

fn name(value: &str) -> Result<String, BackendError> {
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.len() > crate::PATH_MAX
        || value
            .bytes()
            .any(|byte| byte == 0 || b"/*?\\".contains(&byte))
    {
        return Err(BackendError::InvalidName);
    }
    let mut owned = String::new();
    owned
        .try_reserve_exact(value.len())
        .map_err(|_| BackendError::Resource(SystemCallError::OutOfMemory))?;
    owned.push_str(value);
    Ok(owned)
}

impl<C: Capability> MemoryBackend<C> {
    pub fn new(
        account: &AccountView<FalResource>,
        limit: usize,
        wake: alloc::rc::Rc<dyn libexecution::wake::Wake>,
    ) -> Result<Self, BackendError> {
        if limit == 0 {
            return Err(BackendError::Resource(SystemCallError::IllegalArgument));
        }
        let entry_slots =
            Arc::try_new(Counter::new(limit)).map_err(|_| SystemCallError::OutOfMemory)?;
        let root = MemoryNode {
            body: Body::Directory(Directory {
                entries: OrderedTable::new(limit),
            }),
            rights: FalRights::ALL,
            version: 1,
            parent: None,
            take_reserved: false,
        };
        let (nodes, root) = NodeStore::new(root, account, limit, wake)
            .map_err(|failure| BackendError::Resource(failure.error))?;
        Ok(Self {
            nodes,
            root: Some(root),
            entry_slots,
            limit,
            epoch: 1,
        })
    }
    pub fn root(&self) -> Option<&NodeRef> {
        self.root.as_ref()
    }
    pub fn get(&self, reference: &NodeRef) -> Option<&MemoryNode<C>> {
        self.nodes.get(reference)
    }
    pub fn position(
        &self,
        parent: &NodeRef,
        final_name: &str,
        expected: Option<(NodeId, u64)>,
    ) -> Result<Position, BackendError> {
        if !matches!(
            self.nodes.get(parent).map(|node| &node.body),
            Some(Body::Directory(_))
        ) {
            return Err(BackendError::NotDirectory);
        }
        Ok(Position {
            parent: parent.clone(),
            name: name(final_name)?,
            expected,
        })
    }
    pub fn lookup_child(
        &self,
        parent: &NodeRef,
        name: &str,
        access: &AccessSnapshot,
    ) -> Result<NodeRef, BackendError> {
        let node = self.nodes.get(parent).ok_or(BackendError::NotFound)?;
        if !access
            .rights()
            .intersect(node.rights)
            .contains(FalRights::TRAVERSE)
        {
            return Err(BackendError::Permission);
        }
        let Body::Directory(directory) = &node.body else {
            return Err(BackendError::NotDirectory);
        };
        let entry = directory
            .entries
            .get_by(name)
            .ok_or(BackendError::NotFound)?;
        self.nodes
            .pin_linked(entry.node)
            .ok_or(BackendError::NotFound)
    }

    pub fn enumerate<F>(
        &self,
        parent: &NodeRef,
        access: &AccessSnapshot,
        cursor: u64,
        limit: usize,
        mut visit: F,
    ) -> Result<u64, BackendError>
    where
        F: FnMut(&str, &NodeRef, &MemoryNode<C>),
    {
        if limit == 0 {
            return Err(BackendError::InvalidName);
        }
        let node = self.nodes.get(parent).ok_or(BackendError::NotFound)?;
        if !access
            .rights()
            .intersect(node.rights)
            .contains(FalRights::ENUMERATE)
        {
            return Err(BackendError::Permission);
        }
        let Body::Directory(directory) = &node.body else {
            return Err(BackendError::NotDirectory);
        };
        let epoch = u32::try_from(self.epoch)
            .map_err(|_| BackendError::Resource(SystemCallError::ReachLimit))?;
        let ordinal = if cursor == 0 {
            0
        } else {
            if (cursor >> 32) as u32 != epoch {
                return Err(BackendError::Conflict);
            }
            usize::try_from(cursor as u32)
                .map_err(|_| BackendError::Resource(SystemCallError::ReachLimit))?
        };
        let mut after = None;
        let mut index = 0usize;
        let mut emitted = 0usize;
        while let Some((name, entry)) = directory.entries.next_after(after) {
            after = Some(name.as_str());
            if index < ordinal {
                index += 1;
                continue;
            }
            let reference = self
                .nodes
                .pin_linked(entry.node)
                .ok_or(BackendError::NotFound)?;
            let child = self.nodes.get(&reference).ok_or(BackendError::NotFound)?;
            if child.take_reserved {
                return Err(BackendError::Busy);
            }
            visit(name, &reference, child);
            emitted += 1;
            index += 1;
            if emitted == limit {
                let more = directory.entries.next_after(after).is_some();
                return Ok(if more {
                    ((epoch as u64) << 32) | index as u64
                } else {
                    0
                });
            }
        }
        Ok(0)
    }
    fn parent(
        &self,
        position: &Position,
        access: &AccessSnapshot,
        required: FalRights,
    ) -> Result<&MemoryNode<C>, BackendError> {
        if self.nodes.is_sealed() {
            return Err(BackendError::Closed);
        }
        let parent = self
            .nodes
            .get(&position.parent)
            .ok_or(BackendError::NotFound)?;
        if !access.rights().intersect(parent.rights).contains(required) {
            return Err(BackendError::Permission);
        }
        if !matches!(parent.body, Body::Directory(_)) {
            return Err(BackendError::NotDirectory);
        }
        Ok(parent)
    }
    fn target(&self, position: &Position) -> Result<NodeRef, BackendError> {
        let parent = self
            .nodes
            .get(&position.parent)
            .ok_or(BackendError::NotFound)?;
        let Body::Directory(directory) = &parent.body else {
            return Err(BackendError::NotDirectory);
        };
        let entry = directory
            .entries
            .get_by(position.name.as_str())
            .ok_or(BackendError::NotFound)?;
        let target = self
            .nodes
            .pin_linked(entry.node)
            .ok_or(BackendError::NotFound)?;
        if position.expected.is_some_and(|(id, version)| {
            id != target.id()
                || self
                    .nodes
                    .get(&target)
                    .is_none_or(|node| node.version != version)
        }) {
            return Err(BackendError::Conflict);
        }
        if self
            .nodes
            .get(&target)
            .is_some_and(|node| node.take_reserved)
        {
            return Err(BackendError::Busy);
        }
        Ok(target)
    }
    pub fn directory_body(&self) -> Body<C> {
        Body::Directory(Directory {
            entries: OrderedTable::new(self.limit),
        })
    }

    pub fn prepare_create(
        &self,
        position: Position,
        access: &AccessSnapshot,
        body: Body<C>,
        rights: FalRights,
    ) -> Result<PreparedMutation<C>, CreateFailure<C>> {
        let parent = match self.parent(&position, access, FalRights::CREATE) {
            Ok(parent) => parent,
            Err(error) => return Err(CreateFailure { error, body }),
        };
        let reserve = (|| {
            let Body::Directory(directory) = &parent.body else {
                unreachable!()
            };
            if directory.entries.get_by(position.name.as_str()).is_some() {
                return Err(BackendError::Exists);
            }
            if position.expected.is_some() {
                return Err(BackendError::Conflict);
            }
            let next_version = parent
                .version
                .checked_add(1)
                .ok_or(SystemCallError::ReachLimit)?;
            let next_epoch = self
                .epoch
                .checked_add(1)
                .ok_or(SystemCallError::ReachLimit)?;
            let slot = Counter::try_acquire(&self.entry_slots)
                .map_err(|_| SystemCallError::QuotaExceeded)?;
            let charge = access.account().acquire(
                FalResource::Bytes,
                position.name.len() + PreparedEntry::<Entry, String>::allocation_bytes(),
            )?;
            // 候选尚未入表，身份在节点准备完成后确定，不能因此延后名字存储预留。
            let entry = directory
                .entries
                .prepare_insert(
                    position.name,
                    Entry {
                        node: position.parent.id(),
                        slot: Some(slot),
                        _charge: charge,
                    },
                )
                .map_err(|error| match error {
                    ordered_table::InsertError::Limit(_) => SystemCallError::QuotaExceeded,
                    ordered_table::InsertError::Allocation(_) => SystemCallError::OutOfMemory,
                })?;
            Ok((parent.version, next_version, next_epoch, entry))
        })();
        let (version, next_version, next_epoch, mut entry) = match reserve {
            Ok(reserved) => reserved,
            Err(error) => return Err(CreateFailure { error, body }),
        };
        let payload = MemoryNode {
            body,
            rights,
            version: 1,
            parent: Some(position.parent.id()),
            take_reserved: false,
        };
        let node = match self.nodes.prepare(payload, access.account()) {
            Ok(node) => node,
            Err(failure) => {
                return Err(CreateFailure {
                    error: BackendError::Resource(failure.error),
                    body: failure.payload.body,
                });
            }
        };
        entry.value_mut().node = node.id();
        Ok(PreparedMutation {
            epoch: self.epoch,
            next_epoch,
            intent: Intent::Create {
                parent: position.parent,
                version,
                next_version,
                node,
                entry,
            },
        })
    }

    pub fn prepare_delete(
        &self,
        position: Position,
        access: &AccessSnapshot,
    ) -> Result<PreparedMutation<C>, BackendError> {
        let parent = self.parent(&position, access, FalRights::REMOVE)?;
        let target = self.target(&position)?;
        let target_node = self.nodes.get(&target).ok_or(BackendError::NotFound)?;
        let target_version = target_node.version;
        let target_next = target_version
            .checked_add(1)
            .ok_or(SystemCallError::ReachLimit)?;
        if let Body::Directory(directory) =
            &self.nodes.get(&target).ok_or(BackendError::NotFound)?.body
            && !directory.entries.is_empty()
        {
            return Err(BackendError::NotEmpty);
        }
        Ok(PreparedMutation {
            epoch: self.epoch,
            next_epoch: self
                .epoch
                .checked_add(1)
                .ok_or(SystemCallError::ReachLimit)?,
            intent: Intent::Delete {
                parent: position.parent,
                version: parent.version,
                next_version: parent
                    .version
                    .checked_add(1)
                    .ok_or(SystemCallError::ReachLimit)?,
                target,
                name: position.name,
                target_version,
                target_next,
            },
        })
    }

    pub fn prepare_write(
        &self,
        target: NodeRef,
        access: &AccessSnapshot,
        offset: u64,
        bytes: &[u8],
    ) -> Result<PreparedMutation<C>, BackendError> {
        let node = self.nodes.get(&target).ok_or(BackendError::NotFound)?;
        if !access
            .rights()
            .intersect(node.rights)
            .contains(FalRights::WRITE_STREAM)
        {
            return Err(BackendError::Permission);
        }
        let Body::Stream(data) = &node.body else {
            return Err(BackendError::Permission);
        };
        let write = data.prepare_write(offset, bytes, access.account())?;
        Ok(PreparedMutation {
            epoch: self.epoch,
            next_epoch: self.epoch,
            intent: Intent::Write {
                target,
                version: node.version,
                next_version: node
                    .version
                    .checked_add(1)
                    .ok_or(SystemCallError::ReachLimit)?,
                data: write,
            },
        })
    }

    pub fn prepare_property(
        &self,
        target: NodeRef,
        access: &AccessSnapshot,
        value: StoredValue<C>,
    ) -> Result<PreparedMutation<C>, PropertyFailure<C>> {
        let validate = (|| {
            if self.nodes.is_sealed() {
                return Err(BackendError::Closed);
            }
            let node = self.nodes.get(&target).ok_or(BackendError::NotFound)?;
            if !access
                .rights()
                .intersect(node.rights)
                .contains(FalRights::WRITE_PROPERTY)
            {
                return Err(BackendError::Permission);
            }
            if !matches!(node.body, Body::Property(_)) {
                return Err(BackendError::Permission);
            }
            if node.take_reserved {
                return Err(BackendError::Busy);
            }
            Ok((
                node.version,
                node.version
                    .checked_add(1)
                    .ok_or(SystemCallError::ReachLimit)?,
            ))
        })();
        let (version, next_version) = match validate {
            Ok(versions) => versions,
            Err(error) => return Err(PropertyFailure { error, value }),
        };
        Ok(PreparedMutation {
            epoch: self.epoch,
            next_epoch: self.epoch,
            intent: Intent::Property {
                target,
                version,
                next_version,
                value,
            },
        })
    }

    pub fn prepare_take(
        &mut self,
        target: NodeRef,
        access: &AccessSnapshot,
    ) -> Result<PreparedTake<C>, BackendError> {
        if self.nodes.is_sealed() {
            return Err(BackendError::Closed);
        }
        let node = self.nodes.get_mut(&target).ok_or(BackendError::NotFound)?;
        if node.take_reserved {
            return Err(BackendError::Busy);
        }
        if !access
            .rights()
            .intersect(node.rights)
            .contains(FalRights::READ_PROPERTY | FalRights::ACQUIRE_CAPABILITY)
        {
            return Err(BackendError::Permission);
        }
        let Body::Property(value) = &mut node.body else {
            return Err(BackendError::Permission);
        };
        let next_version = node
            .version
            .checked_add(1)
            .ok_or(SystemCallError::ReachLimit)?;
        let mut empty = alloc::vec::Vec::new();
        empty
            .try_reserve_exact(crate::value::HEADER_LEN)
            .map_err(|_| BackendError::Resource(SystemCallError::OutOfMemory))?;
        let taken = value.take(empty).map_err(|error| match error {
            crate::value::ValueError::Affine => BackendError::Permission,
            crate::value::ValueError::Capability(error) => BackendError::Resource(error),
            crate::value::ValueError::Allocation => {
                BackendError::Resource(SystemCallError::OutOfMemory)
            }
            _ => BackendError::Conflict,
        })?;
        let version = node.version;
        node.take_reserved = true;
        Ok(PreparedTake {
            target,
            version,
            next_version,
            value: Some(taken),
        })
    }

    pub fn take_value(prepared: &mut PreparedTake<C>) -> TakenValue<C> {
        prepared.value.take().expect("take value already consumed")
    }

    pub fn commit_take(&mut self, prepared: PreparedTake<C>) {
        let node = self
            .nodes
            .get_mut(&prepared.target)
            .expect("reserved take node disappeared");
        assert!(
            node.take_reserved && node.version == prepared.version,
            "reserved take state changed before commit"
        );
        let Body::Property(stored) = &mut node.body else {
            panic!("reserved take property changed kind");
        };
        stored.commit_take();
        node.take_reserved = false;
        node.version = prepared.next_version;
    }

    pub fn rollback_take(&mut self, mut prepared: PreparedTake<C>, value: TakenValue<C>) {
        let node = self
            .nodes
            .get_mut(&prepared.target)
            .expect("reserved take node disappeared during rollback");
        assert!(
            node.take_reserved && node.version == prepared.version,
            "reserved take state changed before rollback"
        );
        let Body::Property(stored) = &mut node.body else {
            panic!("reserved take property changed kind");
        };
        stored.restore(value);
        node.take_reserved = false;
        prepared.value = None;
    }

    pub fn prepare_move(
        &self,
        source: Position,
        source_access: &AccessSnapshot,
        destination: &AccessSnapshot,
        final_name: &str,
    ) -> Result<PreparedMutation<C>, BackendError> {
        let source_parent = self.parent(&source, source_access, FalRights::REMOVE)?;
        let target = self.target(&source)?;
        let target_node = self.nodes.get(&target).ok_or(BackendError::NotFound)?;
        let target_version = target_node.version;
        let target_next = target_version
            .checked_add(1)
            .ok_or(SystemCallError::ReachLimit)?;
        let destination_position = self.position(destination.root(), final_name, None)?;
        let destination_parent =
            self.parent(&destination_position, destination, FalRights::CREATE)?;
        let Body::Directory(directory) = &destination_parent.body else {
            unreachable!()
        };
        if directory.entries.get_by(final_name).is_some() {
            return Err(BackendError::Exists);
        }
        let charge = destination.account().acquire(
            FalResource::Bytes,
            final_name.len() + PreparedEntry::<Entry, String>::allocation_bytes(),
        )?;
        let entry = directory
            .entries
            .prepare_insert(
                destination_position.name,
                Entry {
                    node: target.id(),
                    slot: None,
                    _charge: charge,
                },
            )
            .map_err(|error| match error {
                ordered_table::InsertError::Limit(_) => SystemCallError::QuotaExceeded,
                ordered_table::InsertError::Allocation(_) => SystemCallError::OutOfMemory,
            })?;
        let ancestor = if self
            .nodes
            .get(&target)
            .is_some_and(|node| node.kind() == NodeKind::Directory)
        {
            Some(destination.root().clone())
        } else {
            None
        };
        let checked = ancestor.is_none();
        Ok(PreparedMutation {
            epoch: self.epoch,
            next_epoch: self
                .epoch
                .checked_add(1)
                .ok_or(SystemCallError::ReachLimit)?,
            intent: Intent::Move {
                source: source.parent,
                destination: destination.root().clone(),
                target,
                source_version: source_parent.version,
                destination_version: destination_parent.version,
                source_next: source_parent
                    .version
                    .checked_add(1)
                    .ok_or(SystemCallError::ReachLimit)?,
                destination_next: destination_parent
                    .version
                    .checked_add(1)
                    .ok_or(SystemCallError::ReachLimit)?,
                target_version,
                target_next,
                source_name: source.name,
                entry,
                ancestor,
                checked,
            },
        })
    }

    /// 目录循环检查是挂起任务的组成部分，结构代次变化后无副作用地拒绝提交。
    pub fn validate_move_step(
        &self,
        mutation: &mut PreparedMutation<C>,
        budget: usize,
    ) -> Result<bool, BackendError> {
        if self.epoch != mutation.epoch {
            return Err(BackendError::Conflict);
        }
        let Intent::Move {
            target,
            ancestor,
            checked,
            ..
        } = &mut mutation.intent
        else {
            return Ok(true);
        };
        for _ in 0..budget {
            let Some(current) = ancestor.take() else {
                *checked = true;
                return Ok(true);
            };
            if current.id() == target.id() {
                return Err(BackendError::Cycle);
            }
            let parent = self
                .nodes
                .get(&current)
                .ok_or(BackendError::Conflict)?
                .parent;
            *ancestor = match parent {
                Some(parent) => Some(
                    self.nodes
                        .pin_linked(parent)
                        .ok_or(BackendError::Conflict)?,
                ),
                None => None,
            };
        }
        if ancestor.is_none() {
            *checked = true;
        }
        Ok(*checked)
    }

    #[expect(
        clippy::result_large_err,
        reason = "提交冲突原样返还预备事务及资助责任，错误路径不新增装箱分配"
    )]
    pub fn commit(
        &mut self,
        mutation: PreparedMutation<C>,
    ) -> Result<CommitResult<C>, CommitFailure<C>> {
        let valid = !self.nodes.is_sealed() && self.epoch == mutation.epoch && match &mutation.intent {
            Intent::Move { source, destination, target, source_version, destination_version, target_version, source_name, entry, checked, .. } => *checked && self.nodes.get(target).is_some_and(|node| node.version == *target_version && !node.take_reserved) && self.nodes.get(source).is_some_and(|node| node.version == *source_version && matches!(&node.body, Body::Directory(directory) if directory.entries.get_by(source_name.as_str()).is_some_and(|entry| entry.node == target.id()))) && self.nodes.get(destination).is_some_and(|node| node.version == *destination_version && matches!(&node.body, Body::Directory(directory) if directory.entries.get_by(entry.key_ref().as_str()).is_none())),
            Intent::Create { parent, version, entry, .. } => self.nodes.get(parent).is_some_and(|node| node.version == *version && matches!(&node.body, Body::Directory(directory) if directory.entries.get_by(entry.key_ref().as_str()).is_none())),
            Intent::Delete { parent, version, target, target_version, name, .. } => self.nodes.get(parent).is_some_and(|node| node.version == *version && matches!(&node.body, Body::Directory(directory) if directory.entries.get_by(name.as_str()).is_some_and(|entry| entry.node == target.id()))) && self.nodes.get(target).is_some_and(|node| node.version == *target_version && !node.take_reserved),
            Intent::Write { target, version, data, .. } => self.nodes.get(target).is_some_and(|node| node.version == *version && matches!(&node.body, Body::Stream(stream) if stream.validates(data))),
            Intent::Property { target, version, .. } => self.nodes.get(target).is_some_and(|node| node.version == *version && matches!(node.body, Body::Property(_))),
        };
        if !valid {
            return Err(CommitFailure {
                error: BackendError::Conflict,
                mutation,
            });
        }
        let result = match mutation.intent {
            Intent::Move {
                source,
                destination,
                target,
                source_name,
                mut entry,
                source_next,
                destination_next,
                target_next,
                ..
            } => {
                let source_node = self
                    .nodes
                    .get_mut(&source)
                    .expect("move source disappeared");
                let Body::Directory(directory) = &mut source_node.body else {
                    unreachable!()
                };
                let mut removed = directory
                    .entries
                    .remove_by(source_name.as_str())
                    .expect("move source entry disappeared");
                entry.value_mut().slot = removed.slot.take();
                source_node.version = source_next;
                let destination_node = self
                    .nodes
                    .get_mut(&destination)
                    .expect("move destination disappeared");
                let Body::Directory(directory) = &mut destination_node.body else {
                    unreachable!()
                };
                directory.entries.insert_prepared(entry);
                destination_node.version = destination_next;
                let target_node = self.nodes.get_mut(&target).expect("moved node disappeared");
                target_node.parent = Some(destination.id());
                target_node.version = target_next;
                CommitResult::Moved(target)
            }
            Intent::Create {
                parent,
                next_version,
                node,
                entry,
                ..
            } => {
                let created = self.nodes.commit(node);
                self.nodes.link(&created);
                let parent = self
                    .nodes
                    .get_mut(&parent)
                    .expect("create parent disappeared");
                let Body::Directory(directory) = &mut parent.body else {
                    unreachable!()
                };
                directory.entries.insert_prepared(entry);
                parent.version = next_version;
                CommitResult::Created(created)
            }
            Intent::Delete {
                parent,
                target,
                name,
                next_version,
                target_next,
                ..
            } => {
                let parent = self
                    .nodes
                    .get_mut(&parent)
                    .expect("delete parent disappeared");
                let Body::Directory(directory) = &mut parent.body else {
                    unreachable!()
                };
                let _ = directory.entries.remove_by(name.as_str());
                parent.version = next_version;
                self.nodes.unlink(&target);
                let target_node = self
                    .nodes
                    .get_mut(&target)
                    .expect("deleted node disappeared");
                target_node.parent = None;
                target_node.version = target_next;
                CommitResult::Deleted(target)
            }
            Intent::Write {
                target,
                data,
                next_version,
                ..
            } => {
                let target = self
                    .nodes
                    .get_mut(&target)
                    .expect("write target disappeared");
                let Body::Stream(stream) = &mut target.body else {
                    unreachable!()
                };
                stream.commit(data);
                target.version = next_version;
                CommitResult::Written
            }
            Intent::Property {
                target,
                value,
                next_version,
                ..
            } => {
                let target = self
                    .nodes
                    .get_mut(&target)
                    .expect("property target disappeared");
                let Body::Property(old) = &mut target.body else {
                    unreachable!()
                };
                let old = core::mem::replace(old, value);
                target.version = next_version;
                CommitResult::PropertyReplaced(old)
            }
        };
        self.epoch = mutation.next_epoch;
        Ok(result)
    }

    pub fn seal(&mut self) {
        self.nodes.seal();
        self.root = None;
    }
    pub fn has_retire_work(&self) -> bool {
        self.nodes.has_retire_work()
    }
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}
impl<C: Capability> MemoryBackend<C> {
    pub fn retire_step(&mut self, budget: usize) -> Result<RetireProgress, SystemCallError> {
        self.nodes.retire_step(budget)
    }
    pub fn close(self) -> Result<(), Self> {
        if self.nodes.is_sealed() && self.nodes.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::value::Value;
    use alloc::{rc::Rc, vec::Vec};
    use erhino_shared::{call::SystemCallError, object::HandleDescription, object::Rights};
    use libbudget::{Budget, Taxonomy};
    use libexecution::wake::Wake;

    #[derive(Clone)]
    struct TestCapability;

    impl Capability for TestCapability {
        fn description(&self) -> Result<HandleDescription, SystemCallError> {
            Ok(HandleDescription {
                object_id: 1,
                related_object_id: 1,
                kind: 0,
                role: erhino_shared::object::HandleRole::MailboxSender as u32,
                rights: Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
                badge: 0,
                reserved: 0,
            })
        }

        fn duplicate(&self, _rights: Rights) -> Result<Self, SystemCallError> {
            Ok(self.clone())
        }

        fn close(self) -> Result<(), (Self, SystemCallError)> {
            Ok(())
        }
    }

    struct TestWake;

    impl Wake for TestWake {
        fn publish(&self) {}
    }

    fn account() -> AccountView<FalResource> {
        let mut limits = [0; FalResource::COUNT];
        limits[FalResource::Node.slot()] = 8;
        limits[FalResource::Bytes.slot()] = 32 * 1024;
        limits[FalResource::Grant.slot()] = 2;
        limits[FalResource::WaitSource.slot()] = 2;
        let budget = Budget::new(&limits, 1).unwrap();
        let account = budget.account(&limits).unwrap();
        let binding: [_; FalResource::COUNT] =
            core::array::from_fn(|index| budget.slot(index).unwrap());
        account.view(&binding).unwrap()
    }

    fn access(root: NodeRef, account: AccountView<FalResource>) -> AccessSnapshot {
        AccessSnapshot {
            root,
            rights: FalRights::ALL,
            account,
        }
    }

    #[test]
    fn prepare_drop_refunds_node_and_conflict_preserves_state() {
        let account = account();
        let mut backend =
            MemoryBackend::<TestCapability>::new(&account, 4, Rc::new(TestWake)).unwrap();
        let root = backend.root().unwrap().clone();
        let access = access(root.clone(), account.clone());

        let mut encoded = [0; 32];
        let encoded_len = Value::Integer(1)
            .encode(&mut encoded)
            .expect("test property encoding failed");
        let bytes_before = account.usage(FalResource::Bytes).0;
        let stored = StoredValue::<TestCapability>::prepare(
            &encoded[..encoded_len],
            Vec::new(),
            1,
            64,
            &account,
        )
        .unwrap_or_else(|_| panic!("property value preparation failed"));
        match backend.prepare_property(root.clone(), &access, stored) {
            Err(failure) => {
                assert_eq!(failure.error, BackendError::Permission);
                drop(failure.value);
            }
            Ok(_) => panic!("directory accepted a property replacement"),
        }
        assert_eq!(account.usage(FalResource::Bytes).0, bytes_before);

        let position = backend.position(&root, "pending", None).unwrap();
        let prepared = backend
            .prepare_create(position, &access, backend.directory_body(), FalRights::ALL)
            .unwrap_or_else(|_| panic!("pending create preparation failed"));
        assert_eq!(account.usage(FalResource::Node).0, 2);
        drop(prepared);
        assert_eq!(account.usage(FalResource::Node).0, 1);

        let position = backend.position(&root, "child", None).unwrap();
        let child = backend
            .commit(
                backend
                    .prepare_create(position, &access, backend.directory_body(), FalRights::ALL)
                    .unwrap_or_else(|_| panic!("child create preparation failed")),
            )
            .unwrap_or_else(|_| panic!("child create commit failed"));
        let CommitResult::Created(child) = child else {
            panic!("create did not return a node");
        };
        let wrong = NodeId::from_raw(child.id().raw() + 1).unwrap();
        let position = backend.position(&root, "child", Some((wrong, 1))).unwrap();
        assert!(matches!(
            backend.prepare_delete(position, &access),
            Err(BackendError::Conflict)
        ));
        assert!(backend.get(&child).is_some());
        drop(child);

        drop(access);
        backend.seal();
        drop(root);
        while !backend.is_empty() {
            backend.retire_step(1).unwrap();
        }
        assert!(backend.close().is_ok());
        assert_eq!(account.usage(FalResource::Node).0, 0);
    }

    #[test]
    fn move_commits_after_bounded_validation() {
        let account = account();
        let mut backend =
            MemoryBackend::<TestCapability>::new(&account, 8, Rc::new(TestWake)).unwrap();
        let root = backend.root().unwrap().clone();
        let access = access(root.clone(), account.clone());
        let mut encoded = [0; 32];
        let encoded_len = Value::Integer(1)
            .encode(&mut encoded)
            .expect("move source value encoding failed");

        let source = backend
            .commit(
                backend
                    .prepare_create(
                        backend.position(&root, "source", None).unwrap(),
                        &access,
                        Body::Property(
                            StoredValue::prepare(
                                &encoded[..encoded_len],
                                Vec::new(),
                                1,
                                64,
                                &account,
                            )
                            .unwrap_or_else(|_| panic!("source value preparation failed")),
                        ),
                        FalRights::READ_PROPERTY,
                    )
                    .unwrap_or_else(|_| panic!("source create preparation failed")),
            )
            .unwrap_or_else(|_| panic!("source create commit failed"));
        let CommitResult::Created(source) = source else {
            panic!("source create did not return a node");
        };
        drop(source);
        let destination = backend
            .commit(
                backend
                    .prepare_create(
                        backend.position(&root, "destination", None).unwrap(),
                        &access,
                        backend.directory_body(),
                        FalRights::ALL,
                    )
                    .unwrap_or_else(|_| panic!("destination create preparation failed")),
            )
            .unwrap_or_else(|_| panic!("destination create commit failed"));
        let CommitResult::Created(destination) = destination else {
            panic!("destination create did not return a node");
        };
        let source_position = backend.position(&root, "source", None).unwrap();
        let mut mutation = backend
            .prepare_move(source_position, &access, &access, "moved")
            .unwrap_or_else(|_| panic!("move preparation failed"));
        assert!(
            backend
                .validate_move_step(&mut mutation, 1)
                .unwrap_or_else(|_| panic!("move validation failed"))
        );
        let CommitResult::Moved(moved) = backend
            .commit(mutation)
            .unwrap_or_else(|_| panic!("move commit failed"))
        else {
            panic!("move did not return the moved node");
        };
        assert_eq!(
            backend.get(&moved).expect("moved node missing").version(),
            2,
            "Move did not advance the target node generation"
        );
        drop(moved);
        assert!(matches!(
            backend.lookup_child(&root, "source", &access),
            Err(BackendError::NotFound)
        ));
        assert!(backend.lookup_child(&root, "moved", &access).is_ok());
        drop(destination);

        drop(access);
        backend.seal();
        drop(root);
        while !backend.is_empty() {
            backend.retire_step(1).unwrap();
        }
        assert!(backend.close().is_ok());
        assert_eq!(account.usage(FalResource::Node).0, 0);
    }
}
