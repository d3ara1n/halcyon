//! 扁平持久电平集合；成员挂链直到 Remove/Close，无 arm 或历史结果槽。

use crate::object::{Handle, ObjectSignals};

pub const RECEIVE_MAX: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct MemberRequest {
    pub handle: Handle,
    pub signals: ObjectSignals,
    pub cookie: u64,
}

impl MemberRequest {
    pub const fn new(handle: Handle, signals: ObjectSignals, cookie: u64) -> Self {
        Self {
            handle,
            signals,
            cookie,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct ReadyRecord {
    pub token: u64,
    pub cookie: u64,
    pub observed: ObjectSignals,
    pub reason: u32,
    pub reserved: u32,
}

const _: () = {
    assert!(core::mem::size_of::<MemberRequest>() == 24);
    assert!(core::mem::size_of::<ReadyRecord>() == 32);
};
