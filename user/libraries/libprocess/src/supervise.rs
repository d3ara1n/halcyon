//! 监督状态机：REAPABLE/CLOSED 已观察后的收束（Drain → Verify → Close）
//! 由单一 [`Collector`] 机器拥有；同步门面 [`collect_process`] 与执行核心
//! 任务 [`SuperviseTask`] 共用同一状态机，不另写第二套循环。
//!
//! 观察到 CLOSED 不等于收束完成：终态发布后成员摘除与祖先传播仍可能
//! 跨批继续，Drain Complete 才是关闭 control 的前置。对象繁忙按绝对
//! 期限重试（REAPABLE 是持久电平，不能靠重新观察重试）。

use crate::observation::{Observation, ObservationResult, ObservationSlot, ObservationState};
use crate::{
    CollectedProcess, SupervisionCause, SupervisionPolicy, SupervisionProgress, SupervisionStage,
    SupervisionTarget,
};
use erhino_shared::{
    call::SystemCallError, object::ObjectSignals, proc::ProcessState, time::Deadline,
};
use libexecution::runtime::{
    Advance, Input, RequestFailure, Requests, SourceId, SourceKind, Step, Task,
};
use rinlib::{ipc::object::close, process};

pub trait ProcessOperations: Clone + core::fmt::Debug {
    /// 期限到达时的最终非阻塞检查，遵循内核 ready 优先于 timeout 的裁决。
    fn probe(
        &self,
        control: erhino_shared::object::Handle,
        signals: ObjectSignals,
    ) -> Result<ObservationResult, SystemCallError>;
    fn drain(
        &self,
        control: erhino_shared::object::Handle,
        max_work: u32,
    ) -> Result<erhino_shared::proc::ProcessDrainResult, SystemCallError>;
    fn query(
        &self,
        control: erhino_shared::object::Handle,
    ) -> Result<erhino_shared::proc::ProcessSnapshot, SystemCallError>;
    fn close(&self, control: erhino_shared::object::Handle) -> Result<(), SystemCallError>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct RealProcessOperations;

impl ProcessOperations for RealProcessOperations {
    fn probe(
        &self,
        control: erhino_shared::object::Handle,
        signals: ObjectSignals,
    ) -> Result<ObservationResult, SystemCallError> {
        use erhino_shared::wait::{WaitItem, WaitReason};
        let result =
            rinlib::ipc::wait::wait_until(&[WaitItem::new(control, signals, 0)], Deadline::at(0))?;
        Ok(
            if WaitReason::from_u32(result.reason) == Some(WaitReason::Timeout) {
                ObservationResult::TimedOut
            } else {
                ObservationResult::Ready(result.observed)
            },
        )
    }
    fn drain(
        &self,
        control: erhino_shared::object::Handle,
        max_work: u32,
    ) -> Result<erhino_shared::proc::ProcessDrainResult, SystemCallError> {
        process::drain(control, max_work)
    }
    fn query(
        &self,
        control: erhino_shared::object::Handle,
    ) -> Result<erhino_shared::proc::ProcessSnapshot, SystemCallError> {
        process::query(control)
    }
    fn close(&self, control: erhino_shared::object::Handle) -> Result<(), SystemCallError> {
        // SAFETY: Collector owns this ProcessControl and closes only after verification.
        unsafe { close(control) }
    }
}

/// 监督观察来源的任务侧标签。
pub const SUPERVISE_SOURCE: SourceKind = 1;

/// 单步收束结果；非终态携带机器继续。
pub enum StepOutcome<O: ProcessOperations = RealProcessOperations> {
    Continue(Collector<O>),
    Observe(Observation, Collector<O>),
    RetryAt(u64, Collector<O>),
    Done(CollectedProcess),
    Escalate(CollectorFailure<O>),
}

#[derive(Debug)]
pub struct CollectorFailure<O: ProcessOperations = RealProcessOperations> {
    pub collector: Collector<O>,
    pub stage: SupervisionStage,
    pub cause: SupervisionCause,
    pub progress: SupervisionProgress,
}

#[derive(Debug)]
enum Phase {
    Waiting,
    Draining,
    Verifying,
    Closing,
}

/// 每步至多一次内核调用的收束机器；持有 supervision authority，
/// 终态时把 [`SupervisionTarget`] 完整带进成功或失败结果。
#[derive(Debug)]
pub struct Collector<O: ProcessOperations = RealProcessOperations> {
    target: SupervisionTarget,
    policy: SupervisionPolicy,
    progress: SupervisionProgress,
    phase: Phase,
    snapshot: Option<erhino_shared::proc::ProcessSnapshot>,
    observation: ObservationState,
    retry_at: Option<u64>,
    drain_remaining: u32,
    query_remaining: u32,
    ops: O,
}

impl Collector<RealProcessOperations> {
    pub fn new(target: SupervisionTarget, policy: SupervisionPolicy) -> Self {
        Self::new_with_ops(target, policy, RealProcessOperations)
    }
}

impl<O: ProcessOperations> Collector<O> {
    pub fn new_with_ops(target: SupervisionTarget, policy: SupervisionPolicy, ops: O) -> Self {
        Self {
            target,
            policy,
            progress: SupervisionProgress::default(),
            phase: Phase::Waiting,
            snapshot: None,
            observation: ObservationState::default(),
            retry_at: None,
            drain_remaining: policy.drain_attempts,
            query_remaining: policy.query_attempts,
            ops,
        }
    }

