#![no_std]

//! capability 纯逻辑：(授权对象, rights) 的持有计数、最后持有判定、
//! 进程本地表与返还预约。
//!
//! 本 crate 不接收回调、不持锁、不认识具体对象种类。所有判定以纯值返回
//! （[`LastCapability`]、[`Restore`]），由内核在自身锁外执行。

extern crate alloc;

mod holders;
mod table;

pub use holders::{Holders, HoldersError, ZeroPolicy};
pub use table::{Cap, CapTable, ClaimTicket, LastCapability, Restore, TableError, TakeNext};

/// 可被 capability 持有的授权对象。实现者通常是内核对象引用（`Arc<dyn ..>`）。
///
/// `Clone` 只复制存储引用，不产生 authority；authority 只能经 [`Cap::mint`]
/// 与 [`Cap::derive`] 取得。
pub trait Authority: Clone {
    /// 本对象的 capability 持有计数；同一对象的每次调用必须返回同一实例。
    fn holders(&self) -> &Holders;
}
