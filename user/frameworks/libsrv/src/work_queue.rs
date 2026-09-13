//! 稳定任务记录与公平 ready FIFO；输入槽和任务额度在公开前准备。

use crate::budget::{Account, Charge, Resource};
use alloc::{collections::VecDeque, sync::Arc};
use core::{
    mem::ManuallyDrop,
    sync::atomic::{AtomicUsize, Ordering},
};
use erhino_shared::{call::SystemCallError, time::Deadline, wait_set::ReadyRecord};
use ordered_table::OrderedTable;
use timer_queue::{TimerQueue, TimerToken};

static NEXT_TASK: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);
static ABANDONED_TASKS: AtomicUsize = AtomicUsize::new(0);

pub fn abandoned_tasks() -> usize {
    ABANDONED_TASKS.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Runnable,
    Parked,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Advance {
    pub work_done: usize,
    pub step: Step,
}

pub struct Events<'a> {
    ready: &'a mut VecDeque<ReadyRecord>,
    timed_out: &'a mut bool,
}

impl Events<'_> {
    pub fn pop(&mut self) -> Option<ReadyRecord> {
        self.ready.pop_front()
    }
    pub fn len(&self) -> usize {
        self.ready.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ready.is_empty()
    }
    pub fn take_timeout(&mut self) -> bool {
        core::mem::take(self.timed_out)
    }
}

pub trait Task<W> {
    /// input 仅能消费，处理体须在 budget 内推进，不能等待客户端或下游完成。
    fn advance(
        &mut self,
        id: u64,
        world: &mut W,
        input: &mut Events<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError>;
    /// 只冻结停止意图；实际资源注销和关闭在后续 advance 内按预算推进。
    fn stop(&mut self, world: &mut W);
    fn deadline(&self) -> Deadline {
        Deadline::INFINITE
    }
}

struct Slot<T> {
    task: T,
    events: VecDeque<ReadyRecord>,
    event_limit: usize,
    previous: u64,
    next: u64,
    queued: bool,
    stopping: bool,
    timer: TimerToken,
    timed_out: bool,
    _task_charge: Charge,
    _event_charge: Charge,
}

pub struct WorkQueue<T> {
    tasks: ManuallyDrop<OrderedTable<Slot<T>>>,
    head: u64,
    tail: u64,
    sealed: bool,
    stop_cursor: u64,
    timers: TimerQueue<u64>,
}

#[derive(Debug)]
pub struct InsertFailure<T> {
    pub error: SystemCallError,
    pub task: T,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskFailure {
    pub task: u64,
    pub error: SystemCallError,
}

impl<T> WorkQueue<T> {
    pub fn new(limit: usize) -> Result<Self, SystemCallError> {
        if limit == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        Ok(Self {
            tasks: ManuallyDrop::new(OrderedTable::new(limit)),
            head: 0,
            tail: 0,
            sealed: false,
            stop_cursor: 0,
            timers: TimerQueue::new(0),
        })
    }

