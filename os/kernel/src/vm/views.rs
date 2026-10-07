//! 每空间的实物 View/use 与写许可来源索引。几何/权限仍由 MemorySpace 拥有；
//! 此处只保存仿射资源，并在准备期物化切界、fence 后逐项移交退休。

use super::backing::{BackingRelease, BackingSplit, BackingUse};
use crate::task::resources::{MetadataCharge, MetadataFunding, map_storage_error};
use crate::{
    sync::{Spinlock, ranks},
    task::memory_object::MemoryObjectCore,
};
use alloc::sync::Arc;
use erhino_shared::call::SystemCallError;
use memory_space::{
    BackingRetire, BackingView, PAGE_SIZE, RegionKindView, RetiringFragment, WritePermit,
};
use ordered_table::{InsertError, OrderedTable, PreparedEntry};

/// 本片段的有界资源游标；next 只在实物 use 已移交后推进，不靠“索引为空”猜完成。
pub(crate) struct ViewRetirement {
    fragment: RetiringFragment,
    next: usize,
    release: Option<BackingRelease>,
}
impl ViewRetirement {
    pub(crate) fn new(fragment: RetiringFragment) -> Self {
        Self {
            next: fragment.range.start(),
            fragment,
            release: None,
        }
    }
}

struct View {
    end: usize,
    origin: BackingView,
    /// None 只存在于未发布的预取节点/同锁切分瞬间；索引内的稳定项必须为 Some。
    usage: Option<BackingUse>,
}
struct ObjectSource {
    object: Arc<MemoryObjectCore>,
    permits: usize,
    views: usize,
}
struct State {
    views: OrderedTable<View, MetadataCharge, usize>,
    sources: OrderedTable<ObjectSource, MetadataCharge>,
}

pub(crate) struct Views {
    state: Spinlock<State>,
    _metadata: MetadataCharge,
}
pub(crate) struct PreparedView {
    node: Option<PreparedEntry<View, MetadataCharge, usize>>,
    object: Option<PreparedEntry<ObjectSource, MetadataCharge>>,
}
impl PreparedView {
    /// 未发布节点还给同一操作的回滚阶段；取消不启动额外维护任务。
    pub(crate) fn cancel(mut self) -> BackingUse {
        self.node
            .as_mut()
            .expect("prepared view already committed")
            .value_mut()
            .usage
            .take()
            .expect("prepared view lost use")
    }
}
pub(crate) struct ViewBoundary {
    node: Option<PreparedEntry<View, MetadataCharge, usize>>,
    split: BackingSplit,
}

fn insert_error<T>(error: InsertError<T, SystemCallError>) -> SystemCallError {
    match error {
        InsertError::Storage(error, _) => map_storage_error(error),
        InsertError::Limit(_) => SystemCallError::ReachLimit,
    }
}

