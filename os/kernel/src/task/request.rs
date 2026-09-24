//! 已准入内核请求：稳定目标、跨暂停预算与 affine 批次责任。

pub(crate) mod selftest;

use alloc::sync::{Arc, Weak};
use core::sync::atomic::Ordering;
use erhino_shared::{
    call::SystemCallError,
    proc::{ProcessDrainResult, ProcessDrainStatus, ProcessExitReason, ProcessFaultCode},
};
use work_debt::{StepResult, StepState};

use super::{
    proc::Process,
    process::ProcessControl,
    retirement::RetirementTicket,
    thread::ThreadResultObligation,
    wait::{WaitIdentity, WaitPlan},
};
use crate::work_ledger::{DebtLedger, Reservation as LedgerReservation};

const SLOTS: usize = super::resources::PROCESS_GLOBAL_LIMIT;
type Debts = DebtLedger<Arc<Process>, SLOTS>;
type Reservation = LedgerReservation<Arc<Process>, SLOTS>;

static DEBTS: Debts = Debts::new(work_debt::TableId::new(8));

pub(crate) trait WaitOperation: Send + Sync {
    fn start(&self, key: crate::deferred_work::WaitKey);
    fn cancel(&self, key: crate::deferred_work::WaitKey);
}

struct RequestState {
    reservation: Option<Reservation>,
    request: Option<DrainRequest>,
    dependency: Option<(crate::deferred_work::WaitKey, RetirementTicket)>,
    activation: Option<Arc<Process>>,
}

pub(crate) struct DrainExecutor {
    waiter: Arc<super::wait::WaitContext>,
    used: core::sync::atomic::AtomicBool,
    state: crate::sync::Spinlock<RequestState>,
}

impl DrainExecutor {
    pub(crate) fn new(
        waiter: Arc<super::wait::WaitContext>,
        reservation: Reservation,
    ) -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            waiter,
            used: core::sync::atomic::AtomicBool::new(false),
            state: crate::sync::Spinlock::new(
                crate::sync::ranks::LEAF,
                RequestState {
                    reservation: Some(reservation),
                    request: None,
                    dependency: None,
                    activation: None,
                },
            ),
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub(crate) fn begin(
        self: &Arc<Self>,
        target: &Arc<Process>,
        request: DrainRequest,
    ) -> Result<WaitPlan, SystemCallError> {
        let first_use = !self.used.swap(true, Ordering::AcqRel);
        let (identity, mut plan) = self.waiter.prepare_reusable_wait(first_use)?;
        let operation: Arc<dyn WaitOperation> = self.clone();
        identity.bind_cancellation(Arc::downgrade(&operation));
        {
            let mut state = self.state.lock();
            assert!(
                state.request.is_none() && state.dependency.is_none() && state.activation.is_none(),
                "request executor began while a previous batch remained"
            );
            assert!(
                state.reservation.is_some(),
                "request executor lost its prepaid slot"
            );
            state.request = Some(request.with_wait(identity));
            state.activation = Some(target.clone());
        }
        plan.bind_operation(operation);
        Ok(plan)
    }

    fn take(&self) -> DrainRequest {
        let (dependency, request) = {
            let mut state = self.state.lock();
            let dependency = state.dependency.take();
            let request = state
                .request
                .take()
                .expect("request debt ran without a captured request");
            (dependency, request)
        };
        drop(dependency);
        request
    }

    fn restore(&self, request: DrainRequest) {
        assert!(self.state.lock().request.replace(request).is_none());
    }

    fn restore_blocked(&self, request: DrainRequest, dependency: RetirementTicket) {
        let key = request.wait_key();
        let mut state = self.state.lock();
        assert!(state.request.replace(request).is_none());
        assert!(state.dependency.replace((key, dependency)).is_none());
    }

    fn complete(&self, reservation: Reservation) {
        let mut state = self.state.lock();
        assert!(state.request.is_none() && state.dependency.is_none());
        assert!(state.reservation.replace(reservation).is_none());
    }

    fn cancel(&self, key: crate::deferred_work::WaitKey) {
        let (dependency, request) = {
            let mut state = self.state.lock();
            let dependency = state
                .dependency
                .as_ref()
                .filter(|(registered, _)| *registered == key)
                .map(|(_, dependency)| dependency.clone());
            let unstarted = state.activation.is_some()
                && state
                    .request
                    .as_ref()
                    .is_some_and(|request| request.wait_key() == key);
            let request = if unstarted {
                state.activation.take();
                assert!(state.dependency.is_none());
                state.request.take()
            } else {
                None
            };
            (dependency, request)
        };
        if let Some(dependency) = dependency {
            dependency.cancel(key);
        }
        drop(request);
    }
}

impl WaitOperation for DrainExecutor {
    fn start(&self, key: crate::deferred_work::WaitKey) {
        let activation = {
            let mut state = self.state.lock();
            let Some(request) = state.request.as_ref() else {
                // 同一轮取消可以在线程发布 Waiting 后、安装者启动请求前取得
                // 所有权。此时 request 与 activation 已共同退休，迟到 start
                // 只完成仲裁，不得重新发布债务。
                return;
            };
            if request.wait_key() != key {
                // 可复用 executor 的旧轮回调不能触及当前轮。
                return;
            }
            let target = state
                .activation
                .take()
                .expect("request operation started twice");
            let reservation = state
                .reservation
                .take()
                .expect("request activation lost capacity");
            Some((target, reservation))
        };
        if let Some((target, reservation)) = activation {
            reservation.publish(target);
        }
    }

