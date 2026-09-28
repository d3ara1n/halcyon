#![cfg_attr(target_arch = "riscv64", feature(alloc_error_handler))]
#![no_std]
#![allow(internal_features)]

//! Rust program environment and process termination policy.

#[cfg(target_arch = "riscv64")]
mod rt;

#[cfg(target_arch = "riscv64")]
pub use rt::__runtime_start;
#[cfg(target_arch = "riscv64")]
pub use rt::finish;

/// Declare the application function consumed by the runtime-owned ELF entry.
///
/// The generated symbol is an internal link-time bridge. Applications should
/// call this macro instead of naming that symbol or calling `finish` manually.
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        #[unsafe(no_mangle)]
        pub extern "C" fn __halcyon_main() -> ! {
            $crate::finish($main())
        }
    };
}

#[cfg(target_arch = "riscv64")]
pub mod startup {
    use erhino_shared::{object::Handle, proc::Pid};

    pub fn pid() -> Pid {
        crate::rt::startup_pid()
    }

    pub fn parent_pid() -> Pid {
        crate::rt::startup_parent_pid()
    }

    pub fn startup_handle(index: usize) -> Option<Handle> {
        crate::rt::startup_handle(index)
    }

    pub fn startup_payload() -> &'static [u8] {
        crate::rt::startup_payload()
    }
}
