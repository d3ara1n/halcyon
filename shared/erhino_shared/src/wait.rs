//! 单对象纯电平等待 ABI；多对象集合及成员 cookie 由 wait_set 拥有。

use crate::{
    object::{Handle, ObjectSignals},
    time::Deadline,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct WaitOneRequest {
    pub handle: Handle,
    pub signals: ObjectSignals,
    pub deadline: Deadline,
}

/// 集合成员的调用者命名空间；单对象等待不附带 cookie。
pub type WaitCookie = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum WaitReason {
    Signaled = 0,
    Closed = 1,
    Timeout = 3,
}

impl WaitReason {
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Signaled),
            1 => Some(Self::Closed),
            3 => Some(Self::Timeout),
            _ => None,
        }
    }
}

/// 单对象返回的解释。Kernel 通过 a1 返当前电平，NONE 唯一表示到期；
/// CLOSED 在关闭时总附加，不依赖调用者是否在兴趣 mask 中写入该位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct WaitResult {
    pub observed: ObjectSignals,
    pub reason: u32,
    pub reserved: u32,
}

impl WaitResult {
    pub const fn from_observed(observed: ObjectSignals) -> Self {
        let reason = if observed.contains(ObjectSignals::CLOSED) {
            WaitReason::Closed
        } else if observed.raw() == 0 {
            WaitReason::Timeout
        } else {
            WaitReason::Signaled
        };
        Self {
            observed,
            reason: reason as u32,
            reserved: 0,
        }
    }
}

const _: () = {
    assert!(core::mem::size_of::<WaitOneRequest>() == 32);
    assert!(core::mem::align_of::<WaitOneRequest>() == 8);
    assert!(core::mem::size_of::<WaitResult>() == 16);
};
