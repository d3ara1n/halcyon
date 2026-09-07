//! 进程与线程：资源容器 / 执行容器（见 notes/impls/task.md）。

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use erhino_shared::{
    call::SystemCallError,
    mem::{MemoryMapRequest, MemoryMapResult, MemoryPlacement, MemoryProtection},
    proc::{Pid, ProcessExitReason, ProcessMapFlags, ProcessState, ThreadStartContext, Tid},
};
use memory_space::{
    AddressRange, AnonymousClass, BackingId, BackingRetire, BackingView, ChangeError, LeaseKey,
    Limits, MapBacking, MapPlacement, MapRequest, MemorySpace, ObjectId, ObjectViewAuthorization,
    PageRange as LedgerPageRange, PermitRequirement, PreparedChange, ProtectRequest, Protection,
    PublishedChange, RegionKey, RegionKindView, RegionOwner, RetireBatch, RetiringChange,
    RetiringFragment, TranslationIntent, UnmapRequest, WritePermit,
};
use page_table::{
    DrainCursor as TableDrainCursor, DrainStep as TableDrainStep, FrameNumber, MapError, Ppn,
    PreparedTranslation, PublishOutcome as TablePublishOutcome, TableFrameMemory, TableFrameOwner,
    TableTree, TranslationPreflight, Vpn, flags,
};

use crate::{
    context::UserContext,
    frame::{self},
    mm,
};

/// 页大小（字节）。
pub const PAGE_SIZE: usize = erhino_shared::proc::PROCESS_PAGE_SIZE;
const _: () = assert!(PAGE_SIZE == 1 << page_table::PAGE_BITS);
// 一个连续 Unmap 只有首尾两个 mapping 可能产生 partial cut；每个 backing
// 最多 64 extents，单个 cut 至多切两次，故四倍是事务级硬上界。
const MAX_BACKING_SPLITS_PER_CHANGE: usize = frame::MAX_FUNDED_EXTENTS * 4;

/// 用户半区顶（256GiB），主线程栈顶。
pub const USER_TOP: usize = erhino_shared::proc::PROCESS_USER_TOP;

/// 主线程栈大小（8MiB），钉在半区顶。
pub const STACK_SIZE: usize = erhino_shared::proc::PROCESS_MAIN_STACK_SIZE;

/// sv39 三级页表。
const LEVELS: usize = 3;

/// 进程地址空间构建/操作错误。它是内核内部的唯一内存错误分类；公开 ABI 错误由
/// [`SystemCallError::from`] 一次投影得到，纯逻辑规划器的 [`ChangeError`] 也先归到
/// 这里，因此全系统只有一张分类表。
#[derive(Debug)]
pub enum SpaceError {
    /// 帧或表帧耗尽。
    NoFrame,
    /// Pool 额度不足。
    QuotaExceeded,
    /// 结构容量或 funded transaction 硬上限耗尽。
    ReachLimit,
    /// 段未页对齐 / 参数非法。
    BadSegment,
    /// 映射冲突（重复装载同一区间）。
    Conflict,
    /// 请求范围未被调用者可操作的 mapping/reservation 完整覆盖。
    NotMapped,
    /// authority 或权限上限不允许该操作。
    PermissionDenied,
    /// 与其它在途 MemoryChange footprint 冲突。
    Busy,
    /// Building 空壳尚未附入 MemoryPool/TranslationTree。
    Unbound,
}

impl From<SpaceError> for SystemCallError {
    /// 用户参数边界的唯一分类：调用者直接提交的地址/长度/权限可以合法地产生
    /// `BadSegment`，因此它是 `IllegalArgument`。
    fn from(error: SpaceError) -> Self {
        match error {
            SpaceError::NoFrame => Self::OutOfMemory,
            SpaceError::QuotaExceeded => Self::QuotaExceeded,
            SpaceError::ReachLimit => Self::ReachLimit,
            SpaceError::BadSegment => Self::IllegalArgument,
            SpaceError::Conflict => Self::AddressConflict,
            SpaceError::NotMapped => Self::NotMapped,
            SpaceError::PermissionDenied => Self::RightsDenied,
            SpaceError::Busy => Self::ObjectBusy,
            SpaceError::Unbound => Self::ObjectNotAvailable,
        }
    }
}

impl From<ChangeError> for SpaceError {
    fn from(error: ChangeError) -> Self {
        match error {
            ChangeError::Conflict => Self::Conflict,
            ChangeError::NotCovered | ChangeError::Guard => Self::NotMapped,
            ChangeError::OwnerDenied | ChangeError::PermissionDenied => Self::PermissionDenied,
            ChangeError::Busy | ChangeError::Stale => Self::Busy,
            ChangeError::PageLimit
            | ChangeError::RegionLimit
            | ChangeError::TransactionLimit
            | ChangeError::KeyExhausted => Self::ReachLimit,
            ChangeError::AllocationFailed => Self::NoFrame,
            ChangeError::BadLimits
            | ChangeError::Range(_)
            | ChangeError::OutOfBounds
            | ChangeError::BackingOutOfRange
            | ChangeError::ObjectAuthorization
            | ChangeError::LeaseInvalid
            | ChangeError::LeaseTooLarge
            | ChangeError::PermitMismatch => Self::BadSegment,
        }
    }
}

#[derive(Debug)]
pub(crate) enum ThreadAttachError {
    Context(SpaceError),
    Closed,
    Limit,
    Oom,
}

impl From<MapError> for SpaceError {
    fn from(e: MapError) -> Self {
        match e {
            MapError::Conflict { .. } => SpaceError::Conflict,
            MapError::FrameExhausted | MapError::AllocationFailed => SpaceError::NoFrame,
            MapError::OutOfRange
            | MapError::InvalidFlags
            | MapError::NotMapped { .. }
            | MapError::ProtectionMismatch { .. } => SpaceError::BadSegment,
        }
    }
}

impl TableFrameOwner for frame::FundedTableFrame {
    fn number(&self) -> FrameNumber {
        self.frame()
    }
}

struct TableMem;

impl TableFrameMemory for TableMem {
    type FrameOwner = frame::FundedTableFrame;

    fn table_mut(&mut self, frame: FrameNumber) -> &mut [page_table::Pte; page_table::ENTRIES] {
        // SAFETY: owner ledger 强持每张表帧；页对齐且经直映射独占访问。
        unsafe { &mut *(mm::phys_to_virt(frame.addr()) as *mut _) }
    }
}

pub(crate) fn supply_funded_table_frames(
    pool: &Arc<super::memory_pool::MemoryPool>,
    count: usize,
) -> Result<Vec<frame::FundedTableFrame>, SpaceError> {
    let mut owners = Vec::new();
    owners
        .try_reserve_exact(count)
        .map_err(|_| SpaceError::NoFrame)?;
    for _ in 0..count {
        let owner = frame::fund_user_table_frame(pool).map_err(map_funded_error)?;
        // SAFETY: newly claimed table frame is exclusively owned and page aligned.
        unsafe {
            core::ptr::write_bytes(
                mm::phys_to_virt(owner.frame().addr()) as *mut u8,
                0,
                PAGE_SIZE,
            );
        }
        owners.push(owner);
    }
    Ok(owners)
}

pub(crate) fn fund_table_preflights(
    pool: &Arc<super::memory_pool::MemoryPool>,
    preflights: &[TranslationPreflight],
) -> Result<Vec<Vec<frame::FundedTableFrame>>, SpaceError> {
    let mut funded = Vec::new();
    funded
        .try_reserve_exact(preflights.len())
        .map_err(|_| SpaceError::NoFrame)?;
    for preflight in preflights {
        funded.push(supply_funded_table_frames(
            pool,
            preflight.required_frames(),
        )?);
    }
    Ok(funded)
}

fn map_funded_error(
    error: funded_frame::FundError<memory_pool::PoolError, crate::frame::UserClaimError>,
) -> SpaceError {
    match error {
        funded_frame::FundError::ZeroPages
        | funded_frame::FundError::InvalidClaim
        | funded_frame::FundError::Physical(_) => SpaceError::NoFrame,
        funded_frame::FundError::PageLimit | funded_frame::FundError::ExtentLimit => {
            SpaceError::ReachLimit
        }
        funded_frame::FundError::Quota(memory_pool::PoolError::QuotaExceeded) => {
            SpaceError::QuotaExceeded
        }
        funded_frame::FundError::Quota(_) => SpaceError::NoFrame,
    }
}

const MEMORY_SPACE_LIMITS: Limits = Limits {
    max_regions: super::resources::REGION_SLOTS_PER_ADDRESS_SPACE,
    max_transactions: super::resources::MEMORY_CHANGES_PER_ADDRESS_SPACE,
    max_pages_per_change: (256 << 20) / PAGE_SIZE,
    max_lease_bytes: 1 << 20,
    max_lease_segments: 64,
};

enum BackingExtentOwner {
    Funded {
        extent: frame::FundedExtent,
        permit: super::resources::BackingSlicePermit,
    },
    Boot(frame::BootFundedExtent),
    /// Bootstrap Prepare 期间由外层 `BootFundedExtent` 强持的只读几何；进程发布前
    /// 必须由 `install_bootstrap_funding` 替换为 Boot。
    BootBorrowed {
        base: FrameNumber,
        pages: usize,
    },
}

impl BackingExtentOwner {
    fn base(&self) -> FrameNumber {
        match self {
            Self::Funded { extent, .. } => extent.base(),
            Self::Boot(extent) => extent.base(),
            Self::BootBorrowed { base, .. } => *base,
        }
    }

    fn count(&self) -> usize {
        match self {
            Self::Funded { extent, .. } => extent.pages(),
            Self::Boot(extent) => extent.pages(),
            Self::BootBorrowed { pages, .. } => *pages,
        }
    }

    #[inline(never)]
    fn split_at(
        self,
        pages: usize,
        right_permit: Option<super::resources::BackingSlicePermit>,
    ) -> (Self, Self) {
        match self {
            Self::Funded { extent, permit } => {
                let right_permit =
                    right_permit.expect("funded backing split requires a reserved metadata permit");
                let (left, right) = extent.split_at(pages);
                (
                    Self::Funded {
                        extent: left,
                        permit,
                    },
                    Self::Funded {
                        extent: right,
                        permit: right_permit,
                    },
                )
            }
            Self::Boot(extent) => {
                let (left, right) = extent.split_at(pages);
                (Self::Boot(left), Self::Boot(right))
            }
            Self::BootBorrowed { .. } => {
                panic!("bootstrap borrowed extent cannot be split before owner installation")
            }
        }
    }
}

struct BackingExtent {
    offset_pages: usize,
    owner: BackingExtentOwner,
}

enum RetiredSpaceResource {
    Backing(BackingExtentOwner),
    Table(frame::FundedTableFrame),
    /// object view 的锁外收束：先把 WritePermit 归还对象状态机（对象锁秩低于
    /// AddressSpace），再释放强引用——最后一个引用消散会归还 backing 与 Pool charge。
    View {
        core: Arc<super::memory_object::MemoryObjectCore>,
        permit: Option<WritePermit>,
    },
    Root {
        owner: frame::FundedTableFrame,
        binding: super::resources::PoolBinding,
    },
}

impl RetiredSpaceResource {
    fn release(self) {
        match self {
            Self::Backing(owner) => drop(owner),
            Self::Table(owner) => drop(owner),
            Self::View { core, permit } => {
                if let Some(permit) = permit {
                    core.retire_write(permit);
                }
                drop(core);
            }
            Self::Root { owner, binding } => {
                drop(owner);
                drop(binding);
            }
        }
    }
}

pub(crate) struct PreparedBacking {
    pages: usize,
    extents: Vec<BackingExtent>,
}

pub(crate) struct OwnedBacking {
    identity: BackingId,
    pages: usize,
    extents: Vec<BackingExtent>,
}

pub(crate) enum BackingPlanFailure<E> {
    Prepared(E, PreparedBacking),
    Owned(E, OwnedBacking),
}

/// 本地址空间对一个 MemoryObject 的 view 所有权：强引用使对象独立于 Handle 存活。
///
/// 每地址空间每对象一枚，与引用它的区域数无关——区域切割、降权与合并都由账本表达，
/// 这里不重复计数。「是否仍有区域引用该对象」的真值只在账本里，退役时现场查询。
struct ObjectViewOwner {
    object: ObjectId,
    core: Arc<super::memory_object::MemoryObjectCore>,
    _permit: super::resources::ObjectViewPermit,
}

/// Commit 前预留、Commit 时安装的 view 所有权。对象已有 view 时它在 Commit 中被
/// 丢弃并自然退款——预留是悲观的，不构成第二份真值。
///
/// 身份在构造时（AddressSpace 锁外）取得：Commit 运行在 `ADDRESS_SPACE → LIFECYCLE`
/// 之下，而对象状态锁秩低于两者，因此发布路径不得回取对象锁。
pub(crate) struct PreparedObjectView {
    object: ObjectId,
    core: Arc<super::memory_object::MemoryObjectCore>,
    permit: super::resources::ObjectViewPermit,
}

impl PreparedObjectView {
    pub(crate) fn new(
        core: Arc<super::memory_object::MemoryObjectCore>,
        sponsor: &Arc<super::resources::MetadataSponsor>,
    ) -> Result<Self, SpaceError> {
        let permit = super::resources::MetadataSponsor::reserve_object_view(sponsor)
            .map_err(|_| SpaceError::ReachLimit)?;
        Ok(Self {
            object: core.identity(),
            core,
            permit,
        })
    }
}

/// 一个退役区域对其来源对象的引用。`core` 是本批 WritePermit 的归还目标；`_owner`
/// 只在账本已无该对象区域时交出，它必须活到本批 permit 全部回到对象状态机之后才能
/// 析构，否则最后一个引用可能先消散、permit 失去归还目标。
struct RetiringObjectView {
    object: ObjectId,
    core: Arc<super::memory_object::MemoryObjectCore>,
    _owner: Option<ObjectViewOwner>,
}

/// 已发布事务的表页 outcome 批次；单项发布是长度为一的退化情形。
pub(crate) struct PublishedTableChanges(Vec<TablePublishOutcome<frame::FundedTableFrame>>);

enum TableRetireStep {
    Owner(frame::FundedTableFrame),
    Progress,
    Complete,
}

impl PublishedTableChanges {
    fn retire_step(&mut self) -> TableRetireStep {
        let Some(outcome) = self.0.last_mut() else {
            return TableRetireStep::Complete;
        };
        if let Some(owner) = outcome.retired.pop().or_else(|| outcome.unused.pop()) {
            TableRetireStep::Owner(owner)
        } else {
            self.0.pop();
            TableRetireStep::Progress
        }
    }
}

struct BackingRetireCursor {
    identity: BackingId,
    next_offset: usize,
    remaining: usize,
}

pub(crate) struct RetiringSpaceChange {
    ledger: Option<RetiringChange>,
    batch: RetireBatch,
    tables: PublishedTableChanges,
    backing: Option<BackingRetireCursor>,
    backing_permits: Vec<super::resources::BackingSlicePermit>,
    /// Commit 前已按对象去重并预留容量；Commit 后只消费既有槽位。
    retiring_views: Vec<RetiringObjectView>,
    tables_complete: bool,
    ledger_complete: bool,
}

pub(crate) struct PublishedSpaceChange {
    ledger: PublishedChange,
    tables: PublishedTableChanges,
    backing_permits: Vec<super::resources::BackingSlicePermit>,
    /// Commit 前已预留的对象退役 owner 容器；Commit 后不得扩容。
    retiring_views: Vec<RetiringObjectView>,
}

struct PinnedWriteChunk {
    physical: usize,
    result_offset: usize,
    bytes: usize,
}

struct PinnedMapResult {
    chunks: Vec<PinnedWriteChunk>,
    value: MemoryMapResult,
    cookie: u64,
}

/// 统一地址空间事务的 plan 阶段：Validate 之后、表页供给之前的账本 reservation
/// 与 PTE preflight。四个维度由字段存在性表达，不再每种组合各自成型：
/// source（`backing` 有无）、output（`result` cookie 有无）、Unmap 切分预算
/// （`backing_permits`）与 Building 的映像推进（`image_end`）。
pub(crate) struct MemoryChangePlan {
    change: PreparedChange,
    preflights: Vec<TranslationPreflight>,
    backing: Option<OwnedBacking>,
    result: Option<PinnedMapResult>,
    backing_permits: Vec<super::resources::BackingSlicePermit>,
    image_end: Option<usize>,
    /// 本事务在 Commit 时发布的新 object view 身份；撤销既有 view 不产生它。
    published_view: Option<ObjectMappingLease>,
    /// Commit 时装入地址空间的 view 所有权（强引用 + admission）。
    view_owner: Option<PreparedObjectView>,
}

impl MemoryChangePlan {
    /// 表页供给按段进行：每个 preflight 各自预算，多 extent 与多段投影同形。
    pub(crate) fn preflights(&self) -> &[TranslationPreflight] {
        &self.preflights
    }
}

struct MemoryChangeReservation {
    change: PreparedChange,
    translations: Vec<PreparedTranslation<frame::FundedTableFrame>>,
    table_outcomes: Vec<TablePublishOutcome<frame::FundedTableFrame>>,
    backing: Option<OwnedBacking>,
    result: Option<PinnedMapResult>,
    backing_permits: Vec<super::resources::BackingSlicePermit>,
    image_end: Option<usize>,
    published_view: Option<ObjectMappingLease>,
    view_owner: Option<PreparedObjectView>,
    /// 退役 owner 槽位由 Validate 结果确定，并在 Commit 前完成分配。
    retiring_views: Vec<RetiringObjectView>,
}

/// Commit 前的唯一发布权。盒化使深层事务不把整份 reservation 留在调用栈上。
pub(crate) struct PreparedMemoryChange(Box<Option<MemoryChangeReservation>>);

/// AddressSpace 锁内失败时摘出、必须由调用者在锁外归还的 affine owner。
/// 每种事务输入各有一格：表页 owner、已准备的 translation、backing、WritePermit。
pub(crate) struct ReclaimedTableFrames {
    funded: Vec<Vec<frame::FundedTableFrame>>,
    translations: Vec<PreparedTranslation<frame::FundedTableFrame>>,
    failed_owners: Option<Vec<frame::FundedTableFrame>>,
    backing: Option<OwnedBacking>,
    permits: Vec<WritePermit>,
    /// 未发布的 view 所有权。它持对象强引用，析构可能归还 backing 与 Pool charge，
    /// 因此必须随本结构一起在 AddressSpace 锁外释放。
    view_owner: Option<PreparedObjectView>,
}

impl ReclaimedTableFrames {
    /// 摘出 WritePermit 交回来源对象。permits 必须在 AddressSpace 锁外归还，
    /// 因为对象状态锁的秩低于 AddressSpace。
    pub(crate) fn take_permits(&mut self) -> Vec<WritePermit> {
        core::mem::take(&mut self.permits)
    }
}

/// 一个已发布 object view 在本地址空间的位置与身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ObjectMappingLease {
    pub(crate) lease: LeaseKey,
    pub(crate) region: RegionKey,
    pub(crate) range: LedgerPageRange,
    pub(crate) object: ObjectId,
    pub(crate) object_offset: usize,
    /// view 的当前权限，与 ledger 中的 current/maximum 一致。
    pub(crate) protection: Protection,
}

/// 对象事务失败：错误与原样退回的 WritePermit 一同交回调用者，由它在
/// AddressSpace 锁外归还给对象状态机。
pub(crate) struct ObjectMapFailure {
    pub(crate) error: SpaceError,
    pub(crate) permits: Vec<WritePermit>,
}

impl PreparedMemoryChange {
    fn allocate() -> Result<Self, SpaceError> {
        Box::try_new(None)
            .map(Self)
            .map_err(|_| SpaceError::NoFrame)
    }

    fn install(&mut self, reservation: MemoryChangeReservation) {
        let previous = self.0.replace(reservation);
        assert!(previous.is_none(), "memory change token filled twice");
    }

    fn get(&self) -> &MemoryChangeReservation {
        self.0
            .as_ref()
            .as_ref()
            .expect("memory change token must be filled")
    }

    fn take(mut self) -> MemoryChangeReservation {
        self.0.take().expect("memory change token consumed twice")
    }
}

#[inline(never)]
fn fund_owned_backing(
    pages: usize,
    pool: &Arc<super::memory_pool::MemoryPool>,
) -> Result<Vec<frame::FundedExtent>, SpaceError> {
    frame::fund_user_frames(
        pool,
        pages,
        funded_frame::Limits {
            max_pages: pages,
            max_extents: frame::MAX_FUNDED_EXTENTS,
        },
    )
    .map_err(map_funded_error)
    .and_then(|funded| {
        let mut extents = Vec::new();
        funded
            .into_extents(&mut extents)
            .map_err(|_| SpaceError::NoFrame)?;
        Ok(extents)
    })
}

