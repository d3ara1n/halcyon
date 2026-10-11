extern crate alloc;

use alloc::boxed::Box;
use core::{alloc::Layout, panic::PanicInfo};

use erhino_shared::mem::MemoryProtection;
use erhino_shared::sync::spin::SimpleLock;
use libkernel::{
    ThreadControl, dbg,
    mm::{MappedRegion, Placement},
    startup as kernel_startup, sys_process_abort, sys_thread_exit,
};
use libprocess::context::StartupContext;
use libthread::{NoEnvironment, ResultReceiver, ThreadFactory, ThreadRegistry};
use lock_api::Mutex;
use talc::{TalcLock, base::Talc, base::binning::Binning, source::Source};

unsafe extern "C" {
    #[link_name = "main"]
    fn rust_main(argc: isize, argv: *const *const u8) -> isize;
}

trait MainOutcome {
    fn finish(self) -> !;
}

impl MainOutcome for () {
    fn finish(self) -> ! {
        // SAFETY: successful application completion leaves no user result work.
        unsafe { sys_thread_exit() }
    }
}

impl<E: core::fmt::Debug> MainOutcome for Result<(), E> {
    fn finish(self) -> ! {
        match self {
            Ok(()) => {
                // SAFETY: successful application completion leaves no user result work.
                unsafe { sys_thread_exit() }
            }
            Err(error) => {
                dbg::debug(format_args!("application main failed: {:?}", error));
                // SAFETY: an unsuccessful application result is process failure.
                unsafe { sys_process_abort() }
            }
        }
    }
}

fn finish<T: MainOutcome>(outcome: T) -> ! {
    outcome.finish()
}

type Lock = SimpleLock;

const DEFAULT_STACK_BYTES: usize = 1024 * 1024;
const RUNTIME_REGISTRY_CAPACITY: usize = 64;

static ENTRY_REGISTRY: Mutex<Lock, Option<&'static ThreadRegistry<NoEnvironment>>> =
    Mutex::const_new(Lock::new(), None);
static STARTUP_CONTEXT: Mutex<Lock, Option<StartupContext>> = Mutex::const_new(Lock::new(), None);

fn registry() -> &'static ThreadRegistry<NoEnvironment> {
    let mut guard = ENTRY_REGISTRY.lock();
    if let Some(registry) = *guard {
        return registry;
    }
    let registry = Box::leak(Box::new(
        ThreadRegistry::new(RUNTIME_REGISTRY_CAPACITY)
            .expect("runtime thread registry capacity is valid"),
    ));
    *guard = Some(registry);
    registry
}

fn adopt_entry_thread(seed: libprocess::context::StartupSeed) {
    let adoption = match StartupContext::adopt(seed) {
        Ok(adoption) => adoption,
        Err((_, error)) => {
            dbg::debug(format_args!("startup grant adoption failed: {:?}", error));
            unsafe { sys_process_abort() }
        }
    };
    let control = adoption.entry_control.into_raw();
    let registry = registry();
    // SAFETY: StartupBlock validation proves the reservation geometry, and the
    // claimed control is the descriptor-selected unique ThreadControl owner.
    let result = unsafe {
        registry.adopt_entry(
            adoption.entry,
            ThreadControl::from_raw(control),
            NoEnvironment,
            (),
        )
    };
    if let Err(error) = result {
        dbg::debug(format_args!(
            "startup entry thread adoption failed: {:?}",
            error
        ));
        unsafe { sys_process_abort() }
    }
    *STARTUP_CONTEXT.lock() = Some(adoption.context);
}

pub mod thread {
    use super::{NoEnvironment, ResultReceiver, ThreadFactory, registry};
    use erhino_shared::call::SystemCallError;
    pub use libthread::yield_now;

    pub struct Builder {
        stack_bytes: usize,
    }

    impl Builder {
        pub const fn new() -> Self {
            Self {
                stack_bytes: super::DEFAULT_STACK_BYTES,
            }
        }

