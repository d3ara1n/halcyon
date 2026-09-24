use super::*;
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals},
};
use libbudget::Budget;
use libexecution::{
    ExecutionResource,
    runtime::{
        Advance, DriveState, Input, RequestFailure, Requests, Runtime, SourceId, SourceKind,
        SourcePlan, Step, Task,
    },
};
use rinlib::ipc::wait_set::WaitSet;

const SOURCE_DATA: SourceKind = 1;
const TARGET_DATA: SourceKind = 2;
const SOURCE_CLOSED: SourceKind = 3;
const TARGET_CLOSED: SourceKind = 4;
const CANCEL: SourceKind = 5;
const BUFFER_BYTES: usize = 1024;
const RETIRE_TURNS: usize = 4 * CANCEL as usize + 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpFault {
    Source(librunnel::IoError),
    Target(librunnel::IoError),
    System(SystemCallError),
    Cancelled,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PumpSummary {
    pub source_bytes: u64,
    pub target_bytes: u64,
    pub fault: Option<PumpFault>,
}

pub struct PumpRunFailure {
    pub summary: PumpSummary,
    runtime: Option<Runtime<PumpTask, WaitSet>>,
    set: Option<WaitSet>,
    snapshot: Option<PumpSnapshot>,
}

impl PumpRunFailure {
    // Runtime 关闭失败时需原样返还持有 WaitSet 的 owner。
    #[allow(clippy::result_large_err)]
    pub fn retry_cleanup(
        mut self,
        source: &mut Stream,
        target: &mut Stream,
    ) -> Result<PumpSummary, Self> {
        if let Some(set) = self.set.take()
            && let Err((set, _)) = set.close()
        {
            self.set = Some(set);
            return Err(self);
        }
        let Some(mut runtime) = self.runtime.take() else {
            return Ok(self.summary);
        };
        if runtime.drive_state() != DriveState::Drained {
            let Some(snapshot) = self.snapshot.take() else {
                self.runtime = Some(runtime);
                return Err(self);
            };
            let mut world = snapshot.into_world(source, target);
            world.outcome.get_or_insert(PumpFault::Cancelled);
            for _ in 0..RETIRE_TURNS {
                if runtime.drive_state() == DriveState::Drained {
                    break;
                }
                if runtime.shutdown_turn(&mut world, 1).is_err() {
                    break;
                }
            }
            self.summary = world.summary();
            if runtime.drive_state() != DriveState::Drained {
                self.snapshot = Some(world.snapshot());
                self.runtime = Some(runtime);
                return Err(self);
            }
        }
        match runtime.close() {
            Ok(()) => Ok(self.summary),
            Err((runtime, error)) => {
                self.summary.fault = Some(PumpFault::System(error));
                self.runtime = Some(runtime);
                self.snapshot = None;
                Err(self)
            }
        }
    }
}

struct PumpSnapshot {
    cancel: Option<Handle>,
    buffer: [u8; BUFFER_BYTES],
    have: usize,
    sent: usize,
    source_bytes: u64,
    target_bytes: u64,
    outcome: Option<PumpFault>,
    drained: bool,
}

impl PumpSnapshot {
    fn into_world<'a>(self, source: &'a mut Stream, target: &'a mut Stream) -> PumpWorld<'a> {
        PumpWorld {
            source,
            target,
            cancel: self.cancel,
            buffer: self.buffer,
            have: self.have,
            sent: self.sent,
            source_bytes: self.source_bytes,
            target_bytes: self.target_bytes,
            outcome: self.outcome,
            drained: self.drained,
        }
    }
}

struct PumpWorld<'a> {
    source: &'a mut Stream,
    target: &'a mut Stream,
    cancel: Option<Handle>,
    buffer: [u8; BUFFER_BYTES],
    have: usize,
    sent: usize,
    source_bytes: u64,
    target_bytes: u64,
    outcome: Option<PumpFault>,
    drained: bool,
}

impl PumpWorld<'_> {
    fn summary(&self) -> PumpSummary {
        PumpSummary {
            source_bytes: self.source_bytes,
            target_bytes: self.target_bytes,
            fault: self.outcome,
        }
    }

    fn snapshot(&self) -> PumpSnapshot {
        PumpSnapshot {
            cancel: self.cancel,
            buffer: self.buffer,
            have: self.have,
            sent: self.sent,
            source_bytes: self.source_bytes,
            target_bytes: self.target_bytes,
            outcome: self.outcome,
            drained: self.drained,
        }
    }
}