#[inline(never)]
fn assemble_prepared_backing(
    pages: usize,
    funded_extents: Vec<frame::FundedExtent>,
    sponsor: &Arc<super::resources::MetadataSponsor>,
) -> Result<PreparedBacking, SpaceError> {
    let mut permits = Vec::new();
    let permit_count = funded_extents.len();
    permits
        .try_reserve_exact(permit_count)
        .map_err(|_| SpaceError::NoFrame)?;
    for _ in 0..permit_count {
        permits.push(reserve_backing_metadata(sponsor)?);
    }
    let mut extents = Vec::new();
    extents
        .try_reserve(funded_extents.len())
        .map_err(|_| SpaceError::NoFrame)?;
    let mut offset = 0;
    for extent in funded_extents {
        let count = extent.pages();
        extents.push(BackingExtent {
            offset_pages: offset,
            owner: BackingExtentOwner::Funded {
                extent,
                permit: permits
                    .pop()
                    .expect("funded backing metadata permit missing"),
            },
        });
        offset += count;
    }
    assert_eq!(offset, pages, "funded backing geometry is incomplete");
    debug_assert!(permits.is_empty());
    Ok(PreparedBacking { pages, extents })
}

#[inline(never)]
fn reserve_backing_metadata(
    sponsor: &Arc<super::resources::MetadataSponsor>,
) -> Result<super::resources::BackingSlicePermit, SpaceError> {
    super::resources::MetadataSponsor::reserve_backing_slice(sponsor)
        .map_err(|_| SpaceError::ReachLimit)
}

fn reserve_backing_split_metadata(
    sponsor: &Arc<super::resources::MetadataSponsor>,
) -> Result<Vec<super::resources::BackingSlicePermit>, SpaceError> {
    let mut permits = Vec::new();
    permits
        .try_reserve_exact(MAX_BACKING_SPLITS_PER_CHANGE)
        .map_err(|_| SpaceError::NoFrame)?;
    for _ in 0..MAX_BACKING_SPLITS_PER_CHANGE {
        permits.push(reserve_backing_metadata(sponsor)?);
    }
    Ok(permits)
}

impl PreparedBacking {
    #[inline(never)]
    pub(crate) fn allocate(
        pages: usize,
        pool: &Arc<super::memory_pool::MemoryPool>,
        sponsor: &Arc<super::resources::MetadataSponsor>,
    ) -> Result<Self, SpaceError> {
        if pages == 0 {
            return Err(SpaceError::BadSegment);
        }
        let funded_extents = fund_owned_backing(pages, pool)?;
        assemble_prepared_backing(pages, funded_extents, sponsor)
    }

    fn bind(self, identity: BackingId) -> OwnedBacking {
        OwnedBacking {
            identity,
            pages: self.pages,
            extents: self.extents,
        }
    }
}

impl OwnedBacking {
    /// 在 backing 尚未发布时从起点回填；调用方保证 source 不越过逻辑长度。
    fn write_from_start(&mut self, source: &[u8]) {
        assert!(
            source.len() <= self.pages * PAGE_SIZE,
            "backing initialization exceeds its logical length"
        );
        let mut copied = 0;
        for extent in &self.extents {
            let extent_start = extent.offset_pages * PAGE_SIZE;
            if extent_start >= source.len() {
                break;
            }
            let count = (extent.owner.count() * PAGE_SIZE).min(source.len() - extent_start);
            // SAFETY: backing owner 独占对应物理 extent，且尚未发布到任何地址空间；
            // source/count 已由逻辑长度与 extent 几何共同约束。
            unsafe {
                core::ptr::copy_nonoverlapping(
                    source[extent_start..].as_ptr(),
                    mm::phys_to_virt(extent.owner.base().addr()) as *mut u8,
                    count,
                );
            }
            copied += count;
        }
        assert_eq!(copied, source.len(), "backing geometry is not contiguous");
    }

    fn preflight_install(
        &self,
        tree: &mut TableTree<TableMem, LEVELS>,
        range: LedgerPageRange,
        backing_offset: usize,
        protection: Protection,
    ) -> Result<Vec<TranslationPreflight>, SpaceError> {
        if !backing_offset.is_multiple_of(PAGE_SIZE) {
            return Err(SpaceError::BadSegment);
        }
        let first_page = backing_offset / PAGE_SIZE;
        let end_page = first_page
            .checked_add(range.pages())
            .ok_or(SpaceError::BadSegment)?;
        if end_page > self.pages {
            return Err(SpaceError::BadSegment);
        }
        let mut preflights = Vec::new();
        preflights
            .try_reserve(self.extents.len())
            .map_err(|_| SpaceError::NoFrame)?;
        for extent in &self.extents {
            let extent_start = extent.offset_pages;
            let extent_end = extent_start + extent.owner.count();
            let start = extent_start.max(first_page);
            let end = extent_end.min(end_page);
            if start >= end {
                continue;
            }
            let page_offset = start - first_page;
            let physical_offset = start - extent_start;
            let preflight = tree
                .preflight_map(
                    Vpn(range.start() / PAGE_SIZE + page_offset),
                    end - start,
                    Ppn(extent.owner.base().addr() / PAGE_SIZE + physical_offset),
                    protection_flags(protection),
                )
                .map_err(SpaceError::from)?;
            preflights.push(preflight);
        }
        let prepared_pages: usize = self
            .extents
            .iter()
            .map(|extent| {
                let start = extent.offset_pages.max(first_page);
                let end = (extent.offset_pages + extent.owner.count()).min(end_page);
                end.saturating_sub(start)
            })
            .sum();
        if prepared_pages != range.pages() {
            return Err(SpaceError::BadSegment);
        }
        Ok(preflights)
    }

    /// Remote ack 后至多切出一个物理 extent owner。调用者在 AddressSpace 锁外
    /// 析构返回 owner，并以返回页数推进稳定游标；本次事务的 split permits
    /// 已在 Commit 前取得，因而这里不再申请 metadata。
    #[inline(never)]
    fn release_one(
        &mut self,
        offset: usize,
        bytes: usize,
        permits: &mut Vec<super::resources::BackingSlicePermit>,
    ) -> (BackingExtentOwner, usize) {
        assert!(
            offset.is_multiple_of(PAGE_SIZE) && bytes.is_multiple_of(PAGE_SIZE) && bytes != 0,
            "backing retire range must be nonempty and page aligned"
        );
        let release_start = offset / PAGE_SIZE;
        let release_end = release_start
            .checked_add(bytes / PAGE_SIZE)
            .expect("backing retire range overflowed");
        assert!(release_end <= self.pages, "backing retire escaped object");

        let index = self
            .extents
            .iter()
            .position(|extent| {
                let start = extent.offset_pages;
                let end = start + extent.owner.count();
                start < release_end && release_start < end
            })
            .expect("backing retire cursor lost its next owned extent");
        let extent = self.extents.remove(index);
        let extent_start = extent.offset_pages;
        let extent_end = extent_start + extent.owner.count();
        let cut_start = extent_start.max(release_start);
        let cut_end = extent_end.min(release_end);
        assert_eq!(
            cut_start, release_start,
            "backing retire cursor encountered an ownership gap"
        );
        let left_pages = cut_start - extent_start;
        let retired_pages = cut_end - cut_start;
        let right_pages = extent_end - cut_end;
        let mut retired = extent.owner;

        if left_pages != 0 {
            let permit = permits
                .pop()
                .expect("backing split metadata permit missing");
            let (left, tail) = retired.split_at(left_pages, Some(permit));
            self.extents.insert(
                index,
                BackingExtent {
                    offset_pages: extent_start,
                    owner: left,
                },
            );
            retired = tail;
        }
        if right_pages != 0 {
            let permit = permits
                .pop()
                .expect("backing split metadata permit missing");
            let (middle, right) = retired.split_at(retired_pages, Some(permit));
            retired = middle;
            self.extents.insert(
                index + usize::from(left_pages != 0),
                BackingExtent {
                    offset_pages: cut_end,
                    owner: right,
                },
            );
        }
        assert_eq!(retired.count(), retired_pages);
        (retired, retired_pages)
    }
}

impl PinnedMapResult {
    const COMMITTED_OFFSET: usize = core::mem::offset_of!(MemoryMapResult, committed);

    fn write_payload(&self) {
        let source = core::ptr::addr_of!(self.value).cast::<u8>();
        for chunk in &self.chunks {
            if chunk.result_offset >= Self::COMMITTED_OFFSET {
                break;
            }
            let bytes = chunk
                .bytes
                .min(Self::COMMITTED_OFFSET - chunk.result_offset);
            // SAFETY: projection 在 AddressSpace reservation 下由有效可写 PTE 固定；
            // result_offset/bytes 是 MemoryMapResult 已初始化对象表示的子区间。
            unsafe {
                core::ptr::copy_nonoverlapping(
                    source.add(chunk.result_offset),
                    mm::phys_to_virt(chunk.physical) as *mut u8,
                    bytes,
                );
            }
        }
    }

    fn commit_cookie(&self) {
        let chunk = self
            .chunks
            .iter()
            .find(|chunk| {
                chunk.result_offset <= Self::COMMITTED_OFFSET
                    && Self::COMMITTED_OFFSET + core::mem::size_of::<u64>()
                        <= chunk.result_offset + chunk.bytes
            })
            .expect("committed cookie must fit one pinned page");
        let physical = chunk.physical + Self::COMMITTED_OFFSET - chunk.result_offset;
        let pointer = mm::phys_to_virt(physical) as *mut AtomicU64;
        assert_eq!(
            pointer.addr() % core::mem::align_of::<AtomicU64>(),
            0,
            "committed cookie lost natural alignment"
        );
        // SAFETY: result ABI 与调用地址共同保证 AtomicU64 对齐；UserWriteLease
        // 独占映射变更，调用者在 syscall 期间不得并发非原子访问 committed。
        unsafe { AtomicU64::from_ptr(pointer.cast()).store(self.cookie, Ordering::Release) };
    }
}

fn protection_flags(protection: Protection) -> u64 {
    match protection {
        Protection::ReadOnly => flags::V | flags::U | flags::A | flags::R,
        Protection::ReadWrite => flags::V | flags::U | flags::A | flags::R | flags::W | flags::D,
        Protection::ReadExecute => flags::V | flags::U | flags::A | flags::R | flags::X,
    }
}

/// 纯逻辑规划器错误的公开投影：经 `SpaceError` 的单一分类表推导，不另建一张表。
fn public_change_error(error: ChangeError) -> SystemCallError {
    SpaceError::from(error).into()
}

/// Validate 之后的内部边界：公开 Map/Unmap/Protect 已在 Validate 阶段拒绝非法
/// 参数，此后出现的 `BadSegment` 是内核不变量失败而不是调用者过错。
fn post_validate_error(error: SpaceError) -> SystemCallError {
    match error {
        SpaceError::BadSegment => SystemCallError::InternalError,
        other => other.into(),
    }
}

fn process_protection(flags_value: ProcessMapFlags) -> Result<Protection, SpaceError> {
    let read = flags_value.contains(ProcessMapFlags::READ);
    let write = flags_value.contains(ProcessMapFlags::WRITE);
    let execute = flags_value.contains(ProcessMapFlags::EXECUTE);
    match (read, write, execute) {
        (true, false, false) => Ok(Protection::ReadOnly),
        (true, true, false) => Ok(Protection::ReadWrite),
        (true, false, true) => Ok(Protection::ReadExecute),
        _ => Err(SpaceError::BadSegment),
    }
}

fn public_protection(value: MemoryProtection) -> Protection {
    match value {
        MemoryProtection::ReadOnly => Protection::ReadOnly,
        MemoryProtection::ReadWrite => Protection::ReadWrite,
        MemoryProtection::ReadExecute => Protection::ReadExecute,
    }
}

/// 有界收束游标（REAPABLE 后由管理者分批驱动；见 lifecycle 模块）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DrainStage {
    /// 未进入收束（进程尚活）。
    Idle,
    /// 丢弃已不可达的 VA/transaction 账本。
    Ledger,
    /// 逐个归还新 ledger backing 的 owned extent。
    Backings,
    /// page_table 内部层级无关游标逐批交出中间表 owner。
    Tables { cursor: TableDrainCursor<LEVELS> },
    /// 全部 owned 槽与分支 owner 已空，交出 root owner 与 PoolBinding。
    Root,
    /// 资源全空（root 已释放）；仅剩空壳。
    Done,
}

/// 地址空间稳定 epoch 快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EpochSnapshot {
    pub translation: u64,
    pub instruction: u64,
}

fn advance_epoch(epoch: &AtomicU64) -> Option<u64> {
    let mut current = epoch.load(Ordering::Acquire);
    loop {
        let next = current.checked_add(1)?;
        match epoch.compare_exchange_weak(current, next, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Some(next),
            Err(observed) => current = observed,
        }
    }
}

static NEXT_ADDRESS_SPACE_ID: AtomicUsize = AtomicUsize::new(1);

/// 进程地址空间的稳定外壳。identity 与 epoch 不随 ledger/页表状态锁借用而移动，
/// Remote Call 和 execution gate 可在不复制 active 集合的前提下引用它们。
pub struct AddressSpace {
    identity: usize,
    translation_epoch: AtomicU64,
    instruction_epoch: AtomicU64,
    state: crate::sync::Spinlock<AddressSpaceState>,
}

/// 稳定 AddressSpace 身份下的一次性资源状态。Unbound 不持页额度、ledger 或页表；
/// Bound 的全部资源由同一个 PoolBinding 与可恢复 drain 生命周期拥有。
pub(crate) enum AddressSpaceState {
    Unbound,
    Bound(Box<BoundAddressSpace>),
}

impl AddressSpaceState {
    pub(crate) fn is_bound(&self) -> bool {
        matches!(self, Self::Bound(_))
    }

    pub(crate) fn bind(
        &mut self,
        bound: Box<BoundAddressSpace>,
    ) -> Result<(), Box<BoundAddressSpace>> {
        if !matches!(self, Self::Unbound) {
            return Err(bound);
        }
        *self = Self::Bound(bound);
        Ok(())
    }

    pub(crate) fn plan_building_mapping(
        &mut self,
        vaddr: usize,
        len: usize,
        permissions: ProcessMapFlags,
        prepared: PreparedBacking,
    ) -> Result<MemoryChangePlan, BackingPlanFailure<SpaceError>> {
        let bound = match self.bound_mut() {
            Ok(bound) => bound,
            Err(error) => return Err(BackingPlanFailure::Prepared(error, prepared)),
        };
        bound.plan_building_anonymous(vaddr, len, permissions, prepared)
    }

    pub(crate) fn complete_bound_mapping(
        &mut self,
        plan: MemoryChangePlan,
        funded: Vec<Vec<frame::FundedTableFrame>>,
    ) -> Result<PublishedTableChanges, (SpaceError, ReclaimedTableFrames)> {
        let bound = match self.bound_mut() {
            Ok(bound) => bound,
            Err(error) => {
                log!(
                    Memory,
                    "unexpected anonymous mapping completion in an unbound address space"
                );
                let MemoryChangePlan {
                    backing,
                    view_owner,
                    ..
                } = plan;
                return Err((
                    error,
                    ReclaimedTableFrames {
                        funded,
                        translations: Vec::new(),
                        failed_owners: None,
                        backing,
                        permits: Vec::new(),
                        view_owner,
                    },
                ));
            }
        };
        bound.complete_bound_mapping(plan, funded)
    }

    pub fn write_building(&mut self, target: usize, source: &[u8]) -> Result<(), SpaceError> {
        self.bound_mut()?.write_building(target, source)
    }

    pub fn validate_initial_context(
        &mut self,
        entry: usize,
        stack_pointer: usize,
    ) -> Result<(), SpaceError> {
        self.bound_mut()?
            .validate_initial_context(entry, stack_pointer)
    }

    pub fn drain(&mut self, budget: usize) -> (usize, bool) {
        match self {
            Self::Unbound => (0, true),
            Self::Bound(bound) => bound.drain(budget),
        }
    }

    fn take_retired(&mut self) -> Option<RetiredSpaceResource> {
        match self {
            Self::Unbound => None,
            Self::Bound(bound) => bound.retired.take(),
        }
    }

    pub(crate) fn bound(&self) -> Result<&BoundAddressSpace, SpaceError> {
        match self {
            Self::Unbound => Err(SpaceError::Unbound),
            Self::Bound(bound) => Ok(bound),
        }
    }

    fn bound_mut(&mut self) -> Result<&mut BoundAddressSpace, SpaceError> {
        match self {
            Self::Unbound => Err(SpaceError::Unbound),
            Self::Bound(bound) => Ok(bound),
        }
    }
}

impl core::ops::Deref for AddressSpaceState {
    type Target = BoundAddressSpace;

    fn deref(&self) -> &Self::Target {
        self.bound()
            .expect("Unbound AddressSpace reached a Bound-only internal path")
    }
}

impl core::ops::DerefMut for AddressSpaceState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.bound_mut()
            .expect("Unbound AddressSpace reached a Bound-only internal path")
    }
}

static SHOOTDOWN_SELFTEST_STARTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
static MULTI_HART_SHOOTDOWN_OBSERVED: AtomicBool = AtomicBool::new(false);

struct ShootdownSelfTestCompletion;

impl crate::remote_call::Completion for ShootdownSelfTestCompletion {
    fn complete(self: Arc<Self>) {
        log!(
            Memory,
            "epoch self-test passed: active snapshot and shootdown acknowledged"
        );
    }
}

/// object-owned lease 的收束通知面。资源所有权（view 强引用与 WritePermit）由
/// AddressSpace 统一持有与归还，sink 只在自己的 lease 完成时推进对象侧生命周期。
pub(crate) trait MemoryRetireSink: Send + Sync {
    fn retire_fragment(&self, fragment: RetiringFragment);
    fn finish(&self);
}

impl RetiringSpaceChange {
    /// 推进恰一个固定粒度：一个 table owner/outcome、一个 fragment、一个 backing
    /// extent、一个 WritePermit，或最终 Complete。返回 true 表示资源与 ledger 已收口。
    pub(crate) fn advance(
        &mut self,
        space: &AddressSpace,
        retire: Option<&dyn MemoryRetireSink>,
    ) -> bool {
        if !self.tables_complete {
            match self.tables.retire_step() {
                TableRetireStep::Owner(owner) => {
                    drop(owner);
                    return false;
                }
                TableRetireStep::Progress => return false,
                TableRetireStep::Complete => self.tables_complete = true,
            }
        }

        if let Some(mut cursor) = self.backing.take() {
            let (owner, pages) = space.lock().retire_backing_one(
                cursor.identity,
                cursor.next_offset,
                cursor.remaining,
                &mut self.backing_permits,
            );
            let bytes = pages
                .checked_mul(PAGE_SIZE)
                .expect("retired backing progress overflowed");
            cursor.next_offset += bytes;
            cursor.remaining -= bytes;
            if cursor.remaining != 0 {
                self.backing = Some(cursor);
            }
            drop(owner);
            return false;
        }

        if let Some(fragment) = self.batch.pop_fragment() {
            match fragment.kind {
                RegionKindView::Mapping {
                    backing:
                        BackingView::Anonymous {
                            identity, offset, ..
                        },
                    ..
                } => {
                    assert_eq!(
                        fragment.owner,
                        RegionOwner::AddressSpace,
                        "anonymous retire escaped AddressSpace authority"
                    );
                    if fragment.backing_retire == BackingRetire::Release {
                        self.backing = Some(BackingRetireCursor {
                            identity,
                            next_offset: offset,
                            remaining: fragment.range.bytes(),
                        });
                    }
                }
                RegionKindView::Mapping {
                    backing: BackingView::Object { object: _, .. },
                    ..
                } => {
                    // 一个事务可能产生同一对象的多个 retiring fragment；owner 只由
                    // batch-local 槽位保存一次，避免第二片重复摘除并触发 panic。
                    // owner 交接延迟到本批 ledger Complete 后统一决定。多个已发布
                    // 批次可逆序退役；在 fragment 步骤中摘 owner 会让后续批次失去
                    // 唯一交接点。core 来源已在 Commit 冻结，因而这里无需回查 live 表。
                    // object-owned lease 还要推进对象侧生命周期；进程自有 view 无 sink。
                    if let Some(retire) = retire {
                        retire.retire_fragment(fragment);
                    }
                }
                RegionKindView::Guard => {
                    assert_eq!(
                        fragment.owner,
                        RegionOwner::AddressSpace,
                        "guard retire escaped AddressSpace authority"
                    );
                }
            }
            return false;
        }

        if let Some(permit) = self.batch.pop_permit() {
            // 对象状态锁秩低于 AddressSpace：先取得强引用，解锁后再归还 permit。
            let object = permit.object();
            let core = Arc::clone(
                &self
                    .retiring_views
                    .iter()
                    .find(|view| view.object == object)
                    .expect("retiring permit source was not frozen")
                    .core,
            );
            core.retire_write(permit);
            return false;
        }

        assert!(self.batch.is_empty());
        if !self.ledger_complete {
            let ledger = self
                .ledger
                .take()
                .expect("Retiring memory change completed twice");
            {
                let mut space = space.lock();
                space.complete_retiring_change(ledger, &self.batch);
                for view in &mut self.retiring_views {
                    if view._owner.is_none() {
                        view._owner = space.release_view_region(view.object);
                    }
                }
            }
            self.ledger_complete = true;
            return false;
        }
        // 每次只析构一个交出的 view owner；其强引用可能归还对象 backing
        // 与 charge，不能把整批析构隐藏在一个 retire step 内。
        if let Some(view) = self.retiring_views.pop() {
            drop(view);
            return false;
        }
        if let Some(retire) = retire {
            retire.finish();
        }
        true
    }
}

