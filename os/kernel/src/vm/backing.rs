//! backing 的真实帧/资金 owner。普通 use 由产生责任的事务逐段退休；
//! 仅完整域 owner 的最终放弃使用 backing 出生内嵌 Work，不派生逐 use 任务。

use crate::{
    frame::{FundedExtent, FundedFrames},
    maintenance::{self as mt, Work, WorkRecord},
    sync::{Spinlock, ranks},
    task::memory_pool::MemoryPool,
    task::resources::{MetadataCharge, MetadataFunding, map_storage_error},
};
use alloc::sync::Arc;
use erhino_shared::call::SystemCallError;
use maintenance::record::Outcome;
use memory_space::{BackingError, NodeReserve, RangeUse, ReleaseCursor, ReleaseStep, SegmentMap};

static NEXT_BACKING: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

struct State {
    segments: SegmentMap<PhysicalExtent, MetadataCharge>,
    root: Option<ReleaseCursor>,
    acquisitions: usize,
}

pub(crate) enum PhysicalExtent {
    Funded(FundedExtent),
    Boot(crate::frame::BootFundedExtent),
}
pub(crate) type BackingStorage = memory_space::SegmentStorage<PhysicalExtent, MetadataCharge>;

impl State {
    fn take_idle_storage(&mut self) -> Option<BackingStorage> {
        if !self.segments.storage_idle() {
            return None;
        }
        self.segments.begin_retire();
        Some(self.segments.take_empty_storage())
    }
}
impl PhysicalExtent {
    fn base(&self) -> usize {
        match self {
            Self::Funded(e) => e.base().0,
            Self::Boot(e) => e.base().0,
        }
    }
    fn pages(&self) -> usize {
        match self {
            Self::Funded(e) => e.pages(),
            Self::Boot(e) => e.pages(),
        }
    }
}
impl memory_space::SegmentOwner for PhysicalExtent {
    fn split_at(self, pages: u64) -> (Self, Self) {
        let pages = usize::try_from(pages).expect("physical split exceeds address width");
        match self {
            Self::Funded(e) => {
                let (a, b) = e.split_at(pages);
                (Self::Funded(a), Self::Funded(b))
            }
            Self::Boot(e) => {
                let (a, b) = e.split_at(pages);
                (Self::Boot(a), Self::Boot(b))
            }
        }
    }
}

pub(crate) struct Backing {
    id: memory_space::BackingId,
    state: Spinlock<State>,
    record: WorkRecord,
    _metadata: crate::task::resources::MetadataCharge,
}

/// 完整域业务 owner；public 对象/Connection 放弃它时启动唯一最终退休记录。
pub(crate) struct BackingOwner {
    usage: Option<BackingUse>,
}
impl BackingOwner {
    /// 匿名 backing 不保留独立 public root；完整使用权直接交给创建它的 View/事务。
    pub(crate) fn into_use(mut self) -> BackingUse {
        self.usage
            .take()
            .expect("backing owner already transferred")
    }
    pub(crate) fn backing(&self) -> &Arc<Backing> {
        self.usage
            .as_ref()
            .expect("backing owner already retired")
            .backing()
    }

    /// 初始化尚未交给执行主体的完整域；一次只写页内数据。
    ///
    /// # Safety
    /// 本域没有可运行消费者、其他写入者或可发布的映射；调用者须在任何
    /// 消费者获准执行前结束全部初始化。此函数不能绕过已发布对象的写权限。
    pub(crate) unsafe fn initialize_page(
        &self,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), SystemCallError> {
        let page = offset / memory_space::PAGE_SIZE;
        let in_page = offset % memory_space::PAGE_SIZE;
        if bytes.len() > memory_space::PAGE_SIZE - in_page {
            return Err(SystemCallError::IllegalArgument);
        }
        let usage = self.usage.as_ref().ok_or(SystemCallError::ObjectClosed)?;
        let (physical, _) = usage
            .physical_span(page as u64)
            .ok_or(SystemCallError::IllegalArgument)?;
        let destination = crate::mm::phys_to_virt(physical.0 * memory_space::PAGE_SIZE) + in_page;
        // SAFETY: 完整域 owner 保活物理页；上方限定页内范围，调用者保证初始化独占。
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), destination as *mut u8, bytes.len())
        };
        Ok(())
    }
}
impl Drop for BackingOwner {
    fn drop(&mut self) {
        let Some(mut usage) = self.usage.take() else {
            return;
        };
        let range = usage.range.take().expect("backing owner lost use");
        {
            let mut state = usage.backing.state.lock();
            state.segments.begin_retire();
            assert!(state.root.is_none(), "backing root retired twice");
            state.root = Some(
                state
                    .segments
                    .begin_release(range)
                    .expect("root use changed owner"),
            );
            // 与 acquisition 归还、root 终态同门发布，不能把可迟到的 activate 带出门。
            let root: Arc<dyn Work> = usage.backing.clone();
            mt::publish(root);
        }
        mt::kick();
    }
}

