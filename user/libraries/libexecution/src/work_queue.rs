//! 稳定任务记录与公平 ready FIFO；任务额度在公开前准备。
//! 来源登记、事件路由与推进驱动见 [`crate::runtime`]。

use crate::{ExecutionResource, runtime::Task};
use alloc::{boxed::Box, vec::Vec};
use core::pin::Pin;
use core::sync::atomic::{AtomicUsize, Ordering};
use erhino_shared::{call::SystemCallError, time::Deadline};
use libbudget::{AccountView, Charge};
use ordered_table::OrderedTable;
use timer_queue::{TimerNode, TimerQueue};

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
    timers: Pin<Box<TaskTimers>>,
    pub(crate) timed_out: bool,
    deadline_value: Option<u64>,
    deadline_pending: bool,
    suspended: bool,
    pub(crate) _task_charge: Charge,
}

/// 任务表与公平 ready FIFO 的内部算法；公开驱动面见 [`crate::runtime::Runtime`]。
pub(crate) struct WorkQueue<T> {
    tasks: OrderedTable<Slot<T>, ()>,
    head: u64,
    tail: u64,
    sealed: bool,
    stop_cursor: u64,
    finalizing_head: u64,
    finalizing_tail: u64,
    unmarked_sources: usize,
    retire_ready: usize,
    timers: TimerQueue<Wake>,
}

#[derive(Clone, Copy)]
enum Wake {
    Deadline(u64),
    Retry(u64),
}

/// 两个独立期限共享稳定宿主，节点自身分别登记/取消。
struct TaskTimers {
    deadline: TimerNode<Wake>,
    retry: TimerNode<Wake>,
}

impl TaskTimers {
    fn deadline(self: Pin<&Self>) -> Pin<&TimerNode<Wake>> {
        // SAFETY: 私有字段不移动，宿主整体pin至两节点都注销后才析构。
        unsafe { self.map_unchecked(|timers| &timers.deadline) }
    }

    fn retry(self: Pin<&Self>) -> Pin<&TimerNode<Wake>> {
        // SAFETY: 同deadline；投影不暴露可移动字段的可变引用。
        unsafe { self.map_unchecked(|timers| &timers.retry) }
    }
}

