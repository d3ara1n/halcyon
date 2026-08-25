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

use page_table::{FrameNumber, PAGE_BITS, Ppn, Pte, flags, ENTRIES};
use stack_layout::StackWindowLayout;
use stack_layout::PAGE_SIZE;

use crate::{board::BoardInfo, external};

/// 内核高半区基址：VMA = PA + KERNEL_VA_BASE（与链接脚本常量一致）。
pub const KERNEL_VA_BASE: usize = 0xFFFF_FFC0_0000_0000;

/// satp 模式：Sv39。
const SV39: usize = 8;

/// 直映射粒度：1GiB mega 项（sv39 顶层）。
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

pub fn phys_to_virt(pa: usize) -> usize {
    pa + KERNEL_VA_BASE
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

static WINDOW_TABLES: WindowTables = WindowTables(UnsafeCell::new([[Pte::invalid(); ENTRIES]; 1 + WINDOW_LEAF_MAX]));

/// 正式内核 satp 值（init 发布、`kernel_satp()` 消费填 record）。
static KERNEL_SATP: AtomicUsize = AtomicUsize::new(0);

/// 构建并启用内核直映射：PA `[0, N GiB)` 以 1GiB mega 项映射到高半区，
/// N 覆盖全部 DRAM 与首 GiB 内的 MMIO 窗口；随后切换 satp 并广播。
///
/// 切换安全性：镜像/栈/跳板表都在 DRAM 槽内，切换前后 VMA 不变
/// （跳板别名与直映射对同一物理段呈现相同 VMA），执行流无缝。
/// 直映射 vpn2 槽位数（init 后恒定，用户表 root 拷贝用）。
static DIRECT_SLOT_COUNT: AtomicUsize = AtomicUsize::new(0);

/// 已建立的直映射顶层槽数（高半区 [256, 256+n)）。
pub fn direct_slots() -> usize {
    DIRECT_SLOT_COUNT.load(Ordering::Relaxed)
}

/// 用户 root 中归内核所有的顶层槽区间（直映射区 + 栈窗口），
/// 与拷贝范围严格对应；剥离用。
pub fn kernel_top_level_range() -> (usize, usize, usize) {
    // (direct_start, direct_end, window_slot)
    let slots = direct_slots();
    (DIRECT_VPN2_BASE, DIRECT_VPN2_BASE + slots, STACK_WINDOW_SLOT)
}

/// 把内核高半区顶层项拷进用户表 root：用户表创建后立即调用，
/// 此后任意用户 satp 下内核代码恒可执行（共享映射，见 notes/impls/internals.md）。
/// 配对纪律：进程 teardown 前必须先 `detach_kernel_top_level` 剥离，
/// 否则树回收会把内核共享子表当用户页表拆掉。
///
/// # Safety
/// `root` 必须是刚分配、尚未映射任何用户页的页表 root 帧。
pub unsafe fn install_kernel_top_level(root: page_table::FrameNumber) {
    // SAFETY: 静态表 init 后只读；目标 root 帧刚分配归调用方。
    let src = unsafe { &*KERNEL_PG_DIR.0.get() };
    let dst = unsafe {
        &mut *(phys_to_virt(root.addr()) as *mut [Pte; ENTRIES])
    };
    let slots = direct_slots();
    dst[DIRECT_VPN2_BASE..DIRECT_VPN2_BASE + slots]
        .copy_from_slice(&src[DIRECT_VPN2_BASE..DIRECT_VPN2_BASE + slots]);
    // 栈窗口槽同样共享：trap 在用户 satp 下即取调度栈指针（无 U 位，
    // 用户态不可访问）。子表 init 后只读，跨地址空间共享安全。
    dst[STACK_WINDOW_SLOT] = src[STACK_WINDOW_SLOT];
}

pub fn init(board: &BoardInfo) {
    let dram_end = board
        .memories()
        .iter()
        .map(|r| r.start + r.len)
        .max()
        .expect("DTB has no memory node");
    let slots = (dram_end + GIB - 1) / GIB;
    assert!(
        (1..=DIRECT_VPN2_LIMIT).contains(&slots),
        "unexpected direct-map slot count: {slots}"
    );

    // SAFETY: boot 早期单 hart 独占（UnsafeCell 隔离），此后只读。
    let dir: *mut [Pte; ENTRIES] = KERNEL_PG_DIR.0.get();
    for slot in 0..slots {
        // SAFETY: 同上，独占写静态表。
        unsafe {
            (*dir)[DIRECT_VPN2_BASE + slot] =
                Pte::leaf(Ppn(slot << 18), flags::KERNEL_DIRECT);
        }
    }

    let satp = (SV39 << 60) | (virt_to_phys(dir as usize) >> 12);

    map_stack_window();

    KERNEL_SATP.store(satp, Ordering::Release);
    DIRECT_SLOT_COUNT.store(slots, Ordering::Relaxed);
    // SAFETY: satp 装载与全量 sfence 是 S 态特权指令；直映射已覆盖
    // 当前执行流的全部后续访问（代码/数据/栈同 VMA 换底）。
    unsafe {
        asm!(
            "csrw  satp, {satp}",
            "sfence.vma",
            satp = in(reg) satp,
        );
    }

    log!(
        MM,
        "direct map [0, {:#x}), kernel @ {:#x}",
        slots * GIB,
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
                leaves[unit].as_ptr() as usize,
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
        (*KERNEL_PG_DIR.0.get())[STACK_WINDOW_SLOT] =
            Pte::branch(FrameNumber::from_addr(virt_to_phys(middle.as_ptr() as usize)));
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

/// 调度循环入口归一：当前 satp 非正式内核表则切回并同步翻译。
///
/// 离开用户执行点后，刚结束线程的 root 可能已被 teardown 剥离内核
/// 顶层项（`AddressSpace::drop`），不得再作为内核执行的翻译来源。
/// Resume 热路径不经调度循环，此处切换零热路径开销。
/// 已知简化：归一分布于本处与 report_exit；接入新终止来源（kill/
/// 线程退出）时收敛为非 Resume 出口统一切内核表
/// （notes/impls/execution-context.md「地址空间归属纪律」）。
pub fn normalize_satp() {
    let want = kernel_satp();
    let cur: usize;
    // SAFETY: 只读 satp。
    unsafe { asm!("csrr {}, satp", out(reg) cur, options(nomem)) };
    if cur != want {
        // SAFETY: 换表后全量 sfence；内核高半区两表同 VA，执行流无缝。
        unsafe { asm!("csrw satp, {satp}", "sfence.vma", satp = in(reg) want) };
    }
}

/// 用户内存访问的 RAII guard：构造时临时开启 SUM，Drop 恢复关闭。
/// 内核稳态 SUM=0 是不变量；只有完成地址验证并持有所需对象锁后，
/// user-copy 才允许经本 guard 临时开启。
pub struct SumGuard;

impl SumGuard {
    /// # Safety
    /// 调用方须已完成用户区间验证并持有所需锁，guard 存活期不重入调度。
    pub unsafe fn open() -> Self {
        // SAFETY: 仅置 sstatus.SUM 位。
        unsafe { asm!("csrs sstatus, {bit}", bit = in(reg) 1 << 18, options(nomem)) };
        SumGuard
    }
}

impl Drop for SumGuard {
    fn drop(&mut self) {
        // SAFETY: 仅清 sstatus.SUM 位。
        unsafe { asm!("csrc sstatus, {bit}", bit = in(reg) 1 << 18, options(nomem)) };
    }
}
