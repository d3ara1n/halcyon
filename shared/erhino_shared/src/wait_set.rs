//! 持久观察集合；每轮 arm 只交付一个快照，结果槽由注册预付。

pub const RECEIVE_MAX: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct ReadyRecord {
    pub token: u64,
    pub arm_generation: u64,
    pub cookie: u64,
    pub observed: crate::object::ObjectSignals,
    pub reason: u32,
    pub error: u32,
}

const _: () = {
    assert!(core::mem::size_of::<ReadyRecord>() == 40);
};
