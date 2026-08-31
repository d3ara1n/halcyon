//! 物理帧库存内核适配：平台供给分类、启动元数据 reservation、真实帧清零与
//! RAII 所有权。
//!
//! 分级库存算法位于 `os/frame_pool`。本模块在帧池建立前合并平台永久排除、
//! 内核永久占用与 boot-held 区间，再从补集中保留库存元数据；只有最终补集
//! 发布为空闲。

use alloc::sync::Arc;

use frame_pool::{ArenaMetadata, ExtentGeometry, FramePool, MAX_ARENAS, metadata_bytes};
use funded_frame::{
    Limits as FundingLimits, PhysicalClaim, PhysicalSource, QuotaReservation, QuotaSource,
};
use memory_supply::{HeapChunkTicket, Planner, Range as SupplyRange, Requirements, SystemSupply};
use page_table::{FrameNumber, PAGE_BITS};

use crate::{
    board::{BoardInfo, MAX_MEMORY_REGIONS, MAX_PLATFORM_RESERVATIONS},
    external, mm,
    sync::Spinlock,
    task::memory_pool::{MemoryCharge, MemoryPool, PreparedMemoryCharge},
};

const PAGE_SIZE: usize = 1 << PAGE_BITS;
const MAX_PERMANENT_RESERVATIONS: usize = MAX_PLATFORM_RESERVATIONS + 2;
const MAX_BOOT_HOLDS: usize = 3;
// permanent 裁剪最多 M×P；boot 扣除 permanent 最多 M×P+M×B；
// unavailable 合并两者和 system ranges，user-free 补集再增加最多 M 段。
const MAX_CLASSIFIED_RANGES: usize = MAX_MEMORY_REGIONS
    + 2 * MAX_MEMORY_REGIONS * MAX_PERMANENT_RESERVATIONS
    + MAX_MEMORY_REGIONS * MAX_BOOT_HOLDS
    + 1
    + HEAP_CHUNK_LIMIT
    + RECOVERY_TICKET_LIMIT;
const HEAP_CHUNK_SIZE: usize = 1 << 20;
const HEAP_CHUNK_LIMIT: usize = 16;
const RECOVERY_TICKET_LIMIT: usize = 0;
/// 单事务 extent storage 的独立硬上限。
const MAX_FUNDED_EXTENTS: usize = 64;

type KernelFramePool = FramePool<'static>;

static POOL: Spinlock<Option<KernelFramePool>> = Spinlock::new(crate::sync::ranks::POOL, None);
type KernelSystemSupply = SystemSupply<HEAP_CHUNK_LIMIT, RECOVERY_TICKET_LIMIT>;
static SYSTEM_SUPPLY: Spinlock<Option<KernelSystemSupply>> =
    Spinlock::new(crate::sync::ranks::SYSTEM_SUPPLY, None);
type KernelSupplyPlanner = Planner<MAX_CLASSIFIED_RANGES, HEAP_CHUNK_LIMIT, RECOVERY_TICKET_LIMIT>;
static SUPPLY_PLANNER: Spinlock<KernelSupplyPlanner> =
    Spinlock::new(crate::sync::ranks::LEAF, KernelSupplyPlanner::new());

/// 平台 user supply 的一次性额度凭证；只冻结页数，不代表物理连续库存。
pub(crate) struct RootPoolSeed {
    pages: u64,
}

impl RootPoolSeed {
    pub(crate) const fn into_pages(self) -> u64 {
        self.pages
    }
}

static ROOT_POOL_SEED: Spinlock<Option<RootPoolSeed>> =
    Spinlock::new(crate::sync::ranks::LEAF, None);

struct SupplyInputs {
    managed: [SupplyRange; MAX_MEMORY_REGIONS],
    permanent_raw: [(usize, usize); MAX_PERMANENT_RESERVATIONS],
    boot_raw: [(usize, usize); MAX_BOOT_HOLDS],
    permanent: [SupplyRange; MAX_PERMANENT_RESERVATIONS],
    boot_held: [SupplyRange; MAX_BOOT_HOLDS],
}

