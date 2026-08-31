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
    AddressRange, AnonymousClass, BackingId, BackingView, ChangeError, LeaseKey, Limits,
    MapBacking, MapPlacement, MapRequest, MemorySpace, ObjectId, ObjectViewAuthorization,
    PageRange as LedgerPageRange, PreparedChange, ProtectRequest, Protection, PublishedChange,
    RegionKey, RegionKindView, RegionOwner, RetireBatch, RetiredChange, TranslationIntent,
    UnmapRequest, WritePermit,
};
use page_table::{
    FrameMemory, FrameNumber, MapError, Ppn, PreparedTranslation, ReservedTableFrame,
    RootSlotState, SlotState, TableTree, Vpn, flags,
};

use crate::{
    context::UserContext,
    frame::{self, FrameTracker},
    mm,
};

/// 页大小（字节）。
pub const PAGE_SIZE: usize = erhino_shared::proc::PROCESS_PAGE_SIZE;
const _: () = assert!(PAGE_SIZE == 1 << page_table::PAGE_BITS);

/// 用户半区顶（256GiB），主线程栈顶。
pub const USER_TOP: usize = erhino_shared::proc::PROCESS_USER_TOP;

/// 主线程栈大小（8MiB），钉在半区顶。
pub const STACK_SIZE: usize = erhino_shared::proc::PROCESS_MAIN_STACK_SIZE;

/// sv39 三级页表。
const LEVELS: usize = 3;

/// 进程地址空间构建/操作错误。
#[derive(Debug)]
pub enum SpaceError {
    /// 帧或表帧耗尽。
    NoFrame,
    /// 段未页对齐 / 参数非法。
    BadSegment,
    /// 映射冲突（重复装载同一区间）。
    Conflict,
    /// 与其它在途 MemoryChange footprint 冲突。
    Busy,
    /// Building 空壳尚未附入 MemoryPool/TranslationTree。
    Unbound,
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

enum TableFrameToken {
    /// root 物理页由 BoundAddressSpace 的 PoolBinding 保活；TableTree 只借用几何。
    BorrowedRoot(FrameNumber),
    /// 切片 6 前的过渡中间表帧，仍由 TableTree 直接拥有。
    Owned(FrameTracker),
}

impl ReservedTableFrame for TableFrameToken {
    fn number(&self) -> FrameNumber {
        match self {
            Self::BorrowedRoot(frame) => *frame,
            Self::Owned(tracker) => FrameNumber(tracker.base().addr() / PAGE_SIZE),
        }
    }

    fn commit(self) -> FrameNumber {
        match self {
            Self::BorrowedRoot(frame) => frame,
            Self::Owned(tracker) => tracker.into_table_frame(),
        }
    }
}

/// [`TableTree`] 的帧来源。root 只借用 PoolBinding 的 funded frame；中间表在
/// 切片 6 全面资金化前继续走登记过的 transitional raw adapter。
struct TableMem {
    borrowed_root: FrameNumber,
    initial_root: Option<FrameNumber>,
}

impl TableMem {
    fn new(root: FrameNumber) -> Self {
        Self {
            borrowed_root: root,
            initial_root: Some(root),
        }
    }
}

impl FrameMemory for TableMem {
    type ReservedFrame = TableFrameToken;

    fn reserve_frame(&mut self) -> Result<Self::ReservedFrame, page_table::FrameExhausted> {
        if let Some(root) = self.initial_root.take() {
            return Ok(TableFrameToken::BorrowedRoot(root));
        }
        frame::alloc_user_order(0)
            .map(TableFrameToken::Owned)
            .ok_or(page_table::FrameExhausted)
    }

    fn free_frame(&mut self, frame: FrameNumber) {
        if frame == self.borrowed_root {
            return;
        }
        // SAFETY: FrameMemory 只回传此前由 Owned token 唯一移交给该树的表帧。
        drop(unsafe { FrameTracker::adopt_table_frame(frame) });
    }

