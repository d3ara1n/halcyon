//! 稳定任务记录与公平 ready FIFO；任务额度在公开前准备。
//! 来源登记、事件路由与推进驱动见 [`crate::runtime`]。

use crate::{ExecutionResource, runtime::Task};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicUsize, Ordering};
use erhino_shared::{call::SystemCallError, time::Deadline};
use libbudget::{AccountView, Charge};
use ordered_table::OrderedTable;
use timer_queue::{TimerQueue, TimerToken};

static NEXT_TASK: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);
static ABANDONED_TASKS: AtomicUsize = AtomicUsize::new(0);

pub fn abandoned_tasks() -> usize {
    ABANDONED_TASKS.load(Ordering::Relaxed)
}

pub(crate) struct Slot<T> {
    pub(crate) task: T,
    pub(crate) sources: Vec<u64>,
    pub(crate) max_sources: usize,
    pub(crate) retiring_sources: usize,
    pub(crate) input_head: u64,
    pub(crate) input_tail: u64,
    pub(crate) previous: u64,
    pub(crate) next: u64,
    pub(crate) queued: bool,
    pub(crate) stopping: bool,
    pub(crate) lifecycle: Lifecycle,
    pub(crate) timer: TimerToken,
    pub(crate) timed_out: bool,
    deadline_value: Option<u64>,
    deadline_pending: bool,
    retry_timer: TimerToken,
    suspended: bool,
    pub(crate) _task_charge: Charge,
}

/// 任务表与公平 ready FIFO 的内部算法；公开驱动面见 [`crate::runtime::Runtime`]。
pub(crate) struct WorkQueue<T> {
    tasks: OrderedTable<Slot<T>>,
    head: u64,
    tail: u64,
    sealed: bool,
    stop_cursor: u64,
    retire_cursor: u64,
    unmarked_sources: usize,
    retire_ready: usize,
    timers: TimerQueue<Wake>,
}

#[derive(Clone, Copy)]
enum Wake {
    Deadline(u64),
    Retry(u64),
}

#[derive(Debug)]
pub struct InsertFailure<T> {
    pub error: SystemCallError,
    pub task: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Lifecycle {
    Active,
    Finalizing,
    Retiring,
}

impl<T> WorkQueue<T> {
    pub(crate) fn new(limit: usize) -> Result<Self, SystemCallError> {
        if limit == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        Ok(Self {
            tasks: OrderedTable::new(limit),
            head: 0,
            tail: 0,
            sealed: false,
            stop_cursor: 0,
            retire_cursor: 0,
            unmarked_sources: 0,
            retire_ready: 0,
            timers: TimerQueue::new(0),
        })
    }