impl SupplyInputs {
    const fn new() -> Self {
        Self {
            managed: [SupplyRange::EMPTY; MAX_MEMORY_REGIONS],
            permanent_raw: [(0, 0); MAX_PERMANENT_RESERVATIONS],
            boot_raw: [(0, 0); MAX_BOOT_HOLDS],
            permanent: [SupplyRange::EMPTY; MAX_PERMANENT_RESERVATIONS],
            boot_held: [SupplyRange::EMPTY; MAX_BOOT_HOLDS],
        }
    }
}

static SUPPLY_INPUTS: Spinlock<SupplyInputs> =
    Spinlock::new(crate::sync::ranks::DRAIN_GATE, SupplyInputs::new());

/// 消费启动供给账本冻结的唯一 root Pool 凭证。
pub(crate) fn take_root_pool_seed() -> RootPoolSeed {
    ROOT_POOL_SEED
        .lock()
        .take()
        .expect("root Pool seed unavailable or already consumed")
}

/// 持锁访问帧库存（初始化前访问为致命错误）。
fn with_pool<R>(f: impl FnOnce(&mut KernelFramePool) -> R) -> R {
    f(POOL.lock().as_mut().expect("frame pool not initialized"))
}

/// 解析板级信息并初始化帧库存。
pub fn init(board: &BoardInfo) {
    let mut inputs = SUPPLY_INPUTS.lock();
    let memory_count = board.memories().len();
    for (output, region) in inputs.managed.iter_mut().zip(board.memories()) {
        *output = SupplyRange::new(region.start, region.end()).expect("invalid managed range");
    }
    inputs.managed[..memory_count].sort_unstable_by_key(|region| region.start());
    validate_memories(&inputs.managed[..memory_count]);

    let total_frames = inputs.managed[..memory_count]
        .iter()
        .try_fold(0usize, |total, region| {
            total.checked_add(region.len() / PAGE_SIZE)
        })
        .expect("managed frame count overflow");
    let tree_metadata_len = metadata_bytes(total_frames).expect("frame metadata size overflow");
    let arena_metadata_len = core::mem::size_of::<ArenaMetadata>()
        .checked_mul(MAX_ARENAS)
        .expect("arena metadata size overflow");
    let metadata_len = arena_metadata_len
        .checked_add(tree_metadata_len)
        .expect("frame metadata size overflow");

    let permanent_count = build_permanent_reservations(board, &mut inputs.permanent_raw);

    let mut boot_hold_count = 0usize;
    let dtb = board.dtb_range();
    push_reservation(
        &mut inputs.boot_raw,
        &mut boot_hold_count,
        dtb.start,
        dtb.end(),
    );
    let bootstrap = external::bootstrap_range();
    assert_no_overlap(
        bootstrap,
        &inputs.permanent_raw[..permanent_count],
        "bootstrap range overlaps permanent memory",
    );
    assert!(
        !overlaps(bootstrap, (dtb.start, dtb.end())),
        "bootstrap range overlaps the device tree"
    );
    push_reservation(
        &mut inputs.boot_raw,
        &mut boot_hold_count,
        bootstrap.0,
        bootstrap.1,
    );
    if let Some((address, len)) = board.boot_package {
        let package = page_cover(address, len, "BootPackage range");
        assert_no_overlap(
            package,
            &inputs.permanent_raw[..permanent_count],
            "BootPackage range overlaps permanent memory",
        );
        assert!(
            !inputs.boot_raw[..boot_hold_count]
                .iter()
                .any(|range| overlaps(*range, package)),
            "BootPackage range overlaps another boot-held range"
        );
        push_reservation(
            &mut inputs.boot_raw,
            &mut boot_hold_count,
            package.0,
            package.1,
        );
    }
    boot_hold_count = normalize_reservations(&mut inputs.boot_raw, boot_hold_count);

    for index in 0..permanent_count {
        let (start, end) = inputs.permanent_raw[index];
        inputs.permanent[index] = SupplyRange::new(start, end).expect("invalid permanent range");
    }
    for index in 0..boot_hold_count {
        let (start, end) = inputs.boot_raw[index];
        inputs.boot_held[index] = SupplyRange::new(start, end).expect("invalid boot-held range");
    }

    let mut planner = SUPPLY_PLANNER.lock();
    let plan = planner
        .plan(
            &inputs.managed[..memory_count],
            &inputs.permanent[..permanent_count],
            &inputs.boot_held[..boot_hold_count],
            Requirements {
                page_size: PAGE_SIZE,
                metadata_bytes: metadata_len,
                heap_chunk_size: HEAP_CHUNK_SIZE,
                heap_chunk_count: HEAP_CHUNK_LIMIT,
                recovery_ticket_size: PAGE_SIZE,
                recovery_ticket_count: RECOVERY_TICKET_LIMIT,
            },
        )
        .expect("system memory supply cannot satisfy the configured budgets");
    let (inventory, system_supply) = plan.into_parts();

    let metadata = system_supply.metadata().range();
    clear_system_range(metadata);
    for range in system_supply.heap_ranges() {
        clear_system_range(range);
    }
    for range in system_supply.recovery_ranges() {
        clear_system_range(range);
    }

    // SAFETY: metadata ticket 从 user inventory 永久剔除；两个不重叠切片随全局
    // FramePool 存活，没有其它可变引用。
    let (arenas, tree_metadata) = unsafe {
        let ptr = mm::phys_to_virt(metadata.start()) as *mut u8;
        let arenas = core::slice::from_raw_parts_mut(ptr.cast::<ArenaMetadata>(), MAX_ARENAS);
        let tree_metadata =
            core::slice::from_raw_parts_mut(ptr.add(arena_metadata_len), tree_metadata_len);
        (arenas, tree_metadata)
    };
    let mut pool = FramePool::new(tree_metadata, arenas);

    for region in &inputs.managed[..memory_count] {
        pool.add_managed_region(
            FrameNumber::from_addr(region.start()),
            FrameNumber::from_addr(region.end()),
        )
        .expect("DT memory exceeds frame inventory metadata");
    }
    for range in inventory.user_free() {
        pool.release_range(
            FrameNumber::from_addr(range.start()),
            FrameNumber::from_addr(range.end()),
        )
        .expect("planned user-free range must be a reserved managed interval");
    }

    let free = pool.free_frames();
    let permanent_frames = inventory.permanent_bytes() / PAGE_SIZE;
    let boot_held_frames = inventory.boot_held_bytes() / PAGE_SIZE;
    let system_frames = inventory.system_bytes() / PAGE_SIZE;
    let metadata_frames = metadata.len() / PAGE_SIZE;
    let heap_frames = HEAP_CHUNK_LIMIT * (HEAP_CHUNK_SIZE / PAGE_SIZE);
    let recovery_frames = RECOVERY_TICKET_LIMIT;
    assert_eq!(
        system_frames,
        metadata_frames + heap_frames + recovery_frames,
        "system supply subaccounts do not close"
    );
    assert_eq!(
        total_frames,
        permanent_frames + boot_held_frames + system_frames + free,
        "physical supply classification does not close"
    );
    assert_eq!(
        free,
        inventory.user_free_bytes() / PAGE_SIZE,
        "FramePool published supply differs from the plan"
    );
    let root_pool_pages = free
        .checked_add(boot_held_frames)
        .and_then(|pages| u64::try_from(pages).ok())
        .expect("root Pool page count overflow");
    drop(inventory);
    drop(planner);
    drop(inputs);
    let mut seed = ROOT_POOL_SEED.lock();
    assert!(seed.is_none(), "root Pool seed initialized twice");
    *seed = Some(RootPoolSeed {
        pages: root_pool_pages,
    });
    drop(seed);
    log!(
        Frame,
        "{} arena(s), total {} frame(s): permanent {}, boot-held {}, system {}, user-free {}",
        pool.arena_count(),
        total_frames,
        permanent_frames,
        boot_held_frames,
        system_frames,
        free
    );
    log!(
        Frame,
        "system {} frame(s): metadata {}, heap {}, recovery {}",
        system_frames,
        metadata_frames,
        heap_frames,
        recovery_frames
    );
    *POOL.lock() = Some(pool);
    *SYSTEM_SUPPLY.lock() = Some(system_supply);
}

