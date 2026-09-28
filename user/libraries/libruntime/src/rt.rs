use core::{alloc::Layout, panic::PanicInfo};
#[cfg(target_arch = "riscv64")]
use core::arch::asm;

use erhino_shared::mem::MemoryProtection;
use erhino_shared::sync::spin::SimpleLock;
use libkernel::{
    dbg,
    mm::{MappedRegion, Placement},
    startup as kernel_startup,
    sys_process_abort, sys_thread_exit,
    ThreadControl,
};
use libprocess::context::StartupContext;
use libthread::{NoEnvironment, ThreadRegistry};
use lock_api::Mutex;
use talc::{base::binning::Binning, base::Talc, source::Source, TalcLock};

unsafe extern "C" {
    fn __halcyon_main() -> !;
}

pub trait MainOutcome {
    fn finish(self) -> !;
}

impl MainOutcome for () {
    fn finish(self) -> ! {
        // SAFETY: successful application completion leaves no user result work.
        unsafe { sys_thread_exit() }
    }
}

impl<E> MainOutcome for Result<(), E> {
    fn finish(self) -> ! {
        match self {
            Ok(()) => {
                // SAFETY: successful application completion leaves no user result work.
                unsafe { sys_thread_exit() }
            }
            Err(_) => {
                // SAFETY: an unsuccessful application result is process failure.
                unsafe { sys_process_abort() }
            }
        }
    }
}

pub fn finish<T: MainOutcome>(outcome: T) -> ! {
    outcome.finish()
}

type Lock = SimpleLock;

static ENTRY_REGISTRY: Mutex<Lock, Option<ThreadRegistry<NoEnvironment>>> =
    Mutex::const_new(Lock::new(), None);
static STARTUP_CONTEXT: Mutex<Lock, Option<StartupContext>> = Mutex::const_new(Lock::new(), None);

fn adopt_entry_thread() {
    let mut context = unsafe { StartupContext::adopt() }.expect("startup grant adoption failed");
    let descriptor = context.entry();
    let control = context
        .take_handle(descriptor.control_index as usize)
        .expect("entry thread control grant is missing");
    *STARTUP_CONTEXT.lock() = Some(context);
    let registry = ThreadRegistry::new(64).expect("entry thread registry allocation failed");
    // SAFETY: StartupBlock validation proves the reservation geometry, and the
    // claimed control is the descriptor-selected unique ThreadControl owner.
    unsafe {
        registry
            .adopt_entry(
                descriptor,
                ThreadControl::from_raw(control),
                NoEnvironment,
                (),
            )
            .expect("entry thread adoption failed");
    }
    *ENTRY_REGISTRY.lock() = Some(registry);
}

pub fn startup_pid() -> erhino_shared::proc::Pid {
    STARTUP_CONTEXT.lock().as_ref().expect("startup context missing").pid()
}

pub fn startup_parent_pid() -> erhino_shared::proc::Pid {
    STARTUP_CONTEXT
        .lock()
        .as_ref()
        .expect("startup context missing")
        .parent_pid()
}

pub fn startup_handle(index: usize) -> Option<erhino_shared::object::Handle> {
    STARTUP_CONTEXT
        .lock()
        .as_mut()?
        .take_or_duplicate_handle(index)
}

pub fn startup_payload() -> &'static [u8] {
    STARTUP_CONTEXT
        .lock()
        .as_ref()
        .expect("startup context missing")
        .payload()
}

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
        // SAFETY: this is a fresh, exclusively owned mapping.
        if unsafe { talc.claim(usable.start as *mut u8, usable.end - usable.start) }.is_none() {
            let region = talc.source.arenas[slot]
                .take()
                .expect("heap arena token disappeared before claim");
            let _ = region.unmap();
            return Err(());
        }
        talc.source.arena_count += 1;
        talc.source.next_arena_bytes =
            arena_bytes.saturating_mul(2).min(MAX_GEOMETRIC_ARENA_BYTES);
        Ok(())
    }
}

#[global_allocator]
static HEAP_ALLOCATOR: TalcLock<SimpleLock, HeapSource> = TalcLock::new(HeapSource::new());

#[unsafe(no_mangle)]
/// # Safety
///
/// The platform must enter this symbol with a valid StartupBlock pointer and
/// length in the ABI argument registers.
pub unsafe extern "C" fn _start(block: *const u8, block_len: usize) -> ! {
    // SAFETY: `_start` is the ELF entry and establishes the ABI global pointer
    // before any runtime code can access static data.
    unsafe {
        asm!(
            "la gp, __global_pointer$",
            options(nostack, preserves_flags)
        );
    }
    // SAFETY: `_start` received the platform's valid StartupBlock pair.
    unsafe { __runtime_start(block, block_len) }
}

#[unsafe(no_mangle)]
/// # Safety
///
/// Called only by the runtime-owned `_start` bridge after gp initialization.
pub unsafe extern "C" fn __runtime_start(block: *const u8, block_len: usize) -> ! {
    // SAFETY: the platform launcher places a stable, read-only StartupBlock
    // at the a0/a1 pair consumed by the runtime entry.
    unsafe { kernel_startup::init(block, block_len) };
    {
        let mut talc = HEAP_ALLOCATOR.lock();
        <HeapSource as Source>::acquire(
            &mut talc,
            Layout::from_size_align(INITIAL_ARENA_BYTES, core::mem::align_of::<usize>())
                .expect("initial heap arena layout is invalid"),
        )
        .expect("initial heap arena allocation failed");
    }
    adopt_entry_thread();
    // SAFETY: the binary exports exactly one runtime-owned application entry.
    unsafe { __halcyon_main() }
}

#[panic_handler]
fn handle_panic(info: &PanicInfo) -> ! {
    if let Some(location) = info.location() {
        dbg::debug(format_args!(
            "Panicking in {} at line {}: {}",
            location.file(),
            location.line(),
            info.message()
        ));
    } else {
        dbg::debug(format_args!("Panicking: no information available."));
    }
    // SAFETY: panic is an unrecoverable process failure.
    unsafe { sys_process_abort() }
}

#[alloc_error_handler]
fn handle_alloc_error(layout: Layout) -> ! {
    panic!("Heap allocation error, layout = {:?}", layout);
}
