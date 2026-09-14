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
/// MemoryObject capability
pub mod memory_object;
/// MemoryPool capability 账户
pub mod memory_pool;
/// Messaging primitives
pub mod message;
/// 内核对象、Handle、rights 与对象状态
pub mod object;
/// Process types
pub mod proc;
/// 系统复位语义
pub mod reset;
/// Service
pub mod service;
/// 启动资源交付（StartupBlock：实际 Handle 数组 + opaque payload）
pub mod startup;
/// Locks
pub mod sync;
/// Time-related functions
pub mod time;
/// Tunnel 几何与请求
pub mod tunnel;
/// 统一对象等待
pub mod wait;
/// 持久观察集合
pub mod wait_set;
