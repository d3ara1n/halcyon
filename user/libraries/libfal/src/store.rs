//! 稳定节点身份、链接/pin 分账与有界退休。节点内容不拥有递归目录树。

use crate::resource::FalResource;
use alloc::{
    rc::{Rc, Weak},
    sync::Arc,
};
use core::{
    cell::{Cell, RefCell},
    mem::ManuallyDrop,
    sync::atomic::{AtomicUsize, Ordering},
};
use erhino_shared::call::SystemCallError;
use libbudget::{AccountView, Charge};
use metadata_admission::{Counter, Permit};
use ordered_table::{OrderedTable, PreparedEntry};

static NEXT_NODE: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);
static ABANDONED: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeId(u64);
impl NodeId {
    pub const fn from_raw(raw: u64) -> Option<Self> {
        if raw == 0 { None } else { Some(Self(raw)) }
    }

    pub fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetireProgress {
    pub work_done: usize,
    pub done: bool,
}

pub trait Payload: Sized {
    /// 完成表示业务 owner 和对子节点的链接已全部释放，剩余壳的析构有界。
    fn retire(
        &mut self,
        context: &mut RetireContext<'_, Self>,
        budget: usize,
    ) -> Result<RetireProgress, SystemCallError>;
}

struct RetireQueue {
    head: RefCell<Option<Rc<Lease>>>,
    wake: Rc<dyn libexecution::wake::Wake>,
}
struct Lease {
    id: NodeId,
    pins: Cell<usize>,
    links: Cell<usize>,
    resident: Cell<bool>,
    queued: Cell<bool>,
    next: RefCell<Option<Rc<Lease>>>,
    retire: Weak<RetireQueue>,
}

impl Lease {
    fn enqueue(self: &Rc<Self>) {
        if !self.resident.get()
            || self.links.get() != 0
            || self.pins.get() != 0
            || self.queued.replace(true)
        {
            return;
        }
        if let Some(queue) = self.retire.upgrade() {
            let previous = queue.head.borrow_mut().take();
            *self.next.borrow_mut() = previous;
            *queue.head.borrow_mut() = Some(self.clone());
            queue.wake.publish();
        }
    }
}

pub struct NodeRef {
    lease: Rc<Lease>,
}
impl NodeRef {
    pub fn id(&self) -> NodeId {
        self.lease.id
    }
}
impl Clone for NodeRef {
    fn clone(&self) -> Self {
        self.lease.pins.set(
            self.lease
                .pins
                .get()
                .checked_add(1)
                .expect("node pin count overflow"),
        );
        Self {
            lease: self.lease.clone(),
        }
    }
}
impl Drop for NodeRef {
    fn drop(&mut self) {
        let pins = self.lease.pins.get();
        assert!(pins != 0, "node pin count underflow");
        self.lease.pins.set(pins - 1);
        self.lease.enqueue();
    }
}
impl core::fmt::Debug for NodeRef {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_tuple("NodeRef").field(&self.id()).finish()
    }
}

struct Record<P> {
    lease: Rc<Lease>,
    payload: Option<P>,
    _slot: Permit,
    _node_charge: Charge,
    _storage_charge: Charge,
}

pub struct PreparedNode<P> {
    entry: PreparedEntry<Record<P>>,
    reference: NodeRef,
}
impl<P> PreparedNode<P> {
    pub fn id(&self) -> NodeId {
        self.reference.id()
    }
}

pub struct NodeStore<P> {
    records: ManuallyDrop<OrderedTable<Record<P>>>,
    slots: Arc<Counter>,
    retire: Rc<RetireQueue>,
    current: Option<NodeId>,
    root: NodeId,
    sealed: bool,
}

pub struct RetireContext<'a, P> {
    store: &'a mut NodeStore<P>,
}
impl<P> RetireContext<'_, P> {
    pub fn unlink(&mut self, child: NodeId) {
        let record = self
            .store
            .records
            .get(child.raw())
            .expect("retired directory child missing");
        let links = record.lease.links.get();
        assert!(links != 0, "node link count underflow");
        record.lease.links.set(links - 1);
        record.lease.enqueue();
    }
}