struct PumpTask {
    sources: [Option<SourceId>; 5],
    requested: [bool; 5],
    removing: [bool; 5],
    armed: [bool; 5],
    stopping: bool,
    deadline: rinlib::time::Deadline,
}

impl PumpTask {
    fn new(deadline: rinlib::time::Deadline) -> Self {
        Self {
            sources: [None; 5],
            requested: [false; 5],
            removing: [false; 5],
            armed: [false; 5],
            stopping: false,
            deadline,
        }
    }

    fn index(kind: SourceKind) -> Option<usize> {
        usize::try_from(kind)
            .ok()?
            .checked_sub(1)
            .filter(|index| *index < 5)
    }

    fn plan(kind: SourceKind, world: &mut PumpWorld<'_>) -> Result<SourcePlan, PumpFault> {
        match kind {
            SOURCE_DATA => world
                .source
                .reader_mut()
                .ok_or(PumpFault::System(SystemCallError::InternalError))?
                .wait_plan()
                .map_err(PumpFault::Source),
            TARGET_DATA => world
                .target
                .writer_mut()
                .ok_or(PumpFault::System(SystemCallError::InternalError))?
                .wait_plan()
                .map_err(PumpFault::Target),
            SOURCE_CLOSED => Ok(world
                .source
                .reader_mut()
                .ok_or(PumpFault::System(SystemCallError::InternalError))?
                .terminal_wait_plan()),
            TARGET_CLOSED => Ok(world
                .target
                .writer_mut()
                .ok_or(PumpFault::System(SystemCallError::InternalError))?
                .terminal_wait_plan()),
            CANCEL => Ok(SourcePlan::new(
                world
                    .cancel
                    .ok_or(PumpFault::System(SystemCallError::InternalError))?,
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
            )),
            _ => Err(PumpFault::System(SystemCallError::InternalError)),
        }
    }

    fn retire(&mut self, requests: &mut Requests<Self>, expected: usize) -> Advance {
        let mut remaining = false;
        for index in 0..expected {
            if let Some(source) = self.sources[index] {
                remaining = true;
                if !self.removing[index] && requests.remove(source).is_ok() {
                    self.removing[index] = true;
                }
            } else if self.requested[index] {
                remaining = true;
            }
        }
        Advance {
            work_done: 1,
            step: if remaining {
                Step::Runnable
            } else {
                Step::Complete
            },
        }
    }
}

