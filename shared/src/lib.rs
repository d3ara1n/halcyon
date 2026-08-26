#![no_std]

//! # eRhino shared lib
//!
//! Predefined types and system calls

extern crate alloc;

/// System calls
pub mod call;
/// Memory related
pub mod mem;
/// Process types
pub mod proc;
/// Service
pub mod service;
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