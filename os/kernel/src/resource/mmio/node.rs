//! MMIO 控制树节点：派生只链接已付节点，撤销逐子/成员移交，完成由真实回执决定。
use super::*;
use crate::maintenance::{self as mt, WakeToken, Work, WorkRecord};
use maintenance::record::Outcome;
use resource_tree::PreparedNode;

pub(super) struct Link {
    pub(super) node: Weak<MmioNode>,
    pub(super) head: Option<u64>,
    pub(super) tail: Option<u64>,
    pub(super) revoke_next: Option<u64>,
    ranges: OrderedTable<usize, MetadataCharge>,
}
impl Link {
    fn new() -> Self {
        Self {
            node: Weak::new(),
            head: None,
            tail: None,
            revoke_next: None,
            ranges: OrderedTable::new(usize::MAX),
        }
    }
}
struct Dispatch {
    closed: bool,
    wake: Option<WakeToken>,
}

pub(crate) struct MmioNode {
    pub(super) root: Arc<MmioMappings>,
    pub(super) id: NodeId,
    pub(crate) physical: usize,
    pub(crate) bytes: usize,
    pub(crate) writable: bool,
    pub(crate) completion: Arc<SignalSource>,
    record: WorkRecord,
    dispatch: Spinlock<Dispatch>,
    _metadata: MetadataCharge,
}

pub(crate) struct PreparedMmioNode {
    node: Arc<MmioNode>,
    parent: NodeId,
    entry: Option<PreparedNode<Link, MetadataCharge>>,
    range: Option<PreparedEntry<usize, MetadataCharge>>,
}
impl PreparedMmioNode {
    pub(crate) fn node(&self) -> &Arc<MmioNode> {
        &self.node
    }
    pub(crate) fn commit(&mut self, admission: &Admission) -> Result<(), SystemCallError> {
        let mut gate = self.node.root.state.lock();
        if gate.tree.validate(admission).map_err(tree_error)? != self.parent {
            return Err(SystemCallError::RightsDenied);
        }
        let ranges = &gate
            .tree
            .value(self.parent)
            .expect("MMIO parent missing")
            .ranges;
        let start = self.node.physical as u64;
        let end = self.node.physical + self.node.bytes;
        if ranges
            .floor(&start)
            .is_some_and(|(_, previous_end)| *previous_end > self.node.physical)
            || ranges
                .lower_bound(&start)
                .is_some_and(|(next, _)| *next < end as u64)
        {
            return Err(SystemCallError::AddressConflict);
        }
        if !ranges.has_room() {
            return Err(SystemCallError::ReachLimit);
        }
        match gate.tree.link_child(
            admission,
            self.entry.take().expect("MMIO child committed twice"),
        ) {
            Ok(_) => {
                gate.tree
                    .value_mut(self.parent)
                    .expect("MMIO parent missing")
                    .ranges
                    .insert_prepared(self.range.take().expect("MMIO scope committed twice"));
                Ok(())
            }
            Err((error, entry)) => {
                self.entry = Some(entry);
                Err(tree_error(error))
            }
        }
    }
}