fn validate_memories(memories: &[SupplyRange]) {
    for (index, region) in memories.iter().enumerate() {
        assert!(
            region.start() % PAGE_SIZE == 0 && region.end() % PAGE_SIZE == 0,
            "DT memory region is not page aligned"
        );
        if index > 0 {
            assert!(
                memories[index - 1].end() <= region.start(),
                "DT memory regions overlap"
            );
        }
    }
}

fn push_reservation<const N: usize>(
    reservations: &mut [(usize, usize); N],
    count: &mut usize,
    start: usize,
    end: usize,
) {
    assert!(
        start < end && start % PAGE_SIZE == 0 && end % PAGE_SIZE == 0,
        "boot reservation is not page aligned"
    );
    let slot = reservations
        .get_mut(*count)
        .expect("boot reservation count exceeds fixed capacity");
    *slot = (start, end);
    *count += 1;
}

fn normalize_reservations<const N: usize>(
    reservations: &mut [(usize, usize); N],
    count: usize,
) -> usize {
    reservations[..count].sort_unstable_by_key(|range| range.0);
    let mut output = 0usize;
    for input in 0..count {
        let range = reservations[input];
        assert!(
            range.0 < range.1 && range.0 % PAGE_SIZE == 0 && range.1 % PAGE_SIZE == 0,
            "boot reservation is not page aligned"
        );
        if output > 0 && range.0 <= reservations[output - 1].1 {
            reservations[output - 1].1 = reservations[output - 1].1.max(range.1);
        } else {
            reservations[output] = range;
            output += 1;
        }
    }
    output
}