/// Remote ack 后只把事务推进到 Retiring 并发布 work debt。队列 owner 在安全点
/// 逐批释放 table/backing/object owner，最后才 Complete ledger 与外部义务。
pub(crate) struct MemoryChangeCompletion {
    process: Arc<Process>,
    waiter: Arc<super::wait::WaitContext>,
    retire: Option<Arc<dyn MemoryRetireSink>>,
    published: crate::sync::Spinlock<Option<PublishedSpaceChange>>,
    retiring: crate::sync::Spinlock<Option<RetiringSpaceChange>>,
    work: crate::sync::Spinlock<Option<crate::deferred_work::Reservation>>,
    result_obligation: crate::sync::Spinlock<Option<super::thread::ThreadResultObligation>>,
    _change_metadata: super::resources::MemoryChangePermit,
    _remote_metadata: super::resources::RemoteCompletionPermit,
}

impl MemoryChangeCompletion {
    fn new(
        process: Arc<Process>,
        waiter: Arc<super::wait::WaitContext>,
        retire: Option<Arc<dyn MemoryRetireSink>>,
        result_obligation: Option<super::thread::ThreadResultObligation>,
        change_metadata: super::resources::MemoryChangePermit,
        remote_metadata: super::resources::RemoteCompletionPermit,
        work: crate::deferred_work::Reservation,
    ) -> Self {
        Self {
            process,
            waiter,
            retire,
            published: crate::sync::Spinlock::new(crate::sync::ranks::MEMORY_COMPLETION, None),
            retiring: crate::sync::Spinlock::new(crate::sync::ranks::MEMORY_COMPLETION, None),
            work: crate::sync::Spinlock::new(crate::sync::ranks::MEMORY_COMPLETION, Some(work)),
            result_obligation: crate::sync::Spinlock::new(
                crate::sync::ranks::MEMORY_COMPLETION,
                result_obligation,
            ),
            _change_metadata: change_metadata,
            _remote_metadata: remote_metadata,
        }
    }

    pub(crate) fn install(&self, published: PublishedSpaceChange) {
        let previous = self.published.lock().replace(published);
        assert!(previous.is_none(), "memory completion installed twice");
    }

    /// 由 work-debt owner hart 推进不超过 `budget` 个固定粒度；最终批次再兑销
    /// 进程与线程义务。返回 `(实际步骤, 已完成)`。
    pub(crate) fn advance_retire(&self, budget: usize) -> (usize, bool) {
        debug_assert!(budget > 0);
        let mut change = self
            .retiring
            .lock()
            .take()
            .expect("work debt ran without a Retiring memory change");
        for used in 1..=budget {
            if !change.advance(&self.process.space, self.retire.as_deref()) {
                continue;
            }
            if self.process.lifecycle.complete_mandatory()
                && let Some(control) = self.process.control()
            {
                control.publish_reapable();
            }
            // ThreadControl DONE 必须晚于 result lease 与 AddressSpace Complete；
            // 先释放 affine 线程义务，再完成仍存活调用者的 WaitContext。
            let result_obligation = self.result_obligation.lock().take();
            drop(result_obligation);
            self.waiter.clone().complete_kernel();
            return (used, true);
        }
        self.retiring.lock().replace(change);
        (budget, false)
    }
}

impl crate::remote_call::Completion for MemoryChangeCompletion {
    fn complete(self: Arc<Self>) {
        let published = self
            .published
            .lock()
            .take()
            .expect("memory completion ran before Commit publication");
        let retiring = self
            .process
            .space
            .lock()
            .begin_retire_published_change(published);
        let previous = self.retiring.lock().replace(retiring);
        assert!(
            previous.is_none(),
            "memory completion entered Retiring twice"
        );
        let work = self
            .work
            .lock()
            .take()
            .expect("memory completion lost its work debt reservation");
        work.publish(self);
    }
}

pub(crate) fn prepare_memory_completion(
    process: Arc<Process>,
    value: usize,
    retire: Option<Arc<dyn MemoryRetireSink>>,
    result_obligation: Option<super::thread::ThreadResultObligation>,
) -> Result<(Arc<MemoryChangeCompletion>, super::wait::WaitPlan), SystemCallError> {
    let metadata =
        super::resources::MetadataSponsor::reserve_memory_operation(process.resources.metadata())?;
    let (change_metadata, wait_metadata, remote_metadata) = metadata.into_parts();
    let work = crate::deferred_work::reserve().map_err(|_| SystemCallError::ReachLimit)?;
    let (waiter, plan) = super::wait::prepare_memory(value, wait_metadata)?;
    let completion = Arc::try_new(MemoryChangeCompletion::new(
        process,
        waiter,
        retire,
        result_obligation,
        change_metadata,
        remote_metadata,
        work,
    ))
    .map_err(|_| SystemCallError::OutOfMemory)?;
    Ok((completion, plan))
}

/// Commit 前持有 execution snapshot、全部目标槽与完成引用。
pub(crate) struct PreparedShootdown {
    execution: super::lifecycle::ExecutionSnapshot,
    remote: Option<crate::remote_call::ReservedBatch>,
    immediate: Option<Arc<dyn crate::remote_call::Completion>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrepareShootdownError {
    NotRunning,
    Busy,
    InvalidTargets,
    OutOfMemory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShootdownChanged;

/// Commit 后唯一允许的推进：锁外敲门铃，或在目标集为空时直接完成。
#[must_use = "committed shootdown must start synchronization after releasing business locks"]
pub(crate) enum ShootdownSynchronization {
    Remote(crate::remote_call::Doorbell),
    Immediate(Arc<dyn crate::remote_call::Completion>),
}

impl ShootdownSynchronization {
    pub(crate) fn start(self) {
        match self {
            Self::Remote(doorbell) => doorbell.ring(),
            Self::Immediate(completion) => completion.complete(),
        }
    }
}

impl AddressSpace {
    pub fn unbound() -> Self {
        let identity = NEXT_ADDRESS_SPACE_ID.fetch_add(1, Ordering::Relaxed);
        assert!(
            identity != 0 && identity != usize::MAX,
            "address-space identity exhausted"
        );
        Self {
            identity,
            translation_epoch: AtomicU64::new(1),
            instruction_epoch: AtomicU64::new(1),
            state: crate::sync::Spinlock::new(
                crate::sync::ranks::ADDRESS_SPACE,
                AddressSpaceState::Unbound,
            ),
        }
    }

    pub fn lock(&self) -> crate::sync::SpinlockGuard<'_, AddressSpaceState> {
        self.state.lock()
    }

    /// Building/bootstrap 共用的后半段：锁外供给表页后同步完成事务。
    fn complete_building_plan(
        &self,
        plan: MemoryChangePlan,
        pool: Arc<super::memory_pool::MemoryPool>,
    ) -> Result<(), SpaceError> {
        let funded = match fund_table_preflights(&pool, plan.preflights()) {
            Ok(funded) => funded,
            Err(error) => {
                let reclaimed = self.lock().rollback_memory_change_plan(plan);
                drop(reclaimed);
                return Err(error);
            }
        };
        let completed = self.lock().complete_bound_mapping(plan, funded);
        match completed {
            Ok(released) => {
                drop(released);
                Ok(())
            }
            Err((error, reclaimed)) => {
                drop(reclaimed);
                Err(error)
            }
        }
    }

    fn map_building_protection(
        &self,
        vaddr: usize,
        len: usize,
        protection: Protection,
        image_end: Option<usize>,
    ) -> Result<(), SpaceError> {
        let (pool, sponsor) = {
            let state = self.lock();
            let bound = state.bound()?;
            (Arc::clone(bound.pool()), Arc::clone(bound.sponsor()))
        };
        let prepared = PreparedBacking::allocate(len / PAGE_SIZE, &pool, &sponsor)?;
        let plan_result = {
            let mut state = self.lock();
            let bound = state
                .bound_mut()
                .expect("building backing validation lost its bound address space");
            bound.plan_bound_anonymous_mapping(vaddr, len, protection, image_end, prepared)
        };
        let plan = match plan_result {
            Ok(plan) => plan,
            Err(BackingPlanFailure::Prepared(error, backing)) => {
                drop(backing);
                return Err(error);
            }
            Err(BackingPlanFailure::Owned(error, backing)) => {
                drop(backing);
                return Err(error);
            }
        };
        self.complete_building_plan(plan, pool)
    }

    pub fn load_elf(&self, segments: &[elf::LoadSegment], file: &[u8]) -> Result<(), SpaceError> {
        let (runs, image_end) = {
            let mut state = self.lock();
            state.bound_mut()?.plan_elf_mappings(segments)?
        };
        for (vaddr, len, protection) in runs {
            self.map_building_protection(vaddr, len, protection, Some(vaddr + len))?;
        }
        let mut state = self.lock();
        state
            .bound_mut()?
            .write_elf_mappings(segments, file, image_end)
    }

    pub fn map_stack(&self) -> Result<(), SpaceError> {
        self.map_building_protection(
            USER_TOP - STACK_SIZE,
            STACK_SIZE,
            Protection::ReadWrite,
            None,
        )
    }

    pub fn map_bootstrap_block(
        &self,
        prefix: &[u8],
        payload: Option<&frame::BootFundedExtent>,
        payload_len: usize,
    ) -> Result<usize, SpaceError> {
        let (pool, sponsor, base, prefix_pages, pages, end, identity, lease) = {
            let mut state = self.lock();
            let bound = state.bound_mut()?;
            if prefix.is_empty() || prefix.len() % PAGE_SIZE != 0 || bound.image_end == 0 {
                return Err(SpaceError::BadSegment);
            }
            let payload_pages = payload_len.div_ceil(PAGE_SIZE);
            if payload.map_or(0, frame::BootFundedExtent::pages) != payload_pages {
                return Err(SpaceError::BadSegment);
            }
            let base = bound.image_end;
            let prefix_pages = prefix.len() / PAGE_SIZE;
            let pages = prefix_pages
                .checked_add(payload_pages)
                .ok_or(SpaceError::BadSegment)?;
            let span = pages.checked_mul(PAGE_SIZE).ok_or(SpaceError::BadSegment)?;
            let end = base.checked_add(span).ok_or(SpaceError::BadSegment)?;
            if end > USER_TOP - STACK_SIZE {
                return Err(SpaceError::BadSegment);
            }
            let identity = bound.mint_backing()?;
            let lease = bound.mint_lease()?;
            (
                Arc::clone(bound.pool()),
                Arc::clone(bound.sponsor()),
                base,
                prefix_pages,
                pages,
                end,
                identity,
                lease,
            )
        };
        let prepared_prefix = PreparedBacking::allocate(prefix_pages, &pool, &sponsor)?;
        let plan_result: Result<MemoryChangePlan, BackingPlanFailure<SpaceError>> = (|| {
            let mut state = self.lock();
            let bound = state
                .bound_mut()
                .expect("bootstrap backing validation lost its bound address space");
            let mut backing = prepared_prefix.bind(identity);
            if let Some(payload) = payload {
                if backing.extents.try_reserve(1).is_err() {
                    return Err(BackingPlanFailure::Owned(SpaceError::NoFrame, backing));
                }
                backing.extents.push(BackingExtent {
                    offset_pages: prefix_pages,
                    owner: BackingExtentOwner::BootBorrowed {
                        base: payload.base(),
                        pages: payload.pages(),
                    },
                });
                backing.pages = pages;
            }
            backing.write_from_start(prefix);
            bound.plan_bound_anonymous(
                base,
                Protection::ReadOnly,
                AnonymousClass::Data,
                RegionOwner::Lease(lease),
                Some(end),
                backing,
            )
        })();
        let plan = match plan_result {
            Ok(plan) => plan,
            Err(BackingPlanFailure::Prepared(error, backing)) => {
                drop(backing);
                return Err(error);
            }
            Err(BackingPlanFailure::Owned(error, backing)) => {
                drop(backing);
                return Err(error);
            }
        };
        match self.complete_building_plan(plan, pool) {
            Ok(()) => Ok(base),
            Err(error) => Err(error),
        }
    }

    pub(crate) fn epochs(&self) -> EpochSnapshot {
        EpochSnapshot {
            translation: self.translation_epoch.load(Ordering::Acquire),
            instruction: self.instruction_epoch.load(Ordering::Acquire),
        }
    }

    pub(crate) fn synchronize_local(&self) -> EpochSnapshot {
        let epochs = self.epochs();
        crate::remote_call::synchronize_local(
            self.identity,
            epochs.translation,
            epochs.instruction,
        );
        epochs
    }

    pub(crate) fn local_is_current(&self, expected: EpochSnapshot) -> bool {
        self.epochs() == expected
            && crate::remote_call::local_observes(
                self.identity,
                expected.translation,
                expected.instruction,
            )
    }

    /// primordial process 首次 dispatch 的真实锁序/epoch 探针。调用点已登记 active；
    /// 本方法不等待，当前 hart 在返回用户态前有界消费自身请求。
    pub(crate) fn selftest_shootdown(&self, lifecycle: &super::lifecycle::Lifecycle) {
        if SHOOTDOWN_SELFTEST_STARTED.swap(true, Ordering::AcqRel) {
            return;
        }
        let completion: Arc<dyn crate::remote_call::Completion> =
            Arc::try_new(ShootdownSelfTestCompletion)
                .expect("shootdown self-test completion allocation failed");
        let prepared = self
            .prepare_shootdown(lifecycle, completion)
            .expect("shootdown self-test reservation failed");
        let (_, synchronization) = self
            .commit_shootdown(lifecycle, prepared, 0, 1, true, false, |_| ())
            .expect("shootdown self-test execution snapshot changed");
        synchronization.start();
        crate::remote_call::drain_current();
    }

    /// Reserve 阶段快照 active 集合并预留全部 Remote Call 槽。
    pub(crate) fn prepare_shootdown(
        &self,
        lifecycle: &super::lifecycle::Lifecycle,
        completion: Arc<dyn crate::remote_call::Completion>,
    ) -> Result<PreparedShootdown, PrepareShootdownError> {
        let execution = lifecycle
            .snapshot_running()
            .ok_or(PrepareShootdownError::NotRunning)?;
        let active = execution.active();
        if active.count_ones() >= 2 && !MULTI_HART_SHOOTDOWN_OBSERVED.swap(true, Ordering::AcqRel) {
            log!(
                Memory,
                "same-address-space multi-hart shootdown observed: {} active harts",
                active.count_ones()
            );
        }
        if active == 0 {
            return Ok(PreparedShootdown {
                execution,
                remote: None,
                immediate: Some(completion),
            });
        }
        let remote =
            crate::remote_call::reserve(active, completion).map_err(|error| match error {
                crate::remote_call::ReserveError::Busy => PrepareShootdownError::Busy,
                crate::remote_call::ReserveError::InvalidTargets
                | crate::remote_call::ReserveError::EmptyTargets => {
                    PrepareShootdownError::InvalidTargets
                }
                crate::remote_call::ReserveError::AllocationFailed => {
                    PrepareShootdownError::OutOfMemory
                }
            })?;
        Ok(PreparedShootdown {
            execution,
            remote: Some(remote),
            immediate: None,
        })
    }

    /// 在 `ADDRESS_SPACE → LIFECYCLE → REMOTE_CALL` 锁序内完成不可失败 Publish。
    /// stale execution snapshot 在调用 publish 前失败，Prepared 资源自动回滚。
    pub(crate) fn commit_shootdown<R>(
        &self,
        lifecycle: &super::lifecycle::Lifecycle,
        prepared: PreparedShootdown,
        start_vpn: usize,
        page_count: usize,
        instruction: bool,
        mandatory: bool,
        publish: impl FnOnce(&mut AddressSpaceState) -> R,
    ) -> Result<(R, ShootdownSynchronization), ShootdownChanged> {
        assert!(page_count != 0, "shootdown range must be nonempty");
        let PreparedShootdown {
            execution,
            remote,
            immediate,
        } = prepared;
        let mut state = self.state.lock();
        if self.translation_epoch.load(Ordering::Acquire) == u64::MAX
            || (instruction && self.instruction_epoch.load(Ordering::Acquire) == u64::MAX)
        {
            return Err(ShootdownChanged);
        }
        lifecycle
            .commit_if_current(execution, mandatory, |active| {
                debug_assert_eq!(active, execution.active());
                let result = publish(&mut state);
                let epochs = self.publish_epochs(instruction);
                let synchronization = if let Some(remote) = remote {
                    let request = crate::remote_call::FenceRequest::new(
                        self.identity,
                        epochs.translation,
                        if instruction { epochs.instruction } else { 0 },
                        start_vpn,
                        page_count,
                    );
                    ShootdownSynchronization::Remote(remote.publish(request))
                } else {
                    ShootdownSynchronization::Immediate(
                        immediate.expect("empty target shootdown must retain completion"),
                    )
                };
                (result, synchronization)
            })
            .map_err(|_| ShootdownChanged)
    }

    fn publish_epochs(&self, instruction: bool) -> EpochSnapshot {
        let translation = advance_epoch(&self.translation_epoch)
            .expect("address-space translation epoch exhausted before publish");
        let instruction = if instruction {
            advance_epoch(&self.instruction_epoch)
                .expect("address-space instruction epoch exhausted before publish")
        } else {
            self.instruction_epoch.load(Ordering::Acquire)
        };
        EpochSnapshot {
            translation,
            instruction,
        }
    }
}

pub(crate) fn map_shootdown_error(error: PrepareShootdownError) -> SystemCallError {
    match error {
        PrepareShootdownError::NotRunning => SystemCallError::ObjectClosed,
        PrepareShootdownError::Busy => SystemCallError::ObjectBusy,
        PrepareShootdownError::InvalidTargets => SystemCallError::InternalError,
        PrepareShootdownError::OutOfMemory => SystemCallError::OutOfMemory,
    }
}

fn public_page_range(address: u64, bytes: u64) -> Result<LedgerPageRange, SystemCallError> {
    let address = usize::try_from(address).map_err(|_| SystemCallError::IllegalArgument)?;
    let bytes = usize::try_from(bytes).map_err(|_| SystemCallError::IllegalArgument)?;
    LedgerPageRange::new(address, bytes).map_err(|_| SystemCallError::IllegalArgument)
}

/// Map 事务的 backing 来源意图。匿名来源由目标进程绑定池取得 backing；对象来源
/// 只验证 Handle、对象内页对齐 offset、范围与 rights，不重新分配数据页。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MapSource {
    Anonymous,
    Object {
        handle: erhino_shared::object::Handle,
        offset: usize,
    },
}

/// 区域的撤销 authority。进程自有区域可由普通 Unmap/Protect 操作；object-owned
/// lease 只能由持 lease 的对象撤销，普通 Unmap 在 Commit 前失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MapAuthority {
    AddressSpace,
    ObjectLease,
}

/// 公开 `MemoryMap` 请求的一次性解析结果。ABI 整数 → 内部类型的转换只在进入
/// AddressSpace 前做一次；Validate 预检与 Reserve 复检都消费同一份 intent，不各自
/// 重建一套参数规则。
pub(crate) struct MapIntent {
    bytes: usize,
    guard_before: usize,
    guard_after: usize,
    placement: MapPlacement,
    protection: Protection,
    /// 固定宽结果槽与提交 cookie；内部映射（Tunnel/Building）无结果槽。
    result: Option<(AddressRange, u64)>,
    source: MapSource,
    authority: MapAuthority,
}

