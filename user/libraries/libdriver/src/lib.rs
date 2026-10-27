//! 有限支持声明、不可变映像配对及驱动激活协议；不拥有设备占用，不选择运行体。
#![no_std]
#![cfg_attr(target_arch = "riscv64", feature(allocator_ext))]
extern crate alloc;
pub mod activation;
pub mod boot;
#[cfg(target_arch = "riscv64")]
pub mod client;
pub mod protocol;
pub mod view;
#[cfg(target_arch = "riscv64")]
pub mod worker;
