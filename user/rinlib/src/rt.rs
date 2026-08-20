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
        let mut count = 1;
        let single = 4096;
        while count * single < layout.size() {
            count *= 2;
        }
        let old_end = talc.source.heap_end;
        if let Ok(offset) = unsafe { sys_extend(count) } {
            let size = count * single;
            let new_end = offset as *mut u8;
            let base = (offset - size) as *mut u8;
            // SAFETY: [base, offset) 是 sys_extend 刚向内核申请的内存，独占交给分配器；
            // 首次 claim，之后假设新内存与既有 heap 连续而 extend。
            let end = if old_end == 0 {
                unsafe { talc.claim(base, size).expect("initial heap claim failed") }
            } else {
                unsafe { talc.extend(NonNull::new(old_end as *mut u8).unwrap(), new_end) }
            };
            talc.source.heap_end = end.as_ptr() as usize;
            Ok(())
        } else {
            Err(())
        }
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
        if let Ok(offset) = sys_extend(INITIAL_HEAP_SIZE) {
            let start = offset - INITIAL_HEAP_SIZE;
            // SAFETY: [start, offset) 是 sys_extend 刚申请的内存，交给分配器。
            if let Some(heap_end) = talc.claim(start as *mut u8, INITIAL_HEAP_SIZE) {
                talc.source.heap_end = heap_end.as_ptr() as usize;
            } else {
                panic!();
            }
        } else {
            panic!();
        }
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