impl MapIntent {
    fn parse(request: MemoryMapRequest) -> Result<Self, SystemCallError> {
        let bytes = usize::try_from(request.bytes).map_err(|_| SystemCallError::IllegalArgument)?;
        if bytes == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let guard_before =
            usize::try_from(request.guard_before).map_err(|_| SystemCallError::IllegalArgument)?;
        let guard_after =
            usize::try_from(request.guard_after).map_err(|_| SystemCallError::IllegalArgument)?;
        let result_address = usize::try_from(request.result_address)
            .map_err(|_| SystemCallError::IllegalArgument)?;
        let protection = MemoryProtection::from_raw(request.protection)
            .ok_or(SystemCallError::IllegalArgument)?;
        if request.reserved != [0; 1] {
            return Err(SystemCallError::IllegalArgument);
        }
        let source = if request.source == 0 {
            if request.source_offset != 0 {
                return Err(SystemCallError::IllegalArgument);
            }
            // 运行期匿名映射只产生数据权限；可执行字节必须经 MemoryObject 发布。
            if protection == MemoryProtection::ReadExecute {
                return Err(SystemCallError::RightsDenied);
            }
            MapSource::Anonymous
        } else {
            let offset = usize::try_from(request.source_offset)
                .map_err(|_| SystemCallError::IllegalArgument)?;
            if !offset.is_multiple_of(PAGE_SIZE) {
                return Err(SystemCallError::IllegalArgument);
            }
            MapSource::Object {
                handle: erhino_shared::object::Handle::from_raw(request.source),
                offset,
            }
        };
        let placement = match MemoryPlacement::from_raw(request.placement)
            .ok_or(SystemCallError::IllegalArgument)?
        {
            MemoryPlacement::Anywhere if request.address == 0 => MapPlacement::Anywhere,
            MemoryPlacement::FixedEmpty => MapPlacement::FixedEmpty {
                usable_start: usize::try_from(request.address)
                    .map_err(|_| SystemCallError::IllegalArgument)?,
            },
            MemoryPlacement::Anywhere => return Err(SystemCallError::IllegalArgument),
        };
        let result_range =
            AddressRange::new(result_address, core::mem::size_of::<MemoryMapResult>())
                .map_err(|_| SystemCallError::IllegalArgument)?;
        Ok(Self {
            bytes,
            guard_before,
            guard_after,
            placement,
            protection: public_protection(protection),
            result: Some((result_range, request.cookie)),
            source,
            authority: MapAuthority::AddressSpace,
        })
    }

    /// 内部固定 placement 映射（Tunnel view）：无 guard、无结果槽，authority 归对象。
    pub(crate) fn object_lease(va: usize, bytes: usize, protection: Protection) -> Self {
        Self {
            bytes,
            guard_before: 0,
            guard_after: 0,
            placement: MapPlacement::FixedEmpty { usable_start: va },
            protection,
            result: None,
            source: MapSource::Object {
                handle: erhino_shared::object::Handle::INVALID,
                offset: 0,
            },
            authority: MapAuthority::ObjectLease,
        }
    }

    pub(crate) fn source(&self) -> MapSource {
        self.source
    }

    pub(crate) fn protection(&self) -> Protection {
        self.protection
    }

    pub(crate) fn bytes(&self) -> usize {
        self.bytes
    }

    fn pages(&self) -> usize {
        self.bytes.div_ceil(PAGE_SIZE)
    }
}

/// 事务完成后向调用者交付的东西。Map 的结果槽在 Commit 内发布；其余变更
/// 只有完成语义，无输出。
enum ChangeOutput {
    /// Map：Commit 内先写 payload 再以 release 发布 cookie；结果义务随线程。
    MapResult(super::thread::ThreadResultObligation),
    /// Unmap/Protect：只有完成边界，无结果槽。
    None,
}

/// 启动 Running 进程自有地址空间变更的后半段：预留 completion 与全部 Remote 槽，
/// 在 execution gate 内 Commit，锁外敲门铃。
///
/// `instruction` 是真实的物理维度（是否需要推进 instruction epoch 与 `FENCE.I`），
/// 不是调用点区分开关；output 区分 Map 结果槽与纯完成语义。
fn start_running_memory_change(
    process: Arc<Process>,
    mut prepared: Option<PreparedMemoryChange>,
    shootdown_range: LedgerPageRange,
    instruction: bool,
    output: ChangeOutput,
) -> Result<super::wait::WaitPlan, SystemCallError> {
    let writes_map_payload = matches!(output, ChangeOutput::MapResult(_));
    let result_obligation = match output {
        ChangeOutput::MapResult(obligation) => Some(obligation),
        ChangeOutput::None => None,
    };
    macro_rules! rollback {
        ($reason:literal) => {{
            let mut reclaimed = process
                .space
                .lock()
                .rollback_memory_change(prepared.take().expect($reason));
            release_reclaimed_permits(&process, &mut reclaimed);
            drop(reclaimed);
        }};
    }
    let (completion, plan) =
        match prepare_memory_completion(process.clone(), 0, None, result_obligation) {
            Ok(prepared) => prepared,
            Err(error) => {
                rollback!("memory change must roll back");
                return Err(error);
            }
        };
    let sink: Arc<dyn crate::remote_call::Completion> = completion.clone();
    let shootdown = match process.space.prepare_shootdown(&process.lifecycle, sink) {
        Ok(shootdown) => shootdown,
        Err(error) => {
            rollback!("memory change must roll back");
            return Err(map_shootdown_error(error));
        }
    };
    if writes_map_payload {
        process
            .space
            .lock()
            .write_map_payload(prepared.as_ref().expect("Map reservation must exist"));
    }

    let committed = process.space.commit_shootdown(
        &process.lifecycle,
        shootdown,
        shootdown_range.start() / PAGE_SIZE,
        shootdown_range.pages(),
        instruction,
        true,
        |state| {
            state.commit_change(
                prepared
                    .take()
                    .expect("user memory change commits exactly once"),
            )
        },
    );
    let (published, synchronization) = match committed {
        Ok(committed) => committed,
        Err(_) => {
            rollback!("stale change must roll back");
            return Err(SystemCallError::ObjectBusy);
        }
    };
    completion.install(published);
    synchronization.start();
    Ok(plan)
}

/// 为当前 Running process 建立 mapping。backing 来源由请求的 `source` 声明：
/// 匿名页由本进程绑定池取得，MemoryObject view 只验证 rights 与几何。
pub(crate) fn memory_map(
    thread: &Thread,
    request_ptr: usize,
) -> Result<super::wait::WaitPlan, SystemCallError> {
    let process = thread.process.clone();
    let (intent, pool, sponsor) = {
        let mut space = process.space.lock();
        // SAFETY: MemoryMapRequest 只含整数且无 padding，任意位型均有效。
        let request: MemoryMapRequest =
            unsafe { crate::uaccess::read_user_value(&mut space, request_ptr) }?;
        if request.cookie == 0
            || request.result_address
                % u64::try_from(core::mem::align_of::<MemoryMapResult>()).unwrap()
                != 0
        {
            return Err(SystemCallError::IllegalArgument);
        }
        let result_address = usize::try_from(request.result_address)
            .map_err(|_| SystemCallError::IllegalArgument)?;
        // SAFETY: MemoryMapResult 只含整数且无 padding，任意位型均有效。
        let initial: MemoryMapResult =
            unsafe { crate::uaccess::read_user_value(&mut space, result_address) }?;
        if initial.reserved != [0; 3] || initial.committed != 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let intent = MapIntent::parse(request)?;
        (
            intent,
            Arc::clone(space.pool()),
            Arc::clone(space.sponsor()),
        )
    };
    match intent.source() {
        MapSource::Anonymous => map_anonymous_source(thread, process, intent, &pool, &sponsor),
        MapSource::Object { handle, offset } => {
            super::memory_object::map_view(thread, process, intent, handle, offset, &sponsor)
        }
    }
}

fn map_anonymous_source(
    thread: &Thread,
    process: Arc<Process>,
    intent: MapIntent,
    pool: &Arc<super::memory_pool::MemoryPool>,
    sponsor: &Arc<super::resources::MetadataSponsor>,
) -> Result<super::wait::WaitPlan, SystemCallError> {
    {
        let mut space = process.space.lock();
        space.validate_user_map(&intent)?;
    }
    let backing =
        PreparedBacking::allocate(intent.pages(), pool, sponsor).map_err(post_validate_error)?;
    let plan_result = {
        let mut space = process.space.lock();
        space.plan_user_map(&intent, backing)
    };
    let plan = match plan_result {
        Ok(plan) => plan,
        Err(BackingPlanFailure::Prepared(error, backing)) => {
            drop(backing);
            return Err(error);
        }
        Err(BackingPlanFailure::Owned(error, backing)) => {
            drop(backing);
            return Err(error);
        }
    };
    finish_running_map(thread, process, plan)
}

/// Map 的公共尾段：锁外供表页、完成 reservation，再以结果槽输出启动事务。
pub(crate) fn finish_running_map(
    thread: &Thread,
    process: Arc<Process>,
    plan: MemoryChangePlan,
) -> Result<super::wait::WaitPlan, SystemCallError> {
    let prepared = fund_and_complete_running(&process, plan)?;
    let layout = prepared
        .get()
        .change
        .map_result()
        .expect("Map reservation must retain its layout");
    start_running_memory_change(
        process,
        Some(prepared),
        layout.reservation,
        false,
        ChangeOutput::MapResult(thread.result_obligation()),
    )
}

/// 精确解除当前 Running process 的普通 mapping/reservation。
pub(crate) fn memory_unmap(
    thread: &Thread,
    address: u64,
    bytes: u64,
) -> Result<super::wait::WaitPlan, SystemCallError> {
    let process = thread.process.clone();
    let range = public_page_range(address, bytes)?;
    let (validated, requirements, sponsor) = {
        let mut space = process.space.lock();
        let (validated, requirements) = space.validate_user_unmap(range)?;
        let sponsor = Arc::clone(space.sponsor());
        (validated, requirements, sponsor)
    };
    let backing_permits = reserve_backing_split_metadata(&sponsor).map_err(post_validate_error)?;
    start_existing_change(
        thread,
        process,
        validated,
        requirements,
        backing_permits,
        range,
        false,
    )
}

/// 在创建时冻结的最大权限内改变当前 mapping 权限。
pub(crate) fn memory_protect(
    thread: &Thread,
    address: u64,
    bytes: u64,
    protection: usize,
) -> Result<super::wait::WaitPlan, SystemCallError> {
    let process = thread.process.clone();
    let range = public_page_range(address, bytes)?;
    let raw = u32::try_from(protection).map_err(|_| SystemCallError::IllegalArgument)?;
    let protection = MemoryProtection::from_raw(raw).ok_or(SystemCallError::IllegalArgument)?;
    let protection = public_protection(protection);
    let (validated, requirements) = {
        let mut space = process.space.lock();
        space.validate_user_protect(range, protection)?
    };
    // 只有进出可执行权限的变更需要 instruction epoch 与 `FENCE.I`。
    let instruction = validated.translation_intents().iter().any(|intent| {
        matches!(
            intent,
            TranslationIntent::Protect { from, to, .. }
                if *from == Protection::ReadExecute || *to == Protection::ReadExecute
        )
    });
    start_existing_change(
        thread,
        process,
        validated,
        requirements,
        Vec::new(),
        range,
        instruction,
    )
}

/// Unmap/Protect 的公共后半段：先在 AddressSpace 锁外按 Validate 报告的多重集向各
/// 来源对象取得 WritePermit（对象锁秩低于 AddressSpace），再重入完成 reservation。
///
/// 含 W 的 object view 被部分撤销或降权时，存活片段是新铸造的区域，各自需要一枚新
/// permit；纯匿名变更的多重集为空，这条路径退化为原来的零 permit 形态。
fn start_existing_change(
    thread: &Thread,
    process: Arc<Process>,
    validated: memory_space::ValidatedChange,
    requirements: Vec<PermitRequirement>,
    backing_permits: Vec<super::resources::BackingSlicePermit>,
    range: LedgerPageRange,
    instruction: bool,
) -> Result<super::wait::WaitPlan, SystemCallError> {
    let (permits, sources) = match acquire_view_permits(&process, &requirements) {
        Ok(acquired) => acquired,
        Err(error) => {
            // Validate 未预留任何资源，放弃计划无需回滚账本。
            drop(validated);
            return Err(error);
        }
    };
    let plan_result = {
        let mut space = process.space.lock();
        space.prepare_user_existing_change(validated, permits)
    };
    let mut plan = match plan_result {
        Ok(plan) => plan,
        Err(failure) => {
            release_view_permits(&sources, failure.permits);
            return Err(post_validate_error(failure.error));
        }
    };
    plan.backing_permits = backing_permits;
    let prepared = fund_and_complete_running(&process, plan)?;
    let _ = thread;
    start_running_memory_change(
        process,
        Some(prepared),
        range,
        instruction,
        ChangeOutput::None,
    )
}

/// 按 Validate 报告的多重集向每个来源对象取得 WritePermit。任何一项失败时，已取得
/// 的 permit 立即原样归还，账本零副作用。
fn acquire_view_permits(
    process: &Arc<Process>,
    requirements: &[PermitRequirement],
) -> Result<
    (
        Vec<WritePermit>,
        Vec<Arc<super::memory_object::MemoryObjectCore>>,
    ),
    SystemCallError,
> {
    let mut permits = Vec::new();
    let mut sources = Vec::new();
    if requirements.is_empty() {
        return Ok((permits, sources));
    }
    let total = requirements
        .iter()
        .try_fold(0usize, |sum, requirement| {
            sum.checked_add(requirement.count)
        })
        .ok_or(SystemCallError::InternalError)?;
    permits
        .try_reserve_exact(total)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    sources
        .try_reserve_exact(requirements.len())
        .map_err(|_| SystemCallError::OutOfMemory)?;
    for requirement in requirements {
        let core = process.space.lock().view_core(requirement.object);
        match core.reserve_writes(requirement.count) {
            Ok(mut acquired) => {
                permits.append(&mut acquired);
                sources.push(core);
            }
            Err(error) => {
                let failure = super::memory_object::map_object_error(error);
                release_view_permits(&sources, permits);
                return Err(failure);
            }
        }
    }
    Ok((permits, sources))
}

/// 把未提交的 permit 原样归还来源对象。permit 自带来源身份，因此按对象分派。
fn release_view_permits(
    sources: &[Arc<super::memory_object::MemoryObjectCore>],
    permits: Vec<WritePermit>,
) {
    for permit in permits {
        let object = permit.object();
        let core = sources
            .iter()
            .find(|core| core.identity() == object)
            .expect("reserved write permit lost its source object");
        core.cancel_write(permit);
    }
}

fn release_reclaimed_permits(process: &Arc<Process>, reclaimed: &mut ReclaimedTableFrames) {
    let owner_core = reclaimed
        .view_owner
        .as_ref()
        .map(|owner| Arc::clone(&owner.core));
    for permit in reclaimed.take_permits() {
        let object = permit.object();
        let core = match owner_core.as_ref().filter(|core| core.identity() == object) {
            Some(core) => Arc::clone(core),
            None => process.space.lock().view_core(object),
        };
        core.cancel_write(permit);
    }
}

/// 锁外取得表页后重入 AddressSpace 完成 reservation。三个公开入口共用同一段：
/// 表页供给与完成失败都在 Commit 前，因而只需锁外析构摘出的 owner。
fn fund_and_complete_running(
    process: &Arc<Process>,
    plan: MemoryChangePlan,
) -> Result<PreparedMemoryChange, SystemCallError> {
    let pool = Arc::clone(process.space.lock().pool());
    let funded = match fund_table_preflights(&pool, plan.preflights()) {
        Ok(funded) => funded,
        Err(error) => {
            let mut reclaimed = process.space.lock().rollback_memory_change_plan(plan);
            release_reclaimed_permits(process, &mut reclaimed);
            drop(reclaimed);
            return Err(post_validate_error(error));
        }
    };
    let result = process.space.lock().complete_memory_change(plan, funded);
    match result {
        Ok(prepared) => Ok(prepared),
        Err((error, mut reclaimed)) => {
            release_reclaimed_permits(process, &mut reclaimed);
            drop(reclaimed);
            Err(post_validate_error(error))
        }
    }
}

/// 地址空间可变状态：MemorySpace ledger、anonymous backing、页表树与有界 drain
/// 共同组成 VA 所有权真值；稳定 identity/epoch 位于外层 AddressSpace。
pub(crate) struct BoundAddressSpace {
    /// REAPABLE 屏障后由 drain 最终阶段 take 释放 root；之后任何访问
    /// 都是编程错误（Building 操作准入与 active 位图已消除可达性）。
    tree: Option<TableTree<TableMem, LEVELS>>,
    /// root 物理 owner、charge 与进程后续 page-backed storage 的唯一来源。
    binding: Option<super::resources::PoolBinding>,
    satp: usize,
    /// VA 区域与事务真值；Drain 起点 take 后不再可访问。
    ledger: Option<MemorySpace>,
    /// Running/Tunnel Prepared change 的唯一页表发布权；避免旧代次跨事务提交。
    table_transaction_active: bool,
    /// 以 BackingId 关联 ledger logical offset 的 affine anonymous extents。
    backings: Vec<OwnedBacking>,
    /// 本地址空间引用的 MemoryObject view 所有权，按 ObjectId 有序。它使对象
    /// 独立于 Handle 存活：Handle 先关闭不影响已建立的 view。
    views: Vec<ObjectViewOwner>,
    next_backing: u64,
    /// Object-owned mapping authority；单调不复用。
    next_lease: u64,
    /// Building 期映像与 StartupBlock 的页对齐布局终点。
    image_end: usize,
    /// 有界收束游标（drain_gate + space 锁双持下推进）。
    drain_stage: DrainStage,
    /// 已从拥有结构摘下、等待下一 work unit 移出 AddressSpace 锁的 owner。
    pending_free: Option<RetiredSpaceResource>,
    /// 已计入本批 work、必须在释放 AddressSpace 锁后析构的 Pool-backed owner。
    retired: Option<RetiredSpaceResource>,
}

impl BoundAddressSpace {
    /// 从 PoolBinding 构造 Bound 状态；root tree 持有 funded owner。
    pub fn new(binding: super::resources::PoolBinding) -> Result<Box<Self>, SpaceError> {
        let root = frame::fund_user_table_frame(binding.pool()).map_err(map_funded_error)?;
        let mut tree = TableTree::new(TableMem, root);
        mm::install_kernel_top_level(&mut tree);
        let satp = (8usize << 60) | tree.satp_ppn();
        let bounds = LedgerPageRange::new(0, USER_TOP).map_err(|_| SpaceError::BadSegment)?;
        let ledger = MemorySpace::new(bounds, MEMORY_SPACE_LIMITS).map_err(SpaceError::from)?;
        Box::try_new(Self {
            tree: Some(tree),
            binding: Some(binding),
            satp,
            ledger: Some(ledger),
            table_transaction_active: false,
            backings: Vec::new(),
            views: Vec::new(),
            next_backing: 1,
            next_lease: 1,
            image_end: 0,
            drain_stage: DrainStage::Idle,
            pending_free: None,
            retired: None,
        })
        .map_err(|_| SpaceError::NoFrame)
    }

    pub(crate) fn pool(&self) -> &Arc<super::memory_pool::MemoryPool> {
        self.binding
            .as_ref()
            .expect("address-space PoolBinding already retired")
            .pool()
    }

    pub(crate) fn sponsor(&self) -> &Arc<super::resources::MetadataSponsor> {
        self.binding
            .as_ref()
            .expect("address-space PoolBinding already retired")
            .sponsor()
    }

    /// 本地址空间的 satp 组装值（含模式位）。
    pub fn satp(&self) -> usize {
        self.satp
    }

    /// 活树访问（drain 完成 root 释放后为零占位期，任何访问都是编程
    /// 错误——REAPABLE 后 Building 操作准入与线程 active 位图已消除
    /// 可达性）。
    fn tt(&mut self) -> &mut TableTree<TableMem, LEVELS> {
        self.tree.as_mut().expect("address space tree is live")
    }

    fn ledger(&mut self) -> &mut MemorySpace {
        self.ledger.as_mut().expect("address-space ledger is live")
    }

    fn ensure_table_transaction_available(&self) -> Result<(), SpaceError> {
        if self.table_transaction_active {
            Err(SpaceError::Busy)
        } else {
            Ok(())
        }
    }

    fn mark_table_transaction(&mut self) {
        assert!(
            !core::mem::replace(&mut self.table_transaction_active, true),
            "page-table transaction installed twice"
        );
    }

    fn clear_table_transaction(&mut self) {
        self.table_transaction_active = false;
    }

    fn mint_backing(&mut self) -> Result<BackingId, SpaceError> {
        let identity = BackingId::new(self.next_backing).ok_or(SpaceError::NoFrame)?;
        self.next_backing = self
            .next_backing
            .checked_add(1)
            .ok_or(SpaceError::NoFrame)?;
        Ok(identity)
    }

    fn mint_lease(&mut self) -> Result<LeaseKey, SpaceError> {
        let identity = LeaseKey::new(self.next_lease).ok_or(SpaceError::NoFrame)?;
        self.next_lease = self.next_lease.checked_add(1).ok_or(SpaceError::NoFrame)?;
        Ok(identity)
    }

