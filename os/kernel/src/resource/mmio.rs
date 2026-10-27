//! MMIO Map 的稳定资源记录。根门统一保护发布、片段责任和关闭；
//! 记录只弱定位 VM，不持地址空间访问租约，不持有或归还 RAM 帧。

mod node;
pub(crate) mod object;
pub(crate) use node::{MmioNode, PreparedMmioNode};

use crate::{
    sync::{Spinlock, ranks},
    task::{
        memory_pool::MemoryPool,
        resources::{MetadataCharge, MetadataFunding, map_storage_error},
        wait_source::{ObservationSource, SignalSource},
    },
};
use alloc::sync::{Arc, Weak};
use erhino_shared::{call::SystemCallError, object::ObjectSignals};
use memory_space::{MappingId, MmioMapError, MmioMapPhase, MmioMapState, PAGE_SIZE};
use ordered_table::{OrderedTable, PreparedEntry};
use resource_tree::{Admission, NodeId, ResourceTree};

static NEXT_MAPPING: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

struct Entry {
    state: MmioMapState,
    mapping: Weak<MmioMapping>,
    previous: Option<u64>,
    next: Option<u64>,
}
struct State {
    tree: ResourceTree<node::Link, MetadataCharge>,
    mappings: OrderedTable<Entry, MetadataCharge>,
}

/// 同一真实 MMIO 根的成员门；表只保存状态，不强持反向引用根的 Map 记录。
pub(crate) struct MmioMappings {
    state: Spinlock<State>,
    _metadata: MetadataCharge,
}
impl Drop for MmioMappings {
    fn drop(&mut self) {
        assert!(
            self.state.get_mut().mappings.is_empty(),
            "MMIO root dropped with live mappings"
        );
        let tree = &mut self.state.get_mut().tree;
        if !tree.is_empty() {
            // 根宿主已付款但 MmioNode 尚未成功出生的失败路径，最多退这一个空根。
            let root = tree.root();
            assert!(
                tree.value(root)
                    .is_some_and(|link| link.node.strong_count() == 0),
                "MMIO tree dropped with live nodes"
            );
            tree.seal(root).expect("MMIO root missing");
            tree.finish(root)
                .expect("MMIO root retained responsibility");
            drop(tree.remove_finished(root).expect("MMIO root not finished"));
            assert!(tree.is_empty(), "MMIO tree retained descendants");
        }
    }
}

