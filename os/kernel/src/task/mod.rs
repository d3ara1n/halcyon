//! 任务模型（notes/impls/task.md）：进程/线程/生命周期与 ELF 装载。
//! 未 Dead Process core 的生命周期根是 Job 直接成员表；不存在全局进程表。

pub mod delivery;
pub mod handle;
pub mod job;
pub mod lifecycle;
pub mod lifetime;
pub mod mailbox;
pub mod memory_object;
pub mod memory_pool;
pub mod notification;
pub(crate) mod notify_work;
pub mod object;
pub mod proc;
pub mod process;
pub(crate) mod request;
pub mod resources;
pub(crate) mod retirement;
pub(crate) mod selftest;
pub mod system_reset;
pub mod thread;
pub mod tunnel;
pub mod wait;
pub mod wait_set;

pub use job::alloc_pid;
pub use proc::Thread;
pub(crate) use proc::{launch_bootstrap, spawn_from_elf};