    pub fn insert(
        &mut self,
        task: T,
        account: &Arc<Account>,
        event_limit: usize,
    ) -> Result<u64, InsertFailure<T>> {
        let prepare = (|| {
            if self.sealed {
                return Err(SystemCallError::ObjectClosed);
            }
            if event_limit == 0 {
                return Err(SystemCallError::IllegalArgument);
            }
            let id = NEXT_TASK.allocate().ok_or(SystemCallError::ReachLimit)?;
            let task_charge = account.acquire(Resource::Task, 1)?;
            let bytes = event_limit
                .checked_mul(core::mem::size_of::<ReadyRecord>())
                .ok_or(SystemCallError::QuotaExceeded)?;
            let event_charge = account.acquire(Resource::Bytes, bytes)?;
            let mut events = VecDeque::new();
            events
                .try_reserve_exact(event_limit)
                .map_err(|_| SystemCallError::OutOfMemory)?;
            Ok((id, task_charge, event_charge, events))
        })();
        let (id, task_charge, event_charge, events) = match prepare {
            Ok(prepared) => prepared,
            Err(error) => return Err(InsertFailure { error, task }),
        };
        let timer = match self.timers.try_register(0, id) {
            Ok(timer) => timer,
            Err(_) => {
                return Err(InsertFailure {
                    error: SystemCallError::OutOfMemory,
                    task,
                });
            }
        };
        assert!(self.timers.park(timer), "new task timer must exist");
        let slot = Slot {
            task,
            events,
            event_limit,
            previous: 0,
            next: 0,
            queued: false,
            stopping: false,
            timer,
            timed_out: false,
            _task_charge: task_charge,
            _event_charge: event_charge,
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

    pub fn len(&self) -> usize {
        self.tasks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
    pub fn has_ready(&self) -> bool {
        self.head != 0
    }
    pub fn get_mut(&mut self, id: u64) -> Option<&mut T> {
        self.tasks.get_mut(id).map(|slot| &mut slot.task)
    }

    pub fn schedule(&mut self, id: u64) -> Result<(), SystemCallError> {
        let slot = self
            .tasks
            .get_mut(id)
            .ok_or(SystemCallError::ObjectNotFound)?;
        if slot.queued {
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

    pub fn feed(&mut self, record: ReadyRecord) -> Result<(), TaskFailure> {
        let id = record.cookie;
        let Some(slot) = self.tasks.get_mut(id) else {
            // 已注销任务的迟到快照不重建任务，所有 task ID 均不复用。
            return Ok(());
        };
        if slot.events.len() == slot.event_limit {
            return Err(TaskFailure {
                task: id,
                error: SystemCallError::ReachLimit,
            });
        }
        slot.events.push_back(record);
        self.schedule(id)
            .map_err(|error| TaskFailure { task: id, error })
    }

    fn pop_ready(&mut self) -> Option<u64> {
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

    pub fn next_deadline(&self) -> Deadline {
        self.timers
            .peek_expires_at()
            .map_or(Deadline::INFINITE, Deadline::at)
    }

    pub fn expire(&mut self, now: u64, budget: usize) -> usize {
        let mut used = 0;
        while used < budget {
            let Some((token, at, &id)) = self.timers.peek() else {
                break;
            };
            if at > now {
                break;
            }
            assert!(self.timers.park(token), "expired task timer disappeared");
            self.tasks
                .get_mut(id)
                .expect("timer task disappeared")
                .timed_out = true;
            self.schedule(id).expect("expired task must remain owned");
            used += 1;
        }
        used
    }

    pub fn seal(&mut self) {
        self.sealed = true;
    }
    pub fn is_sealed(&self) -> bool {
        self.sealed
    }

    pub fn stop<W>(&mut self, id: u64, world: &mut W) -> Result<(), SystemCallError>
    where
        T: Task<W>,
    {
        let slot = self
            .tasks
            .get_mut(id)
            .ok_or(SystemCallError::ObjectNotFound)?;
        if !slot.stopping {
            slot.stopping = true;
            slot.task.stop(world);
        }
        self.schedule(id)
    }

    /// Seal 后每步只发出一个停止意图，任务自身仍通过 advance 按预算退休。
    pub fn stop_next<W>(&mut self, world: &mut W) -> Result<bool, SystemCallError>
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

    #[expect(
        clippy::result_large_err,
        reason = "关闭失败原样返还任务拥有者，不能在错误路径新增装箱分配"
    )]
    pub fn close(self) -> Result<(), Self> {
        if self.sealed && self.tasks.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }

    pub fn advance_one<W>(
        &mut self,
        world: &mut W,
        budget: usize,
    ) -> Result<Option<(u64, Advance)>, TaskFailure>
    where
        T: Task<W>,
    {
        if budget == 0 {
            return Err(TaskFailure {
                task: 0,
                error: SystemCallError::IllegalArgument,
            });
        }
        let Some(id) = self.pop_ready() else {
            return Ok(None);
        };
        let result = {
            let slot = self.tasks.get_mut(id).expect("runnable task disappeared");
            slot.task.advance(
                id,
                world,
                &mut Events {
                    ready: &mut slot.events,
                    timed_out: &mut slot.timed_out,
                },
                budget,
            )
        };
        let mut advance = match result {
            Ok(advance) if advance.work_done <= budget => advance,
            result => {
                self.schedule(id).expect("failed task must remain owned");
                let error = result.err().unwrap_or(SystemCallError::InternalError);
                return Err(TaskFailure { task: id, error });
            }
        };
        advance.work_done = advance.work_done.max(1);
        let slot = self.tasks.get(id).expect("advanced task disappeared");
        let timer = slot.timer;
        if advance.step != Step::Complete {
            let deadline = match slot.task.deadline().instant() {
                Ok(deadline) => deadline,
                Err(_) => {
                    self.schedule(id)
                        .expect("invalid-deadline task must remain owned");
                    return Err(TaskFailure {
                        task: id,
                        error: SystemCallError::IllegalArgument,
                    });
                }
            };
            assert!(
                match deadline {
                    Some(at) => self.timers.reschedule(timer, at),
                    None => self.timers.park(timer),
                },
                "live task timer disappeared"
            );
        }
        match advance.step {
            Step::Complete => {
                // Complete 的契约是全部真实 owner 已退休，不能用 Drop 模拟取消/drain。
                let _ = self.timers.cancel(timer);
                let _ = self.tasks.remove(id);
            }
            Step::Runnable => self.schedule(id).expect("yielding task must remain owned"),
            Step::Parked => {
                let slot = self.tasks.get(id).expect("parked task disappeared");
                if !slot.events.is_empty() || slot.timed_out {
                    self.schedule(id)
                        .expect("task with pending input must remain owned");
                }
            }
        }
        Ok(Some((id, advance)))
    }
}

impl<T> Drop for WorkQueue<T> {
    fn drop(&mut self) {
        if self.tasks.is_empty() {
            // 空表已无业务 owner，释放表壳不触发增长容器 drain。
            unsafe {
                ManuallyDrop::drop(&mut self.tasks);
            }
        } else {
            // 异常遗漏由 ProcessDrain 回收内核 handle；当前短路径不能递归析构任务。
            ABANDONED_TASKS.fetch_add(self.tasks.len(), Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::Budget;

    #[derive(Debug)]
    struct ToyTask {
        turns: usize,
        stopping: bool,
    }

    impl Task<alloc::vec::Vec<u64>> for ToyTask {
        fn advance(
            &mut self,
            id: u64,
            world: &mut alloc::vec::Vec<u64>,
            _: &mut Events<'_>,
            _: usize,
        ) -> Result<Advance, SystemCallError> {
            world.push(id);
            self.turns -= 1;
            Ok(Advance {
                work_done: 1,
                step: if self.stopping || self.turns == 0 {
                    Step::Complete
                } else {
                    Step::Runnable
                },
            })
        }
        fn stop(&mut self, _: &mut alloc::vec::Vec<u64>) {
            self.stopping = true;
        }
    }

    fn account() -> Arc<Account> {
        let mut limits = [0; Resource::COUNT];
        limits[Resource::Account as usize] = 1;
        limits[Resource::Task as usize] = 4;
        limits[Resource::Bytes as usize] = 1000;
        Budget::new(limits).unwrap().account(limits).unwrap()
    }

    #[test]
    fn yielding_tasks_alternate_and_refund_on_real_completion() {
        let account = account();
        let mut queue = WorkQueue::new(4).unwrap();
        let first = queue
            .insert(
                ToyTask {
                    turns: 2,
                    stopping: false,
                },
                &account,
                1,
            )
            .unwrap();
        let second = queue
            .insert(
                ToyTask {
                    turns: 2,
                    stopping: false,
                },
                &account,
                1,
            )
            .unwrap();
        let mut world = alloc::vec::Vec::new();
        for _ in 0..4 {
            queue.advance_one(&mut world, 1).unwrap();
        }
        assert_eq!(world, alloc::vec![first, second, first, second]);
        assert_eq!(account.usage(Resource::Task).0, 0);
        assert_eq!(account.usage(Resource::Bytes).0, 0);
        queue.seal();
        assert!(queue.close().is_ok());
    }

    #[derive(Debug)]
    struct TimedTask {
        waiting: bool,
    }

    impl Task<()> for TimedTask {
        fn advance(
            &mut self,
            _: u64,
            _: &mut (),
            input: &mut Events<'_>,
            _: usize,
        ) -> Result<Advance, SystemCallError> {
            if input.take_timeout() {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            self.waiting = true;
            Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            })
        }
        fn stop(&mut self, _: &mut ()) {
            self.waiting = false;
        }
        fn deadline(&self) -> Deadline {
            if self.waiting {
                Deadline::at(20)
            } else {
                Deadline::INFINITE
            }
        }
    }

    #[test]
    fn deadline_wakes_parked_task_using_existing_timer_slot() {
        let account = account();
        let mut queue = WorkQueue::new(4).unwrap();
        queue
            .insert(TimedTask { waiting: false }, &account, 1)
            .unwrap();
        queue.advance_one(&mut (), 1).unwrap();
        assert!(!queue.has_ready());
        assert_eq!(queue.next_deadline(), Deadline::at(20));
        assert_eq!(queue.expire(19, 1), 0);
        assert_eq!(queue.expire(20, 1), 1);
        assert!(queue.has_ready());
        queue.advance_one(&mut (), 1).unwrap();
        assert!(queue.is_empty());
        assert!(queue.timers.is_empty());
        queue.seal();
        assert!(queue.close().is_ok());
    }

    #[test]
    fn seal_rejects_new_owner_and_stops_one_task_per_step() {
        let account = account();
        let mut queue = WorkQueue::new(4).unwrap();
        queue
            .insert(
                ToyTask {
                    turns: 100,
                    stopping: false,
                },
                &account,
                1,
            )
            .unwrap();
        queue
            .insert(
                ToyTask {
                    turns: 100,
                    stopping: false,
                },
                &account,
                1,
            )
            .unwrap();
        queue.seal();
        assert!(matches!(
            queue.insert(
                ToyTask {
                    turns: 1,
                    stopping: false
                },
                &account,
                1
            ),
            Err(InsertFailure {
                error: SystemCallError::ObjectClosed,
                ..
            })
        ));
        let mut world = alloc::vec::Vec::new();
        assert!(queue.stop_next(&mut world).unwrap());
        assert!(queue.stop_next(&mut world).unwrap());
        assert!(!queue.stop_next(&mut world).unwrap());
        queue.advance_one(&mut world, 1).unwrap();
        queue.advance_one(&mut world, 1).unwrap();
        assert!(queue.close().is_ok());
    }
}