fn build_permanent_reservations(
    board: &BoardInfo,
    output: &mut [(usize, usize); MAX_PERMANENT_RESERVATIONS],
) -> usize {
    let mut count = 0usize;
    for region in board.platform_reservations() {
        push_reservation(output, &mut count, region.start, region.end());
    }

    let (bootstrap_start, bootstrap_end) = external::bootstrap_range();
    let kernel_start = external::sbi_start();
    let kernel_end = external::kernel_pa_end();
    assert!(
        kernel_start <= bootstrap_start
            && bootstrap_start < bootstrap_end
            && bootstrap_end <= kernel_end,
        "bootstrap range lies outside kernel physical image"
    );
    if kernel_start < bootstrap_start {
        push_reservation(output, &mut count, kernel_start, bootstrap_start);
    }
    if bootstrap_end < kernel_end {
        push_reservation(output, &mut count, bootstrap_end, kernel_end);
    }
    normalize_reservations(output, count)
}

fn overlaps(left: (usize, usize), right: (usize, usize)) -> bool {
    left.0 < right.1 && right.0 < left.1
}

fn assert_no_overlap(range: (usize, usize), reservations: &[(usize, usize)], message: &str) {
    assert!(
        !reservations
            .iter()
            .any(|reservation| overlaps(*reservation, range)),
        "{message}"
    );
}

fn page_cover(start: usize, len: usize, label: &str) -> (usize, usize) {
    let end = start
        .checked_add(len)
        .unwrap_or_else(|| panic!("{label} overflows"));
    (
        align_down(start, PAGE_SIZE),
        align_up(end, PAGE_SIZE).unwrap_or_else(|| panic!("{label} alignment overflows")),
    )
}

/// 从 `[start, end)` 减去地址有序、互不重叠的 reservations。
fn subtract(
    start: usize,
    end: usize,
    reservations: &[(usize, usize)],
    mut emit: impl FnMut(usize, usize),
) {
    let mut cursor = start;
    for &(reserved_start, reserved_end) in reservations {
        if reserved_end <= cursor || reserved_start >= end {
            continue;
        }
        let reserved_start = reserved_start.max(cursor);
        if reserved_start > cursor {
            emit(cursor, reserved_start);
        }
        cursor = reserved_end.min(end);
        if cursor >= end {
            return;
        }
    }
    if cursor < end {
        emit(cursor, end);
    }
}

const fn align_down(value: usize, alignment: usize) -> usize {
    value & !(alignment - 1)
}