    fn control(&self) -> erhino_shared::object::Handle {
        self.target.control()
    }

    pub fn wait_control(&self) -> erhino_shared::object::Handle {
        self.control()
    }

    pub fn pid(&self) -> u64 {
        self.target.pid()
    }

    pub fn stage(&self) -> SupervisionStage {
        match self.phase {
            Phase::Waiting => SupervisionStage::WaitReapable,
            Phase::Draining => SupervisionStage::Drain,
            Phase::Verifying => SupervisionStage::VerifyDead,
            Phase::Closing => SupervisionStage::Close,
        }
    }

    pub fn progress(&self) -> SupervisionProgress {
        self.progress
    }
    pub fn probe(&self, request: Observation) -> Result<ObservationResult, SystemCallError> {
        self.ops.probe(request.control, request.signals)
    }
    pub fn fail_current(self, cause: SupervisionCause) -> CollectorFailure<O> {
        let stage = self.stage();
        self.fail(stage, cause)
    }

    fn retry_ns(&self, now_ns: u64) -> u64 {
        now_ns.saturating_add(self.policy.wait_timeout_ms.saturating_mul(1_000_000))
    }

    fn escalate(self, stage: SupervisionStage, cause: SupervisionCause) -> StepOutcome<O> {
        let progress = self.progress;
        StepOutcome::Escalate(CollectorFailure {
            collector: self,
            stage,
            cause,
            progress,
        })
    }

    /// 停止与故障均保留原机器阶段，不重建 Drain 或丢弃 Close 快照。
    pub fn abort(self) -> CollectorFailure<O> {
        let stage = self.stage();
        self.fail(stage, SupervisionCause::Stopped)
    }

    pub fn fail(self, stage: SupervisionStage, cause: SupervisionCause) -> CollectorFailure<O> {
        let progress = self.progress;
        CollectorFailure {
            collector: self,
            stage,
            cause,
            progress,
        }
    }

    /// 管理者明确给原机器续作额度；历史进度与正在关闭的快照不回退。
    pub fn replenish(&mut self, policy: SupervisionPolicy) {
        self.policy = policy;
        self.drain_remaining = policy.drain_attempts;
        self.query_remaining = policy.query_attempts;
        self.observation.renew();
    }

    pub fn observe(
        &mut self,
        request: Observation,
        result: ObservationResult,
        now: u64,
    ) -> Result<(), SupervisionCause> {
        if !matches!(self.phase, Phase::Waiting) {
            return Err(SupervisionCause::System(SystemCallError::IllegalArgument));
        }
        if self.observation.accept(request, result, now)? {
            self.phase = Phase::Draining;
        }
        Ok(())
    }

