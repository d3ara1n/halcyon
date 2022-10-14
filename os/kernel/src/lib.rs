#![feature(lang_items, alloc_error_handler, panic_info_message, linkage)]
#![no_std]
#![allow(dead_code)]

use core::arch::global_asm;

use board::BoardInfo;
pub use erhino_shared::*;

extern crate alloc;

// public module should be initialized and completely available before board main function
pub mod board;
pub mod console;
pub mod env;
mod external;
mod mm;
mod peripheral;
mod pmp;
pub mod proc;
mod rt;
mod schedule;
pub mod sync;
mod trap;

global_asm!(include_str!("assembly.asm"));

pub fn kernel_init(info: BoardInfo) {
    println!("boot stage #3: kernel initialization");
    println!("{}", info);
    peripheral::init(&info);
    println!("boot stage #4: prepare user environment");
    println!("boot completed, enter user mode");

    // 内核任务完成了， 回收免得 board 占用 uart 设备
    // 把任务转到 console 设备上
}

pub fn kernel_main() {}