fn align_up(value: usize, alignment: usize) -> Option<usize> {
    value
        .checked_add(alignment - 1)
        .map(|end| end & !(alignment - 1))
}

fn clear_system_range(range: SupplyRange) {
    // SAFETY: planner 已从 user inventory 剔除该 system range，启动线程持有唯一准备权。
    unsafe {
        core::ptr::write_bytes(mm::phys_to_virt(range.start()) as *mut u8, 0, range.len());
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UserClaimError {
    OutOfMemory,
}

/// 已从 user inventory 摘出、尚未发布给 backing 的 extent。
struct ClaimedUserExtent {
    geometry: Option<ExtentGeometry>,
    cleared: bool,
}

impl ClaimedUserExtent {
    fn new(geometry: ExtentGeometry) -> Self {
        Self {
            geometry: Some(geometry),
            cleared: false,
        }
    }

    fn geometry(&self) -> ExtentGeometry {
        self.geometry
            .expect("claimed user extent ownership already transferred")
    }
}

impl PhysicalClaim for ClaimedUserExtent {
    fn pages(&self) -> usize {
        self.geometry().count()
    }

    fn clear(&mut self) {
        assert!(!self.cleared, "claimed user extent cleared twice");
        let geometry = self.geometry();
        clear_claimed(geometry.base(), geometry.count());
        self.cleared = true;
    }
}

impl Drop for ClaimedUserExtent {
    fn drop(&mut self) {
        if let Some(geometry) = self.geometry.take() {
            with_pool(|pool| pool.dealloc(geometry.base(), geometry.count()));
        }
    }
}

struct UserInventory;

impl PhysicalSource for UserInventory {
    type Claim = ClaimedUserExtent;
    type Error = UserClaimError;

    fn claim_largest(&self, max_pages: usize) -> Result<Self::Claim, Self::Error> {
        let (base, count) =
            with_pool(|pool| pool.alloc_largest(max_pages)).ok_or(UserClaimError::OutOfMemory)?;
        let geometry =
            ExtentGeometry::new(base, count).expect("FramePool returned invalid geometry");
        Ok(ClaimedUserExtent::new(geometry))
    }
}

struct PoolQuota<'a>(&'a Arc<MemoryPool>);

impl QuotaSource for PoolQuota<'_> {
    type Reservation = PreparedMemoryCharge;
    type Error = memory_pool::PoolError;

    fn reserve(&self, pages: usize) -> Result<Self::Reservation, Self::Error> {
        MemoryPool::reserve_charge(self.0, pages)
    }
}

type UserFundedInner = funded_frame::Funded<MemoryCharge, ClaimedUserExtent, MAX_FUNDED_EXTENTS>;

/// 普通 user supply 的资金化 backing；自然析构先归还物理 extent，再退 Pool charge。
pub(crate) struct FundedFrames {
    inner: UserFundedInner,
}

impl FundedFrames {
    pub(crate) fn pages(&self) -> usize {
        self.inner.pages()
    }

    pub(crate) fn extent_count(&self) -> usize {
        self.inner.extent_count()
    }

    pub(crate) fn extents(&self) -> impl ExactSizeIterator<Item = (FrameNumber, usize)> {
        self.inner
            .claims()
            .map(|claim| (claim.geometry().base(), claim.geometry().count()))
    }
}

/// 取得普通 user-funded backing。页数与 extent 上限由具体消费方的工作边界决定。
pub(crate) fn fund_user_frames(
    pool: &Arc<MemoryPool>,
    pages: usize,
    limits: FundingLimits,
) -> Result<FundedFrames, funded_frame::FundError<memory_pool::PoolError, UserClaimError>> {
    funded_frame::fund::<_, _, MAX_FUNDED_EXTENTS>(&PoolQuota(pool), &UserInventory, pages, limits)
        .map(|inner| {
            assert_eq!(
                inner.credit().pages(),
                inner.pages(),
                "funded backing charge differs from physical geometry"
            );
            FundedFrames { inner }
        })
}

type UserFundedRootInner = funded_frame::Funded<MemoryCharge, ClaimedUserExtent, 1>;

