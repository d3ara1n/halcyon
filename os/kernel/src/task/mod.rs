//! 任务模型（notes/impls/task.md）：进程/线程/生命周期与 ELF 装载。
//! 未 Dead Process core 的生命周期根是 Job 直接成员表；不存在全局进程表。

pub(crate) mod cap;
pub mod delivery;
pub mod handle;
pub mod job;
pub mod lifetime;
pub mod mailbox;
pub mod memory_object;
pub mod memory_pool;
pub mod notification;
pub mod object;
pub mod process;
pub(crate) mod request;
pub mod resources;
pub mod system_reset;
pub mod thread;
pub mod tunnel;
pub mod wait;
pub mod wait_set;
pub(crate) mod wait_source;

pub use process::thread::Thread;
