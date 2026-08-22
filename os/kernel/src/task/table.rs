//! 进程表：全局层容器（见 notes/internals.md「进程表」）。
//!
//! 封装 `Spinlock<BTreeMap>`，只暴露 get/insert/remove；pid 单调分配不复用。
//! 内部实现可替换（slot array 等），调用方无感。

use alloc::{collections::BTreeMap, sync::Arc};
use core::sync::atomic::{AtomicUsize, Ordering};

use erhino_shared::proc::Pid;

use super::proc::Process;
use crate::sync::Spinlock;

struct ProcessTable {
    map: Spinlock<BTreeMap<Pid, Arc<Process>>>,
    next: AtomicUsize,
}

static TABLE: ProcessTable = ProcessTable {
    map: Spinlock::new(BTreeMap::new()),
    next: AtomicUsize::new(1),
};

/// 分配新 pid（从 1 起，不复用）。
pub fn alloc_pid() -> Pid {
    TABLE.next.fetch_add(1, Ordering::Relaxed) as Pid
}

pub fn insert(process: Arc<Process>) {
    TABLE.map.lock().insert(process.pid, process);
}

/// 摘除进程（退出回收的第一步；返回值 Drop 即释放地址空间）。
pub fn remove(pid: Pid) -> Option<Arc<Process>> {
    TABLE.map.lock().remove(&pid)
}