/// AddressSpace root 的单 extent 资金化 owner。专用一槽存储避免把通用 64-extents
/// backing 内联进每个 Unbound shell 与 Bind 调用栈。
pub(crate) struct FundedRootFrame {
    inner: UserFundedRootInner,
}

impl FundedRootFrame {
    pub(crate) fn frame(&self) -> FrameNumber {
        let mut claims = self.inner.claims();
        let claim = claims.next().expect("funded root lost its physical claim");
        assert!(claims.next().is_none(), "funded root must have one extent");
        assert_eq!(claim.geometry().count(), 1, "funded root must own one page");
        claim.geometry().base()
    }
}

pub(crate) fn fund_user_root(
    pool: &Arc<MemoryPool>,
) -> Result<FundedRootFrame, funded_frame::FundError<memory_pool::PoolError, UserClaimError>> {
    funded_frame::fund::<_, _, 1>(
        &PoolQuota(pool),
        &UserInventory,
        1,
        FundingLimits {
            max_pages: 1,
            max_extents: 1,
        },
    )
    .map(|inner| FundedRootFrame { inner })
}

/// 从未发布到 user inventory 的启动期 extent。构造只存在于验证后的 bootstrap
/// owner 移交点；类型本身负责防止普通 funded path 伪造保留内容。
#[must_use = "boot-held extent must be released or adopted into funded backing"]
pub(crate) struct BootHeldExtent {
    tracker: FrameTracker,
}

impl BootHeldExtent {
    /// # Safety
    ///
    /// `[base, base + pages)` 必须属于平台账本中的 boot-held 分类，尚未发布到
    /// FramePool，且本次启动中只允许构造一次 owner。
    pub(crate) unsafe fn adopt(base: FrameNumber, pages: usize) -> Self {
        Self {
            tracker: FrameTracker::from_claimed(base, pages),
        }
    }

    pub(crate) fn base(&self) -> FrameNumber {
        self.tracker.base()
    }

    pub(crate) fn pages(&self) -> usize {
        self.tracker.count()
    }

    pub(crate) fn split_at(self, pages: usize) -> (Self, Self) {
        let (left, right) = self.tracker.split_at(pages);
        (Self { tracker: left }, Self { tracker: right })
    }
}

/// 保留启动内容的 primordial funded extent。字段顺序保证析构先把物理页发布回
/// user inventory，再归还 root Pool charge；split 同步切割两侧 affine owner。
#[must_use = "funded boot extent must remain owned until its mapping retires"]
pub(crate) struct BootFundedExtent {
    physical: BootHeldExtent,
    charge: MemoryCharge,
}

impl BootFundedExtent {
    pub(crate) fn base(&self) -> FrameNumber {
        self.physical.base()
    }

    pub(crate) fn pages(&self) -> usize {
        self.physical.pages()
    }

    pub(crate) fn split_at(mut self, pages: usize) -> (Self, Self) {
        assert!(
            pages > 0 && pages < self.pages(),
            "boot-funded split must be internal"
        );
        let right_pages = self.pages() - pages;
        let right_charge = self
            .charge
            .split(right_pages)
            .expect("boot-funded charge split must preserve its owner");
        let (left_physical, right_physical) = self.physical.split_at(pages);
        (
            Self {
                physical: left_physical,
                charge: self.charge,
            },
            Self {
                physical: right_physical,
                charge: right_charge,
            },
        )
    }
}

pub(crate) fn fund_boot_held(
    pool: &Arc<MemoryPool>,
    extent: BootHeldExtent,
) -> Result<BootFundedExtent, memory_pool::PoolError> {
    let pages = extent.pages();
    let reservation = MemoryPool::reserve_charge(pool, pages)?;
    let charge = reservation.commit();
    Ok(BootFundedExtent {
        physical: extent,
        charge,
    })
}