#[derive(Debug)]
pub struct PrepareFailure<P> {
    pub error: SystemCallError,
    pub payload: P,
}

impl<P> NodeStore<P> {
    pub fn new(
        root: P,
        account: &AccountView<FalResource>,
        limit: usize,
        wake: Rc<dyn libexecution::wake::Wake>,
    ) -> Result<(Self, NodeRef), PrepareFailure<P>> {
        let setup = (|| {
            if limit == 0 {
                return Err(SystemCallError::IllegalArgument);
            }
            let slots =
                Arc::try_new(Counter::new(limit)).map_err(|_| SystemCallError::OutOfMemory)?;
            let retire = Rc::try_new(RetireQueue {
                head: RefCell::new(None),
                wake,
            })
            .map_err(|_| SystemCallError::OutOfMemory)?;
            Ok((slots, retire))
        })();
        let (slots, retire) = match setup {
            Ok(setup) => setup,
            Err(error) => {
                return Err(PrepareFailure {
                    error,
                    payload: root,
                });
            }
        };
        let mut store = Self {
            records: ManuallyDrop::new(OrderedTable::new(limit)),
            slots,
            retire,
            current: None,
            root: NodeId(0),
            sealed: false,
        };
        let prepared = store.prepare(root, account)?;
        store.root = prepared.id();
        let root = store.commit(prepared);
        store.link(&root);
        Ok((store, root))
    }

    pub fn prepare(
        &self,
        payload: P,
        account: &AccountView<FalResource>,
    ) -> Result<PreparedNode<P>, PrepareFailure<P>> {
        let reserve = (|| {
            if self.sealed {
                return Err(SystemCallError::ObjectClosed);
            }
            let slot =
                Counter::try_acquire(&self.slots).map_err(|_| SystemCallError::QuotaExceeded)?;
            let node_charge = account.acquire(FalResource::Node, 1)?;
            let storage_charge = account.acquire(
                FalResource::Bytes,
                PreparedEntry::<Record<P>>::allocation_bytes() + core::mem::size_of::<Lease>(),
            )?;
            let id = NodeId(NEXT_NODE.allocate().ok_or(SystemCallError::ReachLimit)?);
            let lease = Rc::try_new(Lease {
                id,
                pins: Cell::new(1),
                links: Cell::new(0),
                resident: Cell::new(false),
                queued: Cell::new(false),
                next: RefCell::new(None),
                retire: Rc::downgrade(&self.retire),
            })
            .map_err(|_| SystemCallError::OutOfMemory)?;
            Ok((slot, node_charge, storage_charge, lease))
        })();
        let (slot, node_charge, storage_charge, lease) = match reserve {
            Ok(reserved) => reserved,
            Err(error) => return Err(PrepareFailure { error, payload }),
        };
        let reference = NodeRef {
            lease: lease.clone(),
        };
        let record = Record {
            lease,
            payload: Some(payload),
            _slot: slot,
            _node_charge: node_charge,
            _storage_charge: storage_charge,
        };
        match self.records.prepare_insert(reference.id().raw(), record) {
            Ok(entry) => Ok(PreparedNode { entry, reference }),
            Err(error) => {
                let (error, record) = match error {
                    ordered_table::InsertError::Limit(record) => {
                        (SystemCallError::QuotaExceeded, record)
                    }
                    ordered_table::InsertError::Allocation(record) => {
                        (SystemCallError::OutOfMemory, record)
                    }
                };
                Err(PrepareFailure {
                    error,
                    payload: record.payload.expect("prepared payload missing"),
                })
            }
        }
    }

    /// 调用者最终校验 sealed/位置代次后提交；容量已由 prepared slot 独占预留。
    pub fn commit(&mut self, prepared: PreparedNode<P>) -> NodeRef {
        assert!(!self.sealed, "node store sealed before validated commit");
        prepared.reference.lease.resident.set(true);
        self.records.insert_prepared(prepared.entry);
        prepared.reference
    }

