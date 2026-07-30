use core::{
    cell::OnceCell,
    sync::atomic::{AtomicUsize, Ordering},
};

use erhino_shared::proc::Tid;

use crate::{
    external::{_memory_end, _memory_start, _user_trap},
    mm::page::{PageEntryFlag, PageTableEntry, PAGE_BITS},
};

use self::{page::PageEntryImpl, unit::MemoryUnit};

pub mod frame;
pub mod page;
pub mod unit;
pub mod usage;

type KernelUnit = MemoryUnit<PageEntryImpl>;

pub static mut KERNEL_UNIT: OnceCell<KernelUnit> = OnceCell::new();
#[unsafe(export_name = "_kernel_satp")]
pub static KERNEL_SATP: AtomicUsize = AtomicUsize::new(0);

#[allow(unused)]
pub enum ProcessAddressRegion {
    Invalid,
    Unknown,
    Program,
    Heap,
    Stack(Tid),
    TrapFrame(Tid),
}

pub fn init() {
    // NOTE: 有些实现要求 PTE 的 AD 位在访问前得是 1 否则会触发 page fault。内核必须设置 AD 强制全为 1。
    let memory_start = _memory_start as usize >> PAGE_BITS;
    let memory_end = _memory_end as usize >> PAGE_BITS;
    let mut unit = MemoryUnit::<PageEntryImpl>::new(0).unwrap();
    // mmio device space
    unit.map(0x0, 0x0, memory_start, PageEntryFlag::PrefabKernelDevice)
        .expect("map mmio device failed");
    // sbi + whole memory space
    unit.map(
        memory_start,
        memory_start,
        memory_end - memory_start,
        PageEntryFlag::PrefabKernelProgram,
    )
    .expect("map sbi + kernel space failed");
    // trampoline
    unit.map(
        PageEntryImpl::top_address() >> PAGE_BITS,
        _user_trap as usize >> PAGE_BITS,
        1,
        PageEntryFlag::PrefabKernelTrampoline,
    )
    .expect("map kernel trampoline failed");
    // kernel has no trap frame so it has no trap frame mapped
    let satp = unit.satp();
    unsafe {
        if let Err(_) = KERNEL_UNIT.set(unit) {
            panic!("set KERNEL_UNIT shared data failed")
        }
        KERNEL_SATP.store(satp, Ordering::Relaxed);
    }
}
