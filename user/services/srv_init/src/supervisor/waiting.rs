//! root装配操作的有限等待槽；来源与其余职责共用唯一Assembly。

use super::*;
use erhino_shared::{object::ObjectSignals, wait::WaitResult};
use libexecution::{SourcePlan, SourceRegistration};

// 单次RPC最多等待reply、service、cancel、reply-observer四个实际对象。
const SOURCES: usize = librpc::caller::CALL_WAIT_SOURCES;
struct Pending {
    plans: [Option<(Handle, ObjectSignals)>; SOURCES],
    deadline: Deadline,
}
struct World {
    pending: Option<Pending>,
    result: Option<Result<[ObjectSignals; SOURCES], SystemCallError>>,
    closing: bool,
}
struct WaitTask {
    sources: [SourceRegistration; SOURCES],
    observed: [ObjectSignals; SOURCES],
    error: Option<SystemCallError>,
    finished: bool,
    deadline: Deadline,
}
impl Task<World> for WaitTask {
    type Family = Self;
    fn advance(
        &mut self,
        _id: u64,
        world: &mut World,
        requests: &mut Requests<'_, Self>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            if let Some(index) = event
                .kind
                .checked_sub(1)
                .and_then(|index| usize::try_from(index).ok())
                .filter(|index| *index < SOURCES)
            {
                self.observed[index] |= event.observed;
            }
        }
        input.take_timeout();
        if let Some(pending) = &world.pending {
            self.deadline = pending.deadline;
            self.finished |=
                self.observed.iter().any(|signals| signals.raw() != 0) || self.error.is_some();
            if !self.finished
                && pending
                    .deadline
                    .instant()
                    .map_err(|_| SystemCallError::IllegalArgument)?
                    .is_some_and(|at| input.now_ns() >= at)
            {
                // 到期前最后一次核真实电平，和WaitOne相同：ready优先于timeout。
                for (index, plan) in pending.plans.iter().enumerate() {
                    if let Some(plan) = plan {
                        match libkernel::ipc::wait::probe(plan.0, plan.1) {
                            Ok(signals) => self.observed[index] = signals,
                            Err(error) => {
                                self.error.get_or_insert(error);
                            }
                        }
                    }
                }
                self.finished = true;
            }
            if !self.finished && !world.closing {
                for (index, plan) in pending.plans.iter().enumerate() {
                    if let Some(plan) = plan {
                        self.sources[index].install(
                            requests,
                            SourcePlan::new(plan.0, plan.1),
                            index as u64 + 1,
                        )?;
                    }
                }
                return Ok(Advance {
                    work_done: 1,
                    step: if self.sources.iter().any(SourceRegistration::is_installing) {
                        Step::Runnable
                    } else {
                        Step::Parked
                    },
                });
            }
        }
        if self.finished || world.closing {
            self.deadline = Deadline::INFINITE;
            for source in &mut self.sources {
                source.remove(requests)?;
            }
            if self.sources.iter().all(SourceRegistration::is_empty) {
                if world.pending.take().is_some() {
                    world.result = Some(self.error.take().map_or(Ok(self.observed), Err));
                }
                self.observed.fill(ObjectSignals::NONE);
                self.finished = false;
                return Ok(Advance {
                    work_done: 1,
                    step: if world.closing {
                        Step::Complete
                    } else {
                        Step::Parked
                    },
                });
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Parked,
        })
    }
    fn registered(&mut self, _world: &mut World, kind: SourceKind, source: SourceId) {
        self.sources[kind as usize - 1].registered(kind, source);
    }
    fn unregistered(&mut self, _world: &mut World, kind: SourceKind, source: SourceId) {
        self.sources[kind as usize - 1].unregistered(source);
    }
    fn refused(&mut self, _world: &mut World, failure: RequestFailure<Self>) {
        if let RequestFailure::Source { kind, error } = failure {
            self.sources[kind as usize - 1].refused(kind);
            self.error = Some(error);
        }
    }
    fn stop(&mut self, world: &mut World) {
        world.closing = true;
    }
    fn deadline(&self) -> Deadline {
        self.deadline
    }
}
pub(super) struct WaitDuty {
    runtime: Runtime<WaitTask, WaitSet>,
    world: World,
    task: u64,
}
impl WaitDuty {
    pub(crate) fn new(assembly: &Assembly<WaitSet>) -> Result<Self, SystemCallError> {
        let funding = prepare_funding::<WaitTask>(&mut None, 1, SOURCES)?;
        let mut runtime = runtime(assembly, 1, SOURCES, &funding)?;
        let task = runtime
            .spawn(TaskSubmission::new(
                WaitTask {
                    sources: core::array::from_fn(|_| SourceRegistration::new()),
                    observed: [ObjectSignals::NONE; SOURCES],
                    error: None,
                    finished: false,
                    deadline: Deadline::INFINITE,
                },
                SOURCES,
                funding,
            ))
            .map_err(|failure| failure.error)?;
        Ok(Self {
            runtime,
            world: World {
                pending: None,
                result: None,
                closing: false,
            },
            task,
        })
    }
    pub(crate) fn feed(&mut self, record: erhino_shared::wait_set::ReadyRecord) {
        self.runtime.feed(record);
    }
    pub(crate) fn step(&mut self, now: u64) -> Result<(), SystemCallError> {
        if can_drive(&self.runtime, now) {
            self.runtime
                .turn(&mut self.world, 1)
                .map_err(|failure| failure.error)?;
        }
        Ok(())
    }
    pub(crate) fn state(&self) -> (bool, Option<u64>) {
        fold_wait(Some(&self.runtime), &Disposition::default(), false)
    }
    pub(crate) fn complete(&self) -> bool {
        self.world.result.is_some()
    }
}
impl RootSupervisor {
    pub(crate) fn wait_sources(
        &mut self,
        plans: &[(Handle, ObjectSignals)],
        deadline: Deadline,
    ) -> Result<[ObjectSignals; SOURCES], SystemCallError> {
        if plans.len() > SOURCES {
            return Err(SystemCallError::ReachLimit);
        }
        let duty = &mut self.waiting;
        if duty.world.pending.is_some() || duty.world.result.is_some() {
            return Err(SystemCallError::ObjectBusy);
        }
        let mut input = [None; SOURCES];
        for (slot, plan) in input.iter_mut().zip(plans) {
            *slot = Some(*plan);
        }
        duty.world.pending = Some(Pending {
            plans: input,
            deadline,
        });
        duty.runtime.wake(duty.task)?;
        loop {
            if self.turn_and_wait(Some(Await::AssemblyWait)).is_err() {
                // 尚存的来源仍借用调用栈owner，不能带着未注销token返回并释放它。
                self.device_failure("root assembly wait cleanup unavailable");
            }
            if let Some(result) = self.waiting.world.result.take() {
                return result;
            }
        }
    }
    pub(crate) fn wait_until(
        &mut self,
        handle: Handle,
        interest: ObjectSignals,
        deadline: Deadline,
    ) -> Result<WaitResult, SystemCallError> {
        let observed = self.wait_sources(&[(handle, interest)], deadline)?;
        Ok(WaitResult::from_observed(observed[0]))
    }
    pub(crate) fn sleep_until(&mut self, deadline: Deadline) -> Result<(), SystemCallError> {
        self.wait_sources(&[], deadline).map(|_| ())
    }
    pub(crate) fn receive_until(
        &mut self,
        handle: Handle,
        deadline: Deadline,
    ) -> Result<libkernel::ipc::message::ReceivedMessage, SystemCallError> {
        loop {
            match libkernel::ipc::message::receive(handle) {
                Ok(message) => return Ok(message),
                Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => {}
                Err(error) => return Err(error),
            }
            let result = self.wait_until(handle, ObjectSignals::READABLE, deadline)?;
            if !result.observed.contains(ObjectSignals::READABLE) {
                return Err(if result.observed.contains(ObjectSignals::CLOSED) {
                    SystemCallError::ObjectClosed
                } else {
                    SystemCallError::DeadlineExpired
                });
            }
        }
    }
}