    fn pin_map_result(
        &mut self,
        change: &PreparedChange,
        value: MemoryMapResult,
        cookie: u64,
    ) -> Result<PinnedMapResult, SystemCallError> {
        let lease = change
            .user_write_lease()
            .expect("public Map must reserve a result lease");
        let range = lease.range();
        assert_eq!(
            range.bytes(),
            core::mem::size_of::<MemoryMapResult>(),
            "public Map result lease has wrong width"
        );
        let mut cursor = range.start();
        for segment in lease.projection().segments() {
            assert_eq!(segment.user.start(), cursor, "result projection has a gap");
            cursor = segment.user.end();
        }
        assert_eq!(cursor, range.end(), "result projection is incomplete");

        let first_offset = range.start() % PAGE_SIZE;
        let page_count = (first_offset + range.bytes()).div_ceil(PAGE_SIZE);
        let mut chunks = Vec::new();
        chunks
            .try_reserve_exact(page_count)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        let mut result_offset = 0;
        while result_offset < range.bytes() {
            let user = range.start() + result_offset;
            let in_page = user % PAGE_SIZE;
            let bytes = (PAGE_SIZE - in_page).min(range.bytes() - result_offset);
            let physical = self.page_pa(user).ok_or(SystemCallError::InternalError)? + in_page;
            chunks.push(PinnedWriteChunk {
                physical,
                result_offset,
                bytes,
            });
            result_offset += bytes;
        }
        Ok(PinnedMapResult {
            chunks,
            value,
            cookie,
        })
    }

    /// 建立与 `intent` 一致的 ledger 请求。Validate 预检与 Reserve 复检共用它，
    /// 因此两次道口不会因各自组装而失步。`backing` 由来源侧提供：匿名给出待铸造的
    /// 身份，对象给出经状态机认证的 view 授权。
    fn map_request(intent: &MapIntent, owner: RegionOwner, backing: MapBacking) -> MapRequest {
        MapRequest {
            bytes: intent.bytes,
            guard_before: intent.guard_before,
            guard_after: intent.guard_after,
            placement: intent.placement,
            current: intent.protection,
            maximum: intent.protection,
            owner,
            backing,
            result: intent
                .result
                .map(|(range, _)| memory_space::UserWriteLeaseRequest { range }),
        }
    }

    fn anonymous_backing(identity: BackingId) -> MapBacking {
        MapBacking::Anonymous {
            identity,
            class: AnonymousClass::Data,
        }
    }

    /// 取得 backing 前的预检：确认几何、authority 与结果槽合法，不预留任何
    /// 资源。真正的事务在锁外取得 Pool/metadata 后由 `plan_user_map` 复检。
    fn validate_user_map(&mut self, intent: &MapIntent) -> Result<(), SystemCallError> {
        self.ensure_table_transaction_available()
            .map_err(post_validate_error)?;
        let identity = BackingId::new(self.next_backing).ok_or(SystemCallError::OutOfMemory)?;
        self.ledger()
            .validate_map(Self::map_request(
                intent,
                RegionOwner::AddressSpace,
                Self::anonymous_backing(identity),
            ))
            .map_err(public_change_error)
            .map(|_| ())
    }

    /// 从已解析 intent 建立公开 anonymous Map 事务。参数解析已在锁外完成（
    /// [`AnonymousMapIntent::parse`]），本函数只做锁内复检与资源组装。
    fn plan_user_map(
        &mut self,
        intent: &MapIntent,
        prepared_backing: PreparedBacking,
    ) -> Result<MemoryChangePlan, BackingPlanFailure<SystemCallError>> {
        macro_rules! fail_prepared {
            ($error:expr) => {{
                return Err(BackingPlanFailure::Prepared($error, prepared_backing));
            }};
        }
        macro_rules! fail_owned {
            ($error:expr, $owner:expr) => {{
                return Err(BackingPlanFailure::Owned($error, $owner));
            }};
        }
        if let Err(error) = self
            .ensure_table_transaction_available()
            .map_err(post_validate_error)
        {
            fail_prepared!(error);
        }
        let identity = match self.mint_backing().map_err(post_validate_error) {
            Ok(value) => value,
            Err(error) => fail_prepared!(error),
        };
        let validated = match self.ledger().validate_map(Self::map_request(
            intent,
            RegionOwner::AddressSpace,
            Self::anonymous_backing(identity),
        )) {
            Ok(value) => value,
            Err(error) => fail_prepared!(public_change_error(error)),
        };
        let layout = validated
            .map_result()
            .expect("public Map validation must produce a layout");
        if prepared_backing.pages != layout.usable.pages() {
            fail_prepared!(SystemCallError::IllegalArgument);
        }
        if self.backings.try_reserve(1).is_err() {
            fail_prepared!(SystemCallError::OutOfMemory);
        }
        let change = match self
            .ledger()
            .reserve(validated, Vec::new())
            .map_err(|failure| public_change_error(failure.error))
        {
            Ok(change) => change,
            Err(error) => fail_prepared!(error),
        };
        let backing = prepared_backing.bind(identity);
        let value = MemoryMapResult {
            usable_base: layout.usable.start() as u64,
            usable_bytes: layout.usable.bytes() as u64,
            reservation_base: layout.reservation.start() as u64,
            reservation_bytes: layout.reservation.bytes() as u64,
            reserved: [0; 3],
            committed: 0,
        };
        let cookie = intent
            .result
            .expect("public Map intent must carry a result slot")
            .1;
        let result = match self.pin_map_result(&change, value, cookie) {
            Ok(result) => result,
            Err(error) => {
                let permits = self.ledger().rollback(change);
                debug_assert!(permits.is_empty());
                fail_owned!(error, backing);
            }
        };
        let preflights = match Self::preflight_anonymous_install(
            self.tree.as_mut().expect("address space tree is live"),
            &change,
            &backing,
        ) {
            Ok(preflights) => preflights,
            Err(error) => {
                let permits = self.ledger().rollback(change);
                debug_assert!(permits.is_empty());
                fail_owned!(post_validate_error(error), backing);
            }
        };
        self.mark_table_transaction();
        Ok(MemoryChangePlan {
            change,
            preflights,
            backing: Some(backing),
            result: Some(result),
            backing_permits: Vec::new(),
            image_end: None,
            published_view: None,
            view_owner: None,
        })
    }

    /// 从已 reserve 的 change 取唯一 anonymous Install intent 并展开成逐 extent 的
    /// preflight。Running 与 Building 共用：两边的 backing 几何与 intent 形状相同。
    fn preflight_anonymous_install(
        tree: &mut TableTree<TableMem, LEVELS>,
        change: &PreparedChange,
        backing: &OwnedBacking,
    ) -> Result<Vec<TranslationPreflight>, SpaceError> {
        let (range, offset, protection) = match change.translation_intents() {
            [
                TranslationIntent::Install {
                    range,
                    backing:
                        BackingView::Anonymous {
                            identity: intent_identity,
                            offset,
                            ..
                        },
                    protection,
                },
            ] if *intent_identity == backing.identity => (*range, *offset, *protection),
            _ => panic!("anonymous Map planner returned an invalid translation plan"),
        };
        backing.preflight_install(tree, range, offset, protection)
    }

    /// 锁外取得的表页 owner 进入 PTE reservation。Running、Building 与 bootstrap
    /// 共用本函数：三者的差异已在 plan 阶段表达为字段，发布前的步骤完全相同。
    /// 任何失败都回滚账本并把摘出的 owner 交回调用者在 AddressSpace 锁外析构。
    pub(crate) fn complete_memory_change(
        &mut self,
        plan: MemoryChangePlan,
        funded: Vec<Vec<frame::FundedTableFrame>>,
    ) -> Result<PreparedMemoryChange, (SpaceError, ReclaimedTableFrames)> {
        let MemoryChangePlan {
            change,
            preflights,
            backing,
            result,
            backing_permits,
            image_end,
            published_view,
            view_owner,
        } = plan;
        let mut reclaimed = ReclaimedTableFrames {
            funded,
            translations: Vec::new(),
            failed_owners: None,
            backing,
            permits: Vec::new(),
            view_owner: None,
        };
        macro_rules! fail {
            ($error:expr) => {{
                reclaimed.permits = self.ledger().rollback(change);
                reclaimed.view_owner = view_owner;
                self.clear_table_transaction();
                return Err(($error, reclaimed));
            }};
        }
        if reclaimed.funded.len() != preflights.len() {
            fail!(SpaceError::NoFrame);
        }
        if reclaimed
            .translations
            .try_reserve_exact(preflights.len())
            .is_err()
        {
            fail!(SpaceError::NoFrame);
        }
        let mut table_outcomes = Vec::new();
        if table_outcomes.try_reserve_exact(preflights.len()).is_err() {
            fail!(SpaceError::NoFrame);
        }
        let mut retiring_views = Vec::new();
        if retiring_views
            .try_reserve_exact(change.retiring_object_capacity())
            .is_err()
        {
            fail!(SpaceError::NoFrame);
        }
        let mut token = match PreparedMemoryChange::allocate() {
            Ok(token) => token,
            Err(error) => fail!(error),
        };
        for index in 0..preflights.len() {
            let owners = core::mem::take(&mut reclaimed.funded[index]);
            match self.tt().prepare(preflights[index], owners) {
                Ok(translation) => reclaimed.translations.push(translation),
                Err(failure) => {
                    reclaimed.failed_owners = Some(failure.owners);
                    fail!(failure.error.into());
                }
            }
        }
        token.install(MemoryChangeReservation {
            change,
            translations: core::mem::take(&mut reclaimed.translations),
            table_outcomes,
            backing: reclaimed.backing.take(),
            result,
            backing_permits,
            image_end,
            published_view,
            view_owner,
            retiring_views,
        });
        Ok(token)
    }

    fn prepare_user_existing_change(
        &mut self,
        validated: memory_space::ValidatedChange,
        permits: Vec<WritePermit>,
    ) -> Result<MemoryChangePlan, ObjectMapFailure> {
        macro_rules! fail {
            ($error:expr, $permits:expr) => {{
                return Err(ObjectMapFailure {
                    error: $error,
                    permits: $permits,
                });
            }};
        }
        if let Err(error) = self.ensure_table_transaction_available() {
            fail!(error, permits);
        }
        let change = match self.ledger().reserve(validated, permits) {
            Ok(change) => change,
            Err(failure) => {
                let error = SpaceError::from(failure.error);
                let (_, _, permits) = failure.into_parts();
                fail!(error, permits);
            }
        };
        let mut preflights = Vec::new();
        if preflights
            .try_reserve_exact(change.translation_intents().len())
            .is_err()
        {
            let permits = self.ledger().rollback(change);
            fail!(SpaceError::NoFrame, permits);
        }
        for intent in change.translation_intents().iter().copied() {
            let preflight = match intent {
                TranslationIntent::Remove { range } => self
                    .tt()
                    .preflight_unmap(Vpn(range.start() / PAGE_SIZE), range.pages()),
                TranslationIntent::Protect { range, from, to } => self.tt().preflight_protect(
                    Vpn(range.start() / PAGE_SIZE),
                    range.pages(),
                    protection_flags(from),
                    protection_flags(to),
                ),
                TranslationIntent::Install { .. } => {
                    panic!("existing mapping change unexpectedly installs a PTE")
                }
            };
            match preflight {
                Ok(preflight) => preflights.push(preflight),
                Err(error) => {
                    let permits = self.ledger().rollback(change);
                    fail!(error.into(), permits);
                }
            }
        }
        self.mark_table_transaction();
        Ok(MemoryChangePlan {
            change,
            preflights,
            backing: None,
            result: None,
            backing_permits: Vec::new(),
            image_end: None,
            published_view: None,
            view_owner: None,
        })
    }

    /// Unmap 的 Validate：返回计划与它要求的 WritePermit 多重集。含 W 的 object view
    /// 被部分撤销时，存活片段是新铸造的区域，各自需要一枚新 permit——permit 只能在
    /// AddressSpace 锁外向对象取得，因此几何在这里定案、permit 在锁外补齐。
    fn validate_user_unmap(
        &mut self,
        range: LedgerPageRange,
    ) -> Result<(memory_space::ValidatedChange, Vec<PermitRequirement>), SystemCallError> {
        self.ensure_table_transaction_available()
            .map_err(post_validate_error)?;
        let validated = self
            .ledger()
            .validate_unmap(UnmapRequest {
                range,
                authority: RegionOwner::AddressSpace,
            })
            .map_err(public_change_error)?;
        let mut requirements = Vec::new();
        requirements
            .try_reserve_exact(validated.permit_requirements().len())
            .map_err(|_| SystemCallError::OutOfMemory)?;
        requirements.extend_from_slice(validated.permit_requirements());
        Ok((validated, requirements))
    }

    /// Protect 的 Validate：语义同 `validate_user_unmap`——降权/升权同样重铸区域，
    /// 结果含 W 的片段各自需要一枚新 permit。
    fn validate_user_protect(
        &mut self,
        range: LedgerPageRange,
        protection: Protection,
    ) -> Result<(memory_space::ValidatedChange, Vec<PermitRequirement>), SystemCallError> {
        self.ensure_table_transaction_available()
            .map_err(post_validate_error)?;
        let validated = self
            .ledger()
            .validate_protect(ProtectRequest {
                range,
                protection,
                authority: RegionOwner::AddressSpace,
            })
            .map_err(public_change_error)?;
        let mut requirements = Vec::new();
        requirements
            .try_reserve_exact(validated.permit_requirements().len())
            .map_err(|_| SystemCallError::OutOfMemory)?;
        requirements.extend_from_slice(validated.permit_requirements());
        Ok((validated, requirements))
    }

    fn write_map_payload(&self, prepared: &PreparedMemoryChange) {
        prepared
            .get()
            .result
            .as_ref()
            .expect("Map reservation lost its result")
            .write_payload();
    }

    pub(crate) fn rollback_memory_change(
        &mut self,
        prepared: PreparedMemoryChange,
    ) -> ReclaimedTableFrames {
        self.clear_table_transaction();
        let MemoryChangeReservation {
            change,
            translations,
            table_outcomes,
            backing,
            view_owner,
            result: _,
            backing_permits: _,
            image_end: _,
            published_view: _,
            retiring_views: _retiring_views,
        } = prepared.take();
        let permits = self.ledger().rollback(change);
        drop(table_outcomes);
        ReclaimedTableFrames {
            funded: Vec::new(),
            translations,
            failed_owners: None,
            backing,
            permits,
            view_owner,
        }
    }

    pub(crate) fn rollback_memory_change_plan(
        &mut self,
        plan: MemoryChangePlan,
    ) -> ReclaimedTableFrames {
        self.clear_table_transaction();
        let MemoryChangePlan {
            change,
            backing,
            view_owner,
            preflights: _,
            result: _,
            backing_permits: _,
            image_end: _,
            published_view: _,
        } = plan;
        let permits = self.ledger().rollback(change);
        ReclaimedTableFrames {
            funded: Vec::new(),
            translations: Vec::new(),
            failed_owners: None,
            backing,
            permits,
            view_owner,
        }
    }

    /// 不可逆线性化点。所有输出维度在这里汇合：结果 cookie（如有）先以 release
    /// 发布，ledger 与 PTE 批量发布，anonymous backing 入册，Building 映像终点
    /// 推进，object view 的 lease 交给调用者安装。本函数不得失败也不得分配。
    fn commit_inner(
        &mut self,
        prepared: PreparedMemoryChange,
    ) -> (PublishedSpaceChange, Option<ObjectMappingLease>) {
        let MemoryChangeReservation {
            change,
            translations,
            table_outcomes,
            backing,
            result,
            backing_permits,
            image_end,
            published_view,
            view_owner,
            retiring_views,
        } = prepared.take();
        assert!(
            self.table_transaction_active
                && translations
                    .iter()
                    .all(|translation| self.tt().prepared_is_current(translation)),
            "page-table transaction lost exclusive generation"
        );
        self.clear_table_transaction();
        if let Some(result) = &result {
            result.commit_cookie();
        }
        let committed = self.ledger().commit(change);
        let mut retiring_views = retiring_views;
        for fragment in committed.retiring_fragments() {
            let RegionKindView::Mapping {
                backing: BackingView::Object { object, .. },
                ..
            } = fragment.kind
            else {
                continue;
            };
            if retiring_views.iter().any(|view| view.object == object) {
                continue;
            }
            let index = self
                .views
                .binary_search_by_key(&object, |view| view.object)
                .expect("committed retiring fragment lost its view source");
            retiring_views.push(RetiringObjectView {
                object,
                core: Arc::clone(&self.views[index].core),
                _owner: None,
            });
        }
        let table_outcomes = self.tt().publish_batch(translations, table_outcomes);
        let published = self.ledger().publish(committed);
        if let Some(backing) = backing {
            self.backings.push(backing);
        }
        if let Some(view) = view_owner {
            self.install_view_owner(view);
        }
        if let Some(end) = image_end {
            self.image_end = self.image_end.max(end);
        }
        (
            PublishedSpaceChange {
                ledger: published,
                tables: PublishedTableChanges(table_outcomes),
                backing_permits,
                retiring_views,
            },
            published_view,
        )
    }

    /// 对象 view 的 complete：失败时把 WritePermit 从 reclaimed 里摘出，随错误一起
    /// 交回调用者，由它在 AddressSpace 锁外归还对象状态机。
    pub(crate) fn complete_object_change(
        &mut self,
        plan: MemoryChangePlan,
        funded: Vec<Vec<frame::FundedTableFrame>>,
    ) -> Result<PreparedMemoryChange, (ObjectMapFailure, ReclaimedTableFrames)> {
        match self.complete_memory_change(plan, funded) {
            Ok(prepared) => Ok(prepared),
            Err((error, mut reclaimed)) => {
                let permits = reclaimed.take_permits();
                Err((ObjectMapFailure { error, permits }, reclaimed))
            }
        }
    }

    /// 不发布新 view 的事务（匿名 Map/Unmap/Protect、object view 撤销）在此收敛：
    /// 断言事务确实没有待安装的 view 身份，避免调用点各自忽略输出。
    pub(crate) fn commit_change(&mut self, prepared: PreparedMemoryChange) -> PublishedSpaceChange {
        let (published, view) = self.commit_inner(prepared);
        assert!(
            view.is_none(),
            "memory change unexpectedly published a view lease"
        );
        published
    }

    /// 发布新 object view 的事务：Commit 同时交出 view 身份，由调用者安装到对象侧。
    pub(crate) fn commit_view_map(
        &mut self,
        prepared: PreparedMemoryChange,
    ) -> (PublishedSpaceChange, ObjectMappingLease) {
        let (published, view) = self.commit_inner(prepared);
        (
            published,
            view.expect("object view Map must publish its view lease"),
        )
    }

    /// Building/bootstrap 的固定 placement 匿名映射：backing 已绑定身份，image_end
    /// 只在映像区推进。与公开 Map 的差异只在 authority、结果槽与 placement，
    /// 发布之后的路径完全共用。
    fn plan_bound_anonymous(
        &mut self,
        vaddr: usize,
        protection: Protection,
        class: AnonymousClass,
        owner: RegionOwner,
        image_end: Option<usize>,
        backing: OwnedBacking,
    ) -> Result<MemoryChangePlan, BackingPlanFailure<SpaceError>> {
        macro_rules! fail {
            ($error:expr) => {{
                return Err(BackingPlanFailure::Owned($error, backing));
            }};
        }
        if let Err(error) = self.ensure_table_transaction_available() {
            fail!(error);
        }
        let len = match backing.pages.checked_mul(PAGE_SIZE) {
            Some(value) => value,
            None => fail!(SpaceError::BadSegment),
        };
        let validated = match self.ledger().validate_map(MapRequest {
            bytes: len,
            guard_before: 0,
            guard_after: 0,
            placement: MapPlacement::FixedEmpty {
                usable_start: vaddr,
            },
            current: protection,
            maximum: protection,
            owner,
            backing: MapBacking::Anonymous {
                identity: backing.identity,
                class,
            },
            result: None,
        }) {
            Ok(value) => value,
            Err(error) => fail!(SpaceError::from(error)),
        };
        let change = match self
            .ledger()
            .reserve(validated, Vec::new())
            .map_err(|failure| SpaceError::from(failure.error))
        {
            Ok(change) => change,
            Err(error) => fail!(error),
        };
        if self.backings.try_reserve(1).is_err() {
            let permits = self.ledger().rollback(change);
            debug_assert!(permits.is_empty());
            fail!(SpaceError::NoFrame);
        }
        let preflights = match Self::preflight_anonymous_install(
            self.tree.as_mut().expect("address space tree is live"),
            &change,
            &backing,
        ) {
            Ok(preflights) => preflights,
            Err(error) => {
                let permits = self.ledger().rollback(change);
                debug_assert!(permits.is_empty());
                fail!(error);
            }
        };
        self.mark_table_transaction();
        Ok(MemoryChangePlan {
            change,
            preflights,
            backing: Some(backing),
            result: None,
            backing_permits: Vec::new(),
            image_end,
            published_view: None,
            view_owner: None,
        })
    }

