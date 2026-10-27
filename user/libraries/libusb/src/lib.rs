//! USB 的描述符、受限接口会话和 xHCI 硬件知识；不拥有中央设备表或运行体。
#![no_std]
#![cfg_attr(target_arch = "riscv64", feature(allocator_ext))]
extern crate alloc;
pub mod descriptor;
pub mod protocol;
#[cfg(target_arch = "riscv64")]
pub mod xhci;