impl Task<PumpWorld<'_>> for PumpTask {
    type Family = Self;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut PumpWorld<'_>,
        requests: &mut Requests<Self>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if budget == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        while let Some(event) = input.pull() {
            if world.outcome.is_some() {
                continue;
            }
            if let Some(index) = Self::index(event.kind) {
                self.armed[index] = false;
            }
            match event.kind {
                CANCEL => world.outcome = Some(PumpFault::Cancelled),
                SOURCE_CLOSED | TARGET_CLOSED | SOURCE_DATA | TARGET_DATA => {
                    let source = event.kind == SOURCE_CLOSED || event.kind == SOURCE_DATA;
                    let terminal = event.error != 0
                        || event
                            .observed
                            .intersects(ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED);
                    if source {
                        let reader = world
                            .source
                            .reader_mut()
                            .ok_or(SystemCallError::InternalError)?;
                        match reader.eof_reached() {
                            Ok(true) if terminal => {
                                world.drained = true;
                                if world.have == world.sent {
                                    continue;
                                }
                            }
                            Err(error) => world.outcome = Some(PumpFault::Source(error)),
                            _ if terminal => {
                                world.outcome = Some(PumpFault::Source(librunnel::IoError {
                                    error: librunnel::RunnelError::Closed,
                                    completed: 0,
                                }))
                            }
                            _ => {
                                if let Err(error) = reader.poll(event.observed) {
                                    world.outcome = Some(PumpFault::Source(error));
                                }
                            }
                        }
                    } else if terminal {
                        world.outcome = Some(PumpFault::Target(librunnel::IoError {
                            error: librunnel::RunnelError::Closed,
                            completed: 0,
                        }));
                    } else if let Some(writer) = world.target.writer_mut()
                        && let Err(error) = writer.poll(event.observed)
                    {
                        world.outcome = Some(PumpFault::Target(error));
                    }
                }
                _ => world.outcome = Some(PumpFault::System(SystemCallError::InternalError)),
            }
        }
        if input.take_timeout() {
            world.outcome.get_or_insert(PumpFault::Timeout);
        }
        if self.stopping && world.outcome.is_none() {
            world.outcome = Some(PumpFault::Cancelled);
        }
        let expected = if world.cancel.is_some() { 5 } else { 4 };
        if world.outcome.is_some() || (world.drained && world.have == world.sent) {
            return Ok(self.retire(requests, expected));
        }
        for index in 0..expected {
            if self.sources[index].is_none() && !self.requested[index] {
                let kind = index as SourceKind + 1;
                let plan = match Self::plan(kind, world) {
                    Ok(plan) => plan,
                    Err(fault) => {
                        world.outcome = Some(fault);
                        return Ok(self.retire(requests, expected));
                    }
                };
                if requests.arm_source(plan, kind).is_ok() {
                    self.requested[index] = true;
                }
            }
        }
        if (0..expected).any(|index| self.sources[index].is_none()) {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if world.have != world.sent {
            let writer = world
                .target
                .writer_mut()
                .ok_or(SystemCallError::InternalError)?;
            let result = writer.write(&world.buffer[world.sent..world.have]);
            let n = match result {
                Ok(n) => n,
                Err(error) => {
                    world.sent += error.completed;
                    world.target_bytes += error.completed as u64;
                    world.outcome = Some(PumpFault::Target(error));
                    return Ok(self.retire(requests, expected));
                }
            };
            world.sent += n;
            world.target_bytes += n as u64;
            if n == 0 {
                if let Some(source) = self.sources[1]
                    && !self.armed[1]
                {
                    if requests.rearm(source).is_err() {
                        return Ok(Advance {
                            work_done: 1,
                            step: Step::Runnable,
                        });
                    }
                    self.armed[1] = true;
                }
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if world.drained {
            return Ok(self.retire(requests, expected));
        }
        let reader = world
            .source
            .reader_mut()
            .ok_or(SystemCallError::InternalError)?;
        let result = reader.read(&mut world.buffer);
        let n = match result {
            Ok(n) => n,
            Err(error) => {
                world.source_bytes += error.completed as u64;
                world.outcome = Some(PumpFault::Source(error));
                return Ok(self.retire(requests, expected));
            }
        };
        world.source_bytes += n as u64;
        world.have = n;
        world.sent = 0;
        if n != 0 {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        match reader.eof_reached() {
            Ok(true) => {
                world.drained = true;
                Ok(self.retire(requests, expected))
            }
            Ok(false) => {
                if let Some(source) = self.sources[0]
                    && !self.armed[0]
                {
                    if requests.rearm(source).is_err() {
                        return Ok(Advance {
                            work_done: 1,
                            step: Step::Runnable,
                        });
                    }
                    self.armed[0] = true;
                }
                Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                })
            }
            Err(error) => {
                world.outcome = Some(PumpFault::Source(error));
                Ok(self.retire(requests, expected))
            }
        }
    }

    fn refused(&mut self, world: &mut PumpWorld<'_>, failure: RequestFailure<Self>) {
        match failure {
            RequestFailure::Source { kind, error } => {
                if let Some(index) = Self::index(kind) {
                    self.requested[index] = false;
                    self.removing[index] = false;
                    self.armed[index] = false;
                }
                world.outcome.get_or_insert(PumpFault::System(error));
            }
            RequestFailure::Wake { error, .. } => {
                world.outcome.get_or_insert(PumpFault::System(error));
            }
            RequestFailure::Spawn { error, .. } => {
                world.outcome.get_or_insert(PumpFault::System(error));
            }
        }
    }

    fn registered(&mut self, world: &mut PumpWorld<'_>, kind: SourceKind, source: SourceId) {
        if let Some(index) = Self::index(kind) {
            self.requested[index] = false;
            self.sources[index] = Some(source);
            self.armed[index] = true;
        } else {
            world.outcome = Some(PumpFault::System(SystemCallError::InternalError));
        }
    }

    fn unregistered(&mut self, world: &mut PumpWorld<'_>, kind: SourceKind, source: SourceId) {
        if let Some(index) = Self::index(kind)
            && self.sources[index] == Some(source)
        {
            self.sources[index] = None;
            self.removing[index] = false;
            self.armed[index] = false;
        } else {
            world.outcome = Some(PumpFault::System(SystemCallError::InternalError));
        }
    }

    fn stop(&mut self, world: &mut PumpWorld<'_>) {
        self.stopping = true;
        world.outcome.get_or_insert(PumpFault::Cancelled);
    }

    fn deadline(&self) -> rinlib::time::Deadline {
        self.deadline
    }
}