/// 仿射 RAM 使用责任；View/DMA 的准备失败也交回原 owner，不以 Drop 创建新任务。
#[must_use = "a backing use must be transferred to its consumer or retired by its operation owner"]
pub(crate) struct BackingUse {
    backing: Arc<Backing>,
    range: Option<RangeUse>,
}
impl Drop for BackingUse {
    fn drop(&mut self) {
        assert!(
            self.range.is_none(),
            "backing use dropped without responsibility transfer"
        );
    }
}

/// 普通值型释放游标，由已有事务/空间 Work 持有；step 返回 true 时实物已归还。
pub(crate) struct BackingRelease {
    backing: Arc<Backing>,
    cursor: Option<ReleaseCursor>,
}
impl BackingRelease {
    pub(crate) fn step(&mut self) -> bool {
        let (step, storage) = {
            let mut state = self.backing.state.lock();
            let step = state.segments.release_step(
                self.cursor
                    .as_mut()
                    .expect("completed backing release advanced again"),
            );
            (step, state.take_idle_storage())
        };
        drop(storage);
        match step {
            ReleaseStep::Done => {
                self.cursor.take();
                true
            }
            ReleaseStep::Retained => false,
            ReleaseStep::Detached { owner, .. } => {
                // 来源锁已释放；真实 FundedExtent 先还物理库存、再退原 credit。
                drop(owner);
                false
            }
        }
    }
}
impl Drop for BackingRelease {
    fn drop(&mut self) {
        assert!(
            self.cursor.is_none(),
            "backing release abandoned before refund"
        );
    }
}

/// 只预取物理段切界节点；切分 use 不再分配记录或引入完成观察。
pub(crate) struct BackingSplit {
    backing: Arc<Backing>,
    reserve: Option<NodeReserve>,
}
impl Drop for BackingSplit {
    fn drop(&mut self) {
        if let Some(reserve) = self.reserve.take() {
            let storage = {
                let mut state = self.backing.state.lock();
                state.segments.unreserve(reserve);
                state.take_idle_storage()
            };
            drop(storage);
        }
    }
}

fn map_error(error: BackingError) -> SystemCallError {
    match error {
        BackingError::NodeLimit => SystemCallError::ReachLimit,
        BackingError::UseLimit => SystemCallError::ReachLimit,
        BackingError::CapacityChanged => SystemCallError::ObjectBusy,
        BackingError::Retiring => SystemCallError::ObjectClosed,
        BackingError::InvalidRange => SystemCallError::IllegalArgument,
        BackingError::ForeignUse | BackingError::ReserveExhausted => SystemCallError::InternalError,
    }
}

impl Backing {
    /// 同一 Backing owner 完成 funding 准备；固定 extent 值不随 VM 请求栈传播。
    #[inline(never)]
    pub(crate) fn fund(
        pool: Arc<MemoryPool>,
        pages: usize,
        limits: funded_frame::Limits,
    ) -> Result<BackingOwner, SystemCallError> {
        let frames =
            crate::frame::fund_user_frames(&pool, pages, limits).map_err(|error| match error {
                funded_frame::FundError::Quota(error) => {
                    crate::task::memory_pool::map_pool_error(error)
                }
                funded_frame::FundError::PageLimit | funded_frame::FundError::ExtentLimit => {
                    SystemCallError::ReachLimit
                }
                _ => SystemCallError::OutOfMemory,
            })?;
        Self::from_funded(pool, frames)
    }
    /// 构造失败尚无外部 use，原 funding 批的 extent 数已有上界。
    pub(crate) fn from_funded(
        pool: Arc<MemoryPool>,
        frames: FundedFrames,
    ) -> Result<BackingOwner, SystemCallError> {
        if !frames.charge_pool_is(&pool) {
            return Err(SystemCallError::IllegalArgument);
        }
        let mut extents =
            funded_storage::Buffer::try_empty(frames.extent_count(), &MetadataFunding(&pool))
                .map_err(map_storage_error)?;
        frames
            .into_extents(&mut extents)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        Self::from_extents(pool, extents.into_iter().map(PhysicalExtent::Funded))
    }

    pub(crate) fn from_boot(
        pool: Arc<MemoryPool>,
        extent: crate::frame::BootHeldExtent,
    ) -> Result<BackingOwner, SystemCallError> {
        let extent = crate::frame::fund_boot_held(&pool, extent)
            .map_err(crate::task::memory_pool::map_pool_error)?;
        Self::from_extents(pool, core::iter::once(PhysicalExtent::Boot(extent)))
    }