    pub(crate) fn insert(
        &mut self,
        task: T,
        max_sources: usize,
        account: &AccountView<ExecutionResource>,
    ) -> Result<u64, InsertFailure<T>> {
        let prepare = (|| {
            if self.sealed {
                return Err(SystemCallError::ObjectClosed);
            }
            let id = NEXT_TASK.allocate().ok_or(SystemCallError::ReachLimit)?;
            let task_charge = account.acquire(ExecutionResource::Task, 1)?;
            Ok((id, task_charge))
        })();
        let (id, task_charge) = match prepare {
            Ok(prepared) => prepared,
            Err(error) => return Err(InsertFailure { error, task }),
        };
        let timer = match self.timers.try_register(0, Wake::Deadline(id)) {
            Ok(timer) => timer,
            Err(_) => {
                return Err(InsertFailure {
                    error: SystemCallError::OutOfMemory,
                    task,
                });
            }
        };
        assert!(self.timers.park(timer), "new task timer must exist");
        let retry_timer = match self.timers.try_register(0, Wake::Retry(id)) {
            Ok(token) => token,
            Err(_) => {
                self.timers.cancel(timer);
                return Err(InsertFailure {
                    error: SystemCallError::OutOfMemory,
                    task,
                });
            }
        };
        assert!(self.timers.park(retry_timer), "new retry timer must exist");
        let mut sources = Vec::new();
        if sources.try_reserve_exact(max_sources).is_err() {
            let _ = self.timers.cancel(timer);
            let _ = self.timers.cancel(retry_timer);
            return Err(InsertFailure {
                error: SystemCallError::OutOfMemory,
                task,
            });
        }
        let slot = Slot {
            task,
            sources,
            max_sources,
            retiring_sources: 0,
            input_head: 0,
            input_tail: 0,
            previous: 0,
            next: 0,
            queued: false,
            stopping: false,
            lifecycle: Lifecycle::Active,
            timer,
            timed_out: false,
            deadline_value: None,
            deadline_pending: false,
            retry_timer,
            suspended: false,
            _task_charge: task_charge,
        };
        let prepared = match self.tasks.prepare_insert(id, slot) {
            Ok(prepared) => prepared,
            Err(error) => {
                let (error, slot) = match error {
                    ordered_table::InsertError::Limit(slot) => (SystemCallError::ReachLimit, slot),
                    ordered_table::InsertError::Allocation(slot) => {
                        (SystemCallError::OutOfMemory, slot)
                    }
                };
                let _ = self.timers.cancel(timer);
                let _ = self.timers.cancel(retry_timer);
                return Err(InsertFailure {
                    error,
                    task: slot.task,
                });
            }
        };
        self.tasks.insert_prepared(prepared);
        self.schedule(id).expect("new task must exist");
        Ok(id)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub(crate) fn len(&self) -> usize {
        self.tasks.len()
    }

    pub(crate) fn has_ready(&self) -> bool {
        self.head != 0
    }

    pub(crate) fn get_task_mut(&mut self, id: u64) -> Option<&mut T> {
        self.tasks.get_mut(id).map(|slot| &mut slot.task)
    }

    pub(crate) fn slot(&mut self, id: u64) -> Option<&mut Slot<T>> {
        self.tasks.get_mut(id)
    }

    pub(crate) fn schedule(&mut self, id: u64) -> Result<(), SystemCallError> {
        let slot = self
            .tasks
            .get_mut(id)
            .ok_or(SystemCallError::ObjectNotFound)?;
        if slot.queued || slot.suspended || slot.lifecycle != Lifecycle::Active {
            return Ok(());
        }
        slot.queued = true;
        slot.previous = self.tail;
        slot.next = 0;
        let tail = self.tail;
        if tail == 0 {
            self.head = id
        } else {
            self.tasks
                .get_mut(tail)
                .expect("task ready tail disappeared")
                .next = id;
        }
        self.tail = id;
        Ok(())
    }

    fn unschedule(&mut self, id: u64) {
        let Some(slot) = self.tasks.get_mut(id) else {
            return;
        };
        if !slot.queued {
            return;
        }
        let previous = slot.previous;
        let next = slot.next;
        slot.previous = 0;
        slot.next = 0;
        slot.queued = false;
        if previous == 0 {
            self.head = next;
        } else if let Some(previous_slot) = self.tasks.get_mut(previous) {
            previous_slot.next = next;
        }
        if next == 0 {
            self.tail = previous;
        } else if let Some(next_slot) = self.tasks.get_mut(next) {
            next_slot.previous = previous;
        }
    }

    pub(crate) fn pop_ready(&mut self) -> Option<u64> {
        let id = self.head;
        if id == 0 {
            return None;
        }
        let slot = self.tasks.get_mut(id).expect("task ready head disappeared");
        let next = slot.next;
        slot.previous = 0;
        slot.next = 0;
        slot.queued = false;
        self.head = next;
        if next == 0 {
            self.tail = 0
        } else {
            self.tasks
                .get_mut(next)
                .expect("task ready successor disappeared")
                .previous = 0;
        }
        Some(id)
    }

    pub(crate) fn next_deadline(&self) -> Deadline {
        self.timers
            .peek_expires_at()
            .map_or(Deadline::INFINITE, Deadline::at)
    }

    pub(crate) fn expire(&mut self, now: u64, budget: usize) -> usize {
        let mut used = 0;
        while used < budget {
            let Some((token, at, &wake)) = self.timers.peek() else {
                break;
            };
            if at > now {
                break;
            }
            let id = match wake {
                Wake::Deadline(id) => {
                    self.mature_deadline(id, now);
                    id
                }
                Wake::Retry(id) => {
                    assert!(self.timers.park(token), "expired retry timer disappeared");
                    self.tasks
                        .get_mut(id)
                        .expect("retry task disappeared")
                        .suspended = false;
                    id
                }
            };
            self.schedule(id).expect("expired task must remain owned");
            used += 1;
        }
        used
    }

    /// 任务重新执行或Gate改变期限前，兑现它自己已到期的通知义务。
    /// 不依赖有预算的全局到期队列已走到该timer，也不扫描其他任务。
    pub(crate) fn mature_deadline(&mut self, id: u64, now: u64) {
        let slot = self.tasks.get_mut(id).expect("deadline task remains owned");
        if slot.deadline_pending && slot.deadline_value.is_some_and(|at| at <= now) {
            assert!(
                self.timers.park(slot.timer),
                "pending deadline timer remains owned"
            );
            slot.deadline_pending = false;
            slot.timed_out = true;
        }
    }

    /// 执行失败退避独立于业务期限；重试唤醒不伪造 timeout 输入。
    pub(crate) fn defer(&mut self, id: u64, retry_at: Option<u64>) -> Result<(), SystemCallError> {
        self.unschedule(id);
        let slot = self
            .tasks
            .get_mut(id)
            .ok_or(SystemCallError::ObjectNotFound)?;
        slot.suspended = true;
        assert!(
            match retry_at {
                Some(at) => self.timers.reschedule(slot.retry_timer, at),
                None => self.timers.park(slot.retry_timer),
            },
            "task retry timer must remain owned"
        );
        Ok(())
    }

    pub(crate) fn seal(&mut self) {
        self.sealed = true;
    }

    pub(crate) fn is_sealed(&self) -> bool {
        self.sealed
    }

    /// 停止冻结新任务准入，但既有任务的清理仍可安装来源。
    pub(crate) fn accepts_sources(&self, id: u64) -> bool {
        self.tasks
            .get(id)
            .is_some_and(|slot| slot.lifecycle == Lifecycle::Active)
    }

    pub(crate) fn is_finalizing(&self, id: u64) -> bool {
        self.tasks.get(id).is_some_and(|slot| {
            matches!(slot.lifecycle, Lifecycle::Finalizing | Lifecycle::Retiring)
        })
    }

    pub(crate) fn has_retire_ready(&self) -> bool {
        self.retire_ready != 0
    }

    pub(crate) fn has_unmarked_sources(&self) -> bool {
        self.unmarked_sources != 0
    }

    /// 来源从活动输入表移到退休账本；返回 swap_remove 移到该位置的来源。
    pub(crate) fn detach_source(&mut self, id: u64, index: usize) -> Option<u64> {
        let slot = self
            .tasks
            .get_mut(id)
            .expect("source task must remain owned");
        slot.sources.swap_remove(index);
        slot.retiring_sources += 1;
        if slot.lifecycle == Lifecycle::Finalizing {
            self.unmarked_sources -= 1;
        }
        slot.sources.get(index).copied()
    }

    pub(crate) fn source_retired(&mut self, id: u64) {
        let slot = self
            .tasks
            .get_mut(id)
            .expect("retiring source task must remain owned");
        slot.retiring_sources -= 1;
        if slot.lifecycle == Lifecycle::Finalizing
            && slot.sources.is_empty()
            && slot.retiring_sources == 0
        {
            self.retire_ready += 1;
        }
    }

    pub(crate) fn stop<W>(&mut self, id: u64, world: &mut W) -> Result<(), SystemCallError>
    where
        T: Task<W>,
    {
        if self
            .tasks
            .get(id)
            .is_none_or(|slot| slot.lifecycle != Lifecycle::Active)
        {
            return Ok(());
        }
        let slot = self
            .tasks
            .get_mut(id)
            .ok_or(SystemCallError::ObjectNotFound)?;
        slot.suspended = false;
        self.timers.park(slot.retry_timer);
        if !slot.stopping {
            slot.stopping = true;
            slot.task.stop(world);
        }
        self.schedule(id)
    }

    /// Seal 后每步只发出一个停止意图，任务自身仍通过 advance 按预算退休。
    pub(crate) fn stop_next<W>(&mut self, world: &mut W) -> Result<bool, SystemCallError>
    where
        T: Task<W>,
    {
        if !self.sealed {
            return Err(SystemCallError::ObjectNotAvailable);
        }
        let mut ids = [0];
        if self
            .tasks
            .scan_visible(|_| true, self.stop_cursor, &mut ids)
            .0
            == 0
        {
            return Ok(false);
        }
        let id = ids[0];
        self.stop_cursor = id;
        self.stop(id, world)?;
        Ok(true)
    }

    pub(crate) fn begin_finalizing(&mut self, id: u64) {
        self.unschedule(id);
        let slot = self
            .tasks
            .get_mut(id)
            .expect("completed task must remain owned");
        if slot.lifecycle != Lifecycle::Active {
            return;
        }
        slot.lifecycle = Lifecycle::Finalizing;
        self.unmarked_sources += slot.sources.len();
        if slot.sources.is_empty() && slot.retiring_sources == 0 {
            self.retire_ready += 1;
        }
    }

    /// 每次至多检查一个任务。仅在确有可退休任务时扫描，游标跨轮保留。
    pub(crate) fn retire_one(&mut self) -> bool {
        if self.retire_ready == 0 {
            return false;
        }
        let id = self
            .tasks
            .next_after(Some(&self.retire_cursor))
            .or_else(|| self.tasks.next_after(None))
            .map(|(&id, _)| id)
            .expect("retirement count requires an owned task");
        self.retire_cursor = id;
        let slot = self
            .tasks
            .get(id)
            .expect("retirement cursor must name a task");
        if slot.lifecycle != Lifecycle::Finalizing
            || !slot.sources.is_empty()
            || slot.retiring_sources != 0
        {
            return false;
        }
        self.unschedule(id);
        let slot = self
            .tasks
            .get_mut(id)
            .expect("retiring task must remain owned");
        slot.lifecycle = Lifecycle::Retiring;
        let _ = self.timers.cancel(slot.timer);
        let _ = self.timers.cancel(slot.retry_timer);
        let _ = self.tasks.remove(id);
        self.retire_ready -= 1;
        true
    }

    /// 业务期限独立于 ready/Hold/Gate；同一已投递期限不会被重复武装。
    pub(crate) fn update_deadline(
        &mut self,
        id: u64,
        deadline: Result<Option<u64>, SystemCallError>,
    ) -> Result<(), SystemCallError> {
        let deadline = deadline?;
        let slot = self
            .tasks
            .get_mut(id)
            .ok_or(SystemCallError::ObjectNotFound)?;
        if slot.deadline_value == deadline {
            return Ok(());
        }
        assert!(
            match deadline {
                Some(at) => self.timers.reschedule(slot.timer, at),
                None => self.timers.park(slot.timer),
            },
            "live task timer remains owned"
        );
        slot.deadline_value = deadline;
        slot.deadline_pending = deadline.is_some();
        Ok(())
    }

    /// advance 返回后的期限与队列收尾：Complete 进入有界退休。
    pub(crate) fn finish_advance(
        &mut self,
        id: u64,
        deadline: Result<Option<u64>, SystemCallError>,
        step: crate::runtime::Step,
        has_pending: bool,
    ) -> Result<(), SystemCallError> {
        let slot = self.tasks.get(id).expect("advanced task disappeared");
        let timer = slot.timer;
        if step != crate::runtime::Step::Complete
            && let Err(error) = self.update_deadline(id, deadline)
        {
            self.schedule(id)
                .expect("invalid-deadline task must remain owned");
            return Err(error);
        }
        match step {
            crate::runtime::Step::Complete => {
                // Complete 只是业务完成提议；来源与 Gate 清理完成后才退休。
                self.begin_finalizing(id);
                let _ = self.timers.cancel(timer);
            }
            crate::runtime::Step::Runnable => {
                self.schedule(id).expect("yielding task must remain owned")
            }
            crate::runtime::Step::Parked => {
                if has_pending {
                    self.schedule(id)
                        .expect("task with pending input must remain owned");
                }
            }
        }
        Ok(())
    }
}

impl<T> Drop for WorkQueue<T> {
    fn drop(&mut self) {
        if !self.tasks.is_empty() {
            // 非空放弃是缺陷信号：任务 owner 在各自 Drop 中有界收束，
            // 内核 handle 由进程边界（ProcessDrain/进程退出）接管。
            ABANDONED_TASKS.fetch_add(self.tasks.len(), Ordering::Relaxed);
        }
        // 逐槽正常析构；不在放弃路径做递归推进或无界等待。
    }
}