#[derive(Debug)]
pub(crate) enum FinalizingStep {
    Idle,
    Progress,
    Source(u64),
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
            finalizing_head: 0,
            finalizing_tail: 0,
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
        let timers = match Box::try_new(TaskTimers {
            deadline: TimerNode::new(),
            retry: TimerNode::new(),
        }) {
            Ok(timers) => Box::into_pin(timers),
            Err(_) => {
                return Err(InsertFailure {
                    error: SystemCallError::OutOfMemory,
                    task,
                });
            }
        };
        let mut sources = Vec::new();
        if sources.try_reserve_exact(max_sources).is_err() {
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
            timers,
            timed_out: false,
            deadline_value: None,
            deadline_pending: false,
            suspended: false,
            _task_charge: task_charge,
        };
        let prepared = match self
            .tasks
            .prepare_insert(id, slot, &libbudget::AllocatorBacked)
        {
            Ok(prepared) => prepared,
            Err(error) => {
                let (error, slot) = match error {
                    ordered_table::InsertError::Limit(slot) => (SystemCallError::ReachLimit, slot),
                    ordered_table::InsertError::Storage(error, slot) => {
                        (libbudget::allocator_error(error), slot)
                    }
                };
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
            let Some(wake) = self.timers.pop_expired(now) else {
                break;
            };
            let id = match wake {
                Wake::Deadline(id) => {
                    self.mature_deadline(id, now);
                    id
                }
                Wake::Retry(id) => {
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
            // SAFETY: actor 独占队列，任务的 pin 节点保持存活；已弹出则取消幂等。
            unsafe {
                self.timers.cancel(slot.timers.as_ref().deadline());
            }
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
        // SAFETY: 任务出生预付两个独立 pin 节点，actor 独占修改与复用。
        unsafe {
            self.timers.cancel(slot.timers.as_ref().retry());
            if let Some(at) = retry_at {
                self.timers
                    .register(slot.timers.as_ref().retry(), at, Wake::Retry(id));
            }
        }
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

    pub(crate) fn stop<W>(
        &mut self,
        id: u64,
        world: &mut W,
        now: u64,
    ) -> Result<(), SystemCallError>
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
        self.mature_deadline(id, now);
        let slot = self
            .tasks
            .get_mut(id)
            .ok_or(SystemCallError::ObjectNotFound)?;
        slot.suspended = false;
        // SAFETY: 任务仍持有自己的 pin 节点，取消后才允许记录退休。
        unsafe {
            self.timers.cancel(slot.timers.as_ref().retry());
        }
        if !slot.stopping {
            slot.stopping = true;
            slot.task.stop(world);
        }
        let deadline = slot
            .task
            .deadline()
            .instant()
            .map_err(|_| SystemCallError::IllegalArgument);
        // 非法新期限也不能丢失原任务；先恢复执行资格，再交付错误。
        self.schedule(id)?;
        self.update_deadline(id, deadline)
    }

    /// Seal 后每步只发出一个停止意图，任务自身仍通过 advance 按预算退休。
    pub(crate) fn stop_next<W>(
        &mut self,
        world: &mut W,
        now: u64,
    ) -> Result<bool, (u64, SystemCallError)>
    where
        T: Task<W>,
    {
        if !self.sealed {
            return Err((0, SystemCallError::ObjectNotAvailable));
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
        self.stop(id, world, now).map_err(|error| (id, error))?;
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
        self.push_finalizing(id);
    }

    fn push_finalizing(&mut self, id: u64) {
        let tail = self.finalizing_tail;
        let slot = self.tasks.get_mut(id).expect("finalizing task lost owner");
        assert!(
            slot.lifecycle == Lifecycle::Finalizing && !slot.queued,
            "invalid finalizing membership"
        );
        slot.previous = tail;
        slot.next = 0;
        if tail == 0 {
            self.finalizing_head = id;
        } else {
            self.tasks
                .get_mut(tail)
                .expect("finalizing tail lost owner")
                .next = id;
        }
        self.finalizing_tail = id;
    }
    fn pop_finalizing(&mut self) -> Option<u64> {
        let id = self.finalizing_head;
        if id == 0 {
            return None;
        }
        let slot = self.tasks.get_mut(id).expect("finalizing head lost owner");
        let next = slot.next;
        slot.previous = 0;
        slot.next = 0;
        self.finalizing_head = next;
        if next == 0 {
            self.finalizing_tail = 0;
        } else {
            self.tasks
                .get_mut(next)
                .expect("finalizing successor lost owner")
                .previous = 0;
        }
        Some(id)
    }

    /// 新 Finalizing 追加尾部；每次头项同时推进来源发现或任务退休，不能交错跳过。
    pub(crate) fn finalize_one(&mut self) -> FinalizingStep {
        if self.unmarked_sources == 0 && self.retire_ready == 0 {
            return FinalizingStep::Idle;
        }
        let id = self
            .pop_finalizing()
            .expect("finalizing work requires an owned task");
        let slot = self.tasks.get(id).expect("finalizing candidate lost owner");
        if let Some(&token) = slot.sources.first() {
            self.push_finalizing(id);
            return FinalizingStep::Source(token);
        }
        if slot.retiring_sources != 0 {
            self.push_finalizing(id);
            return FinalizingStep::Progress;
        }
        let slot = self
            .tasks
            .get_mut(id)
            .expect("retiring task must remain owned");
        // SAFETY: actor 独占；两份 pin timer 先摘，再移出实际任务存储。
        unsafe {
            self.timers.cancel(slot.timers.as_ref().deadline());
            self.timers.cancel(slot.timers.as_ref().retry());
        }
        self.tasks
            .remove(id)
            .map(ordered_table::PreparedEntry::into_value);
        self.retire_ready -= 1;
        FinalizingStep::Progress
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
        // SAFETY: 该任务 pin 节点在记录退休前存活，取消/登记在同一 actor。
        unsafe {
            self.timers.cancel(slot.timers.as_ref().deadline());
            if let Some(at) = deadline {
                self.timers
                    .register(slot.timers.as_ref().deadline(), at, Wake::Deadline(id));
            }
        }
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
                let slot = self.tasks.get(id).expect("advanced task disappeared");
                // SAFETY: Finalizing 记录仍存活；解除期限节点后才可退休。
                unsafe {
                    self.timers.cancel(slot.timers.as_ref().deadline());
                }
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
        while self.timers.pop_expired(u64::MAX).is_some() {}
        if !self.tasks.is_empty() {
            // 非空放弃是缺陷信号：任务 owner 在各自 Drop 中有界收束，
            // 内核 handle 由进程自动退休边界接管。
            ABANDONED_TASKS.fetch_add(self.tasks.len(), Ordering::Relaxed);
        }
        // 逐槽正常析构；不在放弃路径做递归推进或无界等待。
    }
}
