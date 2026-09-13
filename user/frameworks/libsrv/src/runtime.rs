//! 单 actor 执行驱动；轮转期限、输入和任务三个工作面，max_work=1 也不饿死任何面。

use crate::{
    budget::{Account, Charge, Resource},
    work_queue::{InsertFailure, Task, TaskFailure, WorkQueue},
};
use alloc::{sync::Arc, vec::Vec};
use erhino_shared::{
    call::SystemCallError,
    object::ObjectSignals,
    wait_set::{RECEIVE_MAX, ReadyRecord},
};
use rinlib::ipc::wait_set::WaitSet;

pub struct Runtime<'a, T> {
    set: &'a WaitSet,
    tasks: WorkQueue<T>,
    records: Vec<ReadyRecord>,
    phase: u8,
    _input_charge: Charge,
}

impl<'a, T> Runtime<'a, T> {
    pub fn new(
        set: &'a WaitSet,
        task_limit: usize,
        account: &Arc<Account>,
    ) -> Result<Self, SystemCallError> {
        let input_charge = account.acquire(
            Resource::Bytes,
            RECEIVE_MAX * core::mem::size_of::<ReadyRecord>(),
        )?;
        let mut records = Vec::new();
        records
            .try_reserve_exact(RECEIVE_MAX)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        records.resize(
            RECEIVE_MAX,
            ReadyRecord {
                token: 0,
                arm_generation: 0,
                cookie: 0,
                observed: ObjectSignals::NONE,
                reason: 0,
                error: 0,
            },
        );
        Ok(Self {
            set,
            tasks: WorkQueue::new(task_limit)?,
            records,
            phase: 0,
            _input_charge: input_charge,
        })
    }

    pub fn spawn(
        &mut self,
        task: T,
        account: &Arc<Account>,
        max_sources: usize,
    ) -> Result<u64, InsertFailure<T>> {
        self.tasks.insert(task, account, max_sources)
    }
    pub fn get_mut(&mut self, id: u64) -> Option<&mut T> {
        self.tasks.get_mut(id)
    }
    pub fn wake(&mut self, id: u64) -> Result<(), SystemCallError> {
        self.tasks.schedule(id)
    }
    pub fn stop<W>(&mut self, id: u64, world: &mut W) -> Result<(), SystemCallError>
    where
        T: Task<W>,
    {
        self.tasks.stop(id, world)
    }
    pub fn seal(&mut self) {
        self.tasks.seal();
    }
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
    pub fn has_ready(&self) -> bool {
        self.tasks.has_ready()
    }

    pub fn turn<W>(&mut self, world: &mut W, max_work: usize) -> Result<usize, TaskFailure>
    where
        T: Task<W>,
    {
        if max_work == 0 {
            return Err(TaskFailure {
                task: 0,
                error: SystemCallError::IllegalArgument,
            });
        }
        let slice = max_work.div_ceil(3);
        let mut used = 0;
        while used < max_work {
            let budget = slice.min(max_work - used);
            let phase = self.phase;
            self.phase = (self.phase + 1) % 3;
            let actual = match phase {
                0 => {
                    let now = rinlib::time::snapshot()
                        .map_err(|error| TaskFailure { task: 0, error })?
                        .now_ns;
                    self.tasks.expire(now, budget)
                }
                1 => {
                    let capacity = budget.min(RECEIVE_MAX);
                    match self.set.receive_into(&mut self.records[..capacity]) {
                        Ok(count) => {
                            let mut failure = None;
                            for record in self.records[..count].iter().copied() {
                                if let Err(error) = self.tasks.feed(record) {
                                    failure.get_or_insert(error);
                                }
                            }
                            if let Some(error) = failure {
                                return Err(error);
                            }
                            count
                        }
                        Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => 0,
                        Err(error) => return Err(TaskFailure { task: 0, error }),
                    }
                }
                _ => self
                    .tasks
                    .advance_one(world, budget)?
                    .map_or(0, |(_, advance)| advance.work_done),
            };
            // 查询空工作面也有固定成本，计一单位并让下一轮从后继工作面开始。
            used += actual.max(1);
        }
        Ok(used)
    }

    /// 无用户态 runnable 时原子观察集合；新 ready 或期限到达均由内核唤醒。
    pub fn wait(&self) -> Result<(), SystemCallError> {
        if self.tasks.has_ready() {
            return Ok(());
        }
        self.set.wait(self.tasks.next_deadline()).map(|_| ())
    }

    pub fn shutdown_turn<W>(&mut self, world: &mut W, max_work: usize) -> Result<usize, TaskFailure>
    where
        T: Task<W>,
    {
        if max_work == 0 {
            return Err(TaskFailure {
                task: 0,
                error: SystemCallError::IllegalArgument,
            });
        }
        let stopped = usize::from(
            self.tasks
                .stop_next(world)
                .map_err(|error| TaskFailure { task: 0, error })?,
        );
        if stopped == max_work {
            return Ok(stopped);
        }
        self.turn(world, max_work - stopped)
            .map(|used| used + stopped)
    }

    /// 任务清空之后调用方再 Close 外部 WaitSet，不提前切断业务退休事件。
    #[expect(clippy::result_large_err, reason = "关闭失败返还仍拥有任务的运行时，错误路径不分配")]
    pub fn close(self) -> Result<(), Self> {
        if self.tasks.is_sealed() && self.tasks.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }
}
