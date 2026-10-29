//! 用户态服务注册、发现记录与生命周期控制。
//!
//! 本库拥有服务描述、注册 wire、授权与 Registry；发现读取同一 backing。

#![cfg_attr(not(test), no_std)]
#![feature(allocator_ext)]

extern crate alloc;

pub mod authority;
mod funding;
pub mod instance;
pub use funding::Funding;
#[cfg(target_arch = "riscv64")]
pub mod client;
#[cfg(target_arch = "riscv64")]
pub mod maintenance;
pub mod protocol;
#[cfg(target_arch = "riscv64")]
pub mod provider;
pub mod registry;
pub mod resource;
#[cfg(target_arch = "riscv64")]
pub mod root;
pub mod watch;
