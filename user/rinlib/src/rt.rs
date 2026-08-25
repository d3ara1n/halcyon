use core::{alloc::Layout, panic::PanicInfo, ptr::NonNull};
use erhino_shared::{
    proc::{Pid, Termination},
    startup::{MESSAGE_KIND_STARTUP, STARTUP_VERSION, StartupGrant, StartupHeader},
};
use erhino_shared::sync::spin::SimpleLock;
use talc::{base::binning::Binning, base::Talc, source::Source, TalcLock};

use crate::call::{sys_extend, sys_startup_mailbox};
use crate::env;
use crate::{call::sys_exit, debug};

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
        let startup_mailbox = sys_startup_mailbox().expect("startup mailbox query failed");
        env::set_startup_mailbox(startup_mailbox);
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
    load_startup();
    // 信号分发说明：内核只做置位与唤醒，进程级信号的接收/分发由程序
    // 自行安排（监听线程模式待多线程里程碑接入 rt；见 notes/ideas/signal.md）。
    let code = main().to_exit_code();
    unsafe {
        loop {
            sys_exit(code).expect("this can't be wrong");
        }
    }
}

fn load_startup() {
    let message = crate::ipc::message::receive(env::startup_mailbox())
        .expect("startup message receive failed");
    assert!(message.header.kind == MESSAGE_KIND_STARTUP, "invalid startup message kind");
    assert!(
        message.payload.len() >= core::mem::size_of::<StartupHeader>(),
        "truncated startup message"
    );
    // SAFETY: 长度已检查，read_unaligned 不要求 payload 对齐；结构只有整数字段。
    let header = unsafe {
        core::ptr::read_unaligned(message.payload.as_ptr().cast::<StartupHeader>())
    };
    assert!(
        header.version == STARTUP_VERSION && header.kind == 0 && header.reserved == [0; 2],
        "unsupported startup message"
    );
    let grants_len = (header.grant_count as usize)
        .checked_mul(core::mem::size_of::<StartupGrant>())
        .expect("startup grant length overflow");
    let expected = core::mem::size_of::<StartupHeader>()
        .checked_add(grants_len)
        .expect("startup message length overflow");
    assert!(message.payload.len() == expected, "invalid startup message length");
    assert!(
        message.handles.len() == header.grant_count as usize,
        "startup grant/handle count mismatch"
    );
    let grants_ptr = unsafe { message.payload.as_ptr().add(core::mem::size_of::<StartupHeader>()) };
    // SAFETY: payload 长度覆盖全部元素；逐项 unaligned 读取到本地数组。
    let mut grants = alloc::vec::Vec::new();
    grants
        .try_reserve_exact(header.grant_count as usize)
        .expect("startup grant allocation failed");
    for index in 0..header.grant_count as usize {
        let ptr = unsafe { grants_ptr.add(index * core::mem::size_of::<StartupGrant>()) };
        grants.push(unsafe { core::ptr::read_unaligned(ptr.cast::<StartupGrant>()) });
    }
    assert!(grants.iter().all(|grant| grant.reserved == 0), "invalid startup grant");
    env::set_startup_grants(&grants, &message.handles);
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
