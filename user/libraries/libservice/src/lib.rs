//! 用户态服务注册、发现记录与生命周期控制。
//!
//! 服务权限与普通 FAL 目录权限彼此独立；本库拥有注册 wire、ServiceRecord
//! schema 及 Registry 状态，底层目录与 RPC 机制继续由 libfal/librpc 拥有。

#![cfg_attr(not(test), no_std)]
#![feature(allocator_api)]

extern crate alloc;

pub mod authority;
#[cfg(target_arch = "riscv64")]
pub mod client;
pub mod protocol;
pub mod record;
pub mod registry;
pub mod resource;