impl Views {
    pub(crate) fn try_new(
        pool: &Arc<crate::task::memory_pool::MemoryPool>,
    ) -> Result<Arc<Self>, SystemCallError> {
        let metadata = MetadataCharge::arc::<Self>(pool)?;
        // 地址宽度与每个非空 view 至少一页限制索引高度；资源额度由付款 permit 拥有。
        Arc::try_new(Self {
            state: Spinlock::new(
                ranks::VM_VIEWS,
                State {
                    views: OrderedTable::new(usize::MAX),
                    sources: OrderedTable::new(usize::MAX),
                },
            ),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    /// 调用者先在 ledger 预约对应区间；失败原样返 use，由同一操作回滚。
    pub(crate) fn prepare_map(
        &self,
        intent: memory_space::TranslationIntent,
        usage: BackingUse,
        object: Option<Arc<MemoryObjectCore>>,
        pool: &Arc<crate::task::memory_pool::MemoryPool>,
    ) -> Result<PreparedView, (SystemCallError, BackingUse)> {
        let (range, origin, has_writes) = match intent {
            memory_space::TranslationIntent::Install {
                range,
                backing,
                protection,
            } => (range, backing, protection.writable()),
            _ => return Err((SystemCallError::IllegalArgument, usage)),
        };
        let (lo, hi) = usage.range();
        if hi - lo != range.pages() as u64 || origin.offset() as u64 != lo * PAGE_SIZE as u64 {
            return Err((SystemCallError::IllegalArgument, usage));
        }
        if let BackingView::Anonymous { identity, .. } = origin
            && identity != usage.backing().id()
        {
            return Err((SystemCallError::IllegalArgument, usage));
        }
        if origin.object() != object.as_ref().map(|object| object.identity()) {
            return Err((SystemCallError::IllegalArgument, usage));
        }
        let node = {
            PreparedEntry::try_new(
                range.start(),
                View {
                    end: range.end(),
                    origin,
                    usage: Some(usage),
                },
                &MetadataFunding(pool),
            )
        };
        let mut node = node.map_err(|error| {
            let (error, mut view) = match error {
                InsertError::Storage(error, view) => (map_storage_error(error), view),
                InsertError::Limit(view) => (SystemCallError::ReachLimit, view),
            };
            (
                error,
                view.usage.take().expect("failed preparation lost use"),
            )
        })?;
        let object = if let Some(object) = object.filter(|object| {
            if has_writes {
                // 此接口只在原 Operation 已接管 ledger 预约后调用；其写许可
                // 在提交/回滚移交前不会归还，来源不能被别的 View 退休摘走。
                let identity = object.identity();
                let state = self.state.lock();
                let source = state.sources.get(identity.get());
                assert!(
                    source.is_some_and(|source| source.permits != 0),
                    "writable map lost its reserved source"
                );
            }
            !has_writes
        }) {
            let id = object.identity().get();
            let candidate = {
                PreparedEntry::try_new(
                    id,
                    ObjectSource {
                        object,
                        permits: 0,
                        views: 1,
                    },
                    &MetadataFunding(pool),
                )
            };
            match candidate {
                Ok(candidate) => Some(candidate),
                Err(error) => {
                    return Err((
                        insert_error(error),
                        node.value_mut()
                            .usage
                            .take()
                            .expect("prepared view lost use"),
                    ));
                }
            }
        } else {
            None
        };
        Ok(PreparedView {
            node: Some(node),
            object,
        })
    }

    /// ledger 已完成最终准入，安装资源索引先于第一个有效 PTE store。
    pub(crate) fn commit_map(&self, prepared: &mut PreparedView) {
        let start = prepared
            .node
            .as_ref()
            .expect("view already committed")
            .key();
        let mut state = self.state.lock();
        if let Some((_, left)) = state.views.floor(&start) {
            assert!(left.end <= start, "view commit overlaps predecessor");
        }
        let mut node = prepared.node.take().expect("view already committed");
        if let Some((next, _)) = state.views.lower_bound(&start) {
            assert!(
                node.value_mut().end <= *next,
                "view commit overlaps successor"
            );
        }
        if let Some(identity) = node.value_mut().origin.object() {
            if let Some(owner) = state.sources.get_mut(identity.get()) {
                owner.views = owner
                    .views
                    .checked_add(1)
                    .expect("view source count overflow");
            } else {
                state.sources.insert_prepared(
                    prepared
                        .object
                        .take()
                        .expect("source candidate disappeared"),
                );
            }
        }
        state.views.insert_prepared(node);
        // 重复来源候选仍在prepared内，调用者退出VM门后释放。
    }

    /// 支持不相交事务对同一原 view 的不同内部点切界；保留同一个 backing 身份。
    /// 准备可失败，提交只消费索引/切段节点；无需在失败时合并已物化的等价切界。
    pub(crate) fn prepare_boundary(
        &self,
        at: usize,
        pool: &Arc<crate::task::memory_pool::MemoryPool>,
    ) -> Result<Option<ViewBoundary>, SystemCallError> {
        if !at.is_multiple_of(PAGE_SIZE) {
            return Err(SystemCallError::IllegalArgument);
        }
        let (origin, backing) = {
            let state = self.state.lock();
            let Some((start, view)) = state.views.floor(&at) else {
                return Ok(None);
            };
            if *start == at || view.end <= at {
                return Ok(None);
            }
            let origin = view
                .origin
                .shifted(at - *start)
                .ok_or(SystemCallError::IllegalArgument)?;
            (
                origin,
                view.usage
                    .as_ref()
                    .expect("published view has no use")
                    .backing()
                    .clone(),
            )
        };
        let node = {
            PreparedEntry::try_new(
                at,
                View {
                    end: at,
                    origin,
                    usage: None,
                },
                &MetadataFunding(pool),
            )
        }
        .map_err(insert_error)?;
        let split = backing.prepare_split(pool)?;
        Ok(Some(ViewBoundary {
            node: Some(node),
            split,
        }))
    }

    /// 两端均复检：准备时为None，不能证明现在仍无需切界。调用者持VM门。
    pub(crate) fn boundaries_ready(
        &self,
        range: memory_space::PageRange,
        prepared: &mut [ViewBoundary],
    ) -> bool {
        let state = self.state.lock();
        [range.start(), range.end()].into_iter().all(|at| {
            let Some((start, view)) = state.views.floor(&at) else {
                return true;
            };
            if *start == at || view.end <= at {
                return true;
            }
            prepared.iter_mut().any(|boundary| {
                let node = boundary.node.as_mut().expect("boundary already committed");
                node.key() == at
                    && view.origin.shifted(at - *start) == Some(node.value_mut().origin)
            })
        })
    }

    /// 节点消费后，未用节点/切段credit仍由Operation锁外清理。
    pub(crate) fn commit_boundary(&self, prepared: &mut ViewBoundary) {
        {
            let mut state = self.state.lock();
            let node = prepared.node.as_mut().expect("boundary already committed");
            let at = node.key();
            let Some((start, _)) = state.views.floor(&at) else {
                return;
            };
            let start = *start;
            let old = state.views.get_mut(start).expect("view floor disappeared");
            // 相邻事务可以先物化这个边界，再退掉或替换边界另一侧。
            // 已是任一侧端点就无需切分，不能要求原右半 owner 仍存在。
            if start == at || old.end <= at {
                return;
            }
            assert!(old.end > at, "reserved boundary lost its view");
            assert_eq!(
                old.origin.shifted(at - start),
                Some(node.value_mut().origin),
                "boundary changed backing identity"
            );
            let usage = old.usage.as_mut().expect("view split lost use");
            let right = usage.split_prepared(
                usage.range().0 + ((at - start) / PAGE_SIZE) as u64,
                &mut prepared.split,
            );
            let new = node.value_mut();
            new.end = old.end;
            new.usage = Some(right);
            old.end = at;
            if let Some(id) = node.value_mut().origin.object() {
                let source = state
                    .sources
                    .get_mut(id.get())
                    .expect("split view lost source");
                source.views = source
                    .views
                    .checked_add(1)
                    .expect("view source count overflow");
            }
            state
                .views
                .insert_prepared(prepared.node.take().expect("boundary node disappeared"));
        }
    }

    /// ledger 接走写许可前登记其来源强根；计数只表示本空间当前持有的这批许可。
    pub(crate) fn hold_writes(
        &self,
        object: Arc<MemoryObjectCore>,
        count: usize,
        pool: &Arc<crate::task::memory_pool::MemoryPool>,
    ) -> Result<(), SystemCallError> {
        if count == 0 {
            return Ok(());
        }
        let id = object.identity().get();
        {
            let mut state = self.state.lock();
            if let Some(held) = state.sources.get_mut(id) {
                held.permits = held
                    .permits
                    .checked_add(count)
                    .ok_or(SystemCallError::ReachLimit)?;
                return Ok(());
            }
        }
        let mut candidate = Some(
            PreparedEntry::try_new(
                id,
                ObjectSource {
                    object,
                    permits: count,
                    views: 0,
                },
                &MetadataFunding(pool),
            )
            .map_err(insert_error)?,
        );
        {
            let mut state = self.state.lock();
            if let Some(held) = state.sources.get_mut(id) {
                held.permits = held
                    .permits
                    .checked_add(count)
                    .ok_or(SystemCallError::ReachLimit)?;
            } else {
                state
                    .sources
                    .insert_prepared(candidate.take().expect("source candidate disappeared"));
            }
        }
        drop(candidate);
        Ok(())
    }

    fn take_write_owner(&self, permit: &WritePermit) -> Arc<MemoryObjectCore> {
        let (object, removed) = {
            let mut state = self.state.lock();
            let id = permit.object().get();
            let owner = state
                .sources
                .get_mut(id)
                .expect("write permit lost object root");
            owner.permits = owner
                .permits
                .checked_sub(1)
                .expect("write permit count underflow");
            let object = owner.object.clone();
            let removed = if owner.permits == 0 && owner.views == 0 {
                state.sources.remove(id)
            } else {
                None
            };
            (object, removed)
        };
        drop(removed);
        object
    }

    pub(crate) fn cancel_write(&self, permit: WritePermit) {
        let object = self.take_write_owner(&permit);
        object.cancel_write(permit);
    }
    pub(crate) fn object(&self, id: memory_space::ObjectId) -> Option<Arc<MemoryObjectCore>> {
        self.state
            .lock()
            .sources
            .get(id.get())
            .map(|source| source.object.clone())
    }

    pub(crate) fn is_empty(&self) -> bool {
        let state = self.state.lock();
        state.views.is_empty() && state.sources.is_empty()
    }
}

impl Views {
    pub(crate) fn release_fragment_step(&self, retirement: &mut ViewRetirement) -> bool {
        if let Some(release) = retirement.release.as_mut() {
            if release.step() {
                retirement.release.take();
            }
            return false;
        }
        let fragment = &retirement.fragment;
        if fragment.backing_retire == BackingRetire::Retain
            || matches!(fragment.kind, RegionKindView::Guard)
        {
            return true;
        }
        if retirement.next == fragment.range.end() {
            return true;
        }
        let (mut removed, source) = {
            let mut state = self.state.lock();
            let start = fragment.range.start();
            let key = retirement.next;
            let view = state
                .views
                .get(key)
                .expect("retiring fragment lost contiguous resource owner");
            assert!(
                view.end <= fragment.range.end(),
                "retiring view crosses fragment boundary"
            );
            if let RegionKindView::Mapping { backing, .. } = fragment.kind {
                assert_eq!(
                    backing.shifted(key - start),
                    Some(view.origin),
                    "retiring fragment changed backing"
                );
            }
            retirement.next = view.end;
            let removed = state
                .views
                .remove(key)
                .expect("retiring view disappeared under index lock");
            let source = if let Some(id) = removed.value().origin.object() {
                let owner = state
                    .sources
                    .get_mut(id.get())
                    .expect("retiring view lost source");
                owner.views = owner
                    .views
                    .checked_sub(1)
                    .expect("view source count underflow");
                if owner.views == 0 && owner.permits == 0 {
                    state.sources.remove(id.get())
                } else {
                    None
                }
            } else {
                None
            };
            (removed, source)
        };
        drop(source);
        // 此步只移交一份 use；后续 step 由同一事务/空间 Work 直接退款，无调度 join。
        retirement.release = Some(
            removed
                .value_mut()
                .usage
                .take()
                .expect("retiring view lost use")
                .retire(),
        );
        false
    }

    pub(crate) fn release_permit(&self, permit: WritePermit) {
        let object = self.take_write_owner(&permit);
        object.retire_write(permit);
    }
}

impl Drop for Views {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        assert!(
            state.views.is_empty() && state.sources.is_empty(),
            "view index dropped before bounded retirement"
        );
    }
}
