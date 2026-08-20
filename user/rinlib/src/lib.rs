#![feature(lang_items, alloc_error_handler)]
// Don't link to std. We are std.
#![no_std]
#![allow(internal_features)]

pub use erhino_shared as shared;
pub use flagset;

pub extern crate alloc;

mod call;
pub use call::sys_sleep;
pub mod dbg;
pub mod env;
pub mod fs;
pub mod ipc;
pub mod mm;
pub mod preclude;
pub mod process;
mod rt;
pub mod thread;
