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

mod board;
mod console;
mod external;
mod frame;
mod hart;
mod heap;
mod mm;
mod rt;
mod sbi;
mod sync;
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
    crate::println!("{}", BANNER);

    // SAFETY: dtb PA 来自 boot 契约（a1），位于直映射覆盖的 DRAM 内。
    let fdt = unsafe { fdt_from(rt::dtb()) };
    let board = board::parse(&fdt);

    for region in board.memories() {
        crate::println!("[Memory  ] @{:#x} ({:#x})", region.start, region.len);
    }
    crate::println!("[Timebase] {} Hz", board.timebase);
    if let Some((addr, len)) = board.initfs {
        crate::println!("[InitFS  ] @{:#x} ({:#x})", addr, len);
    }

    for cpu in board.cpus() {
        crate::println!("[Hart #{:>2}] {:?} @ {} Hz", cpu.hartid, cpu.mmu, cpu.freq);
    }

    mm::init(&board);
    frame::init(&board);
    frame::smoke();
    heap::smoke();

    crate::println!("[Hart #{:>2}] online (boot)", hart::hartid());
    wake_secondary_harts(&board);
    hart::park();
}

/// 按 CPU 列表唤醒除 boot hart 外的全部 hart。
fn wake_secondary_harts(board: &board::BoardInfo) {
    let boot = hart::hartid();
    for cpu in board.cpus() {
        if cpu.hartid == boot || cpu.mmu == board::MmuType::Bare {
            continue;
        }
        assert!(
            cpu.hartid < hart::HART_NUM_LIMIT,
            "hart {} 超出 HART_NUM_LIMIT",
            cpu.hartid
        );
        let entry = rt::secondary_entry as *const () as usize;
        let awaken = external::awaken_pa();
        match sbi::hart_start(cpu.hartid, awaken, entry) {
            Ok(_) => {}
            Err(err) => crate::warning!("hart {} 启动失败: {:?}", cpu.hartid, err),
        }
    }
}