    fn cancel(&self, key: crate::deferred_work::WaitKey) {
        DrainExecutor::cancel(self, key);
    }
}

fn wake(token: work_debt::WakeToken) {
    DEBTS.wake_raw(token);
}

pub(crate) fn drain_current(budget: usize) -> usize {
    let owner = crate::hart::current().slot();
    let mut used = 0;
    while used < budget {
        let Some(taken) = DEBTS.take(owner) else {
            break;
        };
        let (token, target) = taken.into_parts();
        let executor = target.drain_executor.clone();
        let mut request = executor.take();
        let turn = (budget - used).min(crate::work_ledger::MAX_STEPS_PER_DEBT_TURN);
        let advance = request.step(&target, turn);
        used += advance.work_done.max(1);
        match advance.state {
            StepState::Complete => {
                let identity = request.wait_identity();
                let reservation = token.rearm();
                executor.complete(reservation);
                drop(request);
                identity.complete_kernel();
            }
            StepState::Runnable => {
                executor.restore(request);
                token.requeue(target);
            }
            StepState::Blocked(dependency) => {
                let identity = request.wait_identity();
                let wake = token.arm_wake().expect("blocked request must own its wake");
                executor.restore_blocked(request, dependency.clone());
                dependency.register(
                    crate::deferred_work::WakeAction::keyed(
                        wake.into_raw(),
                        super::request::wake,
                        identity.key(),
                    ),
                    || identity.is_abandoned(),
                );
                token.park(target);
            }
        }
    }
    DEBTS.ring_if_pending(owner);
    used
}

pub(crate) fn has_current() -> bool {
    DEBTS.pending(crate::hart::current().slot()) != 0
}

pub(crate) fn reserve() -> Result<Reservation, ()> {
    DEBTS.reserve().map_err(|_| ())
}

pub(crate) struct DrainRequest {
    _control: Arc<ProcessControl>,
    caller: Arc<Process>,
    result: Option<ThreadResultObligation>,
    output: usize,
    budget: usize,
    work_done: usize,
    wait: Option<WaitIdentity>,
    permit: Option<DrainBatchPermit>,
}

struct DrainBatchPermit(Weak<Process>);

impl Drop for DrainBatchPermit {
    fn drop(&mut self) {
        let Some(process) = self.0.upgrade() else {
            return;
        };
        process.release_drain();
    }
}

impl DrainRequest {
    pub(crate) fn acquire(
        target: &Arc<Process>,
        control: Arc<ProcessControl>,
        caller: Arc<Process>,
        result: ThreadResultObligation,
        output: usize,
        budget: usize,
    ) -> Result<Self, SystemCallError> {
        if !target.try_acquire_drain() {
            return Err(SystemCallError::ObjectBusy);
        }
        Ok(Self {
            _control: control,
            caller,
            result: Some(result),
            output,
            budget,
            work_done: 0,
            wait: None,
            permit: Some(DrainBatchPermit(Arc::downgrade(target))),
        })
    }

    fn with_wait(mut self, wait: WaitIdentity) -> Self {
        assert!(self.wait.replace(wait).is_none());
        self
    }

    fn wait_key(&self) -> crate::deferred_work::WaitKey {
        self.wait
            .as_ref()
            .expect("request lost its wait identity")
            .key()
    }

    fn wait_identity(&self) -> WaitIdentity {
        self.wait
            .as_ref()
            .expect("request lost its wait identity")
            .clone()
    }

    pub(crate) fn step(
        &mut self,
        target: &Arc<Process>,
        turn: usize,
    ) -> StepResult<RetirementTicket> {
        let wait = self
            .wait
            .as_ref()
            .expect("request stepped before installation");
        let cancelled = wait.is_abandoned() || self.caller.lifecycle.is_terminating();
        let remaining = self.budget - self.work_done;
        let (used, outcome) = if cancelled {
            (0, super::proc::DrainBatchOutcome::Complete)
        } else {
            let (used, outcome) = target.advance_managed_drain(remaining.min(turn));
            assert!(
                used <= remaining.min(turn),
                "drain request exceeded its remaining budget"
            );
            (used, outcome)
        };
        self.work_done += used;
        let complete = cancelled || matches!(&outcome, super::proc::DrainBatchOutcome::Complete);
        let state = if complete || self.work_done == self.budget {
            if !cancelled {
                let result = ProcessDrainResult {
                    work_done: self.work_done as u32,
                    status: if complete {
                        ProcessDrainStatus::Complete as u32
                    } else {
                        ProcessDrainStatus::More as u32
                    },
                    reserved: 0,
                };
                let bytes = unsafe {
                    core::slice::from_raw_parts(
                        (&result as *const ProcessDrainResult).cast::<u8>(),
                        core::mem::size_of::<ProcessDrainResult>(),
                    )
                };
                let copied = {
                    let mut space = self.caller.space.lock();
                    crate::uaccess::put_user_indirect(&mut space, self.output, bytes)
                };
                if copied.is_err() {
                    let todo = self.caller.lifecycle.request_termination(
                        ProcessExitReason::Fault,
                        ProcessFaultCode::StoreAccess as i64,
                        None,
                    );
                    super::process::run_termination_todo(&self.caller, todo);
                }
            }
            StepState::Complete
        } else if let super::proc::DrainBatchOutcome::Blocked(dependency) = outcome {
            StepState::Blocked(dependency)
        } else {
            debug_assert!(used > 0, "drain batch made no progress without blocking");
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
        drop(self.permit.take());
        // caller 强根必须活过结果义务归还及 departure confirmation。
        drop(self.result.take());
    }
}