    fn table_mut(&mut self, frame: FrameNumber) -> &mut [page_table::Pte; page_table::ENTRIES] {
        // SAFETY: 表帧来自 funded root 或帧池（页对齐、已清零），经直映射访问。
        unsafe { &mut *(mm::phys_to_virt(frame.addr()) as *mut _) }
    }
}

const MEMORY_SPACE_LIMITS: Limits = Limits {
    max_regions: 4096,
    max_transactions: 4,
    max_pages_per_change: (256 << 20) / PAGE_SIZE,
    max_lease_bytes: 1 << 20,
    max_lease_segments: 64,
};

enum BackingExtentOwner {
    Raw(FrameTracker),
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
            Self::Raw(tracker) => tracker.base(),
            Self::Boot(extent) => extent.base(),
            Self::BootBorrowed { base, .. } => *base,
        }
    }

    fn count(&self) -> usize {
        match self {
            Self::Raw(tracker) => tracker.count(),
            Self::Boot(extent) => extent.pages(),
            Self::BootBorrowed { pages, .. } => *pages,
        }
    }

    fn split_at(self, pages: usize) -> (Self, Self) {
        match self {
            Self::Raw(tracker) => {
                let (left, right) = tracker.split_at(pages);
                (Self::Raw(left), Self::Raw(right))
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
    Binding(super::resources::PoolBinding),
}

impl RetiredSpaceResource {
    fn release(self) {
        match self {
            Self::Backing(owner) => drop(owner),
            Self::Binding(binding) => drop(binding),
        }
    }
}

struct OwnedBacking {
    identity: BackingId,
    pages: usize,
    extents: Vec<BackingExtent>,
}

struct PreparedOwnedMapping {
    backing: OwnedBacking,
    change: PreparedChange,
    translations: Vec<PreparedTranslation<TableFrameToken>>,
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

struct UserMemoryReservation {
    change: PreparedChange,
    backing: Option<OwnedBacking>,
    translations: Vec<PreparedTranslation<TableFrameToken>>,
    result: Option<PinnedMapResult>,
}

struct PreparedUserMemory(Box<Option<UserMemoryReservation>>);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ObjectMappingLease {
    pub(crate) lease: LeaseKey,
    pub(crate) region: RegionKey,
    pub(crate) range: LedgerPageRange,
    pub(crate) object: ObjectId,
}

struct ObjectMappingReservation {
    change: PreparedChange,
    translation: PreparedTranslation<TableFrameToken>,
    lease: ObjectMappingLease,
}

pub(crate) struct PreparedObjectMapping(Box<Option<ObjectMappingReservation>>);

impl PreparedObjectMapping {
    fn allocate() -> Result<Self, SpaceError> {
        Box::try_new(None)
            .map(Self)
            .map_err(|_| SpaceError::NoFrame)
    }

    fn install(&mut self, reservation: ObjectMappingReservation) {
        let previous = self.0.replace(reservation);
        assert!(previous.is_none(), "object mapping token filled twice");
    }

    fn take(mut self) -> ObjectMappingReservation {
        self.0.take().expect("object mapping token consumed twice")
    }
}

pub(crate) struct ObjectMapFailure {
    pub(crate) error: SpaceError,
    pub(crate) permits: Vec<WritePermit>,
}

struct ObjectUnmapReservation {
    change: PreparedChange,
    translation: PreparedTranslation<TableFrameToken>,
}

pub(crate) struct PreparedObjectUnmap(Box<Option<ObjectUnmapReservation>>);

impl PreparedObjectUnmap {
    fn allocate() -> Result<Self, SpaceError> {
        Box::try_new(None)
            .map(Self)
            .map_err(|_| SpaceError::NoFrame)
    }

    fn install(&mut self, reservation: ObjectUnmapReservation) {
        let previous = self.0.replace(reservation);
        assert!(previous.is_none(), "object unmap token filled twice");
    }

    fn take(mut self) -> ObjectUnmapReservation {
        self.0.take().expect("object unmap token consumed twice")
    }
}

impl PreparedUserMemory {
    fn allocate() -> Result<Self, SystemCallError> {
        Box::try_new(None)
            .map(Self)
            .map_err(|_| SystemCallError::OutOfMemory)
    }

    fn install(&mut self, reservation: UserMemoryReservation) {
        let previous = self.0.replace(reservation);
        assert!(previous.is_none(), "user memory token filled twice");
    }

    fn get(&self) -> &UserMemoryReservation {
        self.0
            .as_ref()
            .as_ref()
            .expect("user memory token must be filled")
    }

    fn get_mut(&mut self) -> &mut UserMemoryReservation {
        self.0
            .as_mut()
            .as_mut()
            .expect("user memory token must be filled")
    }

    fn take(mut self) -> UserMemoryReservation {
        self.0.take().expect("user memory token consumed twice")
    }
}

impl OwnedBacking {
    fn allocate(identity: BackingId, pages: usize) -> Result<Self, SpaceError> {
        let mut extents = Vec::new();
        extents
            .try_reserve(pages)
            .map_err(|_| SpaceError::NoFrame)?;
        let mut allocated = 0;
        while allocated < pages {
            let tracker =
                frame::alloc_user_largest(pages - allocated).ok_or(SpaceError::NoFrame)?;
            let count = tracker.count();
            extents.push(BackingExtent {
                offset_pages: allocated,
                owner: BackingExtentOwner::Raw(tracker),
            });
            allocated += count;
        }
        Ok(Self {
            identity,
            pages,
            extents,
        })
    }

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

    fn prepare_install(
        &self,
        tree: &mut TableTree<TableMem, LEVELS>,
        range: LedgerPageRange,
        backing_offset: usize,
        protection: Protection,
    ) -> Result<Vec<PreparedTranslation<TableFrameToken>>, SpaceError> {
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
        let mut prepared = Vec::new();
        prepared
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
            prepared.push(tree.prepare_map(
                Vpn(range.start() / PAGE_SIZE + page_offset),
                end - start,
                Ppn(extent.owner.base().addr() / PAGE_SIZE + physical_offset),
                protection_flags(protection),
            )?);
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
        Ok(prepared)
    }

    /// Remote ack 后精确归还 backing 的逻辑页区间。`extents` 在创建时按
    /// backing 总页数预留容量，因此这里的最多双切分不再分配。
    fn release_range(&mut self, offset: usize, bytes: usize) {
        assert!(
            offset.is_multiple_of(PAGE_SIZE) && bytes.is_multiple_of(PAGE_SIZE) && bytes != 0,
            "backing retire range must be nonempty and page aligned"
        );
        let release_start = offset / PAGE_SIZE;
        let release_end = release_start
            .checked_add(bytes / PAGE_SIZE)
            .expect("backing retire range overflowed");
        assert!(release_end <= self.pages, "backing retire escaped object");

        let mut released = 0;
        let mut index = 0;
        while index < self.extents.len() {
            let extent_start = self.extents[index].offset_pages;
            let extent_end = extent_start + self.extents[index].owner.count();
            let cut_start = extent_start.max(release_start);
            let cut_end = extent_end.min(release_end);
            if cut_start >= cut_end {
                index += 1;
                continue;
            }

            let extent = self.extents.remove(index);
            let left_pages = cut_start - extent_start;
            let retired_pages = cut_end - cut_start;
            let right_pages = extent_end - cut_end;
            let mut retired = extent.owner;

            if left_pages != 0 {
                let (left, tail) = retired.split_at(left_pages);
                self.extents.insert(
                    index,
                    BackingExtent {
                        offset_pages: extent_start,
                        owner: left,
                    },
                );
                index += 1;
                retired = tail;
            }
            if right_pages != 0 {
                let (middle, right) = retired.split_at(retired_pages);
                retired = middle;
                self.extents.insert(
                    index,
                    BackingExtent {
                        offset_pages: cut_end,
                        owner: right,
                    },
                );
                index += 1;
            }
            released += retired.count();
            drop(retired);
        }
        assert_eq!(
            released,
            release_end - release_start,
            "backing retire range was not owned exactly once"
        );
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

fn map_change_error(error: ChangeError) -> SpaceError {
    match error {
        ChangeError::Conflict | ChangeError::NotCovered | ChangeError::Guard => {
            SpaceError::Conflict
        }
        ChangeError::Busy => SpaceError::Busy,
        ChangeError::RegionLimit
        | ChangeError::TransactionLimit
        | ChangeError::KeyExhausted
        | ChangeError::AllocationFailed => SpaceError::NoFrame,
        ChangeError::BadLimits
        | ChangeError::Range(_)
        | ChangeError::OutOfBounds
        | ChangeError::OwnerDenied
        | ChangeError::PermissionDenied
        | ChangeError::BackingOutOfRange
        | ChangeError::ObjectAuthorization
        | ChangeError::PageLimit
        | ChangeError::LeaseInvalid
        | ChangeError::LeaseTooLarge
        | ChangeError::PermitMismatch
        | ChangeError::Stale => SpaceError::BadSegment,
    }
}

fn map_public_change_error(error: ChangeError) -> SystemCallError {
    match error {
        ChangeError::Conflict => SystemCallError::AddressConflict,
        ChangeError::NotCovered | ChangeError::Guard => SystemCallError::NotMapped,
        ChangeError::Busy | ChangeError::Stale => SystemCallError::ObjectBusy,
        ChangeError::PermissionDenied | ChangeError::OwnerDenied => SystemCallError::RightsDenied,
        ChangeError::PageLimit
        | ChangeError::RegionLimit
        | ChangeError::TransactionLimit
        | ChangeError::KeyExhausted => SystemCallError::ReachLimit,
        ChangeError::AllocationFailed => SystemCallError::OutOfMemory,
        ChangeError::BadLimits
        | ChangeError::Range(_)
        | ChangeError::OutOfBounds
        | ChangeError::BackingOutOfRange
        | ChangeError::ObjectAuthorization
        | ChangeError::LeaseInvalid
        | ChangeError::LeaseTooLarge
        | ChangeError::PermitMismatch => SystemCallError::IllegalArgument,
    }
}

fn map_public_space_error(error: SpaceError) -> SystemCallError {
    match error {
        SpaceError::NoFrame => SystemCallError::OutOfMemory,
        SpaceError::BadSegment => SystemCallError::InternalError,
        SpaceError::Conflict => SystemCallError::AddressConflict,
        SpaceError::Busy => SystemCallError::ObjectBusy,
        SpaceError::Unbound => SystemCallError::ObjectNotAvailable,
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
    /// 逐批回收用户页表子树（L0/L1 表帧）。
    Tables { root: usize, l1: usize },
    /// 全部子表已空：逐项验证 root 512 槽后交出 root 帧。
    Root { slot: usize },
    /// 资源全空（root 已释放）；仅剩空壳。
    Done,
}

/// 地址空间稳定 epoch 快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EpochSnapshot {
    pub translation: u64,
    pub instruction: u64,
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
    Bound(BoundAddressSpace),
}

impl AddressSpaceState {
    pub(crate) fn is_bound(&self) -> bool {
        matches!(self, Self::Bound(_))
    }

    pub(crate) fn bind(&mut self, bound: BoundAddressSpace) -> Result<(), BoundAddressSpace> {
        if !matches!(self, Self::Unbound) {
            return Err(bound);
        }
        *self = Self::Bound(bound);
        Ok(())
    }

    pub fn map_anonymous(
        &mut self,
        vaddr: usize,
        len: usize,
        permissions: ProcessMapFlags,
    ) -> Result<(), SpaceError> {
        self.bound_mut()?.map_anonymous(vaddr, len, permissions)
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

    fn bound(&self) -> Result<&BoundAddressSpace, SpaceError> {
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
    fn complete(&self) {
        log!(
            Memory,
            "epoch self-test passed: active snapshot and shootdown acknowledged"
        );
    }
}

pub(crate) trait MemoryRetireSink: Send + Sync {
    fn retire(&self, batch: RetireBatch);
}

/// Remote ack 后收束一笔已经 Publish 的 AddressSpace 事务。槽在 Commit gate
/// 内填充、Remote request 发布前可见；完成方先取槽再进入 AddressSpace 锁。
pub(crate) struct MemoryChangeCompletion {
    process: Arc<Process>,
    waiter: Arc<super::wait::WaitContext>,
    retire: Option<Arc<dyn MemoryRetireSink>>,
    published: crate::sync::Spinlock<Option<PublishedChange>>,
    result_obligation: crate::sync::Spinlock<Option<super::thread::ThreadResultObligation>>,
}

impl MemoryChangeCompletion {
    fn new(
        process: Arc<Process>,
        waiter: Arc<super::wait::WaitContext>,
        retire: Option<Arc<dyn MemoryRetireSink>>,
        result_obligation: Option<super::thread::ThreadResultObligation>,
    ) -> Self {
        Self {
            process,
            waiter,
            retire,
            published: crate::sync::Spinlock::new(crate::sync::ranks::MEMORY_COMPLETION, None),
            result_obligation: crate::sync::Spinlock::new(
                crate::sync::ranks::MEMORY_COMPLETION,
                result_obligation,
            ),
        }
    }

    pub(crate) fn install(&self, published: PublishedChange) {
        let previous = self.published.lock().replace(published);
        assert!(previous.is_none(), "memory completion installed twice");
    }
}

impl crate::remote_call::Completion for MemoryChangeCompletion {
    fn complete(&self) {
        let published = self
            .published
            .lock()
            .take()
            .expect("memory completion ran before Commit publication");
        let (retired, batch) = self.process.space.lock().retire_published_change(published);
        if let Some(retire) = &self.retire {
            retire.retire(batch);
        } else {
            let (fragments, permits) = batch.into_parts();
            assert!(
                permits.is_empty()
                    && fragments.iter().all(|fragment| {
                        fragment.owner == RegionOwner::AddressSpace
                            && matches!(
                                fragment.kind,
                                RegionKindView::Guard
                                    | RegionKindView::Mapping {
                                        backing: BackingView::Anonymous { .. },
                                        ..
                                    }
                            )
                    }),
                "public memory change retired non-address-space resources without a sink"
            );
        }
        self.process.space.lock().complete_retired_change(retired);
        if self.process.lifecycle.complete_mandatory()
            && let Some(control) = self.process.control()
        {
            control.publish_reapable();
        }
        // ThreadControl DONE 必须晚于 result lease 与 AddressSpace Complete；
        // 先释放 affine 线程义务，再完成仍存活调用者的 WaitContext。
        let result_obligation = {
            let mut slot = self.result_obligation.lock();
            slot.take()
        };
        drop(result_obligation);
        self.waiter.clone().complete_kernel();
    }
}

pub(crate) fn prepare_memory_completion(
    process: Arc<Process>,
    value: usize,
    retire: Option<Arc<dyn MemoryRetireSink>>,
    result_obligation: Option<super::thread::ThreadResultObligation>,
) -> Result<(Arc<MemoryChangeCompletion>, super::wait::WaitPlan), SystemCallError> {
    let (waiter, plan) = super::wait::prepare_kernel(value)?;
    let completion = Arc::try_new(MemoryChangeCompletion::new(
        process,
        waiter,
        retire,
        result_obligation,
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
        let translation = self
            .translation_epoch
            .fetch_add(1, Ordering::Release)
            .checked_add(1)
            .expect("address-space translation epoch exhausted");
        let instruction = if instruction {
            self.instruction_epoch
                .fetch_add(1, Ordering::Release)
                .checked_add(1)
                .expect("address-space instruction epoch exhausted")
        } else {
            self.instruction_epoch.load(Ordering::Acquire)
        };
        EpochSnapshot {
            translation,
            instruction,
        }
    }
}

fn map_shootdown_error(error: PrepareShootdownError) -> SystemCallError {
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

fn start_user_memory_change(
    process: Arc<Process>,
    mut prepared: Option<PreparedUserMemory>,
    shootdown_range: LedgerPageRange,
    instruction: bool,
    write_map_payload: bool,
    value: usize,
    result_obligation: Option<super::thread::ThreadResultObligation>,
) -> Result<super::wait::WaitPlan, SystemCallError> {
    let (completion, plan) =
        match prepare_memory_completion(process.clone(), value, None, result_obligation) {
            Ok(prepared) => prepared,
            Err(error) => {
                process
                    .space
                    .lock()
                    .rollback_user_memory(prepared.take().expect("memory change must roll back"));
                return Err(error);
            }
        };
    let sink: Arc<dyn crate::remote_call::Completion> = completion.clone();
    let shootdown = match process.space.prepare_shootdown(&process.lifecycle, sink) {
        Ok(shootdown) => shootdown,
        Err(error) => {
            process
                .space
                .lock()
                .rollback_user_memory(prepared.take().expect("memory change must roll back"));
            return Err(map_shootdown_error(error));
        }
    };
    if write_map_payload {
        process
            .space
            .lock()
            .write_user_map_payload(prepared.as_ref().expect("Map reservation must exist"));
    }

    let committed = process.space.commit_shootdown(
        &process.lifecycle,
        shootdown,
        shootdown_range.start() / PAGE_SIZE,
        shootdown_range.pages(),
        instruction,
        true,
        |state| {
            let published = state.commit_user_memory(
                prepared
                    .take()
                    .expect("user memory change commits exactly once"),
            );
            completion.install(published);
        },
    );
    let (_, synchronization) = match committed {
        Ok(committed) => committed,
        Err(_) => {
            process
                .space
                .lock()
                .rollback_user_memory(prepared.take().expect("stale change must roll back"));
            return Err(SystemCallError::ObjectBusy);
        }
    };
    synchronization.start();
    Ok(plan)
}

/// 为当前 Running process 建立 anonymous mapping。
pub(crate) fn memory_map(
    thread: &Thread,
    request_ptr: usize,
) -> Result<super::wait::WaitPlan, SystemCallError> {
    let process = thread.process.clone();
    let prepared = {
        let mut space = process.space.lock();
        // SAFETY: MemoryMapRequest 只含整数且无 padding，任意位型均有效。
        let request: MemoryMapRequest =
            unsafe { crate::uaccess::read_user_value(&mut space, request_ptr) }?;
        if request.cookie == 0
            || request.reserved != [0; 3]
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
        space.prepare_user_map(request)?
    };
    let layout = prepared
        .get()
        .change
        .map_result()
        .expect("Map reservation must retain its layout");
    start_user_memory_change(
        process,
        Some(prepared),
        layout.reservation,
        false,
        true,
        0,
        Some(thread.result_obligation()),
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
    let prepared = process.space.lock().prepare_user_unmap(range)?;
    start_user_memory_change(process, Some(prepared), range, false, false, 0, None)
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
    let prepared = process
        .space
        .lock()
        .prepare_user_protect(range, protection)?;
    let instruction = prepared
        .get()
        .change
        .translation_intents()
        .iter()
        .any(|intent| {
            matches!(
                intent,
                TranslationIntent::Protect { from, to, .. }
                    if *from == Protection::ReadExecute || *to == Protection::ReadExecute
            )
        });
    start_user_memory_change(process, Some(prepared), range, instruction, false, 0, None)
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
    /// 以 BackingId 关联 ledger logical offset 的 affine anonymous extents。
    backings: Vec<OwnedBacking>,
    next_backing: u64,
    /// Object-owned mapping authority；单调不复用。
    next_lease: u64,
    /// Building 期映像与 StartupBlock 的页对齐布局终点。
    image_end: usize,
    /// 有界收束游标（drain_gate + space 锁双持下推进）。
    drain_stage: DrainStage,
    /// 已从拥有结构摘下、等待下一 work unit 归还的帧 extent。
    pending_free: Option<BackingExtentOwner>,
    /// 已计入本批 work、必须在释放 AddressSpace 锁后析构的 Pool-backed owner。
    retired: Option<RetiredSpaceResource>,
}

impl BoundAddressSpace {
    /// 从完整 PoolBinding 构造 Bound 状态；root tree 只借用 binding 的 funded frame。
    pub fn new(binding: super::resources::PoolBinding) -> Result<Self, SpaceError> {
        let root = binding.root_frame();
        let mut tree = TableTree::new(TableMem::new(root)).map_err(|_| SpaceError::NoFrame)?;
        mm::install_kernel_top_level(&mut tree);
        let satp = (8usize << 60) | tree.satp_ppn();
        let bounds = LedgerPageRange::new(0, USER_TOP).map_err(|_| SpaceError::BadSegment)?;
        let ledger = MemorySpace::new(bounds, MEMORY_SPACE_LIMITS).map_err(map_change_error)?;
        Ok(Self {
            tree: Some(tree),
            binding: Some(binding),
            satp,
            ledger: Some(ledger),
            backings: Vec::new(),
            next_backing: 1,
            next_lease: 1,
            image_end: 0,
            drain_stage: DrainStage::Idle,
            pending_free: None,
            retired: None,
        })
    }

    fn pool(&self) -> &Arc<super::memory_pool::MemoryPool> {
        self.binding
            .as_ref()
            .expect("address-space PoolBinding already retired")
            .pool()
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

    fn prepare_user_map(
        &mut self,
        request: MemoryMapRequest,
    ) -> Result<PreparedUserMemory, SystemCallError> {
        let mut token = PreparedUserMemory::allocate()?;
        let bytes = usize::try_from(request.bytes).map_err(|_| SystemCallError::IllegalArgument)?;
        let guard_before =
            usize::try_from(request.guard_before).map_err(|_| SystemCallError::IllegalArgument)?;
        let guard_after =
            usize::try_from(request.guard_after).map_err(|_| SystemCallError::IllegalArgument)?;
        let result_address = usize::try_from(request.result_address)
            .map_err(|_| SystemCallError::IllegalArgument)?;
        let protection = MemoryProtection::from_raw(request.protection)
            .ok_or(SystemCallError::IllegalArgument)?;
        if protection == MemoryProtection::ReadExecute {
            return Err(SystemCallError::RightsDenied);
        }
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

        let identity = self.mint_backing().map_err(map_public_space_error)?;
        let protection = public_protection(protection);
        let validated = self
            .ledger()
            .validate_map(MapRequest {
                bytes,
                guard_before,
                guard_after,
                placement,
                current: protection,
                maximum: protection,
                owner: RegionOwner::AddressSpace,
                backing: MapBacking::Anonymous {
                    identity,
                    class: AnonymousClass::Data,
                },
                result: Some(memory_space::UserWriteLeaseRequest {
                    range: result_range,
                }),
            })
            .map_err(map_public_change_error)?;
        let layout = validated
            .map_result()
            .expect("public Map validation must produce a layout");
        let backing = OwnedBacking::allocate(identity, layout.usable.pages())
            .map_err(map_public_space_error)?;
        self.backings
            .try_reserve(1)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        let change = self
            .ledger()
            .reserve(validated, Vec::new())
            .map_err(|failure| map_public_change_error(failure.error))?;
        token.install(UserMemoryReservation {
            change,
            backing: Some(backing),
            translations: Vec::new(),
            result: None,
        });
        self.finish_user_map(token, layout, request.cookie, identity)
    }

    #[inline(never)]
    fn finish_user_map(
        &mut self,
        mut token: PreparedUserMemory,
        layout: memory_space::MapResultLayout,
        cookie: u64,
        identity: BackingId,
    ) -> Result<PreparedUserMemory, SystemCallError> {
        let value = MemoryMapResult {
            usable_base: layout.usable.start() as u64,
            usable_bytes: layout.usable.bytes() as u64,
            reservation_base: layout.reservation.start() as u64,
            reservation_bytes: layout.reservation.bytes() as u64,
            reserved: [0; 3],
            committed: 0,
        };
        let result = match self.pin_map_result(&token.get().change, value, cookie) {
            Ok(result) => result,
            Err(error) => {
                self.rollback_user_memory(token);
                return Err(error);
            }
        };
        let (range, offset, intent_protection) = match token.get().change.translation_intents() {
            [
                TranslationIntent::Install {
                    range,
                    backing:
                        BackingView::Anonymous {
                            identity: intent_identity,
                            offset,
                            class: AnonymousClass::Data,
                        },
                    protection,
                },
            ] if *intent_identity == identity => (*range, *offset, *protection),
            _ => panic!("public anonymous Map planner returned an invalid translation plan"),
        };
        let translations = match token
            .get()
            .backing
            .as_ref()
            .expect("Map reservation lost its backing")
            .prepare_install(self.tt(), range, offset, intent_protection)
        {
            Ok(translations) => translations,
            Err(error) => {
                self.rollback_user_memory(token);
                return Err(map_public_space_error(error));
            }
        };
        let reservation = token.get_mut();
        reservation.result = Some(result);
        reservation.translations = translations;
        Ok(token)
    }

    fn prepare_user_existing_change(
        &mut self,
        validated: memory_space::ValidatedChange,
    ) -> Result<PreparedUserMemory, SystemCallError> {
        let mut token = PreparedUserMemory::allocate()?;
        let mut translations = Vec::new();
        let change = self
            .ledger()
            .reserve(validated, Vec::new())
            .map_err(|failure| map_public_change_error(failure.error))?;
        if translations
            .try_reserve_exact(change.translation_intents().len())
            .is_err()
        {
            let permits = self.ledger().rollback(change);
            debug_assert!(permits.is_empty());
            return Err(SystemCallError::OutOfMemory);
        }
        for intent in change.translation_intents().iter().copied() {
            let prepared = match intent {
                TranslationIntent::Remove { range } => self
                    .tt()
                    .prepare_unmap(Vpn(range.start() / PAGE_SIZE), range.pages()),
                TranslationIntent::Protect { range, from, to } => self.tt().prepare_protect(
                    Vpn(range.start() / PAGE_SIZE),
                    range.pages(),
                    protection_flags(from),
                    protection_flags(to),
                ),
                TranslationIntent::Install { .. } => {
                    panic!("existing mapping change unexpectedly installs a PTE")
                }
            };
            match prepared {
                Ok(prepared) => translations.push(prepared),
                Err(error) => {
                    let permits = self.ledger().rollback(change);
                    debug_assert!(permits.is_empty());
                    drop(translations);
                    return Err(map_public_space_error(error.into()));
                }
            }
        }
        token.install(UserMemoryReservation {
            change,
            backing: None,
            translations,
            result: None,
        });
        Ok(token)
    }

    fn prepare_user_unmap(
        &mut self,
        range: LedgerPageRange,
    ) -> Result<PreparedUserMemory, SystemCallError> {
        let validated = self
            .ledger()
            .validate_unmap(UnmapRequest {
                range,
                authority: RegionOwner::AddressSpace,
            })
            .map_err(map_public_change_error)?;
        self.prepare_user_existing_change(validated)
    }

    fn prepare_user_protect(
        &mut self,
        range: LedgerPageRange,
        protection: Protection,
    ) -> Result<PreparedUserMemory, SystemCallError> {
        let validated = self
            .ledger()
            .validate_protect(ProtectRequest {
                range,
                protection,
                authority: RegionOwner::AddressSpace,
            })
            .map_err(map_public_change_error)?;
        self.prepare_user_existing_change(validated)
    }

    fn write_user_map_payload(&self, prepared: &PreparedUserMemory) {
        prepared
            .get()
            .result
            .as_ref()
            .expect("Map reservation lost its result")
            .write_payload();
    }

    fn rollback_user_memory(&mut self, prepared: PreparedUserMemory) {
        let UserMemoryReservation {
            change,
            translations,
            backing,
            result: _,
        } = prepared.take();
        let permits = self.ledger().rollback(change);
        debug_assert!(permits.is_empty());
        drop(translations);
        drop(backing);
    }

    fn commit_user_memory(&mut self, prepared: PreparedUserMemory) -> PublishedChange {
        let UserMemoryReservation {
            change,
            backing,
            translations,
            result,
        } = prepared.take();
        if let Some(result) = &result {
            result.commit_cookie();
        }
        let committed = self.ledger().commit(change);
        for translation in translations {
            self.tt().publish(translation);
        }
        let published = self.ledger().publish(committed);
        if let Some(backing) = backing {
            self.backings.push(backing);
        }
        published
    }

    fn map_owned_backing_with_owner(
        &mut self,
        vaddr: usize,
        protection: Protection,
        class: AnonymousClass,
        owner: RegionOwner,
        backing: OwnedBacking,
    ) -> Result<(), SpaceError> {
        let prepared = self.prepare_owned_backing(vaddr, protection, class, owner, backing)?;
        let published = self.commit_owned_mapping(prepared);
        self.finish_empty_published_change(published);
        Ok(())
    }

    fn map_owned_anonymous(
        &mut self,
        vaddr: usize,
        len: usize,
        protection: Protection,
    ) -> Result<(), SpaceError> {
        let identity = self.mint_backing()?;
        let backing = OwnedBacking::allocate(identity, len / PAGE_SIZE)?;
        let class = if protection == Protection::ReadExecute {
            AnonymousClass::InitialExecutable
        } else {
            AnonymousClass::Data
        };
        self.map_owned_backing(vaddr, protection, class, backing)
    }

    fn prepare_owned_backing(
        &mut self,
        vaddr: usize,
        protection: Protection,
        class: AnonymousClass,
        owner: RegionOwner,
        backing: OwnedBacking,
    ) -> Result<PreparedOwnedMapping, SpaceError> {
        let len = backing
            .pages
            .checked_mul(PAGE_SIZE)
            .ok_or(SpaceError::BadSegment)?;
        let validated = self
            .ledger()
            .validate_map(MapRequest {
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
            })
            .map_err(map_change_error)?;
        let change = self
            .ledger()
            .reserve(validated, Vec::new())
            .map_err(|failure| map_change_error(failure.error))?;
        if self.backings.try_reserve(1).is_err() {
            let permits = self.ledger().rollback(change);
            debug_assert!(permits.is_empty());
            return Err(SpaceError::NoFrame);
        }
        let intent = match change.translation_intents() {
            [
                TranslationIntent::Install {
                    range,
                    backing:
                        memory_space::BackingView::Anonymous {
                            identity: intent_identity,
                            offset,
                            ..
                        },
                    protection,
                },
            ] if *intent_identity == backing.identity => (*range, *offset, *protection),
            _ => panic!("anonymous map planner returned an invalid translation plan"),
        };
        let translations = match backing.prepare_install(self.tt(), intent.0, intent.1, intent.2) {
            Ok(translations) => translations,
            Err(error) => {
                let permits = self.ledger().rollback(change);
                debug_assert!(permits.is_empty());
                return Err(error);
            }
        };
        Ok(PreparedOwnedMapping {
            backing,
            change,
            translations,
        })
    }

    fn commit_owned_mapping(&mut self, prepared: PreparedOwnedMapping) -> PublishedChange {
        let PreparedOwnedMapping {
            backing,
            change,
            translations,
        } = prepared;
        let committed = self.ledger().commit(change);
        for translation in translations {
            self.tt().publish(translation);
        }
        let published = self.ledger().publish(committed);
        self.backings.push(backing);
        published
    }

    fn anonymous_range_still_live(
        &mut self,
        identity: BackingId,
        offset: usize,
        bytes: usize,
    ) -> bool {
        let end = offset
            .checked_add(bytes)
            .expect("retiring anonymous view overflowed");
        let covered: usize = self
            .ledger()
            .regions()
            .filter_map(|region| match region.kind {
                RegionKindView::Mapping {
                    backing:
                        BackingView::Anonymous {
                            identity: region_identity,
                            offset: region_offset,
                            ..
                        },
                    ..
                } if region_identity == identity => {
                    let region_end = region_offset
                        .checked_add(region.range.bytes())
                        .expect("live anonymous view overflowed");
                    let start = offset.max(region_offset);
                    let end = end.min(region_end);
                    Some(end.saturating_sub(start))
                }
                _ => None,
            })
            .sum();
        assert!(
            covered == 0 || covered == bytes,
            "retiring anonymous view is only partially represented in the live ledger"
        );
        covered == bytes
    }

    pub(crate) fn retire_published_change(
        &mut self,
        published: PublishedChange,
    ) -> (RetiredChange, RetireBatch) {
        let synchronized = self.ledger().synchronize(published);
        let (retired, batch) = self.ledger().retire(synchronized);
        for fragment in batch.fragments().iter().copied() {
            let RegionKindView::Mapping {
                backing:
                    BackingView::Anonymous {
                        identity, offset, ..
                    },
                ..
            } = fragment.kind
            else {
                continue;
            };
            if self.anonymous_range_still_live(identity, offset, fragment.range.bytes()) {
                continue;
            }
            let index = self
                .backings
                .iter()
                .position(|backing| backing.identity == identity)
                .expect("retiring anonymous fragment lost its owned backing");
            self.backings[index].release_range(offset, fragment.range.bytes());
            if self.backings[index].extents.is_empty() {
                self.backings.remove(index);
            }
        }
        (retired, batch)
    }

    pub(crate) fn complete_retired_change(&mut self, retired: RetiredChange) {
        self.ledger().complete(retired);
    }

    fn finish_empty_published_change(&mut self, published: PublishedChange) {
        let (retired, batch) = self.retire_published_change(published);
        let (fragments, permits) = batch.into_parts();
        debug_assert!(fragments.is_empty());
        debug_assert!(permits.is_empty());
        self.complete_retired_change(retired);
    }

    #[inline(never)]
    pub(crate) fn prepare_object_mapping(
        &mut self,
        va: usize,
        pa: usize,
        authorization: ObjectViewAuthorization,
        permits: Vec<WritePermit>,
    ) -> Result<PreparedObjectMapping, ObjectMapFailure> {
        assert_eq!(permits.len(), 1, "Tunnel object Map requires one permit");
        let mut token = match PreparedObjectMapping::allocate() {
            Ok(token) => token,
            Err(error) => return Err(ObjectMapFailure { error, permits }),
        };
        if !va.is_multiple_of(PAGE_SIZE)
            || !pa.is_multiple_of(PAGE_SIZE)
            || va >= USER_TOP - STACK_SIZE
        {
            return Err(ObjectMapFailure {
                error: SpaceError::BadSegment,
                permits,
            });
        }
        let lease_key = match self.mint_lease() {
            Ok(lease) => lease,
            Err(error) => return Err(ObjectMapFailure { error, permits }),
        };
        let object = authorization.object();
        let range = LedgerPageRange::new(va, PAGE_SIZE).expect("single page is aligned");
        let validated = match self.ledger().validate_map(MapRequest {
            bytes: PAGE_SIZE,
            guard_before: 0,
            guard_after: 0,
            placement: MapPlacement::FixedEmpty { usable_start: va },
            current: Protection::ReadWrite,
            maximum: Protection::ReadWrite,
            owner: RegionOwner::Lease(lease_key),
            backing: MapBacking::Object {
                authorization,
                offset: 0,
                object_bytes: PAGE_SIZE,
            },
            result: None,
        }) {
            Ok(validated) => validated,
            Err(error) => {
                return Err(ObjectMapFailure {
                    error: map_change_error(error),
                    permits,
                });
            }
        };
        let change = match self.ledger().reserve(validated, permits) {
            Ok(change) => change,
            Err(failure) => {
                let error = map_change_error(failure.error);
                let (_, _, permits) = failure.into_parts();
                assert_eq!(permits.len(), 1, "object Map must return one permit");
                return Err(ObjectMapFailure { error, permits });
            }
        };
        let region = change
            .mapped_region_key()
            .expect("object Map must reserve one usable region");
        match change.translation_intents() {
            [
                TranslationIntent::Install {
                    range: intent_range,
                    backing:
                        BackingView::Object {
                            object: intent_object,
                            offset: 0,
                        },
                    protection: Protection::ReadWrite,
                },
            ] if *intent_range == range && *intent_object == object => {}
            _ => panic!("object Map planner returned an invalid translation plan"),
        }
        let translation = match self.tt().prepare_map(
            Vpn(va / PAGE_SIZE),
            1,
            Ppn(pa / PAGE_SIZE),
            protection_flags(Protection::ReadWrite),
        ) {
            Ok(translation) => translation,
            Err(error) => {
                let permits = self.ledger().rollback(change);
                assert_eq!(permits.len(), 1, "object Map rollback lost its permit");
                return Err(ObjectMapFailure {
                    error: error.into(),
                    permits,
                });
            }
        };
        token.install(ObjectMappingReservation {
            change,
            translation,
            lease: ObjectMappingLease {
                lease: lease_key,
                region,
                range,
                object,
            },
        });
        Ok(token)
    }

    pub(crate) fn rollback_object_mapping(
        &mut self,
        prepared: PreparedObjectMapping,
    ) -> Vec<WritePermit> {
        let ObjectMappingReservation {
            change,
            translation,
            ..
        } = prepared.take();
        drop(translation);
        let permits = self.ledger().rollback(change);
        assert_eq!(permits.len(), 1, "object Map rollback lost its permit");
        permits
    }

    pub(crate) fn commit_object_mapping(
        &mut self,
        prepared: PreparedObjectMapping,
    ) -> (PublishedChange, ObjectMappingLease) {
        let ObjectMappingReservation {
            change,
            translation,
            lease,
        } = prepared.take();
        let committed = self.ledger().commit(change);
        self.tt().publish(translation);
        (self.ledger().publish(committed), lease)
    }

    #[inline(never)]
    pub(crate) fn prepare_object_unmap(
        &mut self,
        lease: ObjectMappingLease,
    ) -> Result<PreparedObjectUnmap, SpaceError> {
        let mut token = PreparedObjectUnmap::allocate()?;
        let matches_lease = self.ledger().regions().any(|region| {
            region.key == lease.region
                && region.range == lease.range
                && region.owner == RegionOwner::Lease(lease.lease)
                && matches!(
                    region.kind,
                    RegionKindView::Mapping {
                        backing: BackingView::Object { object, offset: 0 },
                        current: Protection::ReadWrite,
                        maximum: Protection::ReadWrite,
                        ..
                    } if object == lease.object
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
            .map_err(map_change_error)?;
        let change = self
            .ledger()
            .reserve(validated, Vec::new())
            .map_err(|failure| map_change_error(failure.error))?;
        match change.translation_intents() {
            [TranslationIntent::Remove { range }] if *range == lease.range => {}
            _ => panic!("object Unmap planner returned an invalid translation plan"),
        }
        let translation = match self
            .tt()
            .prepare_unmap(Vpn(lease.range.start() / PAGE_SIZE), lease.range.pages())
        {
            Ok(translation) => translation,
            Err(error) => {
                let permits = self.ledger().rollback(change);
                debug_assert!(permits.is_empty());
                return Err(error.into());
            }
        };
        token.install(ObjectUnmapReservation {
            change,
            translation,
        });
        Ok(token)
    }

    pub(crate) fn rollback_object_unmap(&mut self, prepared: PreparedObjectUnmap) {
        let ObjectUnmapReservation {
            change,
            translation,
        } = prepared.take();
        drop(translation);
        let permits = self.ledger().rollback(change);
        debug_assert!(permits.is_empty());
    }

    pub(crate) fn commit_object_unmap(&mut self, prepared: PreparedObjectUnmap) -> PublishedChange {
        let ObjectUnmapReservation {
            change,
            translation,
        } = prepared.take();
        let committed = self.ledger().commit(change);
        self.tt().publish(translation);
        self.ledger().publish(committed)
    }

    fn map_owned_backing(
        &mut self,
        vaddr: usize,
        protection: Protection,
        class: AnonymousClass,
        backing: OwnedBacking,
    ) -> Result<(), SpaceError> {
        self.map_owned_backing_with_owner(
            vaddr,
            protection,
            class,
            RegionOwner::AddressSpace,
            backing,
        )
    }

    /// 为 Building process 映射 anonymous zero pages。映像区与固定主栈
    /// 窗口不能由一次调用跨越；只有映像区推进 StartupBlock/heap 基准。
    pub fn map_anonymous(
        &mut self,
        vaddr: usize,
        len: usize,
        permissions: ProcessMapFlags,
    ) -> Result<(), SpaceError> {
        if len == 0
            || vaddr % PAGE_SIZE != 0
            || len % PAGE_SIZE != 0
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
        self.map_owned_anonymous(vaddr, len, protection)?;
        if end <= stack_base {
            self.image_end = self.image_end.max(end);
        }
        Ok(())
    }

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

    /// 装载 ELF：先按页规划权限并集（相邻段共享页取并集，每页恰映射
    /// 一次，杜绝静默改写），再逐页回填段内容（BSS 尾随帧池清零）。
    pub fn load_elf(
        &mut self,
        segments: &[elf::LoadSegment],
        file: &[u8],
    ) -> Result<(), SpaceError> {
        use alloc::collections::BTreeMap;

        // 阶段一：页粒度权限规划。
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

        // 阶段二：按相同权限的连续 VPN 建立 ledger backing 与页表投影。
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
        for (start, end, protection) in runs {
            self.map_owned_anonymous(start * PAGE_SIZE, (end - start) * PAGE_SIZE, protection)?;
        }

        // 阶段三：Building 地址空间不可运行，按已发布 PTE 的物理投影回填内容。
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

        let image_end = top.div_ceil(PAGE_SIZE) * PAGE_SIZE;
        if image_end > self.image_end {
            self.image_end = image_end;
        }
        Ok(())
    }

    /// Bootstrap 专用 init 栈映射：[USER_TOP - STACK_SIZE, USER_TOP)。
    /// 普通进程的栈由组装者（libprocess）经 ProcessMap 供给，内核不参与
    /// （bootstrap 例外：进程未启动、无用户代码可分配）。
    pub fn map_stack(&mut self) -> Result<(), SpaceError> {
        self.map_owned_anonymous(USER_TOP - STACK_SIZE, STACK_SIZE, Protection::ReadWrite)
    }

    /// Bootstrap 专用出生块：prefix 使用普通 root-funded anonymous backing；紧随其后的
    /// opaque payload 由外层强持已资金化 owner，本函数只在可失败映射期建立借用投影。
    /// 该入口不由 syscall 暴露，payload frame 与 root Pool charge 随同一个 owner 退休。
    pub fn map_bootstrap_block(
        &mut self,
        prefix: &[u8],
        payload: Option<&frame::BootFundedExtent>,
        payload_len: usize,
    ) -> Result<usize, SpaceError> {
        if prefix.is_empty() || prefix.len() % PAGE_SIZE != 0 || self.image_end == 0 {
            return Err(SpaceError::BadSegment);
        }
        let payload_pages = payload_len.div_ceil(PAGE_SIZE);
        if payload.as_ref().map_or(0, |extent| extent.pages()) != payload_pages {
            return Err(SpaceError::BadSegment);
        }
        let base = self.image_end;
        let prefix_pages = prefix.len() / PAGE_SIZE;
        let pages = prefix_pages
            .checked_add(payload_pages)
            .ok_or(SpaceError::BadSegment)?;
        let span = pages.checked_mul(PAGE_SIZE).ok_or(SpaceError::BadSegment)?;
        let end = base.checked_add(span).ok_or(SpaceError::BadSegment)?;
        if end > USER_TOP - STACK_SIZE {
            return Err(SpaceError::BadSegment);
        }

        let identity = self.mint_backing()?;
        let mut backing = OwnedBacking::allocate(identity, prefix_pages)?;
        if let Some(payload) = payload {
            backing
                .extents
                .try_reserve(1)
                .map_err(|_| SpaceError::NoFrame)?;
            backing.extents.push(BackingExtent {
                offset_pages: prefix_pages,
                owner: BackingExtentOwner::BootBorrowed {
                    base: payload.base(),
                    pages: payload.pages(),
                },
            });
            backing.pages = pages;
        }
        // 在任何 ledger/PTE 发布前完成 prefix 回填，使成功映射之后只剩不可失败 owner 移交。
        backing.write_from_start(prefix);
        let lease = self.mint_lease()?;
        self.map_owned_backing_with_owner(
            base,
            Protection::ReadOnly,
            AnonymousClass::Data,
            RegionOwner::Lease(lease),
            backing,
        )?;
        self.image_end = end;
        Ok(base)
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
        self.retired = Some(RetiredSpaceResource::Backing(owner));
        // caller 必须先释放 AddressSpace 锁并析构 retired owner，才能继续本批。
        (1, false)
    }

    /// 登记一笔新的 extent 归还（当前无在途归还时调用）。
    fn enqueue_free(&mut self, tracker: FrameTracker) {
        debug_assert!(
            self.pending_free.is_none(),
            "pending free must be consumed before enqueuing"
        );
        self.pending_free = Some(BackingExtentOwner::Raw(tracker));
    }

    /// 从页表结构收回一帧并登记延后归还。
    fn enqueue_table_frame(&mut self, frame: FrameNumber) {
        // SAFETY: 调用点先从唯一所属的页表槽或 root 摘除该帧，之后不再访问。
        self.enqueue_free(unsafe { FrameTracker::adopt_table_frame(frame) });
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
                    if let Some((_fragment, permit)) = self.ledger().drain_one() {
                        assert!(
                            permit.is_none(),
                            "object write permit reached anonymous-only drain batch"
                        );
                        work += 1;
                        continue;
                    }
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
                        self.drain_stage = DrainStage::Tables { root: 0, l1: 0 };
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
                    self.pending_free = Some(extent.owner);
                    let (used, done) = self.step_pending(budget - work);
                    work += used;
                    if !done {
                        return (work, false);
                    }
                    self.pending_free = None;
                }
                DrainStage::Tables { root, l1 } => {
                    let mut root_slot = root;
                    let mut l1_slot = l1;
                    while root_slot < page_table::ENTRIES {
                        if work >= budget {
                            self.drain_stage = DrainStage::Tables {
                                root: root_slot,
                                l1: l1_slot,
                            };
                            return (work, false);
                        }

                        let root_state = self
                            .tree
                            .as_mut()
                            .expect("tree exists until Root stage completes")
                            .root_slot_state(root_slot);
                        let l1_frame = match root_state {
                            RootSlotState::Shared | RootSlotState::Empty => {
                                root_slot += 1;
                                l1_slot = 0;
                                work += 1;
                                continue;
                            }
                            RootSlotState::Leaf => {
                                self.tree
                                    .as_mut()
                                    .expect("tree exists until Root stage completes")
                                    .detach_root_slot(root_slot);
                                root_slot += 1;
                                l1_slot = 0;
                                work += 1;
                                continue;
                            }
                            RootSlotState::Branch(frame) => frame,
                        };

                        while l1_slot < page_table::ENTRIES {
                            if work >= budget {
                                self.drain_stage = DrainStage::Tables {
                                    root: root_slot,
                                    l1: l1_slot,
                                };
                                return (work, false);
                            }
                            let state = self
                                .tree
                                .as_mut()
                                .expect("tree exists until Root stage completes")
                                .slot_state(l1_frame, l1_slot);
                            let branch_frame = match state {
                                SlotState::Branch(_) => self
                                    .tree
                                    .as_mut()
                                    .expect("tree exists until Root stage completes")
                                    .detach_branch(l1_frame, l1_slot),
                                SlotState::Empty | SlotState::Leaf => None,
                            };
                            l1_slot += 1;
                            work += 1;
                            if let Some(frame) = branch_frame {
                                self.enqueue_table_frame(frame);
                                let (used, done) = self.step_pending(budget - work);
                                work += used;
                                if !done {
                                    self.drain_stage = DrainStage::Tables {
                                        root: root_slot,
                                        l1: l1_slot,
                                    };
                                    return (work, false);
                                }
                                self.pending_free = None;
                            }
                        }

                        if work >= budget {
                            self.drain_stage = DrainStage::Tables {
                                root: root_slot,
                                l1: l1_slot,
                            };
                            return (work, false);
                        }
                        let detached = self
                            .tree
                            .as_mut()
                            .expect("tree exists until Root stage completes")
                            .detach_root_slot(root_slot);
                        assert_eq!(
                            detached,
                            Some(l1_frame),
                            "root branch changed during address-space drain"
                        );
                        root_slot += 1;
                        l1_slot = 0;
                        work += 1;
                        self.enqueue_table_frame(l1_frame);
                        let (used, done) = self.step_pending(budget - work);
                        work += used;
                        self.drain_stage = DrainStage::Tables {
                            root: root_slot,
                            l1: l1_slot,
                        };
                        if !done {
                            return (work, false);
                        }
                        self.pending_free = None;
                    }
                    self.drain_stage = DrainStage::Root { slot: 0 };
                }
                DrainStage::Root { slot } => {
                    let mut slot = slot;
                    while slot < page_table::ENTRIES {
                        if work >= budget {
                            self.drain_stage = DrainStage::Root { slot };
                            return (work, false);
                        }
                        let state = self
                            .tree
                            .as_mut()
                            .expect("tree exists until Root stage completes")
                            .root_slot_state(slot);
                        assert!(
                            matches!(state, RootSlotState::Empty | RootSlotState::Shared),
                            "owned subtree outlives Tables stage"
                        );
                        slot += 1;
                        work += 1;
                    }
                    if work + 1 > budget {
                        self.drain_stage = DrainStage::Root { slot };
                        return (work, false);
                    }
                    let tree = self
                        .tree
                        .take()
                        .expect("tree exists until Root stage completes");
                    let root = tree.finish_drain();
                    let binding = self
                        .binding
                        .take()
                        .expect("Bound address space lost its PoolBinding");
                    assert_eq!(
                        root,
                        binding.root_frame(),
                        "TableTree root differs from PoolBinding owner"
                    );
                    // Pool-backed owner 必须在 AddressSpace 锁外退款；本次摘除计一个
                    // work unit，调用层在返回前完成不可失败析构。
                    debug_assert!(self.retired.is_none(), "retired owner was not collected");
                    self.retired = Some(RetiredSpaceResource::Binding(binding));
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
struct DrainState {
    cursor: usize,
    pending_close: Option<super::handle::ProcessHandleEntry>,
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
            .attach_member(|tid| {
                let thread = Thread::new_thread(tid, self, context)
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
            .attach_registered_member(|tid| {
                let thread = Thread::new_thread(tid, self, context)
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
    /// 回调锁外执行，仍可用地址空间解除外部映射），后 AddressSpace。
    /// work unit 诚实计费：Handle 表每个扫描槽位（含空槽，take_next_bounded
    /// 硬性限制本次扫描量）与每次 close 各 1；地址空间部分见
    /// [`AddressSpaceState::drain`]。返回 (work_done, complete)。
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
                    // MemoryPool rank precedes AddressSpace；funded owner 必须在锁外退款。
                    if let Some(retired) = retired {
                        retired.release();
                    }
                    return (work + space_work, complete);
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
    /// 进程内线程号（成员表键；tid 从 1 起，0 保留为非身份值）。
    pub tid: Tid,
    pub process: Arc<Process>,
    frame: UnsafeCell<UserContext>,
    departure: Arc<super::thread::ThreadDeparture>,
    normal_exit: AtomicBool,
    exit_code: AtomicI64,
}

// SAFETY: UserContext 只在两种互斥状态下被访问：线程在本 hart 执行/
// 挂起期间（trap 路径与 dispatcher 经执行点独占写）；或线程已无容器
// （Waiting：发布时序保证完成方只见已离开一切 hart 引用的线程，见
// sched::park_publish）。其余字段原子或只读。
unsafe impl Sync for Thread {}

impl Thread {
    /// 创建线程执行基底：sepc = entry，sp = stack_pointer，a0/a1 = 出生
    /// 参数（首线程为出生块地址与长度，见 rinlib 启动契约）。FP 状态
    /// 创建即全零——不存在依赖 hart 残留的 valid 状态。tid 由
    /// lifecycle 锁内的 attach_member 分配并注入（构造随闭包进入锁内，
    /// Arc 分配取 HEAP 锁为 LIFECYCLE→HEAP 合法秩）。
    pub(super) fn new_thread(
        tid: Tid,
        process: &Arc<Process>,
        context: ThreadStartContext,
    ) -> Result<Self, ()> {
        Self::new_thread_with_control(tid, process, context, None)
    }

    pub(super) fn new_thread_with_control(
        tid: Tid,
        process: &Arc<Process>,
        context: ThreadStartContext,
        control: Option<&Arc<super::thread::ThreadControl>>,
    ) -> Result<Self, ()> {
        let departure = super::thread::ThreadDeparture::new(process, tid, control)?;
        let mut ctx = UserContext::zeroed();
        ctx.sepc = context.entry;
        ctx.x[2] = context.stack_pointer;
        ctx.x[10] = context.arg1; // a0
        ctx.x[11] = context.arg2; // a1
        Ok(Self {
            tid,
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
        .attach_registered_member(|tid| {
            Arc::try_new(
                Thread::new_thread(
                    tid,
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

/// launch 前的进程骨架：ELF 已装载、执行需求已判定、栈已映射、
/// 尚未附线程或入表 runnable。
pub struct SpawnedProcess {
    process: Arc<Process>,
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
            SystemCallError::ReachLimit => SpaceError::Conflict,
            _ => SpaceError::NoFrame,
        }
    })?;
    {
        let mut space = process.space.lock();
        space.load_elf(&image.segments, file)?;
        space.map_stack()?;
    }
    Ok(SpawnedProcess {
        process,
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
) -> Result<Arc<Thread>, SpaceError> {
    let SpawnedProcess {
        process,
        entry,
        requirement,
        root_pool,
    } = spawned;

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
    let block_va = match process.space.lock().map_bootstrap_block(
        &block,
        payload_funded.as_ref(),
        payload.len(),
    ) {
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

    process
        .handles
        .lock()
        .commit(reservation, handles)
        .expect("launch reservation count matches entries");

    // 内嵌 ProcessAttach：出生现场 = 出生块地址与长度（rinlib 启动契约）。
    match process.attach_thread(ThreadStartContext {
        entry: entry as u64,
        stack_pointer: USER_TOP as u64,
        arg1: block_va as u64,
        arg2: block_len as u64,
    }) {
        Ok(_) => {}
        Err(ThreadAttachError::Context(error)) => return Err(error),
        Err(ThreadAttachError::Oom) => return Err(SpaceError::NoFrame),
        Err(ThreadAttachError::Closed | ThreadAttachError::Limit) => {
            unreachable!("bootstrap attach must target an empty Building process")
        }
    }
    // 内嵌 ProcessStart（boot 路径失败不可恢复，直接提交不留 marker）：
    // 成员表插入即启动提交；eligibility 无解属 boot fatal（域表在初始
    // 任务装载前已由 bring_up_runtime 构造）。
    let job = process.job();
    let member = job
        .reserve_member(process.pid)
        .map_err(|_| SpaceError::NoFrame)?;
    job.commit_member(member, process.clone());
    assert!(
        process.lifecycle.enter_building_op(),
        "bootstrap process cannot be terminating"
    );
    // Bootstrap 内嵌同构序列的提交段：冻结需求与域、活体门（1 条
    // 预育线程）与预育提取在同一 gate 临界区内完成（普通 Start 的
    // begin_running(expected, staged) 同构——boot 路径无并发，直接
    // expect）。
    let domain =
        crate::sched::resolve_domain(requirement).expect("initial process has no compatible hart");
    let mut staged = Vec::new();
    staged
        .try_reserve_exact(1)
        .map_err(|_| SpaceError::NoFrame)?;
    process
        .lifecycle
        .begin_running(1, &mut staged)
        .expect("bootstrap process cannot be terminating");
    process.bind_execution(requirement, domain);
    let thread = staged.pop().expect("bootstrap staging thread missing");
    Ok(thread)
}
