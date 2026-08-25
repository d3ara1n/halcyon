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
/// Filesystem abstract layer
pub mod fal;
/// eRhino path string utilities
pub mod path;
/// Time-related functions
pub mod time;
/// 内核对象、Handle、rights 与对象状态
pub mod object;
/// Messaging primitives
pub mod message;
/// Signal primitives
pub mod startup;
/// 统一对象等待
pub mod wait;