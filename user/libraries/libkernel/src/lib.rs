#![no_std]

pub use erhino_shared as shared;

pub extern crate alloc;

mod call;
pub use call::{
    sys_process_abort, sys_process_terminate, sys_sleep, sys_thread_exit, sys_thread_start,
    sys_thread_yield,
};
pub mod dbg;
pub mod ipc;
pub mod memory_object;
pub mod memory_pool;
pub mod mm;
pub mod preclude;
pub mod process;
pub mod startup;
pub mod shared_memory;
pub mod system;
pub mod time;
pub mod thread;

/// A shared, waitable authority for a kernel thread.
///
/// The constructor is unsafe because the caller must own the returned handle.
#[derive(Debug)]
pub struct ThreadControl {
    handle: Option<shared::object::Handle>,
}

impl ThreadControl {
    /// # Safety
    ///
    /// `handle` must be a valid ThreadControl entry whose close responsibility is
    /// transferred to the returned owner.
    pub unsafe fn from_raw(handle: shared::object::Handle) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    pub fn as_handle(&self) -> shared::object::Handle {
        self.handle.expect("ThreadControl owner already consumed")
    }

    pub fn into_raw(mut self) -> shared::object::Handle {
        self.handle.take().expect("ThreadControl owner already consumed")
    }

    pub fn start(&self) -> Result<(), shared::call::SystemCallError> {
        // SAFETY: this owner contains the MANAGE-capable handle returned by ThreadCreate.
        unsafe { call::sys_thread_start(self.as_handle()) }
    }
}

impl Drop for ThreadControl {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // SAFETY: ThreadControl is the unique owner of this non-mapping entry.
            unsafe { ipc::object::close(handle) }
                .unwrap_or_else(|error| panic!("ThreadControl close invariant violated: {error:?}"));
        }
    }
}

/// Start a newly-created thread through its MANAGE-capable control.
pub fn thread_start(control: &ThreadControl) -> Result<(), shared::call::SystemCallError> {
    control.start()
}
