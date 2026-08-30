//! 内核地址空间与启动切换（见 notes/impls/mm.md「内核地址空间与启动协议」）。
//!
//! 汇编侧机构：`_start` 在 bare satp 下写跳板 root 表（DRAM 槽 identity
//! + 高半区别名）开 MMU 跳高半区；本模块在高半区构建正式内核页表
//! （静态 root + 直映射 mega 项 + 栈窗口子树）并切换，`KERNEL_SATP`
//! 经 registry 填入 record，secondary 的 `_enter_hart_high` 从 record
//! 加载同一张表。
//!
//! 页表模式当前固定 Sv39；按 DTB mmu-type 自动选式是后续工作
//! （见 notes/impls/mm.md「页表模式选择」）。

use core::{
    arch::asm,
    cell::UnsafeCell,
    sync::atomic::{AtomicUsize, Ordering},
};

use page_table::{
    ENTRIES, EagerMapper, FrameExhausted, FrameMemory, FrameNumber, PAGE_BITS, Ppn, Pte,
    ReservedTableFrame, TableTree, Vpn, flags, pages_at,
};
use stack_layout::PAGE_SIZE;
use stack_layout::StackWindowLayout;

use crate::{
    board::{BoardInfo, MAX_DIRECT_MAP_REGIONS, MAX_PLATFORM_RESERVATIONS, MemoryRegion},
    external,
};

/// 内核高半区基址：VMA = PA + KERNEL_VA_BASE（与链接脚本常量一致）。
pub const KERNEL_VA_BASE: usize = 0xFFFF_FFC0_0000_0000;

/// satp 模式：Sv39。
const SV39: usize = 8;

/// Sv39 顶层叶覆盖跨度。
const GIB: usize = 1 << 30;

/// 直映射 vpn2 起始槽（高半区首槽；由 KERNEL_VA_BASE 推得）。
const DIRECT_VPN2_BASE: usize = 256;

/// sv39 顶层直映射槽数上限：直映射占 [256, 256+limit)，栈窗口恒占顶槽
/// 511——上限 255 使两分区结构性互斥（满配也只到 510，永不覆盖窗口）。
const DIRECT_VPN2_LIMIT: usize = 255;

/// 栈窗口 vpn2 槽号：sv39 顶层顶槽（链接脚本 STACK_WINDOW_VA_BASE 恒指
/// 此；与直映射槽数解耦的独立分区）。layout 构造时校验两省一致。
const STACK_WINDOW_SLOT: usize = ENTRIES - 1;

/// 栈窗口叶表预留数（2MiB 单元数）：当前平台最大跨度
/// (0x40000 + 2×0x2000) * 8 ≈ 2.13MiB 跨两个单元，余量为扩展预留。
const WINDOW_LEAF_MAX: usize = 4;

/// 每个 `no-map` 区间的两个边界在 Sv39 下至多各需要两张下级表；末端
/// 非 mega 对齐再保留一条路径。容量是平台 admission 契约，不从帧库存借用。
const DIRECT_TABLE_MAX: usize = MAX_PLATFORM_RESERVATIONS * 4 + 2;

/// 栈物理打包区 `[base, kernel_pa_end)`：debug 防护用，map_stack_window
/// 发布。phys_to_virt 拒绝栈 PA——栈内存只经 sp/窗口 VA 引用，直映射
/// 别名绕过 guard 防护（见 impls/mm.md「栈窗口」）。
static STACK_PA_RANGE: (AtomicUsize, AtomicUsize) = (AtomicUsize::new(0), AtomicUsize::new(0));

pub fn phys_to_virt(pa: usize) -> usize {
    debug_assert!(
        direct_map_contains(pa),
        "phys_to_virt on a physical address excluded from the kernel direct map"
    );
    debug_assert!(
        !in_stack_pa_range(pa),
        "phys_to_virt on kernel stack PA: stack memory must be accessed via the stack-window VA"
    );
    pa + KERNEL_VA_BASE
}

fn in_stack_pa_range(pa: usize) -> bool {
    let (base, end) = (
        STACK_PA_RANGE.0.load(Ordering::Relaxed),
        STACK_PA_RANGE.1.load(Ordering::Relaxed),
    );
    base != 0 && pa >= base && pa < end
}

