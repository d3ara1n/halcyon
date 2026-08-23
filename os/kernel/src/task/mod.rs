//! 任务模型（notes/impls/task.md）：进程/线程/进程表与 ELF 装载。

pub mod proc;
pub mod table;

pub use proc::{spawn_from_elf, Thread};