    /// Building 与 ELF 装载的匿名映射：在锁内铸造身份并绑定锁外取得的 backing。
    pub(crate) fn plan_bound_anonymous_mapping(
        &mut self,
        vaddr: usize,
        len: usize,
        protection: Protection,
        image_end: Option<usize>,
        prepared: PreparedBacking,
    ) -> Result<MemoryChangePlan, BackingPlanFailure<SpaceError>> {
        macro_rules! fail_prepared {
            ($error:expr) => {{
                return Err(BackingPlanFailure::Prepared($error, prepared));
            }};
        }
        if len == 0 || !vaddr.is_multiple_of(PAGE_SIZE) || !len.is_multiple_of(PAGE_SIZE) {
            fail_prepared!(SpaceError::BadSegment);
        }
        if prepared.pages != len / PAGE_SIZE {
            fail_prepared!(SpaceError::BadSegment);
        }
        let identity = match self.mint_backing() {
            Ok(value) => value,
            Err(error) => fail_prepared!(error),
        };
        let backing = prepared.bind(identity);
        // Building 可以建立最终可执行的初始映像；Running 匿名映射不能。
        let class = if protection == Protection::ReadExecute {
            AnonymousClass::InitialExecutable
        } else {
            AnonymousClass::Data
        };
        self.plan_bound_anonymous(
            vaddr,
            protection,
            class,
            RegionOwner::AddressSpace,
            image_end,
            backing,
        )
    }

    /// Building/bootstrap 的同步完成：目标尚不可运行，无 active hart 需要确认，
    /// 因而 reservation 立即 Commit 并空批收口，funded table owner 交给调用者锁外析构。
    pub(crate) fn complete_bound_mapping(
        &mut self,
        plan: MemoryChangePlan,
        funded: Vec<Vec<frame::FundedTableFrame>>,
    ) -> Result<PublishedTableChanges, (SpaceError, ReclaimedTableFrames)> {
        let prepared = self.complete_memory_change(plan, funded)?;
        let published = self.commit_change(prepared);
        Ok(self.finish_empty_published_change(published))
    }

    pub(crate) fn begin_retire_published_change(
        &mut self,
        published: PublishedSpaceChange,
    ) -> RetiringSpaceChange {
        let PublishedSpaceChange {
            ledger,
            tables,
            backing_permits,
            retiring_views,
        } = published;
        let synchronized = self.ledger().synchronize(ledger);
        let (retiring, batch) = self.ledger().begin_retire(synchronized);
        RetiringSpaceChange {
            ledger: Some(retiring),
            batch,
            tables,
            backing: None,
            backing_permits,
            retiring_views,
            tables_complete: false,
            ledger_complete: false,
        }
    }

    /// Commit 内安装 view 所有权：对象已被本空间引用时丢弃预留（自然退款）。
    /// 容量已在 plan 阶段预留，本函数不分配。
    fn install_view_owner(&mut self, prepared: PreparedObjectView) {
        let PreparedObjectView {
            object,
            core,
            permit,
        } = prepared;
        match self.views.binary_search_by_key(&object, |view| view.object) {
            Ok(_) => {
                drop(permit);
                drop(core);
            }
            Err(index) => self.views.insert(
                index,
                ObjectViewOwner {
                    object,
                    core,
                    _permit: permit,
                },
            ),
        }
    }

    /// 一个引用该对象的区域已退役。账本中不再有引用该对象的区域时交出 owner，
    /// 由调用者在锁外与本批 WritePermit 一并收束。
    ///
    /// 「是否仍有引用」直接问账本：区域切割与合并都只改变账本，owner 不另记计数。
    fn release_view_region(&mut self, object: ObjectId) -> Option<ObjectViewOwner> {
        let Ok(index) = self.views.binary_search_by_key(&object, |view| view.object) else {
            // 另一个已发布退役批次可能已经交出最后一个 owner；本批次持有的
            // RetiringObjectView::core 足以归还自己的 permit，不得跨批次回查或 panic。
            return None;
        };
        let referenced = self.ledger.as_ref().is_some_and(|ledger| {
            ledger.regions().any(|region| {
                matches!(
                    region.kind,
                    RegionKindView::Mapping {
                        backing: BackingView::Object { object: live, .. },
                        ..
                    } if live == object
                )
            })
        });
        if referenced {
            return None;
        }
        Some(self.views.remove(index))
    }

    /// 供退役路径在锁外把 WritePermit 归还来源对象。
    fn view_core(&self, object: ObjectId) -> Arc<super::memory_object::MemoryObjectCore> {
        let index = self
            .views
            .binary_search_by_key(&object, |view| view.object)
            .expect("write permit lost its view owner");
        Arc::clone(&self.views[index].core)
    }

    fn retire_backing_one(
        &mut self,
        identity: BackingId,
        offset: usize,
        bytes: usize,
        permits: &mut Vec<super::resources::BackingSlicePermit>,
    ) -> (BackingExtentOwner, usize) {
        // `table_transaction_active` 覆盖 backing mint→Commit，因此 push 顺序与单调
        // BackingId 一致，这里的对数查找成立。任何允许并发 Commit 的改动都必须先把
        // 本表改为有序插入或显式索引。
        let index = self
            .backings
            .binary_search_by_key(&identity, |backing| backing.identity)
            .expect("retiring anonymous fragment lost its owned backing");
        let (owner, pages) = self.backings[index].release_one(offset, bytes, permits);
        if self.backings[index].extents.is_empty() {
            self.backings.remove(index);
        }
        (owner, pages)
    }

    pub(crate) fn complete_retiring_change(
        &mut self,
        retiring: RetiringChange,
        batch: &RetireBatch,
    ) {
        let retired = self.ledger().finish_retire(retiring, batch);
        self.ledger().complete(retired);
    }

    /// Building/bootstrap 的空 retire 批次同步收口，并把 funded table owner
    /// 交给调用者在 AddressSpace 锁外析构。
    fn finish_empty_published_change(
        &mut self,
        published: PublishedSpaceChange,
    ) -> PublishedTableChanges {
        let change = self.begin_retire_published_change(published);
        assert!(change.batch.is_empty());
        let RetiringSpaceChange {
            ledger,
            batch,
            tables,
            backing,
            backing_permits,
            retiring_views,
            tables_complete,
            ledger_complete,
        } = change;
        debug_assert!(backing.is_none());
        debug_assert!(backing_permits.is_empty());
        debug_assert!(retiring_views.is_empty());
        debug_assert!(!tables_complete);
        debug_assert!(!ledger_complete);
        self.complete_retiring_change(ledger.expect("empty memory change completed twice"), &batch);
        tables
    }

    /// 从对象 backing 投影出的物理 span 序列建立 view 映射。
    ///
    /// `spans` 是 `ObjectBacking::project` 的输出（调用方在锁外取得，因为对象 backing
    /// 属于 MEMORY_OBJECT 锁阶）；单页 view 退化为长度为一。intent 携带 placement、
    /// guard、结果槽与 authority：进程自有 view 归 AddressSpace（可由普通 Unmap 撤销），
    /// object-owned lease 归对象（只能经对象关闭撤销）。权限真值随 `authorization` 从
    /// 对象状态机流出：含 W 的 view 必须同时交出等量 WritePermit。
    #[inline(never)]
    pub(crate) fn plan_object_map(
        &mut self,
        intent: &MapIntent,
        object_offset: usize,
        spans: &[(FrameNumber, usize)],
        authorization: ObjectViewAuthorization,
        permits: Vec<WritePermit>,
        view_owner: PreparedObjectView,
    ) -> Result<MemoryChangePlan, ObjectMapFailure> {
        macro_rules! fail {
            ($error:expr, $permits:expr) => {{
                return Err(ObjectMapFailure {
                    error: $error,
                    permits: $permits,
                });
            }};
        }
        if let Err(error) = self.ensure_table_transaction_available() {
            fail!(error, permits);
        }
        let protection = authorization.maximum();
        let pages: usize = spans.iter().map(|(_, pages)| *pages).sum();
        let bytes = match pages.checked_mul(PAGE_SIZE) {
            Some(bytes) if bytes != 0 => bytes,
            _ => fail!(SpaceError::BadSegment, permits),
        };
        if bytes != intent.bytes.next_multiple_of(PAGE_SIZE)
            || !object_offset.is_multiple_of(PAGE_SIZE)
        {
            fail!(SpaceError::BadSegment, permits);
        }
        let lease_key = match self.mint_lease() {
            Ok(lease) => lease,
            Err(error) => fail!(error, permits),
        };
        let owner = match intent.authority {
            MapAuthority::AddressSpace => RegionOwner::AddressSpace,
            MapAuthority::ObjectLease => RegionOwner::Lease(lease_key),
        };
        let object = authorization.object();
        let validated = match self.ledger().validate_map(Self::map_request(
            intent,
            owner,
            MapBacking::Object {
                authorization,
                offset: object_offset,
            },
        )) {
            Ok(validated) => validated,
            Err(error) => fail!(SpaceError::from(error), permits),
        };
        let layout = validated
            .map_result()
            .expect("object view validation must produce a layout");
        let range = layout.usable;
        let change = match self.ledger().reserve(validated, permits) {
            Ok(change) => change,
            Err(failure) => {
                let error = SpaceError::from(failure.error);
                let (_, _, permits) = failure.into_parts();
                fail!(error, permits);
            }
        };
        let region = change
            .mapped_region_key()
            .expect("object Map must reserve one usable region");
        // Commit 不得分配：view owner 表的容量在这里预留。
        if self.views.try_reserve(1).is_err() {
            let permits = self.ledger().rollback(change);
            fail!(SpaceError::NoFrame, permits);
        }
        let mut preflights = Vec::new();
        if preflights.try_reserve_exact(spans.len()).is_err() {
            let permits = self.ledger().rollback(change);
            fail!(SpaceError::NoFrame, permits);
        }
        let result = match intent.result {
            Some((_, cookie)) => {
                let value = MemoryMapResult {
                    usable_base: layout.usable.start() as u64,
                    usable_bytes: layout.usable.bytes() as u64,
                    reservation_base: layout.reservation.start() as u64,
                    reservation_bytes: layout.reservation.bytes() as u64,
                    reserved: [0; 3],
                    committed: 0,
                };
                match self.pin_map_result(&change, value, cookie) {
                    Ok(result) => Some(result),
                    Err(_) => {
                        let permits = self.ledger().rollback(change);
                        fail!(SpaceError::NoFrame, permits);
                    }
                }
            }
            None => None,
        };
        let mut cursor = range.start() / PAGE_SIZE;
        for (base, span_pages) in spans.iter().copied() {
            match self.tt().preflight_map(
                Vpn(cursor),
                span_pages,
                Ppn(base.0),
                protection_flags(protection),
            ) {
                Ok(preflight) => preflights.push(preflight),
                Err(error) => {
                    let permits = self.ledger().rollback(change);
                    fail!(error.into(), permits);
                }
            }
            cursor += span_pages;
        }
        self.mark_table_transaction();
        Ok(MemoryChangePlan {
            change,
            preflights,
            backing: None,
            result,
            backing_permits: Vec::new(),
            image_end: None,
            // lease 是地址空间之外的 owner 用来撤销该 view 的凭据。进程自有 view
            // 由账本区间与 AddressSpace authority 直接表达，不需要第二份身份。
            published_view: match intent.authority {
                MapAuthority::ObjectLease => Some(ObjectMappingLease {
                    lease: lease_key,
                    region,
                    range,
                    object,
                    object_offset,
                    protection,
                }),
                MapAuthority::AddressSpace => None,
            },
            view_owner: Some(view_owner),
        })
    }

    /// 撤销一个已发布 object view。lease 是 ledger 真值的复核凭据，不是第二份真值：
    /// 位置、身份与权限都要与账本当前状态一致，否则整笔请求在 Commit 前失败。
    #[inline(never)]
    pub(crate) fn plan_object_unmap(
        &mut self,
        lease: ObjectMappingLease,
    ) -> Result<MemoryChangePlan, SpaceError> {
        self.ensure_table_transaction_available()?;
        let matches_lease = self.ledger().regions().any(|region| {
            region.key == lease.region
                && region.range == lease.range
                && region.owner == RegionOwner::Lease(lease.lease)
                && matches!(
                    region.kind,
                    RegionKindView::Mapping {
                        backing: BackingView::Object {
                            object,
                            offset: region_offset,
                        },
                        current,
                        maximum,
                    } if object == lease.object
                        && region_offset == lease.object_offset
                        && current == lease.protection
                        && maximum == lease.protection
                )
        });
        if !matches_lease {
            return Err(SpaceError::BadSegment);
        }
        let validated = self
            .ledger()
            .validate_unmap(UnmapRequest {
                range: lease.range,
                authority: RegionOwner::Lease(lease.lease),
            })
            .map_err(SpaceError::from)?;
        let change = self
            .ledger()
            .reserve(validated, Vec::new())
            .map_err(|failure| SpaceError::from(failure.error))?;
        let mut preflights = Vec::new();
        if preflights.try_reserve_exact(1).is_err() {
            let permits = self.ledger().rollback(change);
            debug_assert!(permits.is_empty());
            return Err(SpaceError::NoFrame);
        }
        match self
            .tt()
            .preflight_unmap(Vpn(lease.range.start() / PAGE_SIZE), lease.range.pages())
        {
            Ok(preflight) => preflights.push(preflight),
            Err(error) => {
                let permits = self.ledger().rollback(change);
                debug_assert!(permits.is_empty());
                return Err(error.into());
            }
        }
        self.mark_table_transaction();
        Ok(MemoryChangePlan {
            change,
            preflights,
            backing: None,
            result: None,
            backing_permits: Vec::new(),
            image_end: None,
            published_view: None,
            view_owner: None,
        })
    }

    /// Building `ProcessMap` 的几何与权限解析。窗口不能由一次调用跨越；只有映像区
    /// 推进 StartupBlock/heap 基准，因此 image_end 由本函数一并判定。
    fn building_intent(
        vaddr: usize,
        len: usize,
        permissions: ProcessMapFlags,
    ) -> Result<(Protection, Option<usize>), SpaceError> {
        if len == 0
            || !vaddr.is_multiple_of(PAGE_SIZE)
            || !len.is_multiple_of(PAGE_SIZE)
            || !permissions.is_known()
            || permissions.raw() == 0
        {
            return Err(SpaceError::BadSegment);
        }
        let protection = process_protection(permissions)?;
        let end = vaddr.checked_add(len).ok_or(SpaceError::BadSegment)?;
        let stack_base = USER_TOP - STACK_SIZE;
        if end > USER_TOP || vaddr < stack_base && end > stack_base {
            return Err(SpaceError::BadSegment);
        }
        Ok((protection, (end <= stack_base).then_some(end)))
    }

    /// 取得 backing 前的预检；与 `plan_building_anonymous` 共用同一份 intent 判定。
    pub(crate) fn validate_building_anonymous(
        &mut self,
        vaddr: usize,
        len: usize,
        permissions: ProcessMapFlags,
    ) -> Result<(), SpaceError> {
        let (protection, _) = Self::building_intent(vaddr, len, permissions)?;
        self.ensure_table_transaction_available()?;
        let identity = BackingId::new(self.next_backing).ok_or(SpaceError::NoFrame)?;
        self.ledger()
            .validate_map(MapRequest {
                bytes: len,
                guard_before: 0,
                guard_after: 0,
                placement: MapPlacement::FixedEmpty {
                    usable_start: vaddr,
                },
                current: protection,
                maximum: protection,
                owner: RegionOwner::AddressSpace,
                backing: MapBacking::Anonymous {
                    identity,
                    class: if protection == Protection::ReadExecute {
                        AnonymousClass::InitialExecutable
                    } else {
                        AnonymousClass::Data
                    },
                },
                result: None,
            })
            .map_err(SpaceError::from)
            .map(|_| ())
    }

    pub(crate) fn plan_building_anonymous(
        &mut self,
        vaddr: usize,
        len: usize,
        permissions: ProcessMapFlags,
        prepared: PreparedBacking,
    ) -> Result<MemoryChangePlan, BackingPlanFailure<SpaceError>> {
        let (protection, image_end) = match Self::building_intent(vaddr, len, permissions) {
            Ok(intent) => intent,
            Err(error) => return Err(BackingPlanFailure::Prepared(error, prepared)),
        };
        self.plan_bound_anonymous_mapping(vaddr, len, protection, image_end, prepared)
    }

    /// 窗口不能由一次调用跨越；只有映像区推进 StartupBlock/heap 基准。

    /// Building-only 回填；先验证完整目标区间已映射，再经物理直映射写入，
    /// 不要求目标最终 PTE 可写。
    pub fn write_building(&mut self, target: usize, source: &[u8]) -> Result<(), SpaceError> {
        let end = target
            .checked_add(source.len())
            .ok_or(SpaceError::BadSegment)?;
        if end > USER_TOP {
            return Err(SpaceError::BadSegment);
        }
        if !source.is_empty() {
            for vpn in target / PAGE_SIZE..(end - 1) / PAGE_SIZE + 1 {
                let Some(mapping) = self.tt().translate(Vpn(vpn)) else {
                    return Err(SpaceError::BadSegment);
                };
                if mapping.flags & flags::U == 0 {
                    return Err(SpaceError::BadSegment);
                }
            }
        }

        let mut copied = 0;
        while copied < source.len() {
            let va = target + copied;
            let in_page = va % PAGE_SIZE;
            let count = (PAGE_SIZE - in_page).min(source.len() - copied);
            let mapping = self
                .tt()
                .translate(Vpn(va / PAGE_SIZE))
                .expect("prevalidated mapping");
            let pa = mapping.ppn.0 * PAGE_SIZE + in_page;
            // SAFETY: Building process 尚不可运行；目标映射完整验证且其 backing
            // 由本地址空间拥有。
            unsafe {
                core::ptr::copy_nonoverlapping(
                    source[copied..].as_ptr(),
                    mm::phys_to_virt(pa) as *mut u8,
                    count,
                );
            }
            copied += count;
        }
        Ok(())
    }

    pub fn validate_initial_context(
        &mut self,
        entry: usize,
        stack_pointer: usize,
    ) -> Result<(), SpaceError> {
        if stack_pointer == 0 || stack_pointer % 16 != 0 || self.image_end == 0 {
            return Err(SpaceError::BadSegment);
        }
        let entry_mapping = self
            .tt()
            .translate(Vpn(entry / PAGE_SIZE))
            .ok_or(SpaceError::BadSegment)?;
        let stack_mapping = self
            .tt()
            .translate(Vpn((stack_pointer - 1) / PAGE_SIZE))
            .ok_or(SpaceError::BadSegment)?;
        if entry_mapping.flags & (flags::U | flags::X) != (flags::U | flags::X)
            || stack_mapping.flags & (flags::U | flags::W) != (flags::U | flags::W)
        {
            return Err(SpaceError::BadSegment);
        }
        Ok(())
    }

    fn plan_elf_layout(
        &self,
        segments: &[elf::LoadSegment],
    ) -> Result<(Vec<(usize, usize, Protection)>, usize), SpaceError> {
        use alloc::collections::BTreeMap;
        let mut plan: BTreeMap<usize, u64> = BTreeMap::new();
        let mut top = 0usize;
        for seg in segments {
            if seg.filesz > seg.memsz {
                return Err(SpaceError::BadSegment);
            }
            let start = seg.vaddr as usize;
            if start % PAGE_SIZE != seg.offset as usize % PAGE_SIZE {
                return Err(SpaceError::BadSegment);
            }
            let end = start
                .checked_add(seg.memsz as usize)
                .ok_or(SpaceError::BadSegment)?;
            if end > USER_TOP {
                return Err(SpaceError::BadSegment);
            }
            let mut fl = flags::V | flags::U | flags::A;
            if seg.readable {
                fl |= flags::R;
            }
            if seg.writable {
                fl |= flags::W | flags::D;
            }
            if seg.executable {
                fl |= flags::X;
            }
            for vpn in start / PAGE_SIZE..end.div_ceil(PAGE_SIZE) {
                *plan.entry(vpn).or_insert(0) |= fl;
            }
            top = top.max(end);
        }
        if plan.values().any(|fl| {
            fl & flags::R == 0 && fl & (flags::W | flags::X) != 0
                || fl & (flags::W | flags::X) == (flags::W | flags::X)
        }) {
            return Err(SpaceError::BadSegment);
        }
        let mut runs = Vec::new();
        runs.try_reserve(plan.len())
            .map_err(|_| SpaceError::NoFrame)?;
        for (&vpn, &fl) in &plan {
            let protection = if fl & flags::X != 0 {
                Protection::ReadExecute
            } else if fl & flags::W != 0 {
                Protection::ReadWrite
            } else {
                Protection::ReadOnly
            };
            if let Some((_, end, previous)) = runs.last_mut() {
                if *end == vpn && *previous == protection {
                    *end += 1;
                    continue;
                }
            }
            runs.push((vpn, vpn + 1, protection));
        }
        Ok((
            runs.into_iter()
                .map(|(start, end, protection)| {
                    (start * PAGE_SIZE, (end - start) * PAGE_SIZE, protection)
                })
                .collect(),
            top.div_ceil(PAGE_SIZE) * PAGE_SIZE,
        ))
    }

