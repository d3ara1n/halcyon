//! 统一对象等待 ABI。

use crate::object::{Handle, ObjectSignals};

/// 单次 WaitMany 的最大观察项数。
pub const WAIT_MANY_MAX: usize = 64;

/// 调用者不透明的等待关联值。
pub type WaitCookie = u64;

/// 一个对象等待项。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct WaitItem {
    pub handle: Handle,
    pub signals: ObjectSignals,
    pub cookie: WaitCookie,
    pub reserved: u64,
}

impl WaitItem {
    pub const fn new(handle: Handle, signals: ObjectSignals, cookie: WaitCookie) -> Self {
        Self {
            handle,
            signals,
            cookie,
            reserved: 0,
        }
    }
}

/// WaitMany 的完成原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum WaitReason {
    Signaled = 0,
    Closed = 1,
    Cancelled = 2,
    /// 期限到达，无任何观察项完成；此时 `item_index` 为 `u32::MAX`。
    Deadline = 3,
}

impl WaitReason {
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Signaled),
            1 => Some(Self::Closed),
            2 => Some(Self::Cancelled),
            3 => Some(Self::Deadline),
            _ => None,
        }
    }
}

/// WaitMany 的可选期限参数值：无限等待。
pub const WAIT_DEADLINE_INFINITE: u64 = 0;

/// WaitMany 的唯一完成结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct WaitResult {
    pub cookie: WaitCookie,
    pub observed: ObjectSignals,
    pub item_index: u32,
    pub reason: u32,
    pub reserved: u64,
}

impl WaitResult {
    pub const fn new(
        cookie: WaitCookie,
        observed: ObjectSignals,
        item_index: u32,
        reason: WaitReason,
    ) -> Self {
        Self {
            cookie,
            observed,
            item_index,
            reason: reason as u32,
            reserved: 0,
        }
    }
}

const _: () = {
    assert!(core::mem::size_of::<WaitItem>() == 32);
    assert!(core::mem::align_of::<WaitItem>() == 8);
    assert!(core::mem::size_of::<WaitResult>() == 32);
    assert!(core::mem::align_of::<WaitResult>() == 8);
};
