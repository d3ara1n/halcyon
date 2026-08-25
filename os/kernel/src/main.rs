//! erhino 内核（重写版）。
//!
//! 架构与内部机制见 `notes/`；旧实现的设计考古见
//! `plans/ref-2026-08-legacy-kernel-design.md`。

#![no_std]
#![feature(lang_items, alloc_error_handler)]
#![allow(internal_features)]

extern crate alloc;

use core::arch::global_asm;

use dtb::Fdt;

use crate::{
    board::BoardInfo,
    registry::{HartBootRecord, HartRegistry},
};

// 宏经文本作用域全 crate 可见（#[macro_use]），模块内裸用 log!/println!。
#[macro_use]
mod console;
mod abi;
mod board;
#[expect(dead_code)]
mod fp;
// 执行环境准备态模块：原子切换接线后 dead_code 预期消除。
mod context;
mod csr;
mod registry;
mod external;
mod frame;
mod hart;
mod heap;
mod initfs;
mod mm;
mod rt;
mod sbi;
mod sched;
mod sync;
mod syscall;
mod task;
mod trap;
mod uaccess;

// 汇编布局契约：offset_of! 是唯一真值，经 const operands 注入
// assembly.asm（见 abi::asm 常量表）。
global_asm!(
    include_str!("assembly.asm"),
    REC_STATE = const abi::asm::REC_STATE,
    REC_KERNEL_SATP = const abi::asm::REC_KERNEL_SATP,
    REC_ENTRY_HIGH = const abi::asm::REC_ENTRY_HIGH,
    REC_HART_LOCAL = const abi::asm::REC_HART_LOCAL,
    REC_STACK_TOP = const abi::asm::REC_STACK_TOP,
    REC_EMERGENCY_SP = const abi::asm::REC_EMERGENCY_SP,
    REC_SLOT = const core::mem::offset_of!(HartBootRecord, slot),
    HL_HARTID = const abi::asm::HL_HARTID,
    HL_KERNEL_SP = const abi::asm::HL_KERNEL_SP,
    HL_SCHED_SP = const abi::asm::HL_SCHED_SP,
    HL_FRAME_PTR = const abi::asm::HL_FRAME_PTR,
    HL_USER_SATP = const abi::asm::HL_USER_SATP,
    HL_SCRATCH = const abi::asm::HL_SCRATCH,
    HL_SCRATCH2 = const abi::asm::HL_SCRATCH2,
    HL_SLOT = const abi::asm::HL_SLOT,
    HL_EMERGENCY_SP = const abi::asm::HL_EMERGENCY_SP,
    HL_FATAL_GUARD = const abi::asm::HL_FATAL_GUARD,
    HL_FP_ENABLED = const abi::asm::HL_FP_ENABLED,
    HL_FATAL_SP = const abi::asm::HL_FATAL_SP,
    HL_RESERVATION = const abi::asm::HL_RESERVATION,
    UC_X0 = const abi::asm::UC_X0,
    UC_X30 = const abi::asm::UC_X30,
    UC_X31 = const abi::asm::UC_X31,
    UC_SEPC = const abi::asm::UC_SEPC,
    UC_FP = const abi::asm::UC_FP,
    FF_X0 = const abi::asm::FF_X0,
    FF_X2 = const abi::asm::FF_X2,
    FF_X10 = const abi::asm::FF_X10,
    FF_X11 = const abi::asm::FF_X11,
    FF_X30 = const abi::asm::FF_X30,
    FF_X31 = const abi::asm::FF_X31,
    FF_SCAUSE = const abi::asm::FF_SCAUSE,
    FF_STVAL = const abi::asm::FF_STVAL,
    FF_SEPC = const abi::asm::FF_SEPC,
    FF_SATP = const abi::asm::FF_SATP,
    FF_SSTATUS = const abi::asm::FF_SSTATUS,
    FF_SIZE = const abi::asm::FF_SIZE,
    CSR_FS_CLEAN = const abi::asm::CSR_FS_CLEAN,
    CSR_FS_MASK = const abi::asm::CSR_FS_MASK,
    CSR_PRE_SRET_CLEAR = const abi::asm::CSR_PRE_SRET_CLEAR,
    SF_RA = const abi::asm::SF_RA,
    SF_S0 = const abi::asm::SF_S0,
    SF_SIZE = const abi::asm::SF_SIZE,
);

const BANNER: &str = include_str!("../banner.txt");

/// emergency 栈大小（每 hart 正式栈区最高页）。
const EMERGENCY_SIZE: usize = 4096;

