//! 内存后端的稳定名字事务；节点、名字、属性 owner 与流数据分别拥有资源。

use crate::{
    authority::{AccessSnapshot, FalRights},
    data::{Data, PreparedWrite},
    node::NodeKind,
    store::{NodeId, NodeRef, NodeStore, Payload, PreparedNode, RetireContext, RetireProgress},
    value::{Capability, StoredValue},
};
use alloc::{string::String, sync::Arc};
use erhino_shared::call::SystemCallError;
use libsrv::budget::{Account, Charge, Resource};
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

enum Intent<C> {
    Move {
        source: NodeRef,
        destination: NodeRef,
        target: NodeRef,
        source_version: u64,
        destination_version: u64,
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

impl<C> MemoryBackend<C> {
    pub fn new(
        account: &Arc<Account>,
        limit: usize,
        wake: alloc::rc::Rc<dyn libsrv::wake::Wake>,
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
                Resource::Bytes,
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

    pub fn prepare_move(
        &self,
        source: Position,
        source_access: &AccessSnapshot,
        destination: &AccessSnapshot,
        final_name: &str,
    ) -> Result<PreparedMutation<C>, BackendError> {
        let source_parent = self.parent(&source, source_access, FalRights::REMOVE)?;
        let target = self.target(&source)?;
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
            Resource::Bytes,
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
            Intent::Move { source, destination, target, source_version, destination_version, source_name, entry, checked, .. } => *checked && self.nodes.get(source).is_some_and(|node| node.version == *source_version && matches!(&node.body, Body::Directory(directory) if directory.entries.get_by(source_name.as_str()).is_some_and(|entry| entry.node == target.id()))) && self.nodes.get(destination).is_some_and(|node| node.version == *destination_version && matches!(&node.body, Body::Directory(directory) if directory.entries.get_by(entry.key_ref().as_str()).is_none())),
            Intent::Create { parent, version, entry, .. } => self.nodes.get(parent).is_some_and(|node| node.version == *version && matches!(&node.body, Body::Directory(directory) if directory.entries.get_by(entry.key_ref().as_str()).is_none())),
            Intent::Delete { parent, version, target, name, .. } => self.nodes.get(parent).is_some_and(|node| node.version == *version && matches!(&node.body, Body::Directory(directory) if directory.entries.get_by(name.as_str()).is_some_and(|entry| entry.node == target.id()))),
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
                self.nodes
                    .get_mut(&target)
                    .expect("moved node disappeared")
                    .parent = Some(destination.id());
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
                self.nodes
                    .get_mut(&target)
                    .expect("deleted node disappeared")
                    .parent = None;
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