/// VA→PA 全函数：直映射区走线性算术；栈窗口按建表时的槽打包公式
/// 回推（与 `map_stack_window` 互逆）。同一物理页可同时出现在两个
/// 映射里（直映射别名 + 窗口规范地址），PA→VA 无唯一逆——
/// `phys_to_virt` 恒给直映射别名，本函数接受任意已映射内核 VA。
pub fn virt_to_phys(va: usize) -> usize {
    let layout = stack_layout();
    if va >= layout.window() && va < layout.window() + layout.span() {
        layout
            .translate(va)
            .expect("virt_to_phys on unmapped stack-window guard page")
    } else {
        va - KERNEL_VA_BASE
    }
}

/// 栈窗口布局（单一几何真值，见 `os/stack_layout`）：从链接期常量构造并
/// 校验；非法配置在此即刻 panic，绝不静默接受错误布局。
pub fn stack_layout() -> StackWindowLayout {
    let stack_size = external::hart_stack_size();
    let slots = external::hart_num_limit();
    StackWindowLayout::new(
        external::stack_window_base(),
        slots,
        stack_size,
        external::stack_guard(),
        external::emergency_size(),
        external::kernel_pa_end() - stack_size * slots,
        STACK_WINDOW_SLOT,
    )
    .expect("stack window layout violates invariants")
}

/// 正式内核页表 root。静态表（Linux swapper_pg_dir 同构）：帧池就绪前
/// 就要建直映射，root 不入池、永不释放。
#[repr(align(4096))]
struct RootTable(UnsafeCell<[Pte; ENTRIES]>);

// SAFETY: 仅 boot 早期单 hart 写入一次，其后只读；UnsafeCell 隔离
// 初始化期可变访问，避免 static mut。
unsafe impl Sync for RootTable {}

static KERNEL_PG_DIR: RootTable = RootTable(UnsafeCell::new([Pte::invalid(); ENTRIES]));

/// 栈窗口子表（静态预留，不入帧池，与 KERNEL_PG_DIR 同构）：
/// 一张中间表 + `WINDOW_LEAF_MAX` 张叶表，init 前单 hart 独占写、其后只读。
#[repr(align(4096))]
struct WindowTables(UnsafeCell<[[Pte; ENTRIES]; 1 + WINDOW_LEAF_MAX]>);

// SAFETY: 同 KERNEL_PG_DIR。
unsafe impl Sync for WindowTables {}

static WINDOW_TABLES: WindowTables = WindowTables(UnsafeCell::new(
    [[Pte::invalid(); ENTRIES]; 1 + WINDOW_LEAF_MAX],
));

/// 直映射碎片所需的静态下级表，不入任何运行期帧库存。
#[repr(align(4096))]
struct DirectTables(UnsafeCell<[[Pte; ENTRIES]; DIRECT_TABLE_MAX]>);

// SAFETY: boot 早期单 hart 经 EagerMapper 独占写入，此后只读。
unsafe impl Sync for DirectTables {}

static DIRECT_TABLES: DirectTables = DirectTables(UnsafeCell::new(
    [[Pte::invalid(); ENTRIES]; DIRECT_TABLE_MAX],
));
static DIRECT_TABLE_NEXT: AtomicUsize = AtomicUsize::new(0);

/// `phys_to_virt` 的已发布定义域；count=0 表示正式页表尚未建立。
struct DirectRanges(UnsafeCell<[MemoryRegion; MAX_DIRECT_MAP_REGIONS]>);

// SAFETY: 与正式页表一起在 boot 单写发布，之后只读。
unsafe impl Sync for DirectRanges {}

static DIRECT_RANGES: DirectRanges = DirectRanges(UnsafeCell::new(
    [MemoryRegion { start: 0, len: 0 }; MAX_DIRECT_MAP_REGIONS],
));
static DIRECT_RANGE_COUNT: AtomicUsize = AtomicUsize::new(0);

struct DirectTableMemory;

struct DirectReserved {
    index: usize,
    committed: bool,
}

impl ReservedTableFrame for DirectReserved {
    fn number(&self) -> FrameNumber {
        direct_table_frame(self.index)
    }

    fn commit(mut self) -> FrameNumber {
        self.committed = true;
        direct_table_frame(self.index)
    }
}

impl Drop for DirectReserved {
    fn drop(&mut self) {
        if !self.committed {
            DIRECT_TABLE_NEXT
                .compare_exchange(
                    self.index + 1,
                    self.index,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .expect("direct table reservations must roll back in LIFO order");
        }
    }
}

impl FrameMemory for DirectTableMemory {
    type ReservedFrame = DirectReserved;