/// 从物理地址构造 DTB 视图（地址纪律：PA 经 phys_to_virt，totalsize 定界）。
///
/// # Safety
/// DTB PA 必须位于直映射覆盖的 DRAM 内（OpenSBI 契约），头部可信。
unsafe fn fdt_from(pa: usize) -> Fdt<'static> {
    let va = mm::phys_to_virt(pa);
    // SAFETY: 读 DTB 头 totalsize（偏移 4，u32 大端）。
    let total = unsafe { ((va + 4) as *const u32).read_volatile().swap_bytes() } as usize;
    // SAFETY: [va, va+total) 在直映射覆盖内，只读访问。
    let data = unsafe { core::slice::from_raw_parts(va as *const u8, total) };
    Fdt::new(data).expect("device tree unavailable")
}

pub fn main() {
    println!("{}", BANNER);

    let fdt = unsafe { fdt_from(rt::boot_dtb()) };
    let mut board = board::parse(&fdt);

    for region in board.memories() {
        log!(Memory, "@{:#x} ({:#x})", region.start, region.len);
    }
    log!(Timebase, "{} Hz", board.timebase);
    if let Some((addr, len)) = board.initfs {
        log!(InitFS, "@{:#x} ({:#x})", addr, len);
    }

    for cpu in board.cpus() {
        log!(Hart, "#{:>2} {:?} @ {} Hz", cpu.hartid, cpu.mmu, cpu.freq);
    }

    mm::init(&board);
    frame::init(&board);
    frame::selftest();
    heap::selftest();
    // cpu-map 拓扑解析允许用堆，帧池/堆就绪后进行（可选属性）。
    board.load_topology(&fdt);
    sched::init(board.timebase);
    if let Some((addr, len)) = board.initfs {
        rt::set_initfs_region(addr, len);
    }

    construct_registry(&board);
    // 非返回：切换正式执行环境（gp/tp/sscratch/栈/CSR/stvec），发布 Online，
    // 经 rt::bring_up_runtime 完成启动收尾。bootstrap 环境自此废弃。
    let boot_record = registry::with_registry(|reg| {
        let boot_slot = reg
            .slot_of(rt::boot_hartid())
            .expect("boot hart not admitted by device tree");
        reg.record(boot_slot) as *const HartBootRecord as usize
    });
    enter_hart(boot_record, rt::boot_hartid());
}

/// 非返回进入正式执行环境（两条启动路径的唯一汇合点）。
fn enter_hart(record: usize, hartid: usize) -> ! {
    // SAFETY: record 位于直映射静态存储，生命周期 'static；
    // 目标为非返回汇编入口，寄存器契约见 _enter_hart_high。
    unsafe {
        core::arch::asm!(
            "j _enter_hart_high",
            in("a0") hartid,
            in("a1") record,
            options(noreturn)
        );
    }
}

/// boot 单核构造正式 registry 与全部启动记录（notes/impls/execution-context.md
/// 「Bootstrap 与正式环境」）：admitted raw hartid 升序分配稠密 slot；
/// record 先完整构造并发布，HSM start 在 bring_up_runtime 中才发出。
fn construct_registry(board: &BoardInfo) {
    let mut reg = HartRegistry::empty();
    for cpu in board.cpus() {
        if cpu.mmu == board::MmuType::Bare {
            continue; // 无分页 hart 不准入
        }
        let slot = reg.admit(cpu.hartid);
        registry::store_caps(slot, &cpu.caps);
    }
    assert!(
        reg.slot_of(rt::boot_hartid()).is_some(),
        "boot hart not admitted by device tree"
    );

    let kernel_satp = mm::kernel_satp();
    let entry_high = external::enter_hart_high_va();
    let stack_size = external::hart_stack_size();

    for (slot, record) in reg.records_mut() {
        record.kernel_satp = kernel_satp;
        record.entry_high = entry_high;
        record.hart_local = hart::hart_local_addr(slot.0);
        // 栈窗口布局（见 mm::stack_slot_range）：每槽步长 stack_size + guard，
        // 槽内最高页划为 emergency 栈；正式 sp 从其下开始。
        let (_, top) = mm::stack_slot_range(slot.0, stack_size);
        record.emergency_sp = top;
        record.stack_top = top - EMERGENCY_SIZE;
    }
    reg.record_mut(reg.slot_of(rt::boot_hartid()).unwrap()).role_boot = 1;

    registry::install(reg);
}
