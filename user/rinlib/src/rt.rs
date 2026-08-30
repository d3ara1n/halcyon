use core::{alloc::Layout, panic::PanicInfo};
use erhino_shared::sync::spin::SimpleLock;
use erhino_shared::{mem::MemoryProtection, proc::Termination};
use talc::{TalcLock, base::Talc, base::binning::Binning, source::Source};

use crate::env;
use crate::mm::{MappedRegion, Placement};
use crate::{call::sys_exit, debug};

const INITIAL_ARENA_BYTES: usize = 64 * 1024;
const MAX_GEOMETRIC_ARENA_BYTES: usize = 16 * 1024 * 1024;
const MAX_HEAP_ARENAS: usize = 64;

#[derive(Debug)]
struct HeapSource {
    arenas: [Option<MappedRegion>; MAX_HEAP_ARENAS],
    arena_count: usize,
    next_arena_bytes: usize,
}

impl HeapSource {
    const fn new() -> Self {
        Self {
            arenas: [const { None }; MAX_HEAP_ARENAS],
            arena_count: 0,
            next_arena_bytes: INITIAL_ARENA_BYTES,
        }
    }
}

unsafe impl Source for HeapSource {
    fn acquire<B: Binning>(talc: &mut Talc<Self, B>, layout: Layout) -> Result<(), ()> {
        if talc.source.arena_count == MAX_HEAP_ARENAS {
            return Err(());
        }
        let required = layout.size().max(layout.align()).max(1);
        let geometric = required.checked_next_power_of_two().ok_or(())?;
        let arena_bytes = talc.source.next_arena_bytes.max(geometric);
        let region = MappedRegion::map_anonymous(
            arena_bytes,
            0,
            0,
            MemoryProtection::ReadWrite,
            Placement::Anywhere,
        )
        .map_err(|_| ())?;
        let usable = region.usable().ok_or(())?;
        let slot = talc.source.arena_count;
        talc.source.arenas[slot] = Some(region);
        // SAFETY: usable 是新 Map 的独占非空区间；token 已先进入固定容量
        // inventory，claim 成功后 allocator 成为该区间唯一字节分配者。
        if unsafe { talc.claim(usable.start as *mut u8, usable.end - usable.start) }.is_none() {
            let region = talc.source.arenas[slot]
                .take()
                .expect("heap arena token disappeared before claim");
            region
                .unmap()
                .unwrap_or_else(|_| panic!("failed heap claim could not release its mapping"));
            return Err(());
        }
        talc.source.arena_count += 1;
        talc.source.next_arena_bytes = arena_bytes.saturating_mul(2).min(MAX_GEOMETRIC_ARENA_BYTES);
        Ok(())
    }
}

#[global_allocator]
static HEAP_ALLOCATOR: TalcLock<SimpleLock, HeapSource> = TalcLock::new(HeapSource::new());

#[lang = "start"]
fn lang_start<T: Termination + 'static>(
    main: fn() -> T,
    argc: isize,
    argv: *const *const u8,
    _sigpipe: u8,
) -> isize {
    // 启动契约（shared::startup）：launch 在进程 runnable 前只读映射
    // StartupBlock，a0 持块基、a1 持块字节数（分别落在 argc/argv 槽位；
    // argv 槽位因此不再是保留未用，语义即「指针 + 长度」）。
    // 解析失败即拒绝启动，不进入 main。
    env::init(argc as usize as *const u8, argv as usize);
    {
        let mut talc = HEAP_ALLOCATOR.lock();
        <HeapSource as Source>::acquire(
            &mut talc,
            Layout::from_size_align(INITIAL_ARENA_BYTES, core::mem::align_of::<usize>())
                .expect("initial heap arena layout is invalid"),
        )
        .expect("initial heap arena allocation failed");
    }
    // 信号分发说明：内核只做置位与唤醒，进程级信号的接收/分发由程序
    // 自行安排（监听线程模式待多线程里程碑接入 rt；见 notes/ideas/signal.md）。
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
