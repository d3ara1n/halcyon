//! 进程表：全局层容器（见 notes/impls/internals.md「进程表」）。
//!
//! PID 单调不复用。用户态 ProcessStart 先放入 reservation marker，提交区
//! 只替换 marker，不再分配；Building reservation 对普通查找不可见。

use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicUsize, Ordering};

use erhino_shared::proc::Pid;

use super::proc::Process;
use crate::sync::Spinlock;

enum Entry {
    Reserved { pid: Pid, token: u64 },
    Process(Arc<Process>),
}

struct ProcessTable {
    entries: Spinlock<Vec<Entry>>,
    next_pid: AtomicUsize,
    next_token: AtomicUsize,
}

static TABLE: ProcessTable = ProcessTable {
    entries: Spinlock::new(Vec::new()),
    next_pid: AtomicUsize::new(1),
    next_token: AtomicUsize::new(1),
};

#[derive(Debug)]
pub struct InsertReservation {
    pid: Pid,
    token: u64,
}

/// 分配新 pid（从 1 起，不复用）。
pub fn alloc_pid() -> Pid {
    TABLE.next_pid.fetch_add(1, Ordering::Relaxed) as Pid
}

pub fn reserve_insert(pid: Pid) -> Result<InsertReservation, ()> {
    let token = TABLE.next_token.fetch_add(1, Ordering::Relaxed) as u64;
    if token == 0 {
        return Err(());
    }
    let mut entries = TABLE.entries.lock();
    entries.try_reserve(1).map_err(|_| ())?;
    entries.push(Entry::Reserved { pid, token });
    Ok(InsertReservation { pid, token })
}

pub fn commit_insert(reservation: InsertReservation, process: Arc<Process>) {
    assert_eq!(reservation.pid, process.pid, "process-table reservation pid mismatch");
    let mut entries = TABLE.entries.lock();
    let entry = entries
        .iter_mut()
        .find(|entry| matches!(entry, Entry::Reserved { pid, token } if *pid == reservation.pid && *token == reservation.token))
        .expect("process-table reservation disappeared");
    *entry = Entry::Process(process);
}

pub fn rollback_insert(reservation: InsertReservation) {
    let mut entries = TABLE.entries.lock();
    let index = entries
        .iter()
        .position(|entry| matches!(entry, Entry::Reserved { pid, token } if *pid == reservation.pid && *token == reservation.token))
        .expect("process-table reservation disappeared");
    entries.swap_remove(index);
}

/// Boot-only 插入；启动失败不可恢复，但仍把 OOM 转成明确 boot failure。
pub fn insert_boot(process: Arc<Process>) {
    let reservation = reserve_insert(process.pid).expect("process table exhausted during bootstrap");
    commit_insert(reservation, process);
}

/// 摘除 Running process；reservation marker 永不匹配。
pub fn remove(pid: Pid) -> Option<Arc<Process>> {
    let mut entries = TABLE.entries.lock();
    let index = entries
        .iter()
        .position(|entry| matches!(entry, Entry::Process(process) if process.pid == pid))?;
    match entries.swap_remove(index) {
        Entry::Process(process) => Some(process),
        Entry::Reserved { .. } => unreachable!(),
    }
}