/// 启动自检：真实穿过 quota、库存、清零、commit、extent 上限回滚与自然退款。
pub(crate) fn funded_selftest(root: &Arc<MemoryPool>) {
    const PAGES: usize = 3;

    let pool_baseline = root.snapshot();
    let frames_baseline = free_frames();
    let funded = fund_user_frames(
        root,
        PAGES,
        FundingLimits {
            max_pages: PAGES,
            max_extents: PAGES,
        },
    )
    .expect("funded frame self-test failed");
    assert_eq!(funded.pages(), PAGES);
    assert!((1..=PAGES).contains(&funded.extent_count()));
    assert_eq!(
        funded.extents().map(|(_, pages)| pages).sum::<usize>(),
        PAGES
    );
    let committed = root.snapshot();
    assert_eq!(committed.available, pool_baseline.available - PAGES as u64);
    assert_eq!(committed.allocated, pool_baseline.allocated + PAGES as u64);
    assert_eq!(free_frames(), frames_baseline - PAGES);
    drop(funded);
    assert_eq!(root.snapshot(), pool_baseline);
    assert_eq!(free_frames(), frames_baseline);

    let limited = fund_user_frames(
        root,
        PAGES,
        FundingLimits {
            max_pages: PAGES,
            max_extents: 1,
        },
    );
    assert!(matches!(limited, Err(funded_frame::FundError::ExtentLimit)));
    assert_eq!(root.snapshot(), pool_baseline);
    assert_eq!(free_frames(), frames_baseline);
    log!(
        Memory,
        "funded frame self-test passed: commit, rollback, and dual-ledger refund ok"
    );
}

fn clear_claimed(base: FrameNumber, count: usize) {
    let bytes = count
        .checked_mul(PAGE_SIZE)
        .expect("claimed frame byte length overflow");
    // SAFETY: extent 已从 POOL 原子移除，当前调用独占；直映射覆盖托管物理内存。
    unsafe {
        core::ptr::write_bytes(mm::phys_to_virt(base.addr()) as *mut u8, 0, bytes);
    }
}

fn publish_claimed(base: FrameNumber, count: usize) -> FrameTracker {
    clear_claimed(base, count);
    FrameTracker::from_claimed(base, count)
}

/// 从 user inventory 分配 `2^order` 个物理连续帧；解锁后清零，再发布所有权。
pub fn alloc_user_order(order: usize) -> Option<FrameTracker> {
    let base = with_pool(|pool| pool.alloc_order(order))?;
    let count = 1usize
        .checked_shl(order as u32)
        .expect("frame pool returned an invalid order");
    Some(publish_claimed(base, count))
}

/// 从 user inventory 在 `max_count` 内分配当前可用的最大连续 extent。
pub fn alloc_user_largest(max_count: usize) -> Option<FrameTracker> {
    let (base, count) = with_pool(|pool| pool.alloc_largest(max_count))?;
    Some(publish_claimed(base, count))
}

/// 归还一段启动期保留物理区间。
pub fn free_range(start_pa: usize, end_pa: usize) {
    assert!(start_pa % PAGE_SIZE == 0 && end_pa % PAGE_SIZE == 0 && start_pa < end_pa);
    with_pool(|pool| {
        pool.release_range(
            FrameNumber::from_addr(start_pa),
            FrameNumber::from_addr(end_pa),
        )
        .expect("released boot range is not wholly reserved");
    });
}

/// DTB 消费完成后，先撤销 transition 临时叶，再回投未被永久 reservation
/// 覆盖的 boot-held 片段。
pub fn release_device_tree(board: &BoardInfo) {
    let mut permanent = [(0usize, 0usize); MAX_PERMANENT_RESERVATIONS];
    let permanent_count = build_permanent_reservations(board, &mut permanent);
    let dtb = board.dtb_range();
    subtract(
        dtb.start,
        dtb.end(),
        &permanent[..permanent_count],
        |start, end| {
            mm::retire_transition_range(start, end);
            free_range(start, end);
            log!(Memory, "device tree reclaim [{:#x}, {:#x})", start, end);
        },
    );
}

/// 帧库存剩余空闲帧数。
pub fn free_frames() -> usize {
    with_pool(|pool| pool.free_frames())
}

/// Talc Source 在 heap 锁内 O(1) 消费一个预清零 system ticket。
pub fn take_heap_chunk() -> Option<HeapChunkTicket> {
    SYSTEM_SUPPLY
        .lock()
        .as_mut()
        .expect("system supply not initialized")
        .take_heap_chunk()
}

