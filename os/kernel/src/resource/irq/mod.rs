//! IRQ 授权树与有界撤销；硬件事件由 controller 独占。
use crate::{
    maintenance::{self as mt, WakeToken, Work, WorkRecord},
    sync::{Spinlock, ranks},
    task::{
        memory_pool::MemoryPool,
        resources::{MetadataCharge, MetadataFunding, map_storage_error},
        wait_source::SignalSource,
    },
};
use alloc::sync::{Arc, Weak};
use erhino_shared::{call::SystemCallError as Error, irq::IrqScope, object::ObjectSignals};
use resource_tree::{Admission, NodeId, PreparedNode, ResourceTree};
pub(crate) mod controller;
pub(crate) mod object;

struct Link {
    node: Weak<Node>,
    allocated: u128,
}
struct Root {
    tree: Spinlock<ResourceTree<Link, MetadataCharge>>,
    _metadata: MetadataCharge,
}
struct Dispatch {
    closed: bool,
    wake: Option<WakeToken>,
}
struct Node {
    root: Arc<Root>,
    id: NodeId,
    scope: IrqScope,
    line: Option<Arc<controller::Line>>,
    completion: Option<Arc<SignalSource>>,
    record: WorkRecord,
    dispatch: Spinlock<Dispatch>,
    _metadata: MetadataCharge,
}
struct Prepared {
    node: Arc<Node>,
    parent: NodeId,
    entry: Option<PreparedNode<Link, MetadataCharge>>,
}
fn tree_error(error: resource_tree::Error) -> Error {
    match error {
        resource_tree::Error::Closed | resource_tree::Error::Missing => Error::ObjectClosed,
        resource_tree::Error::Busy => Error::ObjectBusy,
        resource_tree::Error::Capacity => Error::ReachLimit,
        resource_tree::Error::WrongRoot => Error::RightsDenied,
        resource_tree::Error::Incomplete => Error::ObjectBusy,
    }
}
fn mask(scope: IrqScope) -> Result<u128, Error> {
    if scope.first == 0
        || scope.count == 0
        || scope.targets == 0
        || scope
            .first
            .checked_add(scope.count)
            .is_none_or(|end| end > controller::SOURCES as u32 + 1)
    {
        return Err(Error::IllegalArgument);
    }
    Ok(((1u128 << scope.count) - 1) << scope.first)
}
fn prepared(pool: &Arc<MemoryPool>) -> Result<PreparedNode<Link, MetadataCharge>, Error> {
    PreparedNode::try_new(
        Link {
            node: Weak::new(),
            allocated: 0,
        },
        &MetadataFunding(pool),
    )
    .map_err(|error| match error {
        resource_tree::PrepareError::Identity(_) => Error::ReachLimit,
        resource_tree::PrepareError::Storage(error, _) => map_storage_error(error),
    })
}
impl Prepared {
    fn commit(&mut self, admission: &Admission) -> Result<(), Error> {
        let mut tree = self.node.root.tree.lock();
        if tree.validate(admission).map_err(tree_error)? != self.parent {
            return Err(Error::RightsDenied);
        }
        let bits = mask(self.node.scope)?;
        if tree
            .value(self.parent)
            .expect("IRQ parent missing")
            .allocated
            & bits
            != 0
        {
            return Err(Error::ObjectBusy);
        }
        match tree.link_child(
            admission,
            self.entry.take().expect("IRQ child committed twice"),
        ) {
            Ok(_) => {
                tree.value_mut(self.parent)
                    .expect("IRQ parent missing")
                    .allocated |= bits;
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
    fn root(pool: &Arc<MemoryPool>, scope: IrqScope) -> Result<Arc<Self>, Error> {
        mask(scope)?;
        if scope.targets & !controller::targets() != 0 {
            return Err(Error::IllegalArgument);
        }
        let entry = prepared(pool)?;
        let id = entry.id();
        let metadata = MetadataCharge::arc::<Root>(pool)?;
        let mut storage = Arc::<Root>::try_new_uninit().map_err(|_| Error::OutOfMemory)?;
        Arc::get_mut(&mut storage)
            .expect("unpublished IRQ root is shared")
            .write(Root {
                tree: Spinlock::new(ranks::RESOURCE_TREE, ResourceTree::new(entry)),
                _metadata: metadata,
            });
        // SAFETY: 唯一未发布 Arc 已完整初始化。
        let root = unsafe { storage.assume_init() };
        let node = match Self::new(root.clone(), id, scope, None, pool) {
            Ok(node) => node,
            Err(error) => {
                let entry = {
                    let mut tree = root.tree.lock();
                    if tree.status(id).is_ok() {
                        tree.seal(id).expect("IRQ root missing");
                        tree.finish(id).expect("unpublished IRQ root has children");
                        Some(tree.remove_finished(id).expect("IRQ root not finished"))
                    } else {
                        None
                    }
                };
                drop(entry);
                return Err(error);
            }
        };
        root.tree
            .lock()
            .value_mut(id)
            .expect("IRQ root missing")
            .node = Arc::downgrade(&node);
        Ok(node)
    }
    fn new(
        root: Arc<Root>,
        id: NodeId,
        scope: IrqScope,
        line: Option<Arc<controller::Line>>,
        pool: &Arc<MemoryPool>,
    ) -> Result<Arc<Self>, Error> {
        let metadata = MetadataCharge::arc::<Self>(pool)?;
        let completion = if line.is_none() {
            Some(SignalSource::try_new(pool.clone())?)
        } else {
            None
        };
        Arc::try_new(Self {
            root,
            id,
            scope,
            line,
            completion,
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
        .map_err(|_| Error::OutOfMemory)
    }
    fn prepare(
        self: &Arc<Self>,
        pool: &Arc<MemoryPool>,
        scope: IrqScope,
        control: bool,
    ) -> Result<Prepared, Error> {
        if mask(scope)? & !mask(self.scope)? != 0 || scope.targets & !self.scope.targets != 0 {
            return Err(Error::RightsDenied);
        }
        if !control && (scope.count != 1 || scope.targets.count_ones() != 1) {
            return Err(Error::IllegalArgument);
        }
        let line = if control {
            None
        } else {
            Some(controller::Line::new(
                pool,
                scope.first,
                scope.targets.trailing_zeros() as usize,
            )?)
        };
        let mut entry = prepared(pool)?;
        let node = Self::new(self.root.clone(), entry.id(), scope, line, pool)?;
        entry.value_mut().node = Arc::downgrade(&node);
        Ok(Prepared {
            node,
            parent: self.id,
            entry: Some(entry),
        })
    }
    fn begin(&self) -> Result<Admission, Error> {
        self.root.tree.lock().begin(self.id).map_err(tree_error)
    }
    fn advance(&self, admission: &mut Admission) -> Result<bool, Error> {
        self.root
            .tree
            .lock()
            .advance(admission)
            .map(|p| p == resource_tree::Progress::Checked)
            .map_err(tree_error)
    }
    fn status(&self) -> resource_tree::Status {
        self.root
            .tree
            .lock()
            .status(self.id)
            .expect("IRQ node missing")
    }
    fn seal(&self) {
        self.root
            .tree
            .lock()
            .seal(self.id)
            .expect("IRQ node missing");
    }
    fn revoke(self: &Arc<Self>) {
        {
            let mut tree = self.root.tree.lock();
            let Ok(status) = tree.status(self.id) else {
                return;
            };
            if status.finished || status.revoking {
                return;
            }
            tree.revoke(self.id).expect("IRQ node missing");
        }
        self.notify();
    }
    fn notify(self: &Arc<Self>) {
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
impl Work for Node {
    fn record(&self) -> &WorkRecord {
        &self.record
    }
    fn step(self: Arc<Self>) -> maintenance::record::Outcome {
        use maintenance::record::Outcome;
        let old = self.dispatch.lock().wake.take();
        drop(old);
        let child = {
            let mut tree = self.root.tree.lock();
            tree.next_child_to_revoke(self.id)
                .expect("IRQ revoke missing")
                .map(|id| {
                    tree.value(id)
                        .expect("IRQ child missing")
                        .node
                        .upgrade()
                        .expect("live IRQ child lost owner")
                })
        };
        if let Some(child) = child {
            child.revoke();
            return Outcome::Ready;
        }
        // close 包含未 activate 的候选：树提交后并发撤销不能留下迟到启源窗口。
        if let Some(line) = &self.line {
            line.close();
        }
        {
            let mut dispatch = self.dispatch.lock();
            let root: Arc<dyn Work> = self.clone();
            dispatch.wake = Some(mt::arm(&root));
        }
        let parent = {
            let mut tree = self.root.tree.lock();
            match tree.finish(self.id) {
                Err(resource_tree::Error::Incomplete) => return Outcome::Blocked,
                Err(_) => panic!("IRQ close lost tree membership"),
                Ok(parent) => parent.map(|id| {
                    let link = tree.value_mut(id).expect("IRQ parent missing");
                    link.allocated &= !mask(self.scope).expect("invalid admitted IRQ scope");
                    link.node.clone()
                }),
            }
        };
        let wake = {
            let mut dispatch = self.dispatch.lock();
            dispatch.closed = true;
            dispatch.wake.take()
        };
        drop(wake);
        if let Some(line) = &self.line {
            line.complete();
        }
        if let Some(completion) = &self.completion {
            completion.publish(ObjectSignals::DONE, false);
        }
        if let Some(parent) = parent.and_then(|parent| parent.upgrade()) {
            parent.notify();
        }
        Outcome::Done
    }
}
impl Drop for Node {
    fn drop(&mut self) {
        let (entry, parent) = {
            let mut tree = self.root.tree.lock();
            if tree.status(self.id).is_err() {
                return;
            }
            tree.seal(self.id).expect("IRQ node missing");
            let parent = tree
                .finish(self.id)
                .expect("IRQ node lost cleanup owner")
                .map(|id| {
                    let link = tree.value_mut(id).expect("IRQ parent missing");
                    link.allocated &= !mask(self.scope).expect("invalid admitted IRQ scope");
                    link.node.clone()
                });
            (
                tree.remove_finished(self.id)
                    .expect("IRQ node not finished"),
                parent,
            )
        };
        drop(entry);
        if let Some(parent) = parent.and_then(|parent| parent.upgrade()) {
            parent.notify();
        }
    }
}