        pub const fn stack_size(mut self, bytes: usize) -> Self {
            self.stack_bytes = bytes;
            self
        }

        pub fn spawn<F, T>(
            self,
            function: F,
        ) -> Result<ResultReceiver<T, NoEnvironment>, SystemCallError>
        where
            F: FnOnce() -> T + Send + 'static,
            T: Send + 'static,
        {
            ThreadFactory::new(registry(), NoEnvironment)
                .stack_size(self.stack_bytes)
                .spawn(function)
        }

        pub fn prepare<F, T>(
            self,
            function: F,
        ) -> Result<libthread::PreparedThread<F, T, NoEnvironment>, SystemCallError>
        where
            F: FnOnce() -> T + Send + 'static,
            T: Send + 'static,
        {
            ThreadFactory::new(registry(), NoEnvironment)
                .stack_size(self.stack_bytes)
                .prepare(function)
        }
    }

    impl Default for Builder {
        fn default() -> Self {
            Self::new()
        }
    }
}

pub fn take_startup_context() -> Option<StartupContext> {
    STARTUP_CONTEXT.lock().take()
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
        let slot = talc.source.arena_count;
        talc.source.arenas[slot] = Some(region);
        let usable = talc.source.arenas[slot]
            .as_ref()
            .and_then(MappedRegion::usable)
            .ok_or(())?;
        // SAFETY: this is a fresh, exclusively owned mapping.
        if unsafe { talc.claim(usable.start as *mut u8, usable.end - usable.start) }.is_none() {
            let region = talc.source.arenas[slot]
                .take()
                .expect("heap arena token disappeared before claim");
            if let Err((region, _error)) = region.unmap() {
                // Startup failure aborts the process; retain the mapping owner
                // for automatic process retirement instead of discarding a rejected Unmap.
                talc.source.arenas[slot] = Some(region);
            }
            return Err(());
        }
        talc.source.arena_count += 1;
        talc.source.next_arena_bytes = arena_bytes.saturating_mul(2).min(MAX_GEOMETRIC_ARENA_BYTES);
        Ok(())
    }
}

#[global_allocator]
static HEAP_ALLOCATOR: TalcLock<SimpleLock, HeapSource> = TalcLock::new(HeapSource::new());

#[unsafe(naked)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start(_: *const u8, _: usize) -> ! {
    core::arch::naked_asm!(
        ".option push",
        ".option norelax",
        "la gp, __global_pointer$",
        ".option pop",
        "tail {bridge}",
        bridge = sym __libruntime_start,
    )
}

unsafe extern "C" fn __libruntime_start(block: *const u8, block_len: usize) -> ! {
    // SAFETY: the platform launcher places a stable, read-only StartupBlock
    // at the a0/a1 pair consumed by the runtime entry.
    let seed = unsafe { kernel_startup::init(block, block_len) };
    // SAFETY: only this platform bridge receives the affine entry input.
    let seed = unsafe { libprocess::context::StartupSeed::from_input(seed) };
    {
        let mut talc = HEAP_ALLOCATOR.lock();
        <HeapSource as Source>::acquire(
            &mut talc,
            Layout::from_size_align(INITIAL_ARENA_BYTES, core::mem::align_of::<usize>())
                .expect("initial heap arena layout is invalid"),
        )
        .expect("initial heap arena allocation failed");
    }
    adopt_entry_thread(seed);
    // SAFETY: rustc emits the binary `main` shim. It calls the lang item
    // below, which never returns after MainOutcome::finish.
    unsafe {
        rust_main(0, core::ptr::null());
        sys_process_abort()
    }
}

#[lang = "start"]
fn lang_start<T: MainOutcome + 'static>(
    main: fn() -> T,
    _argc: isize,
    _argv: *const *const u8,
    _sigpipe: u8,
) -> isize {
    finish(main())
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
