#![no_std]

//! # eRhino shared lib
//!
//! Predefined types and system calls

extern crate alloc;

/// BootPackage 固定外层
pub mod boot;
/// System calls
pub mod call;
/// Memory related
pub mod mem;
/// MemoryPool capability 账户
pub mod memory_pool;
/// Process types
pub mod proc;
/// Service
pub mod service;
/// 系统复位语义
pub mod reset;
/// Locks
pub mod sync;
/// Time-related functions
pub mod time;
/// 内核对象、Handle、rights 与对象状态
pub mod object;
/// Messaging primitives
pub mod message;
/// 启动资源交付（StartupBlock：实际 Handle 数组 + opaque payload）
pub mod startup;
/// 统一对象等待
pub mod wait;
