//! 物理帧库存内核适配：启动元数据 reservation、真实帧清零与 RAII 所有权。
//!
//! 分级库存算法位于 `os/frame_pool`。本模块在帧池建立前按全部 DT memory
//! 计算树元数据大小，从 reservation 补集中保留连续页；随后注册完整托管范围，
//! 只把 SBI、内核、BootPackage 与元数据以外的区间发布为空闲。

use frame_pool::{ArenaMetadata, ExtentGeometry, FramePool, MAX_ARENAS, metadata_bytes};
use page_table::{FrameNumber, PAGE_BITS};

use crate::{
    board::{BoardInfo, MAX_MEMORY_REGIONS, MemoryRegion},
    external, mm,
    sync::Spinlock,
};

const PAGE_SIZE: usize = 1 << PAGE_BITS;
const MAX_RESERVATIONS: usize = 3;

/// 帧大小（字节）。堆供血等帧池消费方使用。
pub const FRAME_SIZE: usize = PAGE_SIZE;

type KernelFramePool = FramePool<'static>;

static POOL: Spinlock<Option<KernelFramePool>> = Spinlock::new(crate::sync::ranks::POOL, None);

/// 持锁访问帧库存（初始化前访问为致命错误）。
fn with_pool<R>(f: impl FnOnce(&mut KernelFramePool) -> R) -> R {
    f(POOL.lock().as_mut().expect("frame pool not initialized"))
}

/// 解析板级信息并初始化帧库存。
pub fn init(board: &BoardInfo) {
    let mut memories = [MemoryRegion { start: 0, len: 0 }; MAX_MEMORY_REGIONS];
    memories[..board.memories().len()].copy_from_slice(board.memories());
    let memory_count = board.memories().len();
    memories[..memory_count].sort_unstable_by_key(|region| region.start);
    validate_memories(&memories[..memory_count]);

    let total_frames = memories[..memory_count]
        .iter()
        .try_fold(0usize, |total, region| {
            total.checked_add(region.len / PAGE_SIZE)
        })
        .expect("managed frame count overflow");
    let tree_metadata_len = metadata_bytes(total_frames).expect("frame metadata size overflow");
    let arena_metadata_len = core::mem::size_of::<ArenaMetadata>()
        .checked_mul(MAX_ARENAS)
        .expect("arena metadata size overflow");
    let metadata_len = arena_metadata_len
        .checked_add(tree_metadata_len)
        .expect("frame metadata size overflow");
    let metadata_reserved_len =
        align_up(metadata_len, PAGE_SIZE).expect("frame metadata alignment overflow");

    let mut reservations = [(0usize, 0usize); MAX_RESERVATIONS];
    let mut reservation_count = 1usize;
    reservations[0] = (external::sbi_start(), external::kernel_pa_end());
    if let Some((address, len)) = board.boot_package {
        let end = address
            .checked_add(len)
            .expect("BootPackage range overflow");
        reservations[reservation_count] = (
            align_down(address, PAGE_SIZE),
            align_up(end, PAGE_SIZE).expect("BootPackage alignment overflow"),
        );
        reservation_count += 1;
    }
    sort_and_validate_reservations(&mut reservations[..reservation_count]);

    let metadata_pa = find_reservation(
        &memories[..memory_count],
        &reservations[..reservation_count],
        metadata_reserved_len,
    )
    .expect("no contiguous memory for frame metadata");
    reservations[reservation_count] = (metadata_pa, metadata_pa + metadata_reserved_len);
    reservation_count += 1;
    sort_and_validate_reservations(&mut reservations[..reservation_count]);

    // SAFETY: metadata reservation 已从即将发布的空闲补集中剔除；直映射覆盖全部
    // DT memory。两个不重叠切片随全局 FramePool 存活，没有其它可变引用。
    let (arenas, tree_metadata) = unsafe {
        let ptr = mm::phys_to_virt(metadata_pa) as *mut u8;
        core::ptr::write_bytes(ptr, 0, metadata_reserved_len);
        let arenas = core::slice::from_raw_parts_mut(ptr.cast::<ArenaMetadata>(), MAX_ARENAS);
        let tree_metadata =
            core::slice::from_raw_parts_mut(ptr.add(arena_metadata_len), tree_metadata_len);
        (arenas, tree_metadata)
    };
    let mut pool = FramePool::new(tree_metadata, arenas);

    for region in &memories[..memory_count] {
        pool.add_managed_region(
            FrameNumber::from_addr(region.start),
            FrameNumber::from_addr(region.start + region.len),
        )
        .expect("DT memory exceeds frame inventory metadata");
    }

    let mut free_regions = 0usize;
    for region in &memories[..memory_count] {
        subtract(
            region.start,
            region.start + region.len,
            &reservations[..reservation_count],
            |start, end| {
                pool.release_range(FrameNumber::from_addr(start), FrameNumber::from_addr(end))
                    .expect("free range must be a reserved managed interval");
                free_regions += 1;
            },
        );
    }
    assert!(
        free_regions > 0,
        "no free memory regions after boot reservations"
    );

    let free = pool.free_frames();
    log!(
        Frame,
        "{} arena(s), {} frame(s) metadata, {} frame(s) free ({:#x} bytes)",
        pool.arena_count(),
        metadata_reserved_len / PAGE_SIZE,
        free,
        free * PAGE_SIZE
    );
    *POOL.lock() = Some(pool);
}