/// 尚未交给内核 heap 的 system chunk 数。
pub fn remaining_heap_chunks() -> usize {
    SYSTEM_SUPPLY
        .lock()
        .as_ref()
        .expect("system supply not initialized")
        .remaining_heap_chunks()
}

/// RAII 帧 extent 所有权：Drop 时按 canonical blocks 有界归还。
#[must_use = "dropping the tracker returns its frame extent to the inventory"]
pub struct FrameTracker {
    geometry: Option<ExtentGeometry>,
}

impl FrameTracker {
    fn from_claimed(base: FrameNumber, count: usize) -> Self {
        Self {
            geometry: Some(
                ExtentGeometry::new(base, count).expect("invalid claimed frame extent geometry"),
            ),
        }
    }

    fn geometry(&self) -> ExtentGeometry {
        self.geometry
            .expect("frame tracker ownership already transferred")
    }

    fn take_geometry(&mut self) -> ExtentGeometry {
        self.geometry
            .take()
            .expect("frame tracker ownership already transferred")
    }

    pub fn base(&self) -> FrameNumber {
        self.geometry().base()
    }

    pub fn count(&self) -> usize {
        self.geometry().count()
    }

    /// 消费原 tracker，在内部边界切成两个互不重叠的 affine tracker。
    pub fn split_at(mut self, offset: usize) -> (Self, Self) {
        let geometry = self.take_geometry();
        let (left, right) = geometry
            .split_at(offset)
            .expect("frame tracker split must be strictly internal");
        (
            Self {
                geometry: Some(left),
            },
            Self {
                geometry: Some(right),
            },
        )
    }

    /// 消费单帧 tracker，把所有权移交给 `page_table::FrameMemory` 契约。
    pub fn into_table_frame(mut self) -> FrameNumber {
        let geometry = self.take_geometry();
        assert_eq!(geometry.count(), 1, "table transfer requires one frame");
        geometry.base()
    }

    /// 从 `page_table::FrameMemory` 契约收回一帧所有权。
    ///
    /// # Safety
    ///
    /// `frame` 必须是此前由 [`Self::into_table_frame`] 唯一移交、且尚未收回的帧。
    pub(crate) unsafe fn adopt_table_frame(frame: FrameNumber) -> Self {
        Self::from_claimed(frame, 1)
    }
}

impl Drop for FrameTracker {
    fn drop(&mut self) {
        if let Some(geometry) = self.geometry.take() {
            with_pool(|pool| pool.dealloc(geometry.base(), geometry.count()));
        }
    }
}

/// 自检：分配→切割→写入→归还→重取验证清零，全程真硬件访问。
pub fn selftest() {
    let tracker = alloc_user_order(3).expect("self-test allocation failed");
    let slots = mm::phys_to_virt(tracker.base().addr()) as *mut usize;
    // SAFETY: 自检持有 8 帧，写首 8 槽不越界；高半区直映射下访问。
    unsafe {
        for index in 0..8 {
            slots.add(index).write_volatile(0xDEAD_0000 + index);
        }
    }
    let before = free_frames();
    let (left, right) = tracker.split_at(4);
    assert_eq!(left.count(), 4, "left split geometry mismatch");
    assert_eq!(right.count(), 4, "right split geometry mismatch");
    assert_eq!(
        right.base(),
        left.base() + left.count(),
        "frame split overlaps or leaves a gap"
    );
    drop(left);
    assert_eq!(
        free_frames(),
        before + 4,
        "partial frame return accounting mismatch"
    );
    drop(right);
    assert_eq!(
        free_frames(),
        before + 8,
        "frame return accounting mismatch"
    );
    let tracker = alloc_user_order(3).expect("reallocation failed");
    // SAFETY: 同上；首帧首槽读回验证锁外初始化已完成。
    let first = unsafe { *(mm::phys_to_virt(tracker.base().addr()) as *const usize) };
    assert!(first == 0, "allocation not zeroed");
    log!(Frame, "self-test passed: alloc/split/dealloc/re-zero ok");
}
