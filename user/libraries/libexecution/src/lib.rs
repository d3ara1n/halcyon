#![no_std]

//! 有界任务推进、观察来源、期限、唤醒、停止与退休。

extern crate alloc;

pub mod runtime;
pub mod wake;
pub mod work_queue;

use libbudget::Taxonomy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionResource {
    Task,
    InputBytes,
}

impl Taxonomy for ExecutionResource {
    const COUNT: usize = 2;

    fn slot(self) -> usize {
        match self {
            Self::Task => 0,
            Self::InputBytes => 1,
        }
    }
}

pub use runtime::{
    Advance, Input, Requests, Runtime, SourceEvent, SourceId, SourceKind, SourcePlan, Step, Task,
    TaskFailure,
};
