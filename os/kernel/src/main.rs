#![no_std]
#![feature(lang_items, alloc_error_handler, panic_info_message)]
#![allow(internal_features)]
#![allow(static_mut_refs)]

use core::{arch::global_asm, slice::from_raw_parts};

use alloc::format;
use erhino_shared::{
    fal::{DentryAttribute, DentryType},
    path::Path,
};
use tar_no_std::TarArchiveRef;

use crate::{
    board::this_board,
    hart::SchedulerImpl,
    task::{proc::Process, sched::Scheduler},
};

extern crate alloc;

mod board;
mod console;
mod external;
mod fal;
mod fs;
mod hart;
mod mm;
mod rng;
mod rt;
mod sbi;
mod sync;
mod task;
mod timer;
mod trap;

const BANNER: &str = include_str!("../banner.txt");

global_asm!(include_str!("assembly.asm"));

pub fn main() {
    println!("{}", BANNER);
    let board = this_board();
    for cpu in board.map().cpus() {
        println!("[Hart #{}] {:?}@{:#x}", cpu.id(), cpu.mmu(), cpu.freq());
    }
    println!(
        "[IntrCtr] @{:#x}({:#x})",
        board.map().intrc().address(),
        board.map().intrc().size()
    );
    if let Some((addr, size)) = board.initfs() {
        println!("[InitFS ] @{:#x}({:#x})", addr, size);
        let ramfs = unsafe { from_raw_parts(addr as *const u8, size) };
        let archive = TarArchiveRef::new(ramfs).expect("initfs archive corrupt");
        let files = archive.entries();
        fs::create(
            Path::from("/boot").unwrap(),
            DentryType::Directory,
            DentryAttribute::Readable
                | DentryAttribute::Executable
                | DentryAttribute::PrivilegedWriteable,
        )
        .unwrap();
        for file in files {
            let filename = file.filename();
            let name = filename.as_str().unwrap_or("");
            let path = Path::from(&format!("/boot/{}", name)).unwrap();
            let parent = path.parent().unwrap();
            fs::make_directory(
                parent,
                DentryAttribute::Readable
                    | DentryAttribute::Executable
                    | DentryAttribute::PrivilegedWriteable,
            )
            .unwrap();
            fs::create_memory_stream(
                path,
                file.data(),
                DentryAttribute::Executable | DentryAttribute::Readable,
            )
            .unwrap();
            if name.starts_with("bin/") {
                let process = Process::from_elf(file.data()).unwrap();
                SchedulerImpl::add(process, None);
            }
        }
        println!("\x1b[0;32m=LINK^START=\x1b[0m");
    } else {
        println!("\x1b[0;32m=STAND^BY=\x1b[0m");
    }
}
