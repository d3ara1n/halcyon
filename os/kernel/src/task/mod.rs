//! 任务模型（notes/impls/task.md）：进程/线程/生命周期与 ELF 装载。
//! 未 Dead Process core 的生命周期根是 Job 直接成员表；不存在全局进程表。

pub mod handle;
pub mod job;
pub mod lifecycle;
pub mod mailbox;
pub mod notification;
pub mod object;
pub mod proc;
pub mod process;
pub mod system_reset;
pub mod tunnel;
pub mod wait;

pub use job::alloc_pid;
pub use proc::{Thread, launch_bootstrap, spawn_from_elf};
