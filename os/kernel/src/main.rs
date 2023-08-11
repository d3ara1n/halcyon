#![no_std]
#![feature(lang_items, alloc_error_handler, panic_info_message)]
#![allow(unused)]

use core::arch::global_asm;

use external::{_kernel_end, _stack_size};
use tar_no_std::TarArchiveRef;

use crate::{mm::unit, task::proc::Process, hart::add_process};

extern crate alloc;

mod console;
mod external;
mod hart;
mod mm;
mod rt;
mod sbi;
mod sync;
mod task;
mod timer;
mod trap;

global_asm!(include_str!("assembly.asm"));

const LOGO: &str = include_str!("../logo.txt");
// 在文件系统未构建时用于测试的文件
const INITFS: &[u8] = include_bytes!("../../../artifacts/initfs.tar");

fn main() {
    // only #0 goes here to kernel init(AKA boot)
    println!("{}", LOGO);
    // device
    // load program with tar-no-std
    let archive = TarArchiveRef::new(INITFS);
    let systems = archive
        .entries()
        .filter(|f| f.filename().starts_with("init"));
    for system in systems {
        let process = Process::from_elf(system.data(), system.filename().as_str()).unwrap();
        add_process(process);
    }
}
