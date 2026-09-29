use core::sync::atomic::{AtomicBool, Ordering};

use erhino_shared::{
    object::Handle,
    startup::{
        EntryThreadDescriptor, StartupBlockHeader, entry_thread_descriptor, validate_startup_block,
    },
};

/// Validated, process-lifetime startup input. It is affine and can only be
/// produced by the platform entry after the once gate succeeds.
#[derive(Debug)]
pub struct StartupInput {
    base: *const u8,
    len: usize,
    header: StartupBlockHeader,
}

unsafe impl Send for StartupInput {}
unsafe impl Sync for StartupInput {}

static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Validate and consume the platform startup arguments exactly once.
///
/// # Safety
///
/// The caller must prove that `block` points to a stable, readable mapping of
/// `block_len` bytes that remains mapped until process teardown.
pub unsafe fn init(block: *const u8, block_len: usize) -> StartupInput {
    assert!(!block.is_null(), "null startup block");
    // SAFETY: the platform entry proves the mapping is readable and stable.
    let bytes = unsafe { core::slice::from_raw_parts(block, block_len) };
    let header = validate_startup_block(bytes)
        .unwrap_or_else(|error| panic!("invalid startup block: {:?}", error));
    entry_thread_descriptor(bytes, header)
        .unwrap_or_else(|error| panic!("invalid entry thread descriptor: {:?}", error));
    if INITIALIZED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        panic!("startup entry initialized twice");
    }
    StartupInput {
        base: block,
        len: block_len,
        header,
    }
}

impl StartupInput {
    pub fn header(&self) -> StartupBlockHeader {
        self.header
    }

    pub fn grant_count(&self) -> usize {
        self.header.handle_count as usize
    }

    pub fn handle(&self, index: usize) -> Option<Handle> {
        if index >= self.grant_count() {
            return None;
        }
        let offset = core::mem::size_of::<StartupBlockHeader>()
            .checked_add(index.checked_mul(core::mem::size_of::<Handle>())?)?;
        // SAFETY: validate_startup_block established the full handle range.
        Some(unsafe { core::ptr::read_unaligned(self.base.add(offset).cast::<Handle>()) })
    }

    pub fn entry_thread(&self) -> EntryThreadDescriptor {
        // SAFETY: init validated the complete immutable block.
        let bytes = unsafe { core::slice::from_raw_parts(self.base, self.len) };
        entry_thread_descriptor(bytes, self.header)
            .expect("validated startup entry descriptor disappeared")
            .0
    }

    pub fn payload(&self) -> &[u8] {
        // SAFETY: init validated the payload range and the input keeps the
        // mapping alive for the process lifetime.
        let bytes = unsafe { core::slice::from_raw_parts(self.base, self.len) };
        entry_thread_descriptor(bytes, self.header)
            .expect("validated startup payload disappeared")
            .1
    }
}