    /// 每步至多一次系统操作；等待与实际重试期限由机器保留。
    pub fn step(mut self, now_ns: u64) -> StepOutcome<O> {
        if !crate::valid_policy(self.policy) {
            let stage = self.stage();
            return self.escalate(stage, SupervisionCause::InvalidPolicy);
        }
        if let Some(at) = self.retry_at
            && now_ns < at
        {
            return StepOutcome::RetryAt(at, self);
        }
        self.retry_at = None;
        match self.phase {
            Phase::Waiting => {
                let new_attempt = self.observation.pending().is_none();
                match self.observation.request(
                    self.control(),
                    ObjectSignals::REAPABLE | ObjectSignals::CLOSED,
                    now_ns,
                    self.policy,
                ) {
                    Ok(request) => {
                        if new_attempt {
                            self.progress.wait_attempts =
                                self.progress.wait_attempts.saturating_add(1);
                        }
                        StepOutcome::Observe(request, self)
                    }
                    Err(cause) => self.escalate(SupervisionStage::WaitReapable, cause),
                }
            }
            Phase::Draining => self.step_drain(now_ns),
            Phase::Verifying => self.step_verify(now_ns),
            Phase::Closing => self.step_close(),
        }
    }

    fn step_drain(mut self, now_ns: u64) -> StepOutcome<O> {
        if self.drain_remaining == 0 {
            return self.escalate(SupervisionStage::Drain, SupervisionCause::Timeout);
        }
        self.drain_remaining -= 1;
        self.progress.drain_attempts = self.progress.drain_attempts.saturating_add(1);
        match self.ops.drain(self.control(), self.policy.drain_work) {
            Ok(result) => {
                self.progress.work_done = self.progress.work_done.saturating_add(result.work_done);
                if result.status == erhino_shared::proc::ProcessDrainStatus::Complete as u32 {
                    self.phase = Phase::Verifying;
                }
                StepOutcome::Continue(self)
            }
            Err(SystemCallError::ObjectBusy) => {
                let at = self.retry_ns(now_ns);
                self.retry_at = Some(at);
                StepOutcome::RetryAt(at, self)
            }
            Err(error) => self.escalate(SupervisionStage::Drain, SupervisionCause::System(error)),
        }
    }

    fn step_verify(mut self, now_ns: u64) -> StepOutcome<O> {
        if self.query_remaining == 0 {
            return self.escalate(SupervisionStage::VerifyDead, SupervisionCause::Timeout);
        }
        self.query_remaining -= 1;
        self.progress.query_attempts = self.progress.query_attempts.saturating_add(1);
        match self.ops.query(self.control()) {
            Ok(snapshot) => {
                if snapshot.pid != self.target.pid() || snapshot.state != ProcessState::Dead as u32
                {
                    return self.escalate(
                        SupervisionStage::VerifyDead,
                        SupervisionCause::InconsistentSnapshot {
                            pid: snapshot.pid,
                            state: snapshot.state,
                        },
                    );
                }
                self.phase = Phase::Closing;
                self.snapshot = Some(snapshot);
                StepOutcome::Continue(self)
            }
            Err(SystemCallError::ObjectBusy) => {
                let at = self.retry_ns(now_ns);
                self.retry_at = Some(at);
                StepOutcome::RetryAt(at, self)
            }
            Err(error) => self.escalate(
                SupervisionStage::VerifyDead,
                SupervisionCause::System(error),
            ),
        }
    }

    fn step_close(self) -> StepOutcome<O> {
        let snapshot = self.snapshot.expect("closing collector lost its snapshot");
        let control = self.control();
        // SAFETY: 完整 Drain/Query 已验证 ProcessControl role 与稳定终态；
        // 关闭的只有 control 本身，不涉及 Endpoint。
        if let Err(error) = self.ops.close(control) {
            return self.escalate(SupervisionStage::Close, SupervisionCause::System(error));
        }
        // control 已关闭，authority 随成功结果兑现后释放。
        let _ = self.target.into_control();
        StepOutcome::Done(CollectedProcess {
            pid: snapshot.pid,
            snapshot,
            progress: self.progress,
        })
    }
}

/// 监督结果交付；失败携带完整 authority，由接收方决定重试或升级。
pub struct SuperviseResult<O: ProcessOperations = RealProcessOperations> {
    pub pid: u64,
    pub outcome: Result<CollectedProcess, CollectorFailure<O>>,
}

/// 监督任务的收束交付面。
pub trait SuperviseSink<O: ProcessOperations = RealProcessOperations> {
    fn supervised(&mut self, result: SuperviseResult<O>);
}

/// 执行核心上的常驻监督任务：观察 REAPABLE/CLOSED，就绪后驱动
/// [`Collector`]，每 advance 一次有界内核调用，繁忙经任务期限重试。
pub struct SuperviseTask<O: ProcessOperations = RealProcessOperations> {
    machine: Option<Collector<O>>,
    observation: ObservationSlot,
    retry_at: Option<u64>,
    stopping: bool,
}

impl SuperviseTask<RealProcessOperations> {
    pub fn new(target: SupervisionTarget, policy: SupervisionPolicy) -> Self {
        Self::from_collector(Collector::new(target, policy))
    }
}

impl<O: ProcessOperations> SuperviseTask<O> {
    pub fn from_collector(machine: Collector<O>) -> Self {
        Self {
            machine: Some(machine),
            observation: ObservationSlot::default(),
            retry_at: None,
            stopping: false,
        }
    }