    fn from_extents(
        pool: Arc<MemoryPool>,
        extents: impl ExactSizeIterator<Item = PhysicalExtent>,
    ) -> Result<BackingOwner, SystemCallError> {
        let id = NEXT_BACKING.allocate().ok_or(SystemCallError::ReachLimit)?;
        let mut segments = SegmentMap::new(id);
        let mut storage = BackingStorage::try_empty(extents.len(), &MetadataFunding(&pool))
            .map_err(map_storage_error)?;
        let mut reserve = segments
            .reserve(extents.len(), &mut storage)
            .map_err(map_error)?;
        let mut pages = 0;
        for extent in extents {
            pages += extent.pages() as u64;
            segments
                .push_extent(extent.pages() as u64, extent, &mut reserve)
                .map_err(|(error, _owner)| map_error(error))?;
        }
        if pages == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        // 完整域两端已存在，不需要任何额外节点；先取得 root use 再公开存储。
        let range = segments
            .acquire(0, pages, &mut reserve)
            .map_err(map_error)?;
        segments.unreserve(reserve);
        let metadata = crate::task::resources::MetadataCharge::arc::<Self>(&pool)?;
        let backing = Arc::try_new(Self {
            id: memory_space::BackingId::new(id).expect("backing identity is nonzero"),
            state: Spinlock::new(
                ranks::BACKING,
                State {
                    segments,
                    root: None,
                    acquisitions: 0,
                },
            ),
            record: WorkRecord::new(pool.maintenance_account().clone()),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(BackingOwner {
            usage: Some(BackingUse {
                backing,
                range: Some(range),
            }),
        })
    }

    pub(crate) fn id(&self) -> memory_space::BackingId {
        self.id
    }

    /// 调用者在取得此 use 前已拥有准备/回滚记录；错误不取得任何新责任。
    pub(crate) fn acquire(
        self: &Arc<Self>,
        lo: u64,
        hi: u64,
        storage: &mut BackingStorage,
    ) -> Result<Option<BackingUse>, SystemCallError> {
        let range = {
            let mut state = self.state.lock();
            if state.segments.is_retiring() {
                return Err(SystemCallError::ObjectClosed);
            }
            let mut reserve = match state.segments.reserve(2, storage) {
                Ok(reserve) => reserve,
                Err(BackingError::CapacityChanged) => return Ok(None),
                Err(error) => return Err(map_error(error)),
            };
            let result = state.segments.acquire(lo, hi, &mut reserve);
            state.segments.unreserve(reserve);
            result.map_err(map_error)?
        };
        Ok(Some(BackingUse {
            backing: self.clone(),
            range: Some(range),
        }))
    }

    /// 源授权门内接受；完整域最终退休在这个游标退出前不能递减 root use。
    pub(crate) fn begin_acquire(
        self: &Arc<Self>,
        lo: u64,
        hi: u64,
        storage: &mut BackingStorage,
    ) -> Result<Option<BackingAcquisition>, SystemCallError> {
        let cursor = {
            let mut state = self.state.lock();
            let count = state
                .acquisitions
                .checked_add(1)
                .ok_or(SystemCallError::ReachLimit)?;
            let mut reserve = match state.segments.reserve(2, storage) {
                Ok(reserve) => reserve,
                Err(BackingError::CapacityChanged) => return Ok(None),
                Err(error) => return Err(map_error(error)),
            };
            let result = state.segments.begin_acquire(lo, hi, &mut reserve);
            state.segments.unreserve(reserve);
            let cursor = result.map_err(map_error)?;
            state.acquisitions = count;
            cursor
        };
        Ok(Some(BackingAcquisition {
            backing: self.clone(),
            cursor: Some(cursor),
        }))
    }

    pub(crate) fn prepare_storage(
        &self,
        nodes: usize,
        pool: &Arc<MemoryPool>,
    ) -> Result<BackingStorage, SystemCallError> {
        let capacity = self
            .state
            .lock()
            .segments
            .growth_capacity(nodes)
            .map_err(map_error)?;
        BackingStorage::try_empty(capacity, &MetadataFunding(pool)).map_err(map_storage_error)
    }
    pub(crate) fn prepare_split(
        self: &Arc<Self>,
        pool: &Arc<MemoryPool>,
    ) -> Result<BackingSplit, SystemCallError> {
        let mut storage = BackingStorage::new();
        let reserve = loop {
            let result = {
                let mut state = self.state.lock();
                if state.segments.is_retiring() && state.segments.is_empty() {
                    return Err(SystemCallError::ObjectClosed);
                }
                state.segments.reserve(1, &mut storage)
            };
            match result {
                Ok(reserve) => break reserve,
                Err(BackingError::CapacityChanged) => {
                    drop(storage);
                    storage = self.prepare_storage(1, pool)?;
                }
                Err(error) => return Err(map_error(error)),
            }
        };
        Ok(BackingSplit {
            backing: self.clone(),
            reserve: Some(reserve),
        })
    }
}

impl BackingUse {
    pub(crate) fn backing(&self) -> &Arc<Backing> {
        &self.backing
    }
    pub(crate) fn range(&self) -> (u64, u64) {
        self.range
            .as_ref()
            .expect("backing use already transferred")
            .range()
    }
    pub(crate) fn split_prepared(&mut self, at: u64, prepared: &mut BackingSplit) -> Self {
        assert!(
            Arc::ptr_eq(&self.backing, &prepared.backing),
            "split reservation belongs to another backing"
        );
        let (left, right) = self
            .backing
            .state
            .lock()
            .segments
            .split_use(
                self.range.take().expect("split lost use"),
                at,
                prepared
                    .reserve
                    .as_mut()
                    .expect("split reservation consumed"),
            )
            .expect("prepared view split invalidated");
        self.range = Some(left);
        Self {
            backing: self.backing.clone(),
            range: Some(right),
        }
    }
    pub(crate) fn retire(mut self) -> BackingRelease {
        let cursor = self
            .backing
            .state
            .lock()
            .segments
            .begin_release(self.range.take().expect("retired use lost owner"))
            .expect("backing use changed its segment owner");
        BackingRelease {
            backing: self.backing.clone(),
            cursor: Some(cursor),
        }
    }
    pub(crate) fn physical_span(&self, page: u64) -> Option<(page_table::Ppn, usize)> {
        let (lo, hi) = self.range();
        if page < lo || page >= hi {
            return None;
        }
        let state = self.backing.state.lock();
        let (start, end, owner) = state.segments.segment_at(page)?;
        Some((
            page_table::Ppn(owner.base() + (page - start) as usize),
            (end.min(hi) - page) as usize,
        ))
    }
}

/// 已授权的分步取得；调用者的预付 owner 在退出/失败时归还已持前缀。
pub(crate) struct BackingAcquisition {
    backing: Arc<Backing>,
    cursor: Option<memory_space::AcquireCursor>,
}
impl BackingAcquisition {
    pub(crate) fn step(&mut self) -> Result<bool, SystemCallError> {
        self.backing
            .state
            .lock()
            .segments
            .acquire_step(self.cursor.as_mut().expect("acquisition finished"))
            .map_err(map_error)
    }
    /// 只返还已取得前缀；成功者必须先由 step 确认完整，取消者负责退休此前缀。
    pub(crate) fn finish(mut self) -> Option<BackingUse> {
        let (range, wake) = {
            let mut state = self.backing.state.lock();
            let range = state
                .segments
                .finish_acquire(self.cursor.take().expect("acquisition finished twice"));
            assert!(
                state.acquisitions != 0,
                "backing acquisition count underflow"
            );
            state.acquisitions -= 1;
            let wake = state.acquisitions == 0 && state.root.is_some();
            if wake {
                // 同 BACKING 门下 root 尚未结束；当前 acquisition 还强持 backing，
                // publish 不可能在门内析构最后根。门铃仍在锁外发送。
                let work: Arc<dyn Work> = self.backing.clone();
                mt::publish(work);
            }
            (range, wake)
        };
        if wake {
            mt::kick();
        }
        range.map(|range| BackingUse {
            backing: self.backing.clone(),
            range: Some(range),
        })
    }
}
impl Drop for BackingAcquisition {
    fn drop(&mut self) {
        assert!(
            self.cursor.is_none(),
            "backing acquisition abandoned without rollback"
        );
    }
}

impl Work for Backing {
    fn record(&self) -> &WorkRecord {
        &self.record
    }
    fn step(self: Arc<Self>) -> Outcome {
        let (step, storage) = {
            let mut state = self.state.lock();
            if state.acquisitions != 0 {
                return Outcome::Idle;
            }
            let State { segments, root, .. } = &mut *state;
            let step = segments.release_step(
                root.as_mut()
                    .expect("backing final retirement has no owner"),
            );
            if matches!(step, ReleaseStep::Done) {
                root.take();
            }
            (step, state.take_idle_storage())
        };
        drop(storage);
        match step {
            ReleaseStep::Done => Outcome::Done,
            ReleaseStep::Retained => Outcome::Ready,
            ReleaseStep::Detached { owner, .. } => {
                drop(owner);
                Outcome::Ready
            }
        }
    }
}