    fn reserve_frame(&mut self) -> Result<Self::ReservedFrame, FrameExhausted> {
        let index = DIRECT_TABLE_NEXT
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
                (next < DIRECT_TABLE_MAX).then_some(next + 1)
            })
            .map_err(|_| FrameExhausted)?;
        Ok(DirectReserved {
            index,
            committed: false,
        })
    }

    fn free_frame(&mut self, _frame: FrameNumber) {
        panic!("permanent direct-map tables cannot be freed")
    }

    fn table_mut(&mut self, frame: FrameNumber) -> &mut [Pte; ENTRIES] {
        let root = kernel_root_frame();
        if frame == root {
            // SAFETY: boot 早期 EagerMapper 独占 root。
            return unsafe { &mut *KERNEL_PG_DIR.0.get() };
        }
        let base = direct_table_frame(0).0;
        let index = frame
            .0
            .checked_sub(base)
            .filter(|index| *index < DIRECT_TABLE_MAX)
            .expect("direct mapper accessed a table outside its static arena");
        // SAFETY: EagerMapper 持有唯一的 &mut DirectTableMemory，且 index 已验证。
        unsafe { &mut (*DIRECT_TABLES.0.get())[index] }
    }
}

fn kernel_root_frame() -> FrameNumber {
    FrameNumber::from_addr(virt_to_phys(KERNEL_PG_DIR.0.get() as usize))
}

fn direct_table_frame(index: usize) -> FrameNumber {
    debug_assert!(index < DIRECT_TABLE_MAX);
    let base = DIRECT_TABLES.0.get() as usize;
    FrameNumber::from_addr(virt_to_phys(base) + index * PAGE_SIZE)
}

fn direct_map_contains(pa: usize) -> bool {
    let count = DIRECT_RANGE_COUNT.load(Ordering::Acquire);
    if count == 0 {
        return true;
    }
    // SAFETY: count 的 Release 发布晚于 ranges 初始化，之后数组只读。
    let ranges = unsafe { &*DIRECT_RANGES.0.get() };
    ranges[..count]
        .iter()
        .any(|range| range.start <= pa && pa < range.end())
}

/// 在物理页回投库存前撤销 cold-bootstrap transition 表中的临时叶。
///
/// 调用方必须位于没有 hart 使用 transition satp 的同步点；区间必须完整对应
/// `_start` 建立的 4KiB identity/high-half 共用叶。中间表和 leaf arena 为永久
/// entry 设施，只撤叶、不回收表。
pub(crate) fn retire_transition_range(start_pa: usize, end_pa: usize) {
    assert!(
        start_pa < end_pa && start_pa % PAGE_SIZE == 0 && end_pa % PAGE_SIZE == 0,
        "transition retirement range is not page aligned"
    );
    let root_pa = external::transition_root_pa();
    // SAFETY: transition root 是页对齐的永久单页设施；调用同步点保证独占写。
    let root =
        unsafe { core::slice::from_raw_parts_mut(phys_to_virt(root_pa) as *mut Pte, ENTRIES) };
    for pa in (start_pa..end_pa).step_by(PAGE_SIZE) {
        let root_index = pa >> 30;
        assert!(
            root_index < ENTRIES / 2,
            "transition retirement address exceeds the identity domain"
        );
        let root_branch = root[root_index];
        assert!(
            root_branch.is_branch(),
            "transition retirement encountered a missing middle table"
        );
        // SAFETY: branch 来自已验证的 transition 表，目标 middle/leaf table 永久存在；
        // 当前同步点没有 hart 使用或改写该表。
        let middle = unsafe {
            core::slice::from_raw_parts_mut(
                phys_to_virt(root_branch.next_frame().addr()) as *mut Pte,
                ENTRIES,
            )
        };
        let leaf_branch = middle[(pa >> 21) & (ENTRIES - 1)];
        assert!(
            leaf_branch.is_branch(),
            "transition retirement encountered a missing leaf table"
        );
        let leaf = unsafe {
            core::slice::from_raw_parts_mut(
                phys_to_virt(leaf_branch.next_frame().addr()) as *mut Pte,
                ENTRIES,
            )
        };
        let entry = &mut leaf[(pa >> PAGE_BITS) & (ENTRIES - 1)];
        assert!(
            entry.is_leaf() && entry.ppn().addr() == pa,
            "transition retirement encountered a missing page leaf"
        );
        *entry = Pte::invalid();
    }
    // SAFETY: 后续物理页发布和未来 secondary 的 record Release 都必须晚于 PTE 撤销。
    unsafe { asm!("fence w, w", options(nostack, preserves_flags)) };
}