fn validate_memories(memories: &[MemoryRegion]) {
    for (index, region) in memories.iter().enumerate() {
        assert!(
            region.start % PAGE_SIZE == 0 && region.len > 0 && region.len % PAGE_SIZE == 0,
            "DT memory region is not page aligned"
        );
        let end = region
            .start
            .checked_add(region.len)
            .expect("DT memory range overflow");
        if index > 0 {
            let previous = memories[index - 1];
            let previous_end = previous
                .start
                .checked_add(previous.len)
                .expect("DT memory range overflow");
            assert!(previous_end <= region.start, "DT memory regions overlap");
        }
        let _ = end;
    }
}

fn sort_and_validate_reservations(reservations: &mut [(usize, usize)]) {
    reservations.sort_unstable_by_key(|range| range.0);
    for index in 0..reservations.len() {
        let (start, end) = reservations[index];
        assert!(
            start < end && start % PAGE_SIZE == 0 && end % PAGE_SIZE == 0,
            "boot reservation is not page aligned"
        );
        if index > 0 {
            assert!(
                reservations[index - 1].1 <= start,
                "boot reservations overlap"
            );
        }
    }
}

fn find_reservation(
    memories: &[MemoryRegion],
    reservations: &[(usize, usize)],
    len: usize,
) -> Option<usize> {
    for region in memories {
        let mut candidate = None;
        subtract(
            region.start,
            region.start + region.len,
            reservations,
            |start, end| {
                if candidate.is_none() && end - start >= len {
                    candidate = Some(start);
                }
            },
        );
        if candidate.is_some() {
            return candidate;
        }
    }
    None
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

/// 分配 `2^order` 个物理连续帧；库存解锁后清零，再发布 RAII 所有权。
pub fn alloc_order(order: usize) -> Option<FrameTracker> {
    let base = with_pool(|pool| pool.alloc_order(order))?;
    let count = 1usize
        .checked_shl(order as u32)
        .expect("frame pool returned an invalid order");
    Some(publish_claimed(base, count))
}

/// 在 `max_count` 内分配当前可用的最大连续 extent。
pub fn alloc_largest(max_count: usize) -> Option<FrameTracker> {
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

/// 帧库存剩余空闲帧数。
pub fn free_frames() -> usize {
    with_pool(|pool| pool.free_frames())
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

    /// 消费 tracker，把该 extent 永久移交给不支持归还的内核子系统。
    pub fn into_permanent(mut self) {
        let _geometry = self.take_geometry();
    }

    /// 从 `page_table::FrameMemory` 契约收回一帧所有权。
    ///
    /// # Safety
    ///
    /// `frame` 必须是此前由 [`Self::into_table_frame`] 唯一移交、且尚未收回的帧。
    pub(crate) unsafe fn adopt_table_frame(frame: FrameNumber) -> Self {
        Self::from_claimed(frame, 1)
    }

    /// 收编从未发布到空闲库存的启动 reservation。
    ///
    /// # Safety
    ///
    /// `[base, base + count)` 必须完整托管、尚未进入库存，且所有权只移交一次。
    pub(crate) unsafe fn adopt_reserved(base: FrameNumber, count: usize) -> Self {
        Self::from_claimed(base, count)
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
    let tracker = alloc_order(3).expect("self-test allocation failed");
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
    let tracker = alloc_order(3).expect("reallocation failed");
    // SAFETY: 同上；首帧首槽读回验证锁外初始化已完成。
    let first = unsafe { *(mm::phys_to_virt(tracker.base().addr()) as *const usize) };
    assert!(first == 0, "allocation not zeroed");
    log!(Frame, "self-test passed: alloc/split/dealloc/re-zero ok");
}
