//! 已准入内核请求：稳定目标、跨暂停预算与 affine 批次责任。

pub(crate) mod selftest;

use alloc::sync::Arc;
use core::sync::atomic::Ordering;
use erhino_shared::{
    call::SystemCallError,
    proc::{ProcessDrainResult, ProcessDrainStatus, ProcessExitReason, ProcessFaultCode},
};
use work_debt::{StepResult, StepState};

use super::{Thread, object::ObjectRef, proc::Process, process::ProcessControl};

#[derive(Clone)]
pub(crate) enum FinishDependency {
    Retirement(ObjectRef),
}

impl FinishDependency {
    pub(crate) fn register(
        &self,
        action: crate::deferred_work::WakeAction,
        cancelled: impl FnOnce() -> bool,
    ) {
        match self {
            Self::Retirement(object) => {
                let target = object
                    .retirement()
                    .expect("retirement dependency lost its backend");
                target
                    .completion()
                    .register(action, || target.is_finished() || cancelled());
            }
        }
    }

    pub(crate) fn cancel(&self, key: crate::deferred_work::WaitKey) {
        match self {
            Self::Retirement(object) => object
                .retirement()
                .expect("retirement dependency lost its backend")
                .completion()
                .cancel(key),
        }
    }
}

pub(crate) struct DrainRequest {
    process: Arc<Process>,
    _control: Arc<ProcessControl>,
    output: usize,
    budget: usize,
    work_done: usize,
}

impl DrainRequest {
    pub(crate) fn acquire(
        process: Arc<Process>,
        control: Arc<ProcessControl>,
        output: usize,
        budget: usize,
    ) -> Result<Self, SystemCallError> {
        let gate = process.drain_gate.lock();
        process
            .drain_active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| SystemCallError::ObjectBusy)?;
        drop(gate);
        Ok(Self {
            process,
            _control: control,
            output,
            budget,
            work_done: 0,
        })
    }

    pub(crate) fn step(&mut self, caller: &Thread, turn: usize) -> StepResult<FinishDependency> {
        let remaining = self.budget - self.work_done;
        let (used, complete) = {
            let _gate = self.process.drain_gate.lock();
            self.process.drain_batch(remaining.min(turn))
        };
        assert!(
            used <= remaining.min(turn),
            "drain request exceeded its remaining budget"
        );
        self.work_done += used;
        let state = if complete || self.work_done == self.budget {
            let result = ProcessDrainResult {
                work_done: self.work_done as u32,
                status: if complete {
                    ProcessDrainStatus::Complete as u32
                } else {
                    ProcessDrainStatus::More as u32
                },
                reserved: 0,
            };
            // SAFETY: 固定宽结果全部初始化，无 padding；完成方使用间接目标访问。
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    (&result as *const ProcessDrainResult).cast::<u8>(),
                    core::mem::size_of::<ProcessDrainResult>(),
                )
            };
            let copied = {
                let mut space = caller.process.space.lock();
                crate::uaccess::put_user_indirect(&mut space, self.output, bytes)
            };
            if copied.is_err() {
                let todo = caller.process.lifecycle.request_termination(
                    ProcessExitReason::Fault,
                    ProcessFaultCode::StoreAccess as i64,
                    None,
                );
                super::process::run_termination_todo(&caller.process, todo);
            }
            StepState::Complete
        } else if used == 0 {
            StepState::Blocked(self.process.drain_dependency())
        } else {
            StepState::Runnable
        };
        StepResult {
            work_done: used,
            state,
        }
    }
}

impl Drop for DrainRequest {
    fn drop(&mut self) {
        assert!(
            self.process.drain_active.swap(false, Ordering::AcqRel),
            "drain batch permit released twice"
        );
        self.process.finalization_dependency.notify();
    }
}
