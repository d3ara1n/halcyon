//! erhino 内核（重写版）。
//!
//! 架构与内部机制见 `notes/`；旧实现的设计考古见
//! `plans/2026-08-legacy-kernel-design.md`。

#![no_std]
#![feature(lang_items, alloc_error_handler)]
#![allow(internal_features)]

extern crate alloc;

use core::arch::global_asm;
use dtb_parser::DeviceTree;

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

pub fn main() {
    crate::println!("{}", BANNER);

    // 经高半区直映射访问 DTB（PA 访问仅限 .text.init 的裸机段；正式
    // 内核表无低半区 identity，PA 直访在切表后会 page fault）。
    let dtb_va = mm::phys_to_virt(rt::dtb());
    let board = board::parse(DeviceTree::from_address(dtb_va).expect("设备树不可用"));

    for region in &board.memories {
        crate::println!("[Memory  ] @{:#x} ({:#x})", region.start, region.len);
    }
    crate::println!("[Timebase] {} Hz", board.timebase);
    if let Some((addr, len)) = board.initfs {
        crate::println!("[InitFS  ] @{:#x} ({:#x})", addr, len);
    }

    for cpu in &board.cpus {
        crate::println!("[Hart #{:>2}] {:?} @ {} Hz", cpu.hartid, cpu.mmu, cpu.freq);
    }

    mm::init(&board);
    frame::init(&board);
    frame::smoke();

    crate::println!("[Hart #{:>2}] online (boot)", hart::hartid());
    wake_secondary_harts(&board);
    hart::park();
}

/// 按 CPU 列表唤醒除 boot hart 外的全部 hart。
fn wake_secondary_harts(board: &board::BoardInfo) {
    let boot = hart::hartid();
    for cpu in &board.cpus {
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