pub(crate) struct MmioMapping {
    owner: Arc<MmioNode>,
    id: MappingId,
    physical: usize,
    pages: usize,
    vm: Weak<crate::vm::change::VmShell>,
    completion: Arc<SignalSource>,
    operation: Arc<dyn ObservationSource>,
    queue: Spinlock<Option<crate::vm::forced::QueueNode>>,
    _metadata: MetadataCharge,
}
impl MmioMapping {
    pub(crate) fn prepare(
        owner: Arc<MmioNode>,
        admission: &Admission,
        vm: &Arc<crate::vm::change::VmShell>,
        pool: &Arc<MemoryPool>,
        offset: usize,
        bytes: usize,
        operation: Arc<dyn ObservationSource>,
    ) -> Result<(MmioViewUse, MmioPublication), SystemCallError> {
        let physical = owner.subrange(offset, bytes)?;
        let id = MappingId::new(NEXT_MAPPING.allocate().ok_or(SystemCallError::ReachLimit)?)
            .expect("mapping identity is nonzero");
        let metadata = MetadataCharge::arc::<Self>(pool)?;
        let queue = crate::vm::forced::prepare_node(id.get(), pool)?;
        let mut entry = PreparedEntry::try_new(
            id.get(),
            Entry {
                state: MmioMapState::new(),
                mapping: Weak::new(),
                previous: None,
                next: None,
            },
            &MetadataFunding(pool),
        )
        .map_err(|error| match error {
            ordered_table::InsertError::Storage(error, _) => map_storage_error(error),
            ordered_table::InsertError::Limit(_) => SystemCallError::ReachLimit,
        })?;
        let mapping = Arc::try_new(Self {
            owner: owner.clone(),
            id,
            physical,
            pages: bytes / PAGE_SIZE,
            vm: Arc::downgrade(vm),
            completion: SignalSource::try_new(pool.clone())?,
            operation,
            queue: Spinlock::new(ranks::LEAF, Some(queue)),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        entry.value_mut().mapping = Arc::downgrade(&mapping);
        {
            let mut gate = owner.root.state.lock();
            if gate.tree.validate(admission).map_err(tree_error)? != owner.id {
                return Err(SystemCallError::RightsDenied);
            }
            gate.tree.add_member(admission).map_err(tree_error)?;
            let tail = gate
                .tree
                .value(owner.id)
                .expect("MMIO owner disappeared")
                .tail;
            entry.value_mut().previous = tail;
            if let Some(tail) = tail {
                gate.mappings
                    .get_mut(tail)
                    .expect("MMIO tail disappeared")
                    .next = Some(id.get());
            } else {
                gate.tree
                    .value_mut(owner.id)
                    .expect("MMIO owner disappeared")
                    .head = Some(id.get());
            }
            gate.tree
                .value_mut(owner.id)
                .expect("MMIO owner disappeared")
                .tail = Some(id.get());
            gate.mappings.insert_prepared(entry);
        }
        Ok((
            MmioViewUse {
                mapping: Some(mapping.clone()),
                lo: 0,
                hi: bytes / PAGE_SIZE,
            },
            MmioPublication {
                mapping: Some(mapping),
                admission: None,
            },
        ))
    }

    pub(crate) fn id(&self) -> MappingId {
        self.id
    }

    pub(crate) fn completion(&self) -> Arc<SignalSource> {
        self.completion.clone()
    }
    pub(crate) fn operation_completion(&self) -> Arc<dyn ObservationSource> {
        self.operation.clone()
    }

    fn update(&self, action: impl FnOnce(&mut MmioMapState)) {
        let detached = {
            let mut gate = self.owner.root.state.lock();
            let state = &mut gate
                .mappings
                .get_mut(self.id.get())
                .expect("live MMIO map lost root membership")
                .state;
            let before = state.phase();
            action(state);
            let detached =
                before != MmioMapPhase::Detached && state.phase() == MmioMapPhase::Detached;
            if detached {
                let entry = gate
                    .mappings
                    .get(self.id.get())
                    .expect("MMIO member disappeared");
                let (previous, next) = (entry.previous, entry.next);
                if let Some(previous) = previous {
                    gate.mappings
                        .get_mut(previous)
                        .expect("MMIO predecessor disappeared")
                        .next = next;
                } else {
                    gate.tree
                        .value_mut(self.owner.id)
                        .expect("MMIO owner disappeared")
                        .head = next;
                }
                if let Some(next) = next {
                    gate.mappings
                        .get_mut(next)
                        .expect("MMIO successor disappeared")
                        .previous = previous;
                } else {
                    gate.tree
                        .value_mut(self.owner.id)
                        .expect("MMIO owner disappeared")
                        .tail = previous;
                }
                let link = gate
                    .tree
                    .value_mut(self.owner.id)
                    .expect("MMIO owner disappeared");
                if link.revoke_next == Some(self.id.get()) {
                    link.revoke_next = next;
                }
                gate.tree.finish_member(self.owner.id);
            }
            detached
        };
        if detached {
            self.completion.publish(ObjectSignals::DONE, false);
            self.owner.notify();
        }
    }

    pub(crate) fn request_stop(self: &Arc<Self>) {
        self.update(MmioMapState::request_stop);
        let entry = self.queue.lock().take();
        let Some(mut entry) = entry else {
            return;
        };
        if let Some(vm) = self.vm.upgrade()
            && let Some(forced) = vm.forced()
        {
            entry.value_mut().mapping = Some(self.clone());
            forced.enqueue(entry);
        }
    }

    fn reserve_fragment(self: &Arc<Self>) -> Result<MmioSplit, SystemCallError> {
        self.owner
            .root
            .state
            .lock()
            .mappings
            .get_mut(self.id.get())
            .expect("live MMIO map lost root membership")
            .state
            .reserve_fragment()
            .map_err(map_error)?;
        Ok(MmioSplit {
            mapping: Some(self.clone()),
        })
    }
}
impl Drop for MmioMapping {
    fn drop(&mut self) {
        let removed = self.owner.root.state.lock().mappings.remove(self.id.get());
        if let Some(entry) = removed {
            assert_eq!(
                entry.value().state.phase(),
                MmioMapPhase::Detached,
                "MMIO map lost all owners before retirement"
            );
            drop(entry);
        }
    }
}
fn map_error(error: MmioMapError) -> SystemCallError {
    match error {
        MmioMapError::Closed => SystemCallError::ObjectClosed,
        MmioMapError::Busy => SystemCallError::ObjectBusy,
        MmioMapError::Capacity => SystemCallError::ReachLimit,
    }
}
fn tree_error(error: resource_tree::Error) -> SystemCallError {
    match error {
        resource_tree::Error::Closed => SystemCallError::ObjectClosed,
        resource_tree::Error::Busy => SystemCallError::ObjectBusy,
        resource_tree::Error::Capacity => SystemCallError::ReachLimit,
        resource_tree::Error::Missing
        | resource_tree::Error::Incomplete
        | resource_tree::Error::WrongRoot => {
            unreachable!("internal MMIO root admission lost tree identity")
        }
    }
}

/// 原 Map 操作的最终回执，移入 Completion，绝不由某个 View 代替它完成。
pub(crate) struct MmioPublication {
    mapping: Option<Arc<MmioMapping>>,
    admission: Option<Admission>,
}
impl MmioPublication {
    pub(crate) fn advance(&mut self) -> Result<bool, SystemCallError> {
        let owner = &self
            .mapping
            .as_ref()
            .expect("MMIO publication finished")
            .owner;
        let gate = owner.root.state.lock();
        if self.admission.is_none() {
            self.admission = Some(gate.tree.begin(owner.id).map_err(tree_error)?);
        }
        gate.tree
            .advance(self.admission.as_mut().expect("MMIO admission missing"))
            .map(|progress| progress == resource_tree::Progress::Checked)
            .map_err(tree_error)
    }
    pub(crate) fn grant(&self) -> Result<(), SystemCallError> {
        let mapping = self
            .mapping
            .as_ref()
            .expect("MMIO publication already finished");
        let mut gate = mapping.owner.root.state.lock();
        gate.tree
            .validate(
                self.admission
                    .as_ref()
                    .expect("MMIO publication skipped ancestors"),
            )
            .map_err(tree_error)?;
        gate.mappings
            .get_mut(mapping.id.get())
            .expect("MMIO publication lost membership")
            .state
            .grant_publish()
            .map_err(map_error)
    }
    pub(crate) fn commit(&self) {
        self.mapping
            .as_ref()
            .expect("MMIO publication already finished")
            .update(MmioMapState::commit);
    }
}
impl Drop for MmioPublication {
    fn drop(&mut self) {
        if let Some(mapping) = self.mapping.take() {
            mapping.update(MmioMapState::finish_operation);
        }
    }
}

#[must_use = "MMIO fragment must be transferred or retired after translation synchronization"]
pub(crate) struct MmioViewUse {
    mapping: Option<Arc<MmioMapping>>,
    lo: usize,
    hi: usize,
}
impl MmioViewUse {
    pub(crate) fn mapping(&self) -> &Arc<MmioMapping> {
        self.mapping
            .as_ref()
            .expect("MMIO fragment already transferred")
    }
    pub(crate) fn range(&self) -> (u64, u64) {
        (self.lo as u64, self.hi as u64)
    }
    pub(crate) fn physical_span(&self, page: u64) -> Option<(page_table::Ppn, usize)> {
        let page = usize::try_from(page).ok()?;
        let mapping = self.mapping();
        if page < self.lo || page >= self.hi || self.hi > mapping.pages {
            return None;
        }
        Some((
            page_table::Ppn(mapping.physical / PAGE_SIZE + page),
            self.hi - page,
        ))
    }
    pub(crate) fn split(&mut self, page: u64, prepared: &mut MmioSplit) -> Self {
        let page = usize::try_from(page).expect("MMIO split exceeds address width");
        assert!(
            page > self.lo && page < self.hi,
            "MMIO split outside fragment"
        );
        let mapping = prepared
            .mapping
            .take()
            .expect("MMIO split reservation consumed");
        assert!(
            Arc::ptr_eq(&mapping, self.mapping()),
            "MMIO split changed mapping"
        );
        let right = Self {
            mapping: Some(mapping),
            lo: page,
            hi: self.hi,
        };
        self.hi = page;
        right
    }
    pub(crate) fn retire(mut self) {
        let mapping = self.mapping.take().expect("MMIO fragment retired twice");
        mapping.update(MmioMapState::release_fragment);
    }
}
impl Drop for MmioViewUse {
    fn drop(&mut self) {
        assert!(
            self.mapping.is_none(),
            "MMIO fragment dropped before responsibility transfer"
        );
    }
}

/// 准备即持片段责任，提交仅移动；未使用的准备在 VM/Views 门外取消。
pub(crate) struct MmioSplit {
    mapping: Option<Arc<MmioMapping>>,
}
impl MmioSplit {
    pub(crate) fn prepare(mapping: &Arc<MmioMapping>) -> Result<Self, SystemCallError> {
        mapping.reserve_fragment()
    }
}
impl Drop for MmioSplit {
    fn drop(&mut self) {
        if let Some(mapping) = self.mapping.take() {
            mapping.update(MmioMapState::release_fragment);
        }
    }
}
