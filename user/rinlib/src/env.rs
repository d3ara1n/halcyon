use core::sync::atomic::{AtomicU32, Ordering};

use erhino_shared::proc::Pid;

/// 0 = 未初始化（启动契约：lang_start 在任何用户代码前写入）。
static PID: AtomicU32 = AtomicU32::new(0);
static PARENT_PID: AtomicU32 = AtomicU32::new(0);

pub(crate) fn set_pid(pid: Pid) {
    PID.store(pid, Ordering::Relaxed);
}

pub(crate) fn set_parent_pid(pid: Pid) {
    PARENT_PID.store(pid, Ordering::Relaxed);
}

pub fn pid() -> Pid {
    PID.load(Ordering::Relaxed)
}

pub fn parent_pid() -> Pid {
    PARENT_PID.load(Ordering::Relaxed)
}