    pub(crate) fn plan_elf_mappings(
        &mut self,
        segments: &[elf::LoadSegment],
    ) -> Result<(Vec<(usize, usize, Protection)>, usize), SpaceError> {
        self.plan_elf_layout(segments)
    }

    fn write_elf_segments(
        &mut self,
        segments: &[elf::LoadSegment],
        file: &[u8],
        image_end: usize,
    ) -> Result<(), SpaceError> {
        for seg in segments {
            let start = seg.offset as usize;
            let src = file
                .get(
                    start
                        ..start
                            .checked_add(seg.filesz as usize)
                            .ok_or(SpaceError::BadSegment)?,
                )
                .ok_or(SpaceError::BadSegment)?;
            self.write_building(seg.vaddr as usize, src)?;
        }
        self.image_end = self.image_end.max(image_end);
        Ok(())
    }

    pub(crate) fn write_elf_mappings(
        &mut self,
        segments: &[elf::LoadSegment],
        file: &[u8],
        image_end: usize,
    ) -> Result<(), SpaceError> {
        self.write_elf_segments(segments, file, image_end)
    }

    /// Bootstrap map 完成全部可失败工作后，把对外层 funded owner 的临时投影替换为
    /// owner 本体。init 尚未发布且本操作无分配，错配属于启动所有权不变量破坏。
    pub fn install_bootstrap_funding(&mut self, funded: frame::BootFundedExtent) {
        let backing = self
            .backings
            .last_mut()
            .expect("bootstrap backing disappeared before owner installation");
        let extent = backing
            .extents
            .iter_mut()
            .find(|extent| matches!(extent.owner, BackingExtentOwner::BootBorrowed { .. }))
            .expect("bootstrap borrowed extent disappeared before owner installation");
        let (expected_base, expected_pages) = match &extent.owner {
            BackingExtentOwner::BootBorrowed { base, pages } => (*base, *pages),
            _ => unreachable!(),
        };
        assert_eq!(
            funded.base(),
            expected_base,
            "bootstrap funded base changed"
        );
        assert_eq!(
            funded.pages(),
            expected_pages,
            "bootstrap funded length changed"
        );
        extent.owner = BackingExtentOwner::Boot(funded);
    }

    /// 校验用户区间 [ptr, ptr+len) 逐页可访问：不溢出、不出用户半区、
    /// 每页已映射且含 U 标志与所需方向权限（读 R / 写 W）。
    /// 供 [`crate::uaccess`] 前置校验；限长由调用方先行把关。
    pub(crate) fn check_range(
        &mut self,
        ptr: usize,
        len: usize,
        writable: bool,
    ) -> Result<(), crate::uaccess::AccessError> {
        use crate::uaccess::AccessError;
        let Some(end) = ptr.checked_add(len) else {
            return Err(AccessError::BadRange);
        };
        if end > USER_TOP || ptr >= USER_TOP && len == 0 {
            return Err(AccessError::BadRange);
        }
        let need = if writable { flags::W } else { flags::R };
        if len == 0 {
            return Ok(());
        }
        for vpn in ptr / PAGE_SIZE..(end - 1) / PAGE_SIZE + 1 {
            match self.tt().translate(Vpn(vpn)) {
                Some(m) => {
                    if m.flags & flags::U == 0 || m.flags & need == 0 {
                        return Err(AccessError::Permission);
                    }
                }
                None => return Err(AccessError::NotMapped),
            }
        }
        Ok(())
    }
}

impl BoundAddressSpace {
    /// 查询单页物理地址（跨地址空间完成路径用，见 [`crate::uaccess`]）；
    /// 页必须已映射。仅取地址，权限校验仍由 check_range 承担。
    pub(crate) fn page_pa(&mut self, va: usize) -> Option<usize> {
        self.tt()
            .translate(Vpn(va / PAGE_SIZE))
            .map(|m| m.ppn.0 * PAGE_SIZE)
    }

    /// 推进一笔已摘下的帧 extent 归还；分级库存归还具有地址位宽常数上界，
    /// 因此每个 extent 计一个 work unit，不再保存碎片链扫描游标。
    fn step_pending(&mut self, budget: usize) -> (usize, bool) {
        if self.pending_free.is_none() {
            return (0, true);
        }
        if budget == 0 {
            return (0, false);
        }
        let owner = self.pending_free.take().expect("pending owner disappeared");
        debug_assert!(self.retired.is_none(), "retired owner was not collected");
        self.retired = Some(owner);
        // caller 必须先释放 AddressSpace 锁并析构 retired owner，才能继续本批。
        (1, false)
    }

    /// 登记 page_table 已摘除的 affine owner，等待锁外归还。
    fn enqueue_table_owner(&mut self, owner: frame::FundedTableFrame) {
        debug_assert!(
            self.pending_free.is_none(),
            "pending free must be consumed before enqueuing"
        );
        self.pending_free = Some(RetiredSpaceResource::Table(owner));
    }

    /// 有界收束一批资源。Handle/PTE 检查、所有权摘除与 extent 归还各计一个
    /// work unit；每个 extent 的库存操作另有只依赖地址位宽和 DT region 上限的
    /// 结构常数界，因此单次执行量受 `budget` 线性约束。
    /// 仅在 REAPABLE 后（drain_gate 持有下）调用；返回 (work_done, complete)。
    pub fn drain(&mut self, budget: usize) -> (usize, bool) {
        let (work, complete) = self.drain_inner(budget);
        debug_assert!(
            work <= budget,
            "space drain over budget: {} > {} complete={}",
            work,
            budget,
            complete
        );
        (work, complete)
    }

    fn drain_inner(&mut self, budget: usize) -> (usize, bool) {
        debug_assert!(budget > 0);
        if self.drain_stage == DrainStage::Idle {
            // Handle 阶段先以 lease transaction 清除全部 object-owned region。
            self.drain_stage = DrainStage::Ledger;
        }
        let mut work = 0;

        // 在途归还最优先：完成后才允许推进任何阶段。
        if self.pending_free.is_some() {
            let (used, done) = self.step_pending(budget);
            work += used;
            if !done {
                return (work, false);
            }
            self.pending_free = None;
        }

        loop {
            match self.drain_stage {
                DrainStage::Idle | DrainStage::Done => {
                    return (work, self.drain_stage == DrainStage::Done);
                }
                DrainStage::Ledger => {
                    if work + 1 > budget {
                        return (work, false);
                    }
                    if let Some((fragment, permit)) = self.ledger().drain_one() {
                        work += 1;
                        // object view 区域在丢弃账本时同步释放对象引用与写许可；
                        // 两者都必须在 AddressSpace 锁外收束，因此走 pending 通道。
                        if let RegionKindView::Mapping {
                            backing: BackingView::Object { object, .. },
                            ..
                        } = fragment.kind
                        {
                            let core = self
                                .release_view_region(object)
                                .expect("draining object view lost its owner")
                                .core;
                            self.pending_free = Some(RetiredSpaceResource::View { core, permit });
                            let (used, done) = self.step_pending(budget - work);
                            work += used;
                            if !done {
                                return (work, false);
                            }
                            self.pending_free = None;
                        } else {
                            assert!(
                                permit.is_none(),
                                "anonymous region carried an object write permit"
                            );
                        }
                        continue;
                    }
                    assert!(
                        self.views.is_empty(),
                        "object view owners outlived their ledger regions"
                    );
                    drop(
                        self.ledger
                            .take()
                            .expect("address-space ledger must exist during drain"),
                    );
                    work += 1;
                    self.drain_stage = DrainStage::Backings;
                }
                DrainStage::Backings => {
                    if work + 1 > budget {
                        return (work, false);
                    }
                    let Some(backing) = self.backings.last_mut() else {
                        let cursor = self
                            .tree
                            .as_ref()
                            .expect("tree exists until Root stage completes")
                            .begin_drain();
                        self.drain_stage = DrainStage::Tables { cursor };
                        continue;
                    };
                    let extent = backing
                        .extents
                        .pop()
                        .expect("owned backing must contain an extent");
                    if backing.extents.is_empty() {
                        self.backings.pop();
                    }
                    work += 1;
                    self.pending_free = Some(RetiredSpaceResource::Backing(extent.owner));
                    let (used, done) = self.step_pending(budget - work);
                    work += used;
                    if !done {
                        return (work, false);
                    }
                    self.pending_free = None;
                }
                DrainStage::Tables { mut cursor } => {
                    if work >= budget {
                        self.drain_stage = DrainStage::Tables { cursor };
                        return (work, false);
                    }
                    let step = self
                        .tree
                        .as_mut()
                        .expect("tree exists until Root stage completes")
                        .drain_step(&mut cursor);
                    work += 1;
                    match step {
                        TableDrainStep::Progress => {
                            self.drain_stage = DrainStage::Tables { cursor };
                        }
                        TableDrainStep::Retired(owner) => {
                            self.drain_stage = DrainStage::Tables { cursor };
                            self.enqueue_table_owner(owner);
                            let (used, done) = self.step_pending(budget - work);
                            work += used;
                            if !done {
                                return (work, false);
                            }
                            self.pending_free = None;
                        }
                        TableDrainStep::Complete => {
                            self.drain_stage = DrainStage::Root;
                        }
                    }
                }
                DrainStage::Root => {
                    if work + 1 > budget {
                        return (work, false);
                    }
                    let tree = self
                        .tree
                        .take()
                        .expect("tree exists until Root stage completes");
                    let owner = tree.finish_drain();
                    let binding = self
                        .binding
                        .take()
                        .expect("Bound address space lost its PoolBinding");
                    debug_assert!(self.retired.is_none(), "retired owner was not collected");
                    self.retired = Some(RetiredSpaceResource::Root { owner, binding });
                    work += 1;
                    self.drain_stage = DrainStage::Done;
                    return (work, true);
                }
            }
        }
    }
}

/// 由 drain_gate 串行的 HandleTable 收束状态。pending entry 已推进表
/// 游标、尚待锁外 close；下一批必须优先消费它。
enum DrainFinalization {
    PublishDead,
    PropagateJob(super::job::CompletionCursor),
    Done,
}

struct DrainState {
    cursor: usize,
    pending_close: Option<super::handle::ProcessHandleEntry>,
    finalization: Option<DrainFinalization>,
}

/// 进程资源容器：地址空间、父子身份与进程本地 HandleTable。
///
/// 线程强持 Process；对象与 WaitContext 只在操作期间持线程或进程引用。
/// HandleTable drain 先摘项再执行对象 callback，避免生命周期回调反向进入表锁。
pub struct Process {
    pub pid: Pid,
    /// 仅用于诊断的创建关系；不产生管理、继承或回收权。
    pub parent: Pid,
    /// 创建域仅维持归属（weak；生命周期根是 Job 直接成员表）。
    job: alloc::sync::Weak<super::job::Job>,
    /// 页额度、metadata 与未来 CPU/设备预算的正交绑定容器。
    pub(crate) resources: super::resources::ProcessResources,
    pub space: AddressSpace,
    /// 新对象 ABI 的进程本地 Handle 表。
    pub(crate) handles: crate::sync::Spinlock<super::handle::ProcessHandleTable>,
    /// 生命周期状态机（顶级锁，见 lifecycle 模块锁序契约）。
    pub(crate) lifecycle: super::lifecycle::Lifecycle,
    /// 观察壳的 weak 回指（REAPABLE/Dead 发布触达；HandleTable 条目强持 shell）。
    control: crate::sync::Spinlock<Option<alloc::sync::Weak<super::process::ProcessControl>>>,
    /// Drain 并发批次仲裁（try_lock；持锁期间推进有界收束）。
    pub(crate) drain_gate: crate::sync::Spinlock<()>,
    /// HandleTable 收束游标与待关闭项（均由 drain_gate 串行）。
    drain_state: crate::sync::Spinlock<DrainState>,
    /// ProcessStart 提交点一次性冻结的执行绑定：非零域编号与执行需求；
    /// 0 唯一表示尚未绑定，避免 Base64 与哨兵重合。
    execution: AtomicUsize,
}

impl Drop for Process {
    fn drop(&mut self) {
        // 防御性兜底：预算恰在摘项后耗尽时，entry 已不在表中，必须先
        // 关闭它才能继续收束地址空间。
        if let Some(entry) = self.drain_state.get_mut().pending_close.take() {
            super::handle::close_entry_infallible(entry, self, true);
        }
        // 进程已无外部引用，唯一借用下逐项摘除；对象回调发生在表项
        // 已移除之后，且不持 HandleTable 锁。
        let mut cursor = 1;
        loop {
            let entry = self.handles.get_mut().take_next(&mut cursor);
            let Some(entry) = entry else { break };
            super::handle::close_entry_infallible(entry, self, true);
        }
    }
}

impl Process {
    pub(crate) fn new(
        pid: Pid,
        parent: Pid,
        job: alloc::sync::Weak<super::job::Job>,
        resources: super::resources::ProcessResources,
    ) -> Result<Self, SpaceError> {
        Ok(Self {
            pid,
            parent,
            job,
            resources,
            space: AddressSpace::unbound(),
            handles: crate::sync::Spinlock::chained(
                crate::sync::ranks::HANDLE_TABLE,
                pid,
                super::handle::ProcessHandleTable::new(),
            ),
            lifecycle: super::lifecycle::Lifecycle::building(),
            control: crate::sync::Spinlock::new(crate::sync::ranks::OBJECT_WAIT, None),
            drain_gate: crate::sync::Spinlock::new(crate::sync::ranks::DRAIN_GATE, ()),
            drain_state: crate::sync::Spinlock::new(
                crate::sync::ranks::DRAIN_CURSOR,
                DrainState {
                    cursor: 1,
                    pending_close: None,
                    finalization: None,
                },
            ),
            execution: AtomicUsize::new(0),
        })
    }

    /// 显式附入一条 Building 线程。syscall 与 bootstrap 共用此出生路径；
    /// 调用者负责持有 Building 操作登记，Start 只发布这里已存在的线程。
    pub(crate) fn attach_thread(
        self: &Arc<Self>,
        context: ThreadStartContext,
    ) -> Result<Tid, ThreadAttachError> {
        self.space
            .lock()
            .validate_initial_context(context.entry as usize, context.stack_pointer as usize)
            .map_err(ThreadAttachError::Context)?;
        self.lifecycle
            .attach_member(|tid, member| {
                let thread = Thread::new_thread(tid, member, self, context)
                    .map_err(|_| super::lifecycle::AttachFault::Oom)?;
                Arc::try_new(thread).map_err(|_| super::lifecycle::AttachFault::Oom)
            })
            .map_err(|fault| match fault {
                super::lifecycle::AttachFault::Closed => ThreadAttachError::Closed,
                super::lifecycle::AttachFault::Limit => ThreadAttachError::Limit,
                super::lifecycle::AttachFault::Oom => ThreadAttachError::Oom,
            })
    }

    /// 已登记 Building lease 的 Attach 提交；后到终止由 lifecycle 接管新线程。
    pub(crate) fn attach_thread_registered(
        self: &Arc<Self>,
        context: ThreadStartContext,
    ) -> Result<Tid, ThreadAttachError> {
        self.space
            .lock()
            .validate_initial_context(context.entry as usize, context.stack_pointer as usize)
            .map_err(ThreadAttachError::Context)?;
        let (tid, retired) = self
            .lifecycle
            .attach_registered_member(|tid, member| {
                let thread = Thread::new_thread(tid, member, self, context)
                    .map_err(|_| super::lifecycle::AttachFault::Oom)?;
                Arc::try_new(thread).map_err(|_| super::lifecycle::AttachFault::Oom)
            })
            .map_err(|fault| match fault {
                super::lifecycle::AttachFault::Closed => ThreadAttachError::Closed,
                super::lifecycle::AttachFault::Limit => ThreadAttachError::Limit,
                super::lifecycle::AttachFault::Oom => ThreadAttachError::Oom,
            })?;
        // 终止已截止时，线程从未进入容器；在 lifecycle 锁外消费接管资源。
        drop(retired);
        Ok(tid)
    }

    /// 冻结进程级执行绑定（需求 + 兼容域），不可重复。
    pub(crate) fn bind_execution(
        &self,
        requirement: elf::IsaRequirement,
        domain: &'static crate::sched::SchedDomain,
    ) {
        const REQUIREMENT_BIT: usize = 1;
        let requirement_bit = match requirement {
            elf::IsaRequirement::Base64 => 0,
            elf::IsaRequirement::D64 => REQUIREMENT_BIT,
        };
        let encoded = ((domain.index() + 1) << 1) | requirement_bit;
        self.execution
            .compare_exchange(0, encoded, Ordering::Release, Ordering::Relaxed)
            .expect("execution binding frozen twice");
    }

    fn execution(&self) -> usize {
        let execution = self.execution.load(Ordering::Acquire);
        assert_ne!(
            execution, 0,
            "process execution must be bound before dispatch"
        );
        execution
    }

    /// 执行需求（trap FP 档位判定）。
    pub fn requirement(&self) -> elf::IsaRequirement {
        if self.execution() & 1 == 0 {
            elf::IsaRequirement::Base64
        } else {
            elf::IsaRequirement::D64
        }
    }

    /// 域归属（enqueue/pick 路径）。
    pub fn domain(&self) -> &'static crate::sched::SchedDomain {
        let index = (self.execution() >> 1)
            .checked_sub(1)
            .expect("execution binding lost its scheduler domain");
        crate::sched::domain_by_index(index)
    }

    pub(crate) fn set_control(&self, control: alloc::sync::Weak<super::process::ProcessControl>) {
        let previous = self.control.lock().replace(control);
        debug_assert!(previous.is_none());
    }

    pub(crate) fn control(&self) -> Option<Arc<super::process::ProcessControl>> {
        self.control
            .lock()
            .as_ref()
            .and_then(alloc::sync::Weak::upgrade)
    }

    /// 取存活 ProcessControl shell；已消散则从 core 铸造新 shell，并在
    /// 铸造点重放已达成的电平——派生兑底由此接上 drain 入口。单一 shell
    /// 身份：铸造在 control 槽锁内完成，并发派生只会得到同一对象
    /// （两个 shell 的 wait 电平会分叉，绝不允许）。
    ///
    /// 电平重放含 Dead 补冻结：枚举先于移表的竞争窗口内 core 可能已
    /// Dead——只补 REAPABLE 会漏终态冻结，后续 Query 命中「dead 未
    /// 冻结」不变量升级失败。铸造路径上 snapshot 之后无并发 drain
    /// （无任何存活 shell 可持 MANAGE），两步判定无翻转窗口。
    pub(crate) fn revive_control(
        self: &Arc<Self>,
    ) -> Result<Arc<super::process::ProcessControl>, SystemCallError> {
        let control = {
            let mut slot = self.control.lock();
            if let Some(control) = slot.as_ref().and_then(alloc::sync::Weak::upgrade) {
                return Ok(control);
            }
            let control = super::process::ProcessControl::new(self)?;
            *slot = Some(Arc::downgrade(&control));
            control
        };
        let (state, reason, code) = self.lifecycle.snapshot();
        if state == ProcessState::Dead {
            control.publish_dead(self.pid, self.parent, reason, code);
        } else if self.lifecycle.is_reapable() {
            control.publish_reapable();
        }
        Ok(control)
    }

    /// 所属 Job（生命周期根保证成员存续期 upgrade 必须成功）。
    pub(crate) fn job(&self) -> Arc<super::job::Job> {
        self.job.upgrade().expect("process outlives its job")
    }

    /// 有界收束一批（drain_gate 持有下调用）：先 HandleTable（对象 close
    /// 回调锁外执行，仍可用地址空间解除外部映射），后 AddressSpace，再推进
    /// 持久化终段。终段把 `publish_dead`、Job 成员摘除和祖先 CLOSED 传播
    /// 纳入同一预算；返回 Complete 前这些责任必须全部交付。
    pub(crate) fn drain_batch(&self, budget: usize) -> (usize, bool) {
        debug_assert!(budget > 0);
        let mut work = 0;

        // 先关闭上一批在预算边界摘出的项。该项的扫描已计入前一批，当前
        // 只消耗一次 close callback work unit。
        let pending = self.drain_state.lock().pending_close.take();
        if let Some(entry) = pending {
            let result = super::handle::close_entry(entry, self, true);
            work += 1;
            if let Err(entry) = result {
                self.drain_state.lock().pending_close = Some(entry);
                return (work, false);
            }
            if work == budget {
                return (work, false);
            }
        }

        while work < budget {
            let finalization = self.drain_state.lock().finalization.take();
            if let Some(finalization) = finalization {
                match finalization {
                    DrainFinalization::PublishDead => {
                        let control = self
                            .control()
                            .expect("reapable process must retain a control shell");
                        let (_state, reason, code) = self.lifecycle.snapshot();
                        control.publish_dead(self.pid, self.parent, reason, code);
                        self.lifecycle.mark_dead();
                        let next = self
                            .job()
                            .remove_member(self.pid)
                            .map(DrainFinalization::PropagateJob)
                            .unwrap_or(DrainFinalization::Done);
                        self.drain_state.lock().finalization = Some(next);
                    }
                    DrainFinalization::PropagateJob(mut cursor) => {
                        if !cursor.advance() {
                            self.drain_state.lock().finalization =
                                Some(DrainFinalization::PropagateJob(cursor));
                        } else {
                            self.drain_state.lock().finalization = Some(DrainFinalization::Done);
                        }
                    }
                    DrainFinalization::Done => {
                        self.drain_state.lock().finalization = Some(DrainFinalization::Done);
                        return (work, true);
                    }
                }
                work += 1;
                continue;
            }

            // 本次扫描可用全部剩余预算；若恰好摘到 entry 而已无 close
            // 预算，就把它持久化为 pending。游标已经推进，下一批必先 close。
            let (outcome, scanned) = {
                let mut state = self.drain_state.lock();
                let before = state.cursor;
                let outcome = self
                    .handles
                    .lock()
                    .take_next_bounded(&mut state.cursor, budget - work);
                (outcome, state.cursor - before)
            };
            work += scanned;
            match outcome {
                super::handle::TakeNext::Entry(entry) if work == budget => {
                    self.drain_state.lock().pending_close = Some(entry);
                    return (work, false);
                }
                super::handle::TakeNext::Entry(entry) => {
                    let result = super::handle::close_entry(entry, self, true);
                    work += 1;
                    if let Err(entry) = result {
                        self.drain_state.lock().pending_close = Some(entry);
                        return (work, false);
                    }
                }
                super::handle::TakeNext::Progress => return (work, false),
                super::handle::TakeNext::Exhausted if work == budget => return (work, false),
                super::handle::TakeNext::Exhausted => {
                    let ((space_work, complete), retired) = {
                        let mut space = self.space.lock();
                        let result = space.drain(budget - work);
                        let retired = space.take_retired();
                        (result, retired)
                    };
                    if let Some(retired) = retired {
                        retired.release();
                    }
                    if complete {
                        self.drain_state.lock().finalization = Some(DrainFinalization::PublishDead);
                    }
                    work += space_work;
                    if work == budget {
                        return (work, false);
                    }
                    continue;
                }
            }
        }
        (work, false)
    }
}

