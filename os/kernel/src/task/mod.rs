//! 任务模型（notes/impls/task.md）：进程/线程/进程表与 ELF 装载。

pub mod ipc;
pub mod proc;
pub mod table;
pub mod tunnel;

pub use proc::{spawn_from_elf, Thread};