fn preparation_fault(error: SystemCallError) -> PumpRunFailure {
    PumpRunFailure {
        summary: PumpSummary {
            source_bytes: 0,
            target_bytes: 0,
            fault: Some(PumpFault::System(error)),
        },
        runtime: None,
        set: None,
        snapshot: None,
    }
}

#[allow(clippy::result_large_err)]
pub fn run_pump(
    source: &mut Stream,
    target: &mut Stream,
    cancel: Option<&Capability>,
    deadline: rinlib::time::Deadline,
) -> Result<PumpSummary, PumpRunFailure> {
    let source_limit = if cancel.is_some() { 5 } else { 4 };
    let input =
        Runtime::<PumpTask, WaitSet>::input_budget(source_limit).map_err(preparation_fault)?;
    let limits = [1, input];
    let budget = Budget::new(&limits, 1).map_err(preparation_fault)?;
    let account = budget.account(&limits).map_err(preparation_fault)?;
    let task_slot = budget
        .slot(0)
        .ok_or(SystemCallError::InternalError)
        .map_err(preparation_fault)?;
    let input_slot = budget
        .slot(1)
        .ok_or(SystemCallError::InternalError)
        .map_err(preparation_fault)?;
    let execution = account
        .view::<ExecutionResource>(&[task_slot, input_slot])
        .map_err(preparation_fault)?;
    let set = WaitSet::create(source_limit).map_err(preparation_fault)?;
    let mut runtime = match Runtime::<PumpTask, WaitSet>::try_new(set, 1, source_limit, &execution)
    {
        Ok(runtime) => runtime,
        Err((set, error)) => {
            let mut failure = preparation_fault(error);
            if let Err((set, _)) = set.close() {
                failure.set = Some(set);
            }
            return Err(failure);
        }
    };
    if let Err(failure) = runtime.spawn(PumpTask::new(deadline), source_limit) {
        let mut error = preparation_fault(failure.error);
        if let Err((runtime, close_error)) = runtime.close() {
            error.summary.fault = Some(PumpFault::System(close_error));
            error.runtime = Some(runtime);
        }
        return Err(error);
    }
    let mut world = PumpWorld {
        source,
        target,
        cancel: cancel.map(Capability::as_handle),
        buffer: [0; BUFFER_BYTES],
        have: 0,
        sent: 0,
        source_bytes: 0,
        target_bytes: 0,
        outcome: None,
        drained: false,
    };
    if let Err(failure) = runtime.run(&mut world, 1) {
        world
            .outcome
            .get_or_insert(PumpFault::System(failure.error));
        for _ in 0..RETIRE_TURNS {
            if runtime.drive_state() == DriveState::Drained {
                break;
            }
            if runtime.shutdown_turn(&mut world, 1).is_err() {
                break;
            }
        }
    }
    let summary = world.summary();
    if runtime.drive_state() != DriveState::Drained {
        return Err(PumpRunFailure {
            summary,
            snapshot: Some(world.snapshot()),
            runtime: Some(runtime),
            set: None,
        });
    }
    match runtime.close() {
        Ok(())
            if execution.usage(ExecutionResource::Task).0 == 0
                && execution.usage(ExecutionResource::InputBytes).0 == 0 =>
        {
            Ok(summary)
        }
        Ok(()) => Err(PumpRunFailure {
            summary: PumpSummary {
                fault: Some(PumpFault::System(SystemCallError::InternalError)),
                ..summary
            },
            runtime: None,
            set: None,
            snapshot: None,
        }),
        Err((runtime, error)) => Err(PumpRunFailure {
            summary: PumpSummary {
                fault: Some(PumpFault::System(error)),
                ..summary
            },
            runtime: Some(runtime),
            set: None,
            snapshot: None,
        }),
    }
}
