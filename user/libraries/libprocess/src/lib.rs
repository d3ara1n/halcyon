#![no_std]

extern crate alloc;

pub mod context;

#[cfg(feature = "management")]
#[path = "management.rs"]
mod management;

#[cfg(feature = "management")]
pub use management::*;