fn prepared(pool: &Arc<MemoryPool>) -> Result<PreparedNode<Link, MetadataCharge>, SystemCallError> {
    PreparedNode::try_new(Link::new(), &MetadataFunding(pool)).map_err(|error| match error {
        resource_tree::PrepareError::Identity(_) => SystemCallError::ReachLimit,
        resource_tree::PrepareError::Storage(error, _) => map_storage_error(error),
    })
}
impl MmioNode {
    /// # Safety
    /// 区间必须由启动平台验证为可委托 IO 页，排除 RAM、固件和内核私有设备页。
    pub(crate) unsafe fn root(
        pool: &Arc<MemoryPool>,
        physical: usize,
        bytes: usize,
        writable: bool,
    ) -> Result<Arc<Self>, SystemCallError> {
        if bytes == 0
            || !physical.is_multiple_of(PAGE_SIZE)
            || !bytes.is_multiple_of(PAGE_SIZE)
            || physical
                .checked_add(bytes)
                .is_none_or(|end| end / PAGE_SIZE > page_table::MAX_PPN)
        {
            return Err(SystemCallError::IllegalArgument);
        }
        let entry = prepared(pool)?;
        let id = entry.id();
        let metadata = MetadataCharge::arc::<MmioMappings>(pool)?;
        let mut storage =
            Arc::<MmioMappings>::try_new_uninit().map_err(|_| SystemCallError::OutOfMemory)?;
        Arc::get_mut(&mut storage)
            .expect("unpublished MMIO root is shared")
            .write(MmioMappings {
                state: Spinlock::new(
                    ranks::RESOURCE_TREE,
                    State {
                        tree: ResourceTree::new(entry),
                        mappings: OrderedTable::new(usize::MAX),
                    },
                ),
                _metadata: metadata,
            });
        // SAFETY: 唯一未发布 Arc 的全部字段已经初始化。
        let root = unsafe { storage.assume_init() };
        let node = match Self::new(root.clone(), id, pool, physical, bytes, writable) {
            Ok(node) => node,
            Err(error) => {
                let entry = {
                    let mut gate = root.state.lock();
                    if gate.tree.status(id).is_ok() {
                        gate.tree.seal(id).expect("MMIO root missing");
                        gate.tree
                            .finish(id)
                            .expect("unpublished MMIO root has children");
                        Some(
                            gate.tree
                                .remove_finished(id)
                                .expect("MMIO root not finished"),
                        )
                    } else {
                        None
                    }
                };
                drop(entry);
                return Err(error);
            }
        };
        root.state
            .lock()
            .tree
            .value_mut(id)
            .expect("MMIO root missing")
            .node = Arc::downgrade(&node);
        Ok(node)
    }
    fn new(
        root: Arc<MmioMappings>,
        id: NodeId,
        pool: &Arc<MemoryPool>,
        physical: usize,
        bytes: usize,
        writable: bool,
    ) -> Result<Arc<Self>, SystemCallError> {
        let metadata = MetadataCharge::arc::<Self>(pool)?;
        Arc::try_new(Self {
            root,
            id,
            physical,
            bytes,
            writable,
            completion: SignalSource::try_new(pool.clone())?,
            record: WorkRecord::new(pool.maintenance_account().clone()),
            dispatch: Spinlock::new(
                ranks::LEAF,
                Dispatch {
                    closed: false,
                    wake: None,
                },
            ),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }
    pub(crate) fn subrange(&self, offset: usize, bytes: usize) -> Result<usize, SystemCallError> {
        if bytes == 0
            || !offset.is_multiple_of(PAGE_SIZE)
            || !bytes.is_multiple_of(PAGE_SIZE)
            || offset.checked_add(bytes).is_none_or(|end| end > self.bytes)
        {
            return Err(SystemCallError::IllegalArgument);
        }
        self.physical
            .checked_add(offset)
            .ok_or(SystemCallError::IllegalArgument)
    }
    pub(crate) fn prepare_child(
        self: &Arc<Self>,
        pool: &Arc<MemoryPool>,
        offset: usize,
        bytes: usize,
        writable: bool,
    ) -> Result<PreparedMmioNode, SystemCallError> {
        if writable && !self.writable {
            return Err(SystemCallError::RightsDenied);
        }
        let physical = self.subrange(offset, bytes)?;
        let range =
            PreparedEntry::try_new(physical as u64, physical + bytes, &MetadataFunding(pool))
                .map_err(|error| match error {
                    ordered_table::InsertError::Storage(error, _) => map_storage_error(error),
                    ordered_table::InsertError::Limit(_) => SystemCallError::ReachLimit,
                })?;
        let mut entry = prepared(pool)?;
        let node = Self::new(
            self.root.clone(),
            entry.id(),
            pool,
            physical,
            bytes,
            writable,
        )?;
        entry.value_mut().node = Arc::downgrade(&node);
        Ok(PreparedMmioNode {
            node,
            parent: self.id,
            entry: Some(entry),
            range: Some(range),
        })
    }
    pub(crate) fn begin(&self) -> Result<Admission, SystemCallError> {
        self.root
            .state
            .lock()
            .tree
            .begin(self.id)
            .map_err(tree_error)
    }
    pub(crate) fn advance(&self, admission: &mut Admission) -> Result<bool, SystemCallError> {
        self.root
            .state
            .lock()
            .tree
            .advance(admission)
            .map(|p| p == resource_tree::Progress::Checked)
            .map_err(tree_error)
    }
    pub(crate) fn seal(&self) {
        self.root
            .state
            .lock()
            .tree
            .seal(self.id)
            .expect("MMIO node missing");
    }
    pub(crate) fn status(&self) -> resource_tree::Status {
        self.root
            .state
            .lock()
            .tree
            .status(self.id)
            .expect("MMIO node missing")
    }
    pub(crate) fn revoke(self: &Arc<Self>) {
        {
            let mut gate = self.root.state.lock();
            let Ok(status) = gate.tree.status(self.id) else {
                return;
            }; // 未链接候选的放弃。
            if status.finished || status.revoking {
                return;
            }
            gate.tree.revoke(self.id).expect("MMIO node missing");
            let link = gate.tree.value_mut(self.id).expect("MMIO node missing");
            link.revoke_next = link.head;
        }
        self.notify();
    }
    pub(super) fn notify(self: &Arc<Self>) {
        if !self.status().revoking {
            return;
        }
        let retired = {
            let mut dispatch = self.dispatch.lock();
            if dispatch.closed {
                return;
            }
            if let Some(wake) = dispatch.wake.take() {
                mt::wake(wake)
            } else {
                let root: Arc<dyn Work> = self.clone();
                mt::publish(root);
                None
            }
        };
        drop(retired);
        mt::kick();
    }
}
impl Work for MmioNode {
    fn record(&self) -> &WorkRecord {
        &self.record
    }
    fn step(self: Arc<Self>) -> Outcome {
        let old = self.dispatch.lock().wake.take();
        drop(old);
        let (child, mapping) = {
            let mut gate = self.root.state.lock();
            if let Some(child) = gate
                .tree
                .next_child_to_revoke(self.id)
                .expect("MMIO revoke missing")
            {
                (
                    Some(
                        gate.tree
                            .value(child)
                            .expect("MMIO child missing")
                            .node
                            .upgrade()
                            .expect("live child lost owner"),
                    ),
                    None,
                )
            } else {
                let next = gate
                    .tree
                    .value(self.id)
                    .expect("MMIO node missing")
                    .revoke_next;
                let mapping = next.map(|id| {
                    let entry = gate.mappings.get(id).expect("MMIO member missing");
                    let (next, mapping) = (
                        entry.next,
                        entry.mapping.upgrade().expect("live mapping lost owner"),
                    );
                    gate.tree
                        .value_mut(self.id)
                        .expect("MMIO node missing")
                        .revoke_next = next;
                    mapping
                });
                (None, mapping)
            }
        };
        if let Some(child) = child {
            child.revoke();
            return Outcome::Ready;
        }
        if let Some(mapping) = mapping {
            mapping.request_stop();
            return Outcome::Ready;
        }
        // 普通回执等待没有 fence。先 arm 后同门复检，早到通知由 Record.notified 覆盖。
        {
            let mut dispatch = self.dispatch.lock();
            let root: Arc<dyn Work> = self.clone();
            dispatch.wake = Some(mt::arm(&root));
        }
        let (parent, range) = {
            let mut gate = self.root.state.lock();
            match gate.tree.finish(self.id) {
                Err(resource_tree::Error::Incomplete) => return Outcome::Blocked,
                Err(_) => panic!("MMIO close lost tree membership"),
                Ok(parent) => match parent {
                    Some(id) => {
                        let parent = gate.tree.value_mut(id).expect("MMIO parent missing");
                        let range = parent
                            .ranges
                            .remove(self.physical as u64)
                            .expect("MMIO scope disappeared");
                        (Some(parent.node.clone()), Some(range))
                    }
                    None => (None, None),
                },
            }
        };
        drop(range);
        let wake = {
            let mut dispatch = self.dispatch.lock();
            dispatch.closed = true;
            dispatch.wake.take()
        };
        drop(wake);
        self.completion.publish(ObjectSignals::DONE, false);
        if let Some(parent) = parent.and_then(|parent| parent.upgrade()) {
            parent.notify();
        }
        Outcome::Done
    }
}
impl Drop for MmioNode {
    fn drop(&mut self) {
        let (entry, parent, range) = {
            let mut gate = self.root.state.lock();
            if gate.tree.status(self.id).is_err() {
                return;
            }
            // 无 capability 的出生失败可以同步退一个空节点；有责任时必由 Work 保活。
            gate.tree.seal(self.id).expect("MMIO node missing");
            let parent_id = gate
                .tree
                .finish(self.id)
                .expect("MMIO node lost cleanup owner");
            let (parent, range) = match parent_id {
                Some(id) => {
                    let parent = gate.tree.value_mut(id).expect("MMIO parent missing");
                    (
                        Some(parent.node.clone()),
                        Some(
                            parent
                                .ranges
                                .remove(self.physical as u64)
                                .expect("MMIO scope disappeared"),
                        ),
                    )
                }
                None => (None, None),
            };
            (
                gate.tree
                    .remove_finished(self.id)
                    .expect("MMIO node not finished"),
                parent,
                range,
            )
        };
        drop(entry);
        drop(range);
        if let Some(parent) = parent.and_then(|parent| parent.upgrade()) {
            parent.notify();
        }
    }
}
