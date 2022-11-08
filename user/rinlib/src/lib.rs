#![feature(lang_items, panic_info_message, alloc_error_handler)]
// Don't link to std. We are std.
#![no_std]
#![allow(dead_code)]

pub use erhino_shared as shared;

mod call;
pub mod signal;
mod rt;
pub mod proc;
pub mod io;
pub mod prelude;