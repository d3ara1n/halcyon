//! erhino 内核（重写版）。
//!
//! 架构与内部机制见 `notes/`；旧实现的设计考古见
//! `plans/2026-08-legacy-kernel-design.md`。

#![no_std]
#![feature(lang_items, alloc_error_handler)]
#![allow(internal_features)]

extern crate alloc;

use core::arch::global_asm;
use dtb::Fdt;

// 宏经文本作用域全 crate 可见（#[macro_use]），模块内裸用 log!/println!。
#[macro_use]
mod console;
mod board;
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
global_asm!(include_str!("assembly.asm"));

const BANNER: &str = include_str!("../banner.txt");

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
    Fdt::new(data).expect("设备树不可用")
}

pub fn main() {
    println!("{}", BANNER);

    // SAFETY: dtb PA 来自 boot 契约（a1），位于直映射覆盖的 DRAM 内。
    let fdt = unsafe { fdt_from(rt::dtb()) };
    let board = board::parse(&fdt);

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
    frame::smoke();
    heap::smoke();
    sched::init(board.timebase);

    info!(Hart, "#{:>2} online (boot)", hart::hartid());
    if let Some((addr, len)) = board.initfs {
        initfs::load(addr, len);
    } else {
        log!(InitFS, "设备树无 initfs，无服务可装载");
    }
    wake_secondary_harts(&board);
    sched::run()
}

/// 按 CPU 列表唤醒除 boot hart 外的全部 hart，并登记预期在线集合
/// （静默停机判定要求全员到齐 idle）。
fn wake_secondary_harts(board: &board::BoardInfo) {
    let boot = hart::hartid();
    for cpu in board.cpus() {
        if cpu.mmu == board::MmuType::Bare {
            continue;
        }
        assert!(
            cpu.hartid < hart::HART_NUM_LIMIT,
            "hart {} 超出 HART_NUM_LIMIT",
            cpu.hartid
        );
        sched::expect_hart(cpu.hartid);
        if cpu.hartid == boot {
            continue;
        }
        let entry = rt::secondary_entry as *const () as usize;
        let awaken = external::awaken_pa();
        sbi::require(sbi::hart_start(cpu.hartid, awaken, entry), "HSM.hart_start");
    }
}