/// 正式内核 satp 值（init 发布、`kernel_satp()` 消费填 record；
/// trap 汇编非 Resume 出口经 sym 符号直接装载切回）。
pub(crate) static KERNEL_SATP: AtomicUsize = AtomicUsize::new(0);

/// 按板级 admitted ranges 构建并启用内核直映射；连续段由 eager mapper 自动
/// 选择最大合法叶，`no-map` 保留为洞。
///
/// 切换安全性：镜像/栈/跳板表均由 admitted range 或专用栈窗口覆盖，切换前后
/// VMA 不变（跳板别名与直映射对同一物理段呈现相同 VMA），执行流无缝。
/// 直映射占用的顶层槽跨度（其中允许存在完整 `no-map` 空槽）。
static DIRECT_SLOT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// 直映射顶层槽跨度（高半区 [256, 256+n)）。
pub fn direct_slots() -> usize {
    DIRECT_SLOT_COUNT.load(Ordering::Relaxed)
}

/// 把内核高半区顶层项作为 shared 槽挂入用户表 root。共享所有权登记与
/// PTE 安装由 `TableTree` 同时完成，teardown 不再依赖地址区间配对。
pub fn install_kernel_top_level<M: FrameMemory, const LEVELS: usize>(
    tree: &mut TableTree<M, LEVELS>,
) {
    // SAFETY: 内核页表在 mm::init 后只读；进程只复制顶层 PTE 值。
    let source = unsafe { &*KERNEL_PG_DIR.0.get() };
    let end = DIRECT_VPN2_BASE + direct_slots();
    let mut slot = DIRECT_VPN2_BASE;
    while slot < end {
        while slot < end && !source[slot].is_valid() {
            slot += 1;
        }
        let start = slot;
        while slot < end && source[slot].is_valid() {
            slot += 1;
        }
        if start < slot {
            tree.attach_shared_root(start, &source[start..slot])
                .expect("kernel direct-map root slots must be empty");
        }
    }
    tree.attach_shared_root(
        STACK_WINDOW_SLOT,
        &source[STACK_WINDOW_SLOT..STACK_WINDOW_SLOT + 1],
    )
    .expect("kernel stack-window root slot must be empty");
}

pub fn init(board: &BoardInfo) {
    let direct_end = board
        .direct_map_regions()
        .iter()
        .map(|range| range.end())
        .max()
        .expect("kernel direct map has no admitted range");
    let slots = direct_end.div_ceil(GIB);
    assert!(
        (1..=DIRECT_VPN2_LIMIT).contains(&slots),
        "unexpected direct-map slot count: {slots}"
    );
    let permanent_kernel = (external::awaken_pa(), external::kernel_pa_end());
    assert!(
        board
            .direct_map_regions()
            .iter()
            .any(|range| range.start <= permanent_kernel.0 && permanent_kernel.1 <= range.end()),
        "reserved-memory no-map overlaps the running kernel image"
    );
    assert_eq!(
        DIRECT_TABLE_NEXT.load(Ordering::Relaxed),
        0,
        "kernel direct map initialized twice"
    );

    // SAFETY: boot 早期单 hart 独占（UnsafeCell 隔离），此后只读。
    let dir: *mut [Pte; ENTRIES] = KERNEL_PG_DIR.0.get();
    unsafe { (*dir).fill(Pte::invalid()) };
    let root = kernel_root_frame();
    let mut memory = DirectTableMemory;
    let mut mapper = EagerMapper::<_, 3>::new(&mut memory, root);
    let direct_vpn_base = DIRECT_VPN2_BASE * pages_at(2);
    for range in board.direct_map_regions() {
        assert!(
            range.start % PAGE_SIZE == 0 && range.len % PAGE_SIZE == 0,
            "kernel direct-map range is not page aligned"
        );
        mapper
            .map_range(
                Vpn(direct_vpn_base + (range.start >> PAGE_BITS)),
                range.len >> PAGE_BITS,
                Ppn(range.start >> PAGE_BITS),
                flags::KERNEL_DIRECT,
            )
            .unwrap_or_else(|error| panic!("kernel direct-map construction failed: {error:?}"));
    }
    drop(mapper);

    let satp = (SV39 << 60) | root.0;
    map_stack_window();

    let layout = stack_layout();
    STACK_PA_RANGE
        .0
        .store(layout.phys_base(), Ordering::Relaxed);
    STACK_PA_RANGE.1.store(
        layout.phys_base() + layout.stack_size() * layout.slots(),
        Ordering::Relaxed,
    );

    // SAFETY: 正式页表仍未发布，boot hart 独占写；Release count 同时发布范围内容。
    unsafe {
        let ranges = &mut *DIRECT_RANGES.0.get();
        ranges[..board.direct_map_regions().len()].copy_from_slice(board.direct_map_regions());
    }
    DIRECT_RANGE_COUNT.store(board.direct_map_regions().len(), Ordering::Release);
    KERNEL_SATP.store(satp, Ordering::Release);
    DIRECT_SLOT_COUNT.store(slots, Ordering::Relaxed);
    // SAFETY: satp 装载与全量 sfence 是 S 态特权指令；当前执行流、静态数据与
    // 栈物理区均已由 admitted direct-map range 或栈窗口覆盖。
    unsafe {
        asm!(
            "csrw  satp, {satp}",
            "sfence.vma",
            satp = in(reg) satp,
        );
    }

    log!(
        MM,
        "direct map {} range(s) below {:#x}, {} static table(s), kernel @ {:#x}",
        board.direct_map_regions().len(),
        direct_end,
        DIRECT_TABLE_NEXT.load(Ordering::Relaxed),
        KERNEL_VA_BASE
    );
}

