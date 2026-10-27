//! DMA 域的祖先准入、封口、成员回执与有界身份存储回收。
use crate::{
    maintenance::{self as mt, Work, WorkRecord},
    sync::{Spinlock, ranks},
    task::{
        memory_pool::MemoryPool,
        resources::{MetadataCharge, MetadataFunding, map_storage_error},
        wait_source::SignalSource,
    },
};
use alloc::sync::{Arc, Weak};
use erhino_shared::{call::SystemCallError as Error, object::ObjectSignals};
use ordered_table::{OrderedTable, PreparedEntry};
use resource_tree::{Admission, NodeId, PreparedNode, ResourceTree};
mod mapping;
pub(crate) mod object;
use mapping::Member;
// 封口后有静态保留的域也必须有真实 owner，不能依靠永远停驻的 Work 自保活。
static NODES: Spinlock<OrderedTable<Arc<Node>, MetadataCharge>> =
    Spinlock::new(ranks::LEAF, OrderedTable::new(usize::MAX));

struct Link {
    node: Weak<Node>,
    head: Option<u64>,
    tail: Option<u64>,
    stop_next: Option<u64>,
    record_children: usize,
    dropped: bool,
    reclaim_next: Option<NodeId>,
}
impl Link {
    fn new() -> Self {
        Self {
            node: Weak::new(),
            head: None,
            tail: None,
            stop_next: None,
            record_children: 0,
            dropped: false,
            reclaim_next: None,
        }
    }
}
struct State {
    tree: ResourceTree<Link, MetadataCharge>,
    mappings: OrderedTable<Member, MetadataCharge>,
    reclaim_head: Option<NodeId>,
    reclaim_tail: Option<NodeId>,
}
impl State {
    fn enqueue_reclaim(&mut self, id: NodeId) {
        if let Some(tail) = self.reclaim_tail {
            self.tree
                .value_mut(tail)
                .expect("DMA reclaim tail missing")
                .reclaim_next = Some(id);
        } else {
            self.reclaim_head = Some(id);
        }
        self.reclaim_tail = Some(id);
    }
}
struct Root {
    state: Spinlock<State>,
    record: WorkRecord,
    _metadata: MetadataCharge,
}
struct Node {
    root: Arc<Root>,
    id: NodeId,
    completion: Arc<SignalSource>,
    record: WorkRecord,
    dispatch_closed: Spinlock<bool>,
    _metadata: MetadataCharge,
}
struct Prepared {
    node: Arc<Node>,
    parent: NodeId,
    entry: Option<PreparedNode<Link, MetadataCharge>>,
    retained: Option<PreparedEntry<Arc<Node>, MetadataCharge>>,
}
fn tree_error(error: resource_tree::Error) -> Error {
    match error {
        resource_tree::Error::Closed | resource_tree::Error::Missing => Error::ObjectClosed,
        resource_tree::Error::Busy | resource_tree::Error::Incomplete => Error::ObjectBusy,
        resource_tree::Error::Capacity => Error::ReachLimit,
        resource_tree::Error::WrongRoot => Error::RightsDenied,
    }
}
fn entry<V>(
    id: u64,
    value: V,
    pool: &Arc<MemoryPool>,
) -> Result<PreparedEntry<V, MetadataCharge>, Error> {
    PreparedEntry::try_new(id, value, &MetadataFunding(pool)).map_err(|error| match error {
        ordered_table::InsertError::Storage(error, _) => map_storage_error(error),
        ordered_table::InsertError::Limit(_) => Error::ReachLimit,
    })
}
fn prepared(pool: &Arc<MemoryPool>) -> Result<PreparedNode<Link, MetadataCharge>, Error> {
    PreparedNode::try_new(Link::new(), &MetadataFunding(pool)).map_err(|error| match error {
        resource_tree::PrepareError::Identity(_) => Error::ReachLimit,
        resource_tree::PrepareError::Storage(error, _) => map_storage_error(error),
    })
}
impl Prepared {
    fn commit(&mut self, admission: &Admission) -> Result<(), Error> {
        NODES
            .lock()
            .insert_prepared(self.retained.take().expect("DMA node committed twice"));
        let result = self.link(admission);
        if result.is_err() {
            let retained = NODES.lock().remove(self.node.id.get());
            drop(retained);
        }
        result
    }
    fn link(&mut self, admission: &Admission) -> Result<(), Error> {
        let mut state = self.node.root.state.lock();
        if state.tree.validate(admission).map_err(tree_error)? != self.parent {
            return Err(Error::RightsDenied);
        }
        let count = state
            .tree
            .value(self.parent)
            .expect("DMA parent missing")
            .record_children
            .checked_add(1)
            .ok_or(Error::ReachLimit)?;
        match state.tree.link_child(
            admission,
            self.entry.take().expect("DMA child committed twice"),
        ) {
            Ok(_) => {
                state
                    .tree
                    .value_mut(self.parent)
                    .expect("DMA parent missing")
                    .record_children = count;
                Ok(())
            }
            Err((error, entry)) => {
                self.entry = Some(entry);
                Err(tree_error(error))
            }
        }
    }
}
impl Node {
    fn root(pool: &Arc<MemoryPool>) -> Result<Arc<Self>, Error> {
        let entry = prepared(pool)?;
        let id = entry.id();
        let metadata = MetadataCharge::arc::<Root>(pool)?;
        let mut storage = Arc::<Root>::try_new_uninit().map_err(|_| Error::OutOfMemory)?;
        Arc::get_mut(&mut storage)
            .expect("unpublished DMA root is shared")
            .write(Root {
                state: Spinlock::new(
                    ranks::RESOURCE_TREE,
                    State {
                        tree: ResourceTree::new(entry),
                        mappings: OrderedTable::new(usize::MAX),
                        reclaim_head: None,
                        reclaim_tail: None,
                    },
                ),
                record: WorkRecord::new(pool.maintenance_account().clone()),
                _metadata: metadata,
            });
        // SAFETY: 未发布且已完整初始化。
        let root = unsafe { storage.assume_init() };
        // 出生期间只有局部 owner：尚无能力、成员或子节点，失败不提交正常退休 Work。
        let rollback = || {
            let entry = {
                let mut state = root.state.lock();
                state.tree.seal(id).expect("DMA root missing");
                state
                    .tree
                    .finish(id)
                    .expect("unpublished DMA root has members");
                state
                    .tree
                    .remove_finished(id)
                    .expect("DMA root not finished")
            };
            drop(entry);
        };
        let node = match Self::new(root.clone(), id, pool) {
            Ok(node) => node,
            Err(error) => {
                rollback();
                return Err(error);
            }
        };
        root.state
            .lock()
            .tree
            .value_mut(id)
            .expect("DMA root missing")
            .node = Arc::downgrade(&node);
        let retained = match self::entry(id.get(), node.clone(), pool) {
            Ok(retained) => retained,
            Err(error) => {
                // 最后 Node 在移除空根以后才析构，Drop 不会再排入根回收队列。
                rollback();
                return Err(error);
            }
        };
        NODES.lock().insert_prepared(retained);
        Ok(node)
    }
    fn new(root: Arc<Root>, id: NodeId, pool: &Arc<MemoryPool>) -> Result<Arc<Self>, Error> {
        let metadata = MetadataCharge::arc::<Self>(pool)?;
        let completion = SignalSource::try_new(pool.clone())?;
        // 全部可失败步骤先结束；出生失败不触发未发布节点的正常 Drop 回收队列。
        let mut storage = Arc::<Self>::try_new_uninit().map_err(|_| Error::OutOfMemory)?;
        Arc::get_mut(&mut storage)
            .expect("unpublished DMA node is shared")
            .write(Self {
                root,
                id,
                completion,
                record: WorkRecord::new(pool.maintenance_account().clone()),
                dispatch_closed: Spinlock::new(ranks::LEAF, false),
                _metadata: metadata,
            });
        // SAFETY: 唯一 Arc 的字段完整初始化。
        Ok(unsafe { storage.assume_init() })
    }
    fn prepare(self: &Arc<Self>, pool: &Arc<MemoryPool>) -> Result<Prepared, Error> {
        let mut entry = prepared(pool)?;
        let node = Self::new(self.root.clone(), entry.id(), pool)?;
        entry.value_mut().node = Arc::downgrade(&node);
        let retained = self::entry(node.id.get(), node.clone(), pool)?;
        Ok(Prepared {
            node,
            parent: self.id,
            entry: Some(entry),
            retained: Some(retained),
        })
    }
    fn begin(&self) -> Result<Admission, Error> {
        self.root
            .state
            .lock()
            .tree
            .begin(self.id)
            .map_err(tree_error)
    }
    fn advance(&self, admission: &mut Admission) -> Result<bool, Error> {
        self.root
            .state
            .lock()
            .tree
            .advance(admission)
            .map(|p| p == resource_tree::Progress::Checked)
            .map_err(tree_error)
    }
    fn status(&self) -> resource_tree::Status {
        self.root
            .state
            .lock()
            .tree
            .status(self.id)
            .expect("DMA node missing")
    }
    fn seal(self: &Arc<Self>) {
        {
            let mut state = self.root.state.lock();
            let Ok(status) = state.tree.status(self.id) else {
                return;
            };
            if status.finished || status.revoking {
                return;
            }
            // 树的 revoke 游标只传播封口；DMA 的硬件释放仍必须显式受信 Retire。
            state.tree.revoke(self.id).expect("DMA node missing");
            let link = state.tree.value_mut(self.id).expect("DMA node missing");
            link.stop_next = link.head;
        }
        self.notify();
    }
    fn notify(self: &Arc<Self>) {
        if !self.status().sealed {
            return;
        }
        {
            let closed = self.dispatch_closed.lock();
            if *closed {
                return;
            }
            let work: Arc<dyn Work> = self.clone();
            mt::publish(work);
        }
        mt::kick();
    }
}
impl Work for Node {
    fn record(&self) -> &WorkRecord {
        &self.record
    }
    fn step(self: Arc<Self>) -> maintenance::record::Outcome {
        use maintenance::record::Outcome;
        let (child, mapping) = {
            let mut state = self.root.state.lock();
            if let Some(id) = state
                .tree
                .next_child_to_revoke(self.id)
                .expect("DMA seal missing")
            {
                (
                    Some(
                        state
                            .tree
                            .value(id)
                            .expect("DMA child missing")
                            .node
                            .upgrade()
                            .expect("active DMA child lost owner"),
                    ),
                    None,
                )
            } else {
                let next = state
                    .tree
                    .value(self.id)
                    .expect("DMA node missing")
                    .stop_next;
                let mapping = next.map(|id| {
                    let entry = state.mappings.get(id).expect("DMA member missing");
                    let (next, mapping) = (
                        entry.next,
                        entry.mapping.upgrade().expect("DMA retention lost member"),
                    );
                    state
                        .tree
                        .value_mut(self.id)
                        .expect("DMA node missing")
                        .stop_next = next;
                    mapping
                });
                (None, mapping)
            }
        };
        if let Some(child) = child {
            child.seal();
            return Outcome::Ready;
        }
        if let Some(mapping) = mapping {
            mapping.stop();
            return Outcome::Ready;
        }
        let parent = {
            let mut state = self.root.state.lock();
            match state.tree.finish(self.id) {
                // 已暴露保留没有可推进工作；不挂永远不会来的 WakeToken，也不忙轮询。
                Err(resource_tree::Error::Incomplete) => return Outcome::Idle,
                Err(_) => panic!("DMA node lost tree membership"),
                Ok(parent) => {
                    parent.and_then(|id| state.tree.value(id).map(|link| link.node.clone()))
                }
            }
        };
        *self.dispatch_closed.lock() = true;
        let retained = NODES
            .lock()
            .remove(self.id.get())
            .expect("DMA node retention missing");
        drop(retained);
        self.completion.publish(ObjectSignals::DONE, false);
        if let Some(parent) = parent.and_then(|parent| parent.upgrade()) {
            parent.notify();
        }
        Outcome::Done
    }
}
impl Drop for Node {
    fn drop(&mut self) {
        let parent = {
            let mut state = self.root.state.lock();
            if state.tree.status(self.id).is_err() {
                return;
            }
            state.tree.seal(self.id).expect("DMA node missing");
            let parent = state
                .tree
                .finish(self.id)
                .expect("DMA node lost cleanup owner")
                .and_then(|id| state.tree.value(id).map(|link| link.node.clone()));
            let link = state.tree.value_mut(self.id).expect("DMA node missing");
            link.dropped = true;
            if link.record_children == 0 {
                state.enqueue_reclaim(self.id);
            }
            parent
        };
        if let Some(parent) = parent.and_then(|parent| parent.upgrade()) {
            parent.notify();
        }
        let work: Arc<dyn Work> = self.root.clone();
        mt::publish(work);
        mt::kick();
    }
}
impl Work for Root {
    fn record(&self) -> &WorkRecord {
        &self.record
    }
    fn step(self: Arc<Self>) -> maintenance::record::Outcome {
        let entry = {
            let mut state = self.state.lock();
            let Some(id) = state.reclaim_head else {
                return maintenance::record::Outcome::Idle;
            };
            state.reclaim_head = state
                .tree
                .value(id)
                .expect("DMA reclaim missing")
                .reclaim_next;
            if state.reclaim_head.is_none() {
                state.reclaim_tail = None;
            }
            let parent = state
                .tree
                .status(id)
                .expect("DMA reclaim status missing")
                .parent;
            let entry = state
                .tree
                .remove_finished(id)
                .expect("DMA reclaim before completion");
            if let Some(parent) = parent {
                let link = state.tree.value_mut(parent).expect("DMA lineage missing");
                assert!(link.record_children != 0, "DMA lineage count underflow");
                link.record_children -= 1;
                if link.record_children == 0 && link.dropped {
                    state.enqueue_reclaim(parent);
                }
            }
            entry
        };
        drop(entry);
        maintenance::record::Outcome::Ready
    }
}
