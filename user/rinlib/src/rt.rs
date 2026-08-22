use core::{alloc::Layout, panic::PanicInfo, ptr::NonNull};
use erhino_shared::proc::{Pid, SystemSignal, Termination};
use erhino_shared::sync::spin::SimpleLock;
use talc::{base::binning::Binning, base::Talc, source::Source, TalcLock};

use crate::call::sys_extend;
use crate::env;
use crate::{call::sys_exit, debug, ipc::signal};

const INITIAL_HEAP_SIZE: usize = 1 * 0x1000;

#[derive(Debug)]
struct HeapRecuse {
    heap_end: usize,
}

impl HeapRecuse {
    const fn new() -> Self {
        Self { heap_end: 0 }
    }
}

unsafe impl Source for HeapRecuse {
    fn acquire<B: Binning>(talc: &mut Talc<Self, B>, layout: Layout) -> Result<(), ()> {
        // sbrk 语义：Extend(0) 查询堆顶，Extend(n) 申请 n 字节返回新堆顶。
        // 页大小是内核实现细节，用户态只认字节；实际获得区间由前后两次
        // 堆顶差值得出（含内核取整），不自行推算。
        let old_end = if talc.source.heap_end != 0 {
            talc.source.heap_end
        } else {
            unsafe { sys_extend(0) }.map_err(|_| ())?
        };
        let new_end = unsafe { sys_extend(layout.size()) }.map_err(|_| ())?;
        if new_end <= old_end {
            return Err(());
        }
        let size = new_end - old_end;
        let base = old_end as *mut u8;
        // SAFETY: [base, new_end) 是 sys_extend 刚向内核申请的内存，独占交
        // 给分配器；首次 claim 建区，后续沿旧堆顶向新堆顶 extend。
        let end = unsafe {
            if talc.source.heap_end == 0 {
                talc.claim(base, size).ok_or(())?
            } else {
                talc.extend(NonNull::new(old_end as *mut u8).ok_or(())?, new_end as *mut u8)
            }
        };
        talc.source.heap_end = end.as_ptr() as usize;
        Ok(())
    }
}

#[global_allocator]
static HEAP_ALLOCATOR: TalcLock<SimpleLock, HeapRecuse> = TalcLock::new(HeapRecuse::new());

#[lang = "start"]
fn lang_start<T: Termination + 'static>(
    main: fn() -> T,
    argc: isize,
    argv: *const *const u8,
    _sigpipe: u8,
) -> isize {
    let pid = argc as usize as Pid;
    let parent = argv as usize as Pid;
    unsafe {
        env::set_pid(pid);
        env::set_parent_pid(parent);
        let mut talc = HEAP_ALLOCATOR.lock();
        // sbrk 语义（见 HeapRecuse::acquire）：查询起点、申请 INITIAL_HEAP_SIZE
        // 字节、以返回值差值为实际获得区间。
        let start = sys_extend(0).expect("heap base query failed");
        let end = sys_extend(INITIAL_HEAP_SIZE).expect("initial heap allocation failed");
        debug_assert!(end > start);
        // SAFETY: [start, end) 是刚申请的内存，独占交给分配器。
        let heap_end = talc
            .claim(start as *mut u8, end - start)
            .expect("initial heap claim failed");
        talc.source.heap_end = heap_end.as_ptr() as usize;
    }
    signal::set_handler(SystemSignal::Terminate, default_signal_handler);
    let code = main().to_exit_code();
    unsafe {
        loop {
            sys_exit(code).expect("this can't be wrong");
        }
    }
}

#[panic_handler]
fn handle_panic(info: &PanicInfo) -> ! {
    if let Some(location) = info.location() {
        debug!(
            "Panicking in {} at line {}: {}",
            location.file(),
            location.line(),
            info.message()
        );
    } else {
        debug!("Panicking: no information available.");
    }
    unsafe {
        loop {
            sys_exit(-1).expect("this can't be wrong");
        }
    }
}

#[alloc_error_handler]
fn handle_alloc_error(layout: Layout) -> ! {
    panic!("Heap allocation error, layout = {:?}", layout);
}

fn default_signal_handler(signal: SystemSignal) {
    match signal {
        SystemSignal::Terminate => unsafe {
            sys_exit(1).expect("no wish to die");
        },
        _ => {}
    };
}