/// 正式内核 satp 值（registry 构造 record 用）。
pub fn kernel_satp() -> usize {
    KERNEL_SATP.load(Ordering::Acquire)
}

/// 构建栈窗口映射（init 内、satp 发布前调用）：每槽布局
/// `[槽底 guard | formal | emergency guard | emergency]`（`os/stack_layout`
/// 单一真值），guard 洞不映射，栈溢出立即 page fault；物理侧按槽连续
/// 打包在内核静态占用末段，guard 不占帧。
fn map_stack_window() {
    let layout = stack_layout();
    const MIDDLE_SPAN: usize = 1 << 21;
    assert!(
        layout.span() <= MIDDLE_SPAN * WINDOW_LEAF_MAX,
        "stack window exceeds reserved leaf tables"
    );

    // SAFETY: 静态子表 init 前单 hart 独占写（同 KERNEL_PG_DIR），此后只读。
    unsafe {
        let tables = &mut *WINDOW_TABLES.0.get();
        let (middle, leaves) = tables.split_at_mut(1);
        let middle = &mut middle[0];

        for unit in 0..layout.span().div_ceil(MIDDLE_SPAN) {
            middle[unit] = Pte::branch(FrameNumber::from_addr(virt_to_phys(
                leaves[unit].as_ptr() as usize
            )));
        }
        for slot in 0..layout.slots() {
            for (va, pa) in layout.mappings(slot) {
                let off = va - layout.window();
                let unit = off / MIDDLE_SPAN;
                let idx = (off % MIDDLE_SPAN) / PAGE_SIZE;
                leaves[unit][idx] = Pte::leaf(Ppn(pa >> PAGE_BITS), flags::KERNEL_STACK);
            }
        }
        (*KERNEL_PG_DIR.0.get())[STACK_WINDOW_SLOT] = Pte::branch(FrameNumber::from_addr(
            virt_to_phys(middle.as_ptr() as usize),
        ));
    }

    log!(
        MM,
        "stack window @ {:#x}: {} slots x {:#x} (+{} guard, emergency {:#x})",
        layout.window(),
        layout.slots(),
        layout.stack_size(),
        layout.guard(),
        layout.emergency()
    );
}

/// 故障 VA 是否落在某 guard 洞内：内核栈溢出的第一现场特征。
pub fn is_guard_fault(va: usize) -> bool {
    stack_layout().in_guard(va)
}

/// 用户内存访问的 RAII guard：构造时临时开启 SUM，Drop 恢复关闭。
/// 内核稳态 SUM=0 是不变量；只有完成地址验证并持有所需对象锁后，
/// user-copy 才允许经本 guard 临时开启。
pub struct SumGuard;

impl SumGuard {
    /// # Safety
    /// 调用方须已完成用户区间验证并持有所需锁，guard 存活期不重入调度。
    pub unsafe fn open() -> Self {
        // SAFETY: 仅置 sstatus.SUM 位。不得声明 nomem：guard 必须在优化后仍
        // 包围全部用户内存访问。
        unsafe { asm!("csrs sstatus, {bit}", bit = in(reg) 1 << 18, options(nostack)) };
        SumGuard
    }
}

impl Drop for SumGuard {
    fn drop(&mut self) {
        // SAFETY: 仅清 sstatus.SUM 位；与 open 对称地保留编译器内存屏障。
        unsafe { asm!("csrc sstatus, {bit}", bit = in(reg) 1 << 18, options(nostack)) };
    }
}
