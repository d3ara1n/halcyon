//! 物理帧库存内核适配：平台供给分类、启动元数据 reservation、真实帧清零与
//! RAII 所有权。
//!
//! 分级库存算法位于 `os/frame_pool`。本模块在帧池建立前合并平台永久排除、
//! 内核永久占用与 boot-held 区间，再从补集中保留库存元数据；只有最终补集
//! 发布为空闲。

use alloc::{sync::Arc, vec::Vec};

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

pub(crate) mod selftest;

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
pub(crate) const MAX_FUNDED_EXTENTS: usize = 64;

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
            region.start().is_multiple_of(PAGE_SIZE) && region.end().is_multiple_of(PAGE_SIZE),
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
        start < end && start.is_multiple_of(PAGE_SIZE) && end.is_multiple_of(PAGE_SIZE),
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
            range.0 < range.1
                && range.0.is_multiple_of(PAGE_SIZE)
                && range.1.is_multiple_of(PAGE_SIZE),
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

    fn split_at(mut self, left_pages: usize) -> (Self, Self) {
        let geometry = self
            .geometry
            .take()
            .expect("claimed user extent ownership already transferred");
        let (left, right) = geometry
            .split_at(left_pages)
            .expect("funded extent split must be strictly internal");
        (
            Self {
                geometry: Some(left),
                cleared: self.cleared,
            },
            Self {
                geometry: Some(right),
                cleared: self.cleared,
            },
        )
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
type UserFundedExtentInner = funded_frame::Funded<MemoryCharge, ClaimedUserExtent, 1>;

/// 固定长度、不可分解的对象数据 backing。
///
/// 与匿名 backing 的区别只在生命周期语义：对象 backing 由创建者绑定池一次付清，
/// view 的切割、降权与解除都不切数据 backing，因此这里不暴露 split/merge。
///
/// **堆化常驻形态**：对象 core 是常驻对象，因此这里持堆上的单 extent owner
/// 序列，而不内联固定 `MAX_FUNDED_EXTENTS` 槽的定长 funding 结果。定长容器
/// 只适合做一次性的 funding 事务结果（栈上短暂存在）；把它嵌进常驻对象会使
/// 每个对象无论实际几个 extent 都占满整份槽位，且构造路径逐层复制整个
/// 本体——这是栈帧审计的直接成因。funding 结果在获取后立即堆化。
#[must_use = "object backing must remain owned until the memory object is destroyed"]
pub(crate) struct ObjectBacking {
    extents: Vec<FundedExtent>,
    pages: usize,
}

impl ObjectBacking {
    pub(crate) fn pages(&self) -> usize {
        self.pages
    }

    /// 对象内页区间到物理 span 的投影，追加写入 `spans`。
    ///
    /// view 与 Tunnel 共用有界多 extent translation 组装路径；单页是退化几何。
    /// 越界由调用方在 Validate 阶段排除。
    pub(crate) fn project(
        &self,
        offset_pages: usize,
        length_pages: usize,
        spans: &mut Vec<(FrameNumber, usize)>,
    ) {
        let end = offset_pages
            .checked_add(length_pages)
            .expect("object projection range overflows");
        assert!(
            end <= self.pages,
            "object projection exceeds funded geometry"
        );
        if length_pages == 0 {
            return;
        }
        let mut cursor = 0usize;
        for extent in &self.extents {
            let extent_start = cursor;
            let extent_pages = extent.pages();
            cursor += extent_pages;
            if extent_start >= end || cursor <= offset_pages {
                continue;
            }
            let skip = offset_pages.saturating_sub(extent_start);
            let take = cursor.min(end) - (extent_start + skip);
            spans.push((extent.base() + skip, take));
        }
    }

    /// 投影所需的 span 上限。
    pub(crate) fn projection_capacity(&self) -> usize {
        self.extents.len()
    }
}

/// 取得固定长度、零态的对象 backing；由创建进程绑定池支付。
///
/// 与匿名 backing 走同一条 funding 路径：定长事务结果在返回前立即堆化为常驻
/// extent 列表，定长容器不进入常驻对象（见 `ObjectBacking` 的形态说明）。
#[inline(never)]
pub(crate) fn fund_object_backing(
    pool: &Arc<MemoryPool>,
    pages: usize,
    limits: FundingLimits,
) -> Result<ObjectBacking, funded_frame::FundError<memory_pool::PoolError, UserClaimError>> {
    let funded = fund_user_frames(pool, pages, limits)?;
    let pages = funded.pages();
    let mut extents = Vec::new();
    funded
        .into_extents(&mut extents)
        .map_err(|()| funded_frame::FundError::Physical(UserClaimError::OutOfMemory))?;
    Ok(ObjectBacking { extents, pages })
}

/// 单一物理 extent 的资金化 owner；自然析构先归还物理 extent，再退 Pool charge。
pub(crate) struct FundedExtent {
    inner: UserFundedExtentInner,
}

impl FundedExtent {
    pub(crate) fn pages(&self) -> usize {
        self.inner.pages()
    }

    pub(crate) fn base(&self) -> FrameNumber {
        self.inner
            .claims()
            .next()
            .expect("funded extent has no physical claim")
            .geometry()
            .base()
    }

    pub(crate) fn split_at(self, left_pages: usize) -> (Self, Self) {
        let (left, right) = self
            .inner
            .split_single(left_pages)
            .expect("funded extent split must be prevalidated");
        (Self { inner: left }, Self { inner: right })
    }
}

/// 普通 user supply 的资金化 backing；仅在 funding 事务中暂存多 extent owner。
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

    pub(crate) fn into_extents(self, extents: &mut Vec<FundedExtent>) -> Result<(), ()> {
        let mut inner = self.inner;
        extents
            .try_reserve_exact(inner.extent_count())
            .map_err(|_| ())?;
        while inner.extent_count() != 0 {
            let extent = inner
                .split_first()
                .expect("funded extent extraction failed");
            extents.push(FundedExtent { inner: extent });
        }
        Ok(())
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

type UserFundedTableInner = funded_frame::Funded<MemoryCharge, ClaimedUserExtent, 1>;

/// 页表帧（root 与中间表同形）的单 extent 资金化 owner。专用一槽存储避免把通用
/// 64-extents backing 内联进每个 Unbound shell 与 Bind 调用栈。
pub(crate) struct FundedTableFrame {
    inner: UserFundedTableInner,
}

impl FundedTableFrame {
    pub(crate) fn frame(&self) -> FrameNumber {
        let mut claims = self.inner.claims();
        let claim = claims.next().expect("funded table lost its physical claim");
        assert!(claims.next().is_none(), "funded table must have one extent");
        assert_eq!(
            claim.geometry().count(),
            1,
            "funded table must own one page"
        );
        claim.geometry().base()
    }
}

pub(crate) fn fund_user_table_frame(
    pool: &Arc<MemoryPool>,
) -> Result<FundedTableFrame, funded_frame::FundError<memory_pool::PoolError, UserClaimError>> {
    funded_frame::fund::<_, _, 1>(
        &PoolQuota(pool),
        &UserInventory,
        1,
        FundingLimits {
            max_pages: 1,
            max_extents: 1,
        },
    )
    .map(|inner| FundedTableFrame { inner })
}

/// 从未发布到 user inventory 的启动期 extent。构造只存在于验证后的 bootstrap
/// owner 移交点；类型本身负责防止普通 funded path 伪造保留内容。
#[must_use = "boot-held extent must be released or adopted into funded backing"]
pub(crate) struct BootHeldExtent {
    geometry: Option<ExtentGeometry>,
}

impl BootHeldExtent {
    /// # Safety
    ///
    /// `[base, base + pages)` 必须属于平台账本中的 boot-held 分类，尚未发布到
    /// FramePool，且本次启动中只允许构造一次 owner。
    pub(crate) unsafe fn adopt(base: FrameNumber, pages: usize) -> Self {
        Self {
            geometry: Some(
                ExtentGeometry::new(base, pages).expect("invalid boot-held extent geometry"),
            ),
        }
    }

    fn geometry(&self) -> ExtentGeometry {
        self.geometry
            .expect("boot-held ownership already transferred")
    }

    pub(crate) fn base(&self) -> FrameNumber {
        self.geometry().base()
    }

    pub(crate) fn pages(&self) -> usize {
        self.geometry().count()
    }

    pub(crate) fn split_at(mut self, pages: usize) -> (Self, Self) {
        let (left, right) = self
            .geometry()
            .split_at(pages)
            .expect("boot-held split must be strictly internal");
        self.geometry = None;
        (
            Self {
                geometry: Some(left),
            },
            Self {
                geometry: Some(right),
            },
        )
    }
}

impl Drop for BootHeldExtent {
    fn drop(&mut self) {
        if let Some(geometry) = self.geometry.take() {
            with_pool(|pool| pool.dealloc(geometry.base(), geometry.count()));
        }
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

fn clear_claimed(base: FrameNumber, count: usize) {
    let bytes = count
        .checked_mul(PAGE_SIZE)
        .expect("claimed frame byte length overflow");
    // SAFETY: extent 已从 POOL 原子移除，当前调用独占；直映射覆盖托管物理内存。
    unsafe {
        core::ptr::write_bytes(mm::phys_to_virt(base.addr()) as *mut u8, 0, bytes);
    }
}

/// 归还一段启动期保留物理区间。
pub fn free_range(start_pa: usize, end_pa: usize) {
    assert!(
        start_pa.is_multiple_of(PAGE_SIZE) && end_pa.is_multiple_of(PAGE_SIZE) && start_pa < end_pa
    );
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