/// 线程：执行容器（用户现场 + 调度观测计数）。执行需求是进程级属性
/// （ELF 判定，Building 期冻结于 Process.requirement），线程经 process
/// 间接持有——同一进程的线程共享同一执行需求。
pub struct Thread {
    /// 进程内线程号（ABI 身份；tid 从 1 起，0 保留为非身份值）。
    pub tid: Tid,
    member: super::lifecycle::MemberKey,
    pub process: Arc<Process>,
    frame: UnsafeCell<UserContext>,
    departure: Arc<super::thread::ThreadDeparture>,
    normal_exit: AtomicBool,
    exit_code: AtomicI64,
}

// SAFETY: UserContext 只在两种互斥状态下被访问：线程在本 hart 执行/
// 挂起期间（trap 路径与 dispatcher 经执行点独占写）；或线程已无容器
// （Waiting：发布时序保证完成方只见已离开一切 hart 引用的线程，见
// sched::run 的 Park 发布分支）。其余字段原子或只读。
unsafe impl Sync for Thread {}

impl Thread {
    /// 创建线程执行基底：sepc = entry，sp = stack_pointer，a0/a1 = 出生
    /// 参数（首线程为出生块地址与长度，见 rinlib 启动契约）。FP 状态
    /// 创建即全零——不存在依赖 hart 残留的 valid 状态。tid 由
    /// lifecycle 锁内的 attach_member 分配并注入（构造随闭包进入锁内，
    /// Arc 分配取 HEAP 锁为 LIFECYCLE→HEAP 合法秩）。
    pub(super) fn new_thread(
        tid: Tid,
        member: super::lifecycle::MemberKey,
        process: &Arc<Process>,
        context: ThreadStartContext,
    ) -> Result<Self, ()> {
        Self::new_thread_with_control(tid, member, process, context, None)
    }

    pub(super) fn new_thread_with_control(
        tid: Tid,
        member: super::lifecycle::MemberKey,
        process: &Arc<Process>,
        context: ThreadStartContext,
        control: Option<&Arc<super::thread::ThreadControl>>,
    ) -> Result<Self, ()> {
        let departure = super::thread::ThreadDeparture::new(process, member, control)?;
        let mut ctx = UserContext::zeroed();
        ctx.sepc = context.entry;
        ctx.x[2] = context.stack_pointer;
        ctx.x[10] = context.arg1; // a0
        ctx.x[11] = context.arg2; // a1
        Ok(Self {
            tid,
            member,
            process: process.clone(),
            frame: UnsafeCell::new(ctx),
            departure,
            normal_exit: AtomicBool::new(false),
            exit_code: AtomicI64::new(0),
        })
    }

    pub fn frame_ptr(&self) -> *mut UserContext {
        self.frame.get()
    }

    pub(crate) fn mark_normal_exit(&self, code: i64) {
        self.exit_code.store(code, Ordering::Relaxed);
        assert!(
            self.normal_exit
                .compare_exchange(false, true, Ordering::Release, Ordering::Relaxed)
                .is_ok(),
            "thread normal exit recorded twice"
        );
    }

    pub(crate) fn departure_kind(&self) -> super::thread::DepartureKind {
        if self.normal_exit.load(Ordering::Acquire) {
            super::thread::DepartureKind::Normal(self.exit_code.load(Ordering::Relaxed))
        } else {
            super::thread::DepartureKind::Terminated
        }
    }

    pub(crate) const fn member(&self) -> super::lifecycle::MemberKey {
        self.member
    }

    pub(crate) fn departure(&self) -> Arc<super::thread::ThreadDeparture> {
        self.departure.clone()
    }

    pub(crate) fn result_obligation(&self) -> super::thread::ThreadResultObligation {
        self.departure.acquire_result()
    }

    /// pre-sret FP 档位：D64 进程完整恢复，Base 恒 FS=Off。
    pub fn uses_fp(&self) -> bool {
        self.process.requirement() == elf::IsaRequirement::D64
    }

    /// 用户 satp（进程地址空间不变，直接读缓存）。
    pub fn satp(&self) -> usize {
        self.process.space.lock().satp()
    }
}

/// 启动期覆盖「Attach 先登记、终止后截止、提交资源由终止接管」的确定性 seam。
pub(crate) fn building_cutoff_selftest() {
    let process = Arc::new(
        Process::new(
            0,
            0,
            alloc::sync::Weak::new(),
            super::resources::ProcessResources::try_new()
                .expect("Building cutoff self-test sponsor failed"),
        )
        .expect("Building cutoff self-test process failed"),
    );
    assert!(
        process.lifecycle.enter_building_op(),
        "Building cutoff self-test lease failed"
    );
    let todo = process
        .lifecycle
        .request_termination(ProcessExitReason::Killed, 0, None);
    assert!(
        !todo.reapable,
        "registered Building operation must delay termination"
    );
    let (tid, retired) = process
        .lifecycle
        .attach_registered_member(|tid, member| {
            Arc::try_new(
                Thread::new_thread(
                    tid,
                    member,
                    &process,
                    ThreadStartContext {
                        entry: 0,
                        stack_pointer: 0,
                        arg1: 0,
                        arg2: 0,
                    },
                )
                .map_err(|_| super::lifecycle::AttachFault::Oom)?,
            )
            .map_err(|_| super::lifecycle::AttachFault::Oom)
        })
        .expect("registered Attach must retain commit eligibility after cutoff");
    assert_eq!(tid, 1, "registered Attach must consume one thread identity");
    assert!(
        retired.is_some(),
        "termination must take over a post-cutoff Attach resource"
    );
    assert_eq!(
        process.lifecycle.member_count(),
        0,
        "post-cutoff Attach must not leave a Staging member"
    );
    drop(retired);
    assert!(
        process.lifecycle.leave_building_op(),
        "post-cutoff Attach completion must make the empty process reapable"
    );
}

/// Bind 已成功但尚未发布为可启动进程的唯一 owner。其生命周期内地址空间
/// 不允许通过普通 `Drop` 直接析构；失败必须显式走有界 drain，收束全部页表、
/// ledger、backing 与 PoolBinding。
pub struct UnpublishedBound {
    process: Option<Arc<Process>>,
}

impl UnpublishedBound {
    fn new(process: Arc<Process>) -> Self {
        Self {
            process: Some(process),
        }
    }

    fn publish(mut self) -> Arc<Process> {
        self.process
            .take()
            .expect("unpublished bound owner already consumed")
    }

    fn rollback(mut self) {
        let process = self
            .process
            .take()
            .expect("unpublished bound owner already consumed");
        // 先冻结 Building，摘除仍处于 Staging 的线程强引用；否则
        // Thread → Process 的环会使地址空间无法进入最终 drain。
        process
            .lifecycle
            .request_termination(ProcessExitReason::Killed, 0, None);
        while let Some(thread) = process.lifecycle.take_first_staging() {
            drop(thread);
        }
        // Bootstrap 构造期没有 Running 线程和 mandatory operation；地址空间
        // 收束可由同一有界 drain 机制完成，不能把 TableTree::Drop 当 rollback。
        let _gate = process.drain_gate.lock();
        loop {
            let (_, complete) = process.drain_batch(16);
            if complete {
                break;
            }
        }
        drop(_gate);
        drop(process);
    }
}

impl Drop for UnpublishedBound {
    fn drop(&mut self) {
        let Some(process) = self.process.take() else {
            return;
        };
        // 未发布 owner 的 Drop 只表示失败收束，不表示成功析构；它执行
        // 与显式 rollback 相同的有界 drain，避免 TableTree::Drop 旁路。
        process
            .lifecycle
            .request_termination(ProcessExitReason::Killed, 0, None);
        while let Some(thread) = process.lifecycle.take_first_staging() {
            drop(thread);
        }
        let _gate = process.drain_gate.lock();
        loop {
            let (_, complete) = process.drain_batch(16);
            if complete {
                break;
            }
        }
        drop(_gate);
    }
}

/// launch 前的进程骨架：ELF 已装载、执行需求已判定、栈已映射、
/// 尚未附线程或入表 runnable。
pub struct SpawnedProcess {
    process: Arc<Process>,
    bound: Option<UnpublishedBound>,
    entry: usize,
    requirement: elf::IsaRequirement,
    root_pool: Arc<super::memory_pool::MemoryPool>,
}

pub fn spawn_from_elf(
    pid: Pid,
    parent: Pid,
    job: alloc::sync::Arc<super::job::Job>,
    image: &elf::Elf,
    file: &[u8],
    root_pool: Arc<super::memory_pool::MemoryPool>,
) -> Result<SpawnedProcess, SpaceError> {
    // 执行需求由 ELF `e_flags` 与 `.riscv.attributes` 判定；F-only/Q/V/
    // TSO/未建模状态扩展在 load 时明确拒绝，不降级为 Base。
    let requirement = elf::isa_requirement(file).expect("userspace execution requirement rejected");
    let process = Arc::new(Process::new(
        pid,
        parent,
        alloc::sync::Arc::downgrade(&job),
        super::resources::ProcessResources::bootstrap(),
    )?);
    super::process::bind_memory_internal(&process, Arc::clone(&root_pool)).map_err(|error| {
        match error {
            SystemCallError::QuotaExceeded => SpaceError::QuotaExceeded,
            SystemCallError::ReachLimit => SpaceError::ReachLimit,
            _ => SpaceError::NoFrame,
        }
    })?;
    let bound = UnpublishedBound::new(Arc::clone(&process));
    if let Err(error) = process.space.load_elf(&image.segments, file) {
        bound.rollback();
        return Err(error);
    }
    if let Err(error) = process.space.map_stack() {
        bound.rollback();
        return Err(error);
    }
    Ok(SpawnedProcess {
        process,
        bound: Some(bound),
        entry: image.entry as usize,
        requirement,
        root_pool,
    })
}

/// Bootstrap launch 事务：为 init 预留真实 Handle → 构造 prefix 并把
/// BootPackage payload 借入同一 StartupBlock VA → 原子安装 Handle → 创建
/// 主线程并加入 root Job 成员表。普通 ProcessStart 走 `task::process` 的 copied payload。
///
/// 失败全量回滚：临时 Handle 数值随 reservation 作废，输入 entries 按目标
/// 进程退出语义关闭，Job 成员表不出现半初始化项。W^X 发布边界是后续
/// `sched::enqueue` 的 Release。
pub fn launch_bootstrap(
    spawned: SpawnedProcess,
    payload_extent: Option<frame::BootHeldExtent>,
    payload: &[u8],
    handles: Vec<super::handle::ProcessHandleEntry>,
) -> Result<crate::sched::AdmittedThread, SpaceError> {
    let SpawnedProcess {
        process,
        mut bound,
        entry,
        requirement,
        root_pool,
    } = spawned;
    let unpublished = bound
        .take()
        .expect("spawned bootstrap process lost unpublished bound owner");

    // init 同样获得 Building 起即存在的 ProcessControl（完整 rights，
    // 显式自杀/查询可用；无结构特例）。
    let control = super::process::ProcessControl::new(&process).map_err(|_| SpaceError::NoFrame)?;
    process.set_control(alloc::sync::Arc::downgrade(&control));
    let control_handle = super::handle::entry(
        super::process::ProcessControl::object_ref(&control),
        super::object::HandleRole::ProcessControl,
        erhino_shared::object::Rights::READ
            | erhino_shared::object::Rights::WAIT
            | erhino_shared::object::Rights::MANAGE
            | erhino_shared::object::Rights::DUPLICATE
            | erhino_shared::object::Rights::TRANSIT
            | erhino_shared::object::Rights::GRANT,
    )
    .map_err(|_| SpaceError::NoFrame)?;

    let root_pool_handle = super::handle::entry(
        super::memory_pool::MemoryPool::object_ref(&root_pool),
        super::object::HandleRole::MemoryPool,
        erhino_shared::object::Rights::CREATE
            | erhino_shared::object::Rights::READ
            | erhino_shared::object::Rights::DUPLICATE
            | erhino_shared::object::Rights::TRANSIT
            | erhino_shared::object::Rights::GRANT,
    )
    .map_err(|_| SpaceError::NoFrame)?;

    let mut handles = handles;
    handles.try_reserve(2).map_err(|_| SpaceError::NoFrame)?;
    handles.push(control_handle);
    handles.push(root_pool_handle);
    assert_eq!(
        handles.len(),
        erhino_shared::startup::initial::HANDLE_COUNT,
        "initial capability graph has an unexpected handle count"
    );

    let token = super::handle::transaction_token();
    let reservation = {
        let mut table = process.handles.lock();
        match table.reserve(handles.len(), token) {
            Ok(reservation) => reservation,
            Err(_) => {
                drop(table);
                for handle in handles {
                    super::handle::close_entry_infallible(handle, &process, true);
                }
                return Err(SpaceError::NoFrame);
            }
        }
    };

    let block = match erhino_shared::startup::build_startup_prefix(
        process.pid,
        process.parent,
        reservation.handles(),
        PAGE_SIZE,
        payload.len(),
    ) {
        Ok(block) => block,
        Err(error) => {
            process
                .handles
                .lock()
                .rollback(reservation)
                .expect("launch reservation must remain owned");
            for handle in handles {
                super::handle::close_entry_infallible(handle, &process, true);
            }
            return Err(match error {
                erhino_shared::startup::StartupBuildError::Overflow => SpaceError::BadSegment,
                erhino_shared::startup::StartupBuildError::AllocationFailed => SpaceError::NoFrame,
            });
        }
    };

    let binding_pool = {
        let space = process.space.lock();
        Arc::clone(space.pool())
    };
    debug_assert!(
        Arc::ptr_eq(&binding_pool, &root_pool),
        "bootstrap binding and delivered root Pool diverged"
    );
    // 先建立同时持物理 owner 与 Pool charge 的 prepared owner。后续映射可失败，
    // 但始终只借用该 owner；映射成功后的安装是无分配、不可失败的 owner 移交。
    let payload_funded = match payload_extent {
        Some(extent) => match frame::fund_boot_held(&binding_pool, extent) {
            Ok(funded) => Some(funded),
            Err(_) => {
                process
                    .handles
                    .lock()
                    .rollback(reservation)
                    .expect("launch reservation must remain owned");
                for handle in handles {
                    super::handle::close_entry_infallible(handle, &process, true);
                }
                return Err(SpaceError::NoFrame);
            }
        },
        None if payload.is_empty() => None,
        None => {
            process
                .handles
                .lock()
                .rollback(reservation)
                .expect("launch reservation must remain owned");
            for handle in handles {
                super::handle::close_entry_infallible(handle, &process, true);
            }
            return Err(SpaceError::BadSegment);
        }
    };

    let block_len = block.len() + payload.len();
    let block_va =
        match process
            .space
            .map_bootstrap_block(&block, payload_funded.as_ref(), payload.len())
        {
            Ok(va) => va,
            Err(error) => {
                process
                    .handles
                    .lock()
                    .rollback(reservation)
                    .expect("launch reservation must remain owned");
                for handle in handles {
                    super::handle::close_entry_infallible(handle, &process, true);
                }
                return Err(error);
            }
        };
    if let Some(funded) = payload_funded {
        process.space.lock().install_bootstrap_funding(funded);
    }

    // Commit 前把 Bootstrap 的所有可失败工作收拢：execution domain、Ready
    // 批次容量、Attach、Job member 与 Building operation 都在 Handle commit
    // 之前完成。Handle commit 之后只保留固定容量的不可失败发布序列。
    let domain = match crate::sched::resolve_domain(requirement) {
        Some(domain) => domain,
        None => {
            process
                .handles
                .lock()
                .rollback(reservation)
                .expect("launch reservation must remain owned");
            for handle in handles {
                super::handle::close_entry_infallible(handle, &process, true);
            }
            return Err(SpaceError::BadSegment);
        }
    };
    let mut ready_batch = match domain.reserve_ready(1) {
        Ok(batch) => batch,
        Err(()) => {
            process
                .handles
                .lock()
                .rollback(reservation)
                .expect("launch reservation must remain owned");
            for handle in handles {
                super::handle::close_entry_infallible(handle, &process, true);
            }
            return Err(SpaceError::NoFrame);
        }
    };
    let mut staged = Vec::new();
    if staged.try_reserve_exact(1).is_err() {
        process
            .handles
            .lock()
            .rollback(reservation)
            .expect("launch reservation must remain owned");
        for handle in handles {
            super::handle::close_entry_infallible(handle, &process, true);
        }
        return Err(SpaceError::NoFrame);
    }
    match process.attach_thread(ThreadStartContext {
        entry: entry as u64,
        stack_pointer: USER_TOP as u64,
        arg1: block_va as u64,
        arg2: block_len as u64,
    }) {
        Ok(_) => {}
        Err(ThreadAttachError::Context(error)) => {
            process
                .handles
                .lock()
                .rollback(reservation)
                .expect("launch reservation must remain owned");
            for handle in handles {
                super::handle::close_entry_infallible(handle, &process, true);
            }
            return Err(error);
        }
        Err(ThreadAttachError::Oom) => {
            process
                .handles
                .lock()
                .rollback(reservation)
                .expect("launch reservation must remain owned");
            for handle in handles {
                super::handle::close_entry_infallible(handle, &process, true);
            }
            return Err(SpaceError::NoFrame);
        }
        Err(ThreadAttachError::Closed | ThreadAttachError::Limit) => {
            unreachable!("bootstrap attach must target an empty Building process")
        }
    }
    let job = process.job();
    let member = match job.reserve_member(process.pid) {
        Ok(member) => member,
        Err(_) => {
            process
                .handles
                .lock()
                .rollback(reservation)
                .expect("launch reservation must remain owned");
            for handle in handles {
                super::handle::close_entry_infallible(handle, &process, true);
            }
            return Err(SpaceError::NoFrame);
        }
    };
    assert!(
        process.lifecycle.enter_building_op(),
        "bootstrap process cannot be terminating"
    );

    // 唯一不可逆提交段：Handle、Job member、Building→Running 和 execution
    // binding 均在此后只调用无失败尾段。
    process
        .handles
        .lock()
        .commit(reservation, handles)
        .expect("launch reservation count matches entries");
    job.commit_member(member, process.clone());
    process
        .lifecycle
        .begin_running(1, &mut staged)
        .expect("bootstrap process cannot be terminating");
    process.bind_execution(requirement, domain);
    let thread = staged.pop().expect("bootstrap staging thread missing");
    drop(unpublished.publish());
    Ok(ready_batch.admit(thread))
}
