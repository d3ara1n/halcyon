//! 文件领域知识、线上协议及 provider 选用的实际内存 backing。

#![cfg_attr(not(test), no_std)]
#![feature(allocator_ext)]
extern crate alloc;

pub mod authority;
pub mod backend;
#[cfg(target_arch = "riscv64")]
pub mod client;
pub mod data;
mod funding;
pub use funding::Funding;
pub mod grant;
pub mod management;
pub mod markers;
pub mod node;
pub mod protocol;
#[cfg(target_arch = "riscv64")]
pub mod provider;
pub mod resource;
pub mod route;
pub mod store;
#[cfg(target_arch = "riscv64")]
pub mod stream;
#[cfg(any(target_arch = "riscv64", test))]
#[path = "stream/state.rs"]
mod stream_state;
pub mod value;
pub mod watch;

pub use authority::FsRights;
