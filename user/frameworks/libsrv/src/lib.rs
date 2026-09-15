#![no_std]
#![feature(allocator_api)]

//! 服务执行框架：来源账户、公平任务推进与持久观察的公共组合点。

extern crate alloc;

pub mod budget;
pub mod runtime;
pub mod wake;
pub mod work_queue;

pub use runtime::{Advance, Input, Requests, Runtime, SourceEvent, SourceId, SourceKind, SourcePlan, Step, Task, TaskFailure};