    pub fn into_collector(self) -> Option<Collector<O>> {
        self.machine
    }

    fn deliver<W: SuperviseSink<O>>(
        &mut self,
        world: &mut W,
        outcome: Result<CollectedProcess, CollectorFailure<O>>,
    ) -> Advance {
        let pid = match &outcome {
            Ok(done) => done.pid,
            Err(failed) => failed.collector.pid(),
        };
        world.supervised(SuperviseResult { pid, outcome });
        Advance {
            work_done: 1,
            step: Step::Complete,
        }
    }
}

impl<O: ProcessOperations, W: SuperviseSink<O>> Task<W> for SuperviseTask<O> {
    type Family = Self;
    fn advance(
        &mut self,
        _id: u64,
        world: &mut W,
        requests: &mut Requests<Self>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Result<Advance, SystemCallError> {
        let now = input.now_ns();
        // 观察与重试均以机器持有的绝对期限判定；到期输入每次只消费一次。
        let _ = input.take_timeout();
        if self.stopping {
            self.observation.cancel();
        }
        if let Some((request, result)) = self.observation.poll(input, requests, now, |request| {
            self.machine
                .as_ref()
                .expect("observer owns its machine")
                .probe(request)
        })? && !self.stopping
        {
            let machine = self
                .machine
                .as_mut()
                .expect("observing task owns its collector");
            if let Err(cause) = machine.observe(request, result, now) {
                let failure = self
                    .machine
                    .take()
                    .expect("failed collector remains owned")
                    .fail(SupervisionStage::WaitReapable, cause);
                return Ok(self.deliver(world, Err(failure)));
            }
        }
        if self.observation.is_active() {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        if self.stopping {
            let failure = self
                .machine
                .take()
                .expect("stopped task owns its collector")
                .abort();
            return Ok(self.deliver(world, Err(failure)));
        }
        let Some(machine) = self.machine.take() else {
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        };
        match machine.step(now) {
            StepOutcome::Observe(request, machine) => {
                self.machine = Some(machine);
                self.observation.begin(request, requests)?;
                self.retry_at = None;
                Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                })
            }
            StepOutcome::Continue(machine) => {
                self.machine = Some(machine);
                self.retry_at = None;
                Ok(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                })
            }
            StepOutcome::RetryAt(at, machine) => {
                self.machine = Some(machine);
                self.retry_at = Some(at);
                Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                })
            }
            StepOutcome::Done(done) => Ok(self.deliver(world, Ok(done))),
            StepOutcome::Escalate(failure) => Ok(self.deliver(world, Err(failure))),
        }
    }
    fn registered(&mut self, _world: &mut W, kind: SourceKind, source: SourceId) {
        self.observation.registered(kind, source);
    }
    fn unregistered(&mut self, _world: &mut W, kind: SourceKind, source: SourceId) {
        self.observation.unregistered(kind, source);
    }
    fn refused(&mut self, _world: &mut W, failure: RequestFailure<Self>) {
        match failure {
            RequestFailure::Source { kind, error } => self.observation.refused(kind, error),
            RequestFailure::Spawn { .. } => unreachable!("supervision does not spawn tasks"),
            RequestFailure::Wake { .. } => unreachable!("supervision does not wake tasks"),
        }
    }
    fn stop(&mut self, _world: &mut W) {
        self.stopping = true;
    }
    fn deadline(&self) -> Deadline {
        if self.observation.is_active() {
            self.observation.deadline()
        } else {
            self.retry_at.map_or(Deadline::INFINITE, Deadline::at)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use core::cell::RefCell;
    use erhino_shared::proc::{ProcessDrainResult, ProcessDrainStatus};

    #[derive(Debug, Default)]
    struct FakeState {
        drain_calls: usize,
        close_failures: usize,
        close_successes: usize,
    }

    #[derive(Debug, Clone)]
    struct FakeOps(Rc<RefCell<FakeState>>);

    impl ProcessOperations for FakeOps {
        fn probe(
            &self,
            _: erhino_shared::object::Handle,
            signals: ObjectSignals,
        ) -> Result<ObservationResult, SystemCallError> {
            Ok(ObservationResult::Ready(signals))
        }
        fn drain(
            &self,
            _control: erhino_shared::object::Handle,
            _max_work: u32,
        ) -> Result<ProcessDrainResult, SystemCallError> {
            let mut state = self.0.borrow_mut();
            state.drain_calls += 1;
            if state.drain_calls == 1 {
                return Err(SystemCallError::ObjectBusy);
            }
            Ok(ProcessDrainResult {
                work_done: 1,
                status: ProcessDrainStatus::Complete as u32,
                reserved: 0,
            })
        }

        fn query(
            &self,
            _control: erhino_shared::object::Handle,
        ) -> Result<erhino_shared::proc::ProcessSnapshot, SystemCallError> {
            Ok(erhino_shared::proc::ProcessSnapshot {
                pid: 7,
                parent_pid: 0,
                state: ProcessState::Dead as u32,
                reason: 0,
                code: 0,
                reserved: 0,
            })
        }

        fn close(&self, _control: erhino_shared::object::Handle) -> Result<(), SystemCallError> {
            let mut state = self.0.borrow_mut();
            if state.close_failures != 0 {
                state.close_failures -= 1;
                return Err(SystemCallError::ObjectBusy);
            }
            state.close_successes += 1;
            Ok(())
        }
    }

    #[test]
    fn collector_busy_and_close_failure_preserve_machine() {
        let state = Rc::new(RefCell::new(FakeState {
            drain_calls: 0,
            close_failures: 1,
            close_successes: 0,
        }));
        let target = SupervisionTarget::new(7, erhino_shared::object::Handle::from_raw(9));
        let collector = Collector::new_with_ops(
            target,
            crate::DEFAULT_SUPERVISION_POLICY,
            FakeOps(state.clone()),
        );
        let collector = match collector.step(0) {
            StepOutcome::Observe(request, mut collector) => {
                collector
                    .observe(
                        request,
                        ObservationResult::Ready(ObjectSignals::REAPABLE),
                        1,
                    )
                    .unwrap();
                collector
            }
            _ => panic!("unobserved process must not drain"),
        };
        let collector = match collector.step(0) {
            StepOutcome::RetryAt(_, collector) => collector,
            _ => panic!("first drain must report Busy"),
        };
        let collector = match collector.step(100_000_000) {
            StepOutcome::Continue(collector) => collector,
            _ => panic!("second drain must complete"),
        };
        let failure = collector.fail_current(SupervisionCause::System(SystemCallError::ClockRange));
        assert_eq!(failure.stage, SupervisionStage::VerifyDead);
        assert_eq!(failure.progress.wait_attempts, 1);
        assert_eq!(failure.progress.work_done, 1);
        let collector = match failure.collector.step(100_000_000) {
            StepOutcome::Continue(collector) => collector,
            _ => panic!("query must advance to close"),
        };
        let collector = match collector.step(100_000_000) {
            StepOutcome::Escalate(failure) => {
                assert_eq!(failure.stage, SupervisionStage::Close);
                failure.collector
            }
            _ => panic!("first close must fail"),
        };
        assert_eq!(state.borrow().close_successes, 0);
        match collector.step(100_000_000) {
            StepOutcome::Done(collected) => assert_eq!(collected.pid, 7),
            _ => panic!("retained closing collector must resume"),
        }
        assert_eq!(state.borrow().close_successes, 1);
    }
}