    pub fn get(&self, reference: &NodeRef) -> Option<&P> {
        self.records
            .get(reference.id().raw())
            .and_then(|record| record.payload.as_ref())
    }
    pub fn get_mut(&mut self, reference: &NodeRef) -> Option<&mut P> {
        self.records
            .get_mut(reference.id().raw())
            .and_then(|record| record.payload.as_mut())
    }

    /// 裸 NodeId 不是授权；只有当前目录项还链接的节点可由已鉴权走路流程 pin。
    pub fn pin_linked(&self, id: NodeId) -> Option<NodeRef> {
        let lease = &self.records.get(id.raw())?.lease;
        if lease.links.get() == 0 {
            return None;
        }
        lease.pins.set(
            lease
                .pins
                .get()
                .checked_add(1)
                .expect("node pin count overflow"),
        );
        Some(NodeRef {
            lease: lease.clone(),
        })
    }

    pub fn link(&mut self, reference: &NodeRef) {
        let record = self
            .records
            .get(reference.id().raw())
            .expect("linked node missing");
        assert!(!record.lease.queued.get(), "retiring node cannot be linked");
        record.lease.links.set(
            record
                .lease
                .links
                .get()
                .checked_add(1)
                .expect("node link count overflow"),
        );
    }
    pub fn unlink(&mut self, reference: &NodeRef) {
        RetireContext { store: self }.unlink(reference.id());
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
    pub fn is_sealed(&self) -> bool {
        self.sealed
    }
    pub fn has_retire_work(&self) -> bool {
        self.current.is_some() || self.retire.head.borrow().is_some()
    }

    /// 撤下根链接；grant/流保留的 pin 仍须先退休，不能越过真实存活引用。
    pub fn seal(&mut self) {
        if self.sealed {
            return;
        }
        self.sealed = true;
        let root = self.root;
        RetireContext { store: self }.unlink(root);
    }
    pub fn close(self) -> Result<(), Self> {
        if self.sealed && self.records.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }
}

impl<P: Payload> NodeStore<P> {
    pub fn retire_step(&mut self, budget: usize) -> Result<RetireProgress, SystemCallError> {
        if budget == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        if self.current.is_none() {
            let head = self.retire.head.borrow_mut().take();
            if let Some(head) = head {
                let next = head.next.borrow_mut().take();
                *self.retire.head.borrow_mut() = next;
                self.current = Some(head.id);
            }
        }
        let Some(id) = self.current else {
            return Ok(RetireProgress {
                work_done: 0,
                done: self.records.is_empty(),
            });
        };
        let mut payload = self
            .records
            .get_mut(id.raw())
            .expect("retire node missing")
            .payload
            .take()
            .expect("retire payload missing");
        let result = payload.retire(&mut RetireContext { store: self }, budget);
        self.records
            .get_mut(id.raw())
            .expect("retire node disappeared")
            .payload = Some(payload);
        let progress = result?;
        if progress.work_done > budget {
            return Err(SystemCallError::InternalError);
        }
        let mut work_done = progress.work_done;
        if progress.done && work_done < budget {
            let record = self.records.remove(id.raw()).expect("retired node missing");
            record.lease.resident.set(false);
            drop(record);
            self.current = None;
            work_done += 1;
        }
        Ok(RetireProgress {
            work_done,
            done: self.records.is_empty(),
        })
    }
}

impl<P> Drop for NodeStore<P> {
    fn drop(&mut self) {
        if self.records.is_empty() {
            // 空表不拥有业务内容或退休链，释放表壳有界。
            unsafe {
                ManuallyDrop::drop(&mut self.records);
            }
        } else {
            ABANDONED.fetch_add(self.records.len(), Ordering::Relaxed);
        }
    }
}

pub fn abandoned_nodes() -> usize {
    ABANDONED.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use libbudget::{Budget, Taxonomy};

    #[derive(Debug)]
    struct Directory {
        children: Vec<NodeId>,
    }
    impl Payload for Directory {
        fn retire(
            &mut self,
            context: &mut RetireContext<'_, Self>,
            budget: usize,
        ) -> Result<RetireProgress, SystemCallError> {
            let mut work_done = 0;
            while work_done < budget {
                let Some(child) = self.children.pop() else {
                    break;
                };
                context.unlink(child);
                work_done += 1;
            }
            Ok(RetireProgress {
                work_done,
                done: self.children.is_empty(),
            })
        }
    }

    struct TestWake;
    impl libexecution::wake::Wake for TestWake {
        fn publish(&self) {}
    }
    fn wake() -> Rc<dyn libexecution::wake::Wake> {
        Rc::new(TestWake)
    }

    fn account() -> AccountView<FalResource> {
        let mut limits = [0; FalResource::COUNT];
        limits[FalResource::Node.slot()] = 10;
        limits[FalResource::Bytes.slot()] = 10000;
        let budget = Budget::new(&limits, 1).unwrap();
        let account = budget.account(&limits).unwrap();
        let binding: [_; FalResource::COUNT] =
            core::array::from_fn(|index| budget.slot(index).unwrap());
        account.view(&binding).unwrap()
    }

    #[test]
    fn removed_name_keeps_pinned_identity_until_last_reference() {
        let account = account();
        let (mut store, root) = NodeStore::new(
            Directory {
                children: Vec::new(),
            },
            &account,
            4,
            wake(),
        )
        .unwrap();
        let prepared = store
            .prepare(
                Directory {
                    children: Vec::new(),
                },
                &account,
            )
            .unwrap();
        let child = store.commit(prepared);
        let identity = child.id();
        store.link(&child);
        let retained = child.clone();
        store.unlink(&child);
        assert!(store.pin_linked(identity).is_none());
        assert!(store.get(&retained).is_some());
        drop(child);
        assert!(!store.has_retire_work());
        drop(retained);
        assert!(store.has_retire_work());
        assert_eq!(store.retire_step(1).unwrap().work_done, 1);
        assert_eq!(store.len(), 1);
        store.seal();
        assert!(!store.has_retire_work());
        assert!(!store.retire_step(1).unwrap().done);
        drop(root);
        assert!(store.retire_step(1).unwrap().done);
        assert_eq!(account.usage(FalResource::Node).0, 0);
        assert!(store.close().is_ok());
    }

    #[test]
    fn pending_node_reserves_table_capacity_and_cancel_does_not_queue_ghost() {
        let account = account();
        let (mut store, root) = NodeStore::new(
            Directory {
                children: Vec::new(),
            },
            &account,
            2,
            wake(),
        )
        .unwrap();
        let pending = store
            .prepare(
                Directory {
                    children: Vec::new(),
                },
                &account,
            )
            .unwrap();
        assert!(matches!(
            store.prepare(
                Directory {
                    children: Vec::new()
                },
                &account
            ),
            Err(PrepareFailure {
                error: SystemCallError::QuotaExceeded,
                ..
            })
        ));
        drop(pending);
        assert!(!store.has_retire_work());
        assert_eq!(account.usage(FalResource::Node).0, 1);
        store.seal();
        drop(root);
        assert!(store.retire_step(1).unwrap().done);
        assert!(store.close().is_ok());
    }

    #[test]
    fn directory_retirement_unlinks_one_child_per_work_unit() {
        let account = account();
        let (mut store, root) = NodeStore::new(
            Directory {
                children: Vec::new(),
            },
            &account,
            4,
            wake(),
        )
        .unwrap();
        let prepared = store
            .prepare(
                Directory {
                    children: Vec::new(),
                },
                &account,
            )
            .unwrap();
        let child = store.commit(prepared);
        store.link(&child);
        store.get_mut(&root).unwrap().children.push(child.id());
        drop(child);
        store.seal();
        drop(root);
        assert_eq!(store.retire_step(1).unwrap().work_done, 1);
        assert_eq!(store.len(), 2);
        assert_eq!(store.retire_step(1).unwrap().work_done, 1);
        assert!(store.retire_step(1).unwrap().done);
        assert!(store.close().is_ok());
    }
}
