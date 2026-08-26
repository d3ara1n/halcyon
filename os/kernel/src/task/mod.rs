//! 任务模型（notes/impls/task.md）：进程/线程/进程表与 ELF 装载。

pub mod handle;
pub mod mailbox;
pub mod notification;
pub mod object;
pub mod proc;
pub mod table;
pub mod tunnel;
pub mod wait;

pub use proc::{launch, spawn_from_elf, Thread};
