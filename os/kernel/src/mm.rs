//! 内核地址空间与启动切换（见 notes/mm.md「内核地址空间与启动协议」）。
//!
//! 汇编侧机构：`_start` 在 bare satp 下写跳板 root 表（DRAM 槽 identity
//! + 高半区别名）开 MMU 跳高半区；本模块在高半区构建正式内核页表
//! （静态 root + 直映射 mega 项）并切换，`KERNEL_SATP` 供 secondary
//! hart 的 `_awaken_high` 加载同一张表。
//!
//! 页表模式当前固定 Sv39；按 DTB mmu-type 自动选式是后续工作
//! （见 notes/mm.md「页表模式选择」）。

use core::{
    arch::asm,
    cell::UnsafeCell,
    sync::atomic::{AtomicUsize, Ordering},
};

use page_table::{flags, Ppn, Pte, ENTRIES};

use crate::board::BoardInfo;

/// 内核高半区基址：VMA = PA + KERNEL_VA_BASE（与链接脚本常量一致）。
pub const KERNEL_VA_BASE: usize = 0xFFFF_FFC0_0000_0000;

/// satp 模式：Sv39。
const SV39: usize = 8;

/// 直映射粒度：1GiB mega 项（sv39 顶层）。
const GIB: usize = 1 << 30;

/// 直映射 vpn2 起始槽（高半区首槽；由 KERNEL_VA_BASE 推得）。
const DIRECT_VPN2_BASE: usize = 256;

/// sv39 顶层可用槽数（直映射上限 [0, 2^38) 物理）。
const DIRECT_VPN2_LIMIT: usize = 256;

pub fn phys_to_virt(pa: usize) -> usize {
    pa + KERNEL_VA_BASE
}

pub fn virt_to_phys(va: usize) -> usize {
    va - KERNEL_VA_BASE
}

/// 正式内核页表 root。静态表（Linux swapper_pg_dir 同构）：帧池就绪前
/// 就要建直映射，root 不入池、永不释放。
#[repr(align(4096))]
struct RootTable(UnsafeCell<[Pte; ENTRIES]>);

// SAFETY: 仅 boot 早期单 hart 写入一次，其后只读；UnsafeCell 隔离
// 初始化期可变访问，避免 static mut。
unsafe impl Sync for RootTable {}

static KERNEL_PG_DIR: RootTable = RootTable(UnsafeCell::new([Pte::invalid(); ENTRIES]));

/// 汇编可寻址的内核 satp 值（`_awaken_high` 加载切换）。
#[unsafe(no_mangle)]
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

/// 把内核高半区顶层项拷进用户表 root：用户表创建后立即调用，
/// 此后任意用户 satp 下内核代码恒可执行（共享映射，见 notes/internals.md）。
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
}

pub fn init(board: &BoardInfo) {
    let dram_end = board
        .memories()
        .iter()
        .map(|r| r.start + r.len)
        .max()
        .expect("DTB 无 memory 节点");
    let slots = (dram_end + GIB - 1) / GIB;
    assert!(
        (1..=DIRECT_VPN2_LIMIT).contains(&slots),
        "直映射槽数异常：{slots}"
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
