//! 单 actor 的扁平等待装配：唯一接收/停驻/关闭 owner，运行体只持登记借用。

use crate::source::{Registration, SetState, SourceOps};
use crate::{
    ExecutionResource,
    runtime::{DriveState, Runtime, Task, TaskFailure},
};
use alloc::{rc::Rc, vec::Vec};
use core::cell::Cell;
use erhino_shared::{
    call::SystemCallError,
    time::Deadline,
    wait_set::{RECEIVE_MAX, ReadyRecord},
};
use libbudget::AccountView;

pub struct Assembly<S: SourceOps> {
    state: Rc<SetState<S>>,
    records: Vec<ReadyRecord>,
}
impl<S: SourceOps> Assembly<S> {
    pub fn input_budget() -> Result<usize, SystemCallError> {
        core::mem::size_of::<ReadyRecord>()
            .checked_mul(RECEIVE_MAX)
            .and_then(|n| n.checked_add(core::mem::size_of::<SetState<S>>()))
            .ok_or(SystemCallError::ReachLimit)
    }
    /// 所有 fallible 存储在取得 set 前准备，失败保留原集合 owner。
    pub fn new(
        set: S,
        account: &AccountView<ExecutionResource>,
    ) -> Result<Self, (S, SystemCallError)> {
        let prepared = (|| {
            let charge = account.acquire(ExecutionResource::InputBytes, Self::input_budget()?)?;
            let mut records = Vec::new();
            records
                .try_reserve_exact(RECEIVE_MAX)
                .map_err(|_| SystemCallError::OutOfMemory)?;
            records.resize(
                RECEIVE_MAX,
                ReadyRecord {
                    token: 0,
                    cookie: 0,
                    observed: erhino_shared::object::ObjectSignals::NONE,
                    reason: 0,
                    reserved: 0,
                },
            );
            let state = Rc::try_new(SetState {
                set: None,
                failed: Cell::new(false),
                _charge: charge,
            })
            .map_err(|_| SystemCallError::OutOfMemory)?;
            Ok::<_, SystemCallError>((state, records))
        })();
        let (mut state, records) = match prepared {
            Ok(value) => value,
            Err(error) => return Err((set, error)),
        };
        Rc::get_mut(&mut state)
            .expect("unpublished assembly acquired bindings")
            .set = Some(set);
        Ok(Self { state, records })
    }
    pub fn bind(&self) -> Result<Registration<S>, SystemCallError> {
        if self.state.failed.get() {
            return Err(SystemCallError::ObjectClosed);
        }
        Ok(Registration {
            state: self.state.clone(),
        })
    }
    fn set(&self) -> Result<&S, SystemCallError> {
        if self.state.failed.get() {
            return Err(SystemCallError::ObjectClosed);
        }
        Ok(self.state.set.as_ref().expect("assembly already closed"))
    }
    /// 只接收一个有界批次；短批次不是“集合已空”的证明。
    /// 外层先分流本批，再推进各 Runtime，不能连续排空 Receive。
    pub fn receive(&mut self, budget: usize) -> Result<&[ReadyRecord], SystemCallError> {
        if budget == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let capacity = budget.min(RECEIVE_MAX);
        if self.state.failed.get() {
            return Err(SystemCallError::ObjectClosed);
        }
        let set = self.state.set.as_ref().expect("assembly already closed");
        let count = match set.receive_into(&mut self.records[..capacity]) {
            Ok(count) => count,
            Err(SystemCallError::ObjectNotAvailable) => 0,
            Err(error) => return Err(error),
        };
        if count > capacity {
            return Err(SystemCallError::InternalError);
        }
        Ok(&self.records[..count])
    }
    pub fn wait(&self, deadline: Deadline) -> Result<(), SystemCallError> {
        self.set()?.wait(deadline)
    }
    pub fn is_failed(&self) -> bool {
        self.state.failed.get()
    }

    /// 单运行体同步组合；每轮一个 Receive 批与任务量子，接收不占掉任务份额。
    pub fn run<T, W>(
        &mut self,
        runtime: &mut Runtime<T, S>,
        world: &mut W,
        max_work: usize,
    ) -> Result<(), TaskFailure>
    where
        T: Task<W, Family = T>,
    {
        if max_work == 0 {
            return Err(TaskFailure {
                task: 0,
                error: SystemCallError::IllegalArgument,
            });
        }
        if !Rc::ptr_eq(&self.state, &runtime.binding().state) || Rc::strong_count(&self.state) != 2
        {
            return Err(TaskFailure {
                task: 0,
                error: SystemCallError::ObjectBusy,
            });
        }
        loop {
            if runtime.drive_state() == DriveState::Drained {
                return Ok(());
            }
            let records = self
                .receive(RECEIVE_MAX)
                .map_err(|error| TaskFailure { task: 0, error })?;
            for &record in records {
                runtime.feed(record);
            }
            runtime.turn(world, max_work)?;
            match runtime.drive_state() {
                DriveState::Drained => return Ok(()),
                DriveState::Runnable => {}
                DriveState::Waiting(deadline) => self
                    .wait(deadline)
                    .map_err(|error| TaskFailure { task: 0, error })?,
            }
        }
    }
    /// 全部 Runtime 正常释放借用后才允许关闭；失败保留真实集合 owner。
    pub fn close(mut self) -> Result<(), (Self, SystemCallError)> {
        let Some(state) = Rc::get_mut(&mut self.state) else {
            return Err((self, SystemCallError::ObjectBusy));
        };
        let set = state.set.take().expect("assembly already closed");
        match set.close_set() {
            Ok(()) => Ok(()),
            Err((set, error)) => {
                state.set = Some(set);
                Err((self, error))
            }
        }
    }
}
impl<S: SourceOps> Drop for Assembly<S> {
    fn drop(&mut self) {
        // 接收 owner 消散即使整个装配失效；借用只能继续解除登记，不能独自服务。
        if Rc::strong_count(&self.state) > 1 {
            self.state.failed.set(true);
        }
    }
}
