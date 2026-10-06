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
pub mod shared_memory;
pub mod startup;
pub mod system;
pub mod thread;
pub mod time;

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
        self.handle
            .take()
            .expect("ThreadControl owner already consumed")
    }
}

impl Drop for ThreadControl {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // SAFETY: ThreadControl is the unique owner of this non-mapping entry.
            unsafe { ipc::object::close(handle) }.unwrap_or_else(|error| {
                panic!("ThreadControl close invariant violated: {error:?}")
            });
        }
    }
}

/// Pending 启动权；最后持有消散会放弃尚未启动的线程。
#[derive(Debug)]
pub struct ThreadStartTicket {
    handle: Option<shared::object::Handle>,
}

impl ThreadStartTicket {
    /// # Safety
    ///
    /// `handle` 必须是调用方独占接收的 ThreadStartTicket capability。
    pub unsafe fn from_raw(handle: shared::object::Handle) -> Self {
        Self {
            handle: Some(handle),
        }
    }

    pub fn as_handle(&self) -> shared::object::Handle {
        self.handle
            .expect("ThreadStartTicket owner already consumed")
    }

    pub fn start(&mut self) -> Result<(), shared::call::SystemCallError> {
        // SAFETY: 本 owner 持有启动票据，系统调用只使用 handle 值。
        unsafe { call::sys_thread_start(self.as_handle()) }?;
        self.handle = None;
        Ok(())
    }
}

impl Drop for ThreadStartTicket {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // SAFETY: 本结构唯一拥有该非映射 handle 的关闭责任。
            unsafe { ipc::object::close(handle) }.unwrap_or_else(|error| {
                panic!("ThreadStartTicket close invariant violated: {error:?}")
            });
        }
    }
}

pub fn thread_start(ticket: &mut ThreadStartTicket) -> Result<(), shared::call::SystemCallError> {
    ticket.start()
}
