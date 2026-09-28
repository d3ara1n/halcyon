#![cfg_attr(target_arch = "riscv64", feature(alloc_error_handler))]
#![cfg_attr(target_arch = "riscv64", feature(lang_items))]
#![no_std]
#![allow(internal_features)]

//! Rust program environment and process termination policy.

#[cfg(target_arch = "riscv64")]
mod rt;

#[cfg(target_arch = "riscv64")]
pub use rt::thread;

#[cfg(target_arch = "riscv64")]
pub mod startup {
    pub use libprocess::context::StartupContext;

    pub fn take() -> Option<StartupContext> {
        crate::rt::take_startup_context()
    }
}
