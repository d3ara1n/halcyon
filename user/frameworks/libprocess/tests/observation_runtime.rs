use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals},
    proc::{ProcessDrainResult, ProcessDrainStatus, ProcessSnapshot, ProcessState},
    time::Deadline,
    wait::WaitItem,
    wait_set::ReadyRecord,
};
use libprocess::observation::ObservationResult;
use libprocess::supervise::ProcessOperations;
use libprocess::{
    Collector, DEFAULT_SUPERVISION_POLICY, SuperviseResult, SuperviseSink, SuperviseTask,
    SupervisionCause, SupervisionStage, SupervisionTarget,
};
use libsrv::{
    budget::{Budget, CoreResource},
    runtime::{Runtime, SourceOps, SourceRegistrar, SourceSet},
};
use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    rc::Rc,
};

#[derive(Debug, Default)]
struct State {
    now: u64,
    next: u64,
    entries: BTreeMap<u64, (WaitItem, bool)>,
    records: VecDeque<ReadyRecord>,
    no_signal: bool,
    register_error: Option<SystemCallError>,
    remove_busy: usize,
    close_busy: usize,
    drain_calls: usize,
    drain_busy: usize,
    queries: usize,
    close_calls: usize,
    wait_calls: usize,
    ready_delay: u64,
    probes: usize,
}
#[derive(Debug, Clone)]
struct Environment(Rc<RefCell<State>>);
impl ProcessOperations for Environment {
    fn probe(
        &self,
        _: Handle,
        signals: ObjectSignals,
    ) -> Result<ObservationResult, SystemCallError> {
        let mut state = self.0.borrow_mut();
        state.probes += 1;
        Ok(if state.no_signal {
            ObservationResult::TimedOut
        } else {
            ObservationResult::Ready(signals)
        })
    }
    fn drain(&self, _: Handle, _: u32) -> Result<ProcessDrainResult, SystemCallError> {
        let mut state = self.0.borrow_mut();
        assert!(
            state.entries.is_empty(),
            "Drain must wait for observer retirement"
        );
        state.drain_calls += 1;
        if state.drain_busy != 0 {
            state.drain_busy -= 1;
            return Err(SystemCallError::ObjectBusy);
        }
        Ok(ProcessDrainResult {
            work_done: 1,
            status: ProcessDrainStatus::Complete as u32,
            reserved: 0,
        })
    }
    fn query(&self, _: Handle) -> Result<ProcessSnapshot, SystemCallError> {
        Ok(ProcessSnapshot {
            pid: 7,
            parent_pid: 0,
            state: ProcessState::Dead as u32,
            reason: 0,
            code: 0,
            reserved: 0,
        })
    }
    fn close(&self, _: Handle) -> Result<(), SystemCallError> {
        let mut state = self.0.borrow_mut();
        assert!(state.entries.is_empty());
        state.close_calls += 1;
        if state.close_busy != 0 {
            state.close_busy -= 1;
            return Err(SystemCallError::ObjectBusy);
        }
        Ok(())
    }
}
impl SourceRegistrar for Environment {
    fn register_item(&self, item: WaitItem) -> Result<u64, SystemCallError> {
        let mut state = self.0.borrow_mut();
        if let Some(error) = state.register_error {
            return Err(error);
        }
        state.next += 1;
        let token = state.next;
        state.entries.insert(token, (item, false));
        Ok(token)
    }
}
impl SourceOps for Environment {
    fn rearm(&self, _: u64) -> Result<u64, SystemCallError> {
        panic!("one-shot observation must not rearm")
    }
    fn remove_source(&self, token: u64) -> Result<(), SystemCallError> {
        let mut state = self.0.borrow_mut();
        if state.remove_busy != 0 {
            state.remove_busy -= 1;
            return Err(SystemCallError::ObjectBusy);
        }
        state.entries.remove(&token);
        Ok(())
    }
    fn receive_into(&self, records: &mut [ReadyRecord]) -> Result<usize, SystemCallError> {
        let mut state = self.0.borrow_mut();
        let mut n = 0;
        while n < records.len() {
            let Some(record) = state.records.pop_front() else {
                break;
            };
            records[n] = record;
            n += 1;
        }
        Ok(n)
    }
    fn wait(&self, deadline: Deadline) -> Result<(), SystemCallError> {
        let mut state = self.0.borrow_mut();
        state.wait_calls += 1;
        if !state.no_signal
            && let Some((&token, (item, delivered))) =
                state.entries.iter_mut().find(|(_, (_, fired))| !*fired)
        {
            *delivered = true;
            let record = ReadyRecord {
                token,
                arm_generation: 1,
                cookie: item.cookie,
                observed: ObjectSignals::REAPABLE,
                reason: 0,
                error: 0,
            };
            state.records.push_back(record);
            state.now += state.ready_delay.max(1);
            return Ok(());
        }
        state.now = deadline
            .instant()
            .unwrap()
            .expect("all supervision waits must be finite");
        Ok(())
    }
    fn now_ns(&self) -> Result<u64, SystemCallError> {
        let mut state = self.0.borrow_mut();
        state.queries += 1;
        assert!(
            state.queries < 4096,
            "runtime must reach its finite wait rather than spin"
        );
        Ok(state.now)
    }
}
impl SourceSet for Environment {
    fn close_set(self) -> Result<(), (Self, SystemCallError)> {
        assert!(self.0.borrow().entries.is_empty());
        Ok(())
    }
}
#[derive(Default)]
struct World {
    results: Vec<SuperviseResult<Environment>>,
}
impl SuperviseSink<Environment> for World {
    fn supervised(&mut self, result: SuperviseResult<Environment>) {
        self.results.push(result);
    }
}
fn runtime(env: &Environment) -> Runtime<SuperviseTask<Environment>, Environment> {
    let bytes = Runtime::<SuperviseTask<Environment>, Environment>::input_budget(1).unwrap();
    let budget = Budget::<CoreResource>::new(&[1, bytes], 1).unwrap();
    let account = budget.account(&[1, bytes]).unwrap();
    Runtime::new(env.clone(), 1, 1, CoreResource::EXECUTION_SLOTS, &account).unwrap()
}
fn task(env: &Environment) -> SuperviseTask<Environment> {
    let policy = libprocess::SupervisionPolicy {
        wait_attempts: 1,
        ..DEFAULT_SUPERVISION_POLICY
    };
    SuperviseTask::from_collector(Collector::new_with_ops(
        SupervisionTarget::new(7, Handle::from_raw(42)),
        policy,
        env.clone(),
    ))
}

#[test]
fn observation_retirement_precedes_drain_and_close_failure_resumes_original_phase() {
    let env = Environment(Rc::new(RefCell::new(State {
        remove_busy: 1,
        close_busy: 1,
        ..State::default()
    })));
    let mut rt = runtime(&env);
    let mut world = World::default();
    assert!(rt.spawn(task(&env), 1).is_ok());
    rt.run(&mut world, 1).unwrap();
    assert!(rt.close().is_ok());
    let failure = world.results.pop().unwrap().outcome.err().unwrap();
    assert_eq!(failure.stage, SupervisionStage::Close);
    assert_eq!(env.0.borrow().drain_calls, 1);
    assert!(env.0.borrow().wait_calls >= 2);
    let mut rt = runtime(&env);
    assert!(
        rt.spawn(SuperviseTask::from_collector(failure.collector), 1)
            .is_ok()
    );
    rt.run(&mut world, 1).unwrap();
    assert!(world.results.pop().unwrap().outcome.is_ok());
    assert!(rt.close().is_ok());
    let state = env.0.borrow();
    assert_eq!(state.drain_calls, 1, "Close recovery must not repeat Drain");
    assert_eq!(state.close_calls, 2);
    assert_eq!(state.next, 1, "Close recovery must not reopen observation");
}

#[test]
fn timeout_and_registration_error_return_owned_waiting_machine_without_drain() {
    for register_error in [None, Some(SystemCallError::RightsDenied)] {
        let env = Environment(Rc::new(RefCell::new(State {
            no_signal: true,
            register_error,
            ..State::default()
        })));
        let mut rt = runtime(&env);
        let mut world = World::default();
        assert!(rt.spawn(task(&env), 1).is_ok());
        rt.run(&mut world, 1).unwrap();
        let failure = world.results.pop().unwrap().outcome.err().unwrap();
        assert_eq!(failure.collector.pid(), 7);
        assert_eq!(failure.stage, SupervisionStage::WaitReapable);
        assert_eq!(
            failure.cause,
            register_error.map_or(SupervisionCause::Timeout, SupervisionCause::System)
        );
        assert_eq!(env.0.borrow().drain_calls, 0);
        assert!(rt.close().is_ok());
    }
}

#[test]
fn repeated_drain_busy_consumes_timeout_and_waits_for_each_retry() {
    let env = Environment(Rc::new(RefCell::new(State {
        drain_busy: 2,
        ..State::default()
    })));
    let mut rt = runtime(&env);
    let mut world = World::default();
    assert!(rt.spawn(task(&env), 1).is_ok());
    rt.run(&mut world, 1).unwrap();
    assert!(world.results.pop().unwrap().outcome.is_ok());
    assert_eq!(env.0.borrow().drain_calls, 3);
    assert!(env.0.borrow().wait_calls >= 3);
    assert!(rt.close().is_ok());
}

#[test]
fn queued_ready_wins_when_timer_runs_before_receive_and_cleanup_finishes_late() {
    let env = Environment(Rc::new(RefCell::new(State {
        ready_delay: 100_000_001,
        remove_busy: 1,
        ..State::default()
    })));
    let mut rt = runtime(&env);
    let mut world = World::default();
    assert!(rt.spawn(task(&env), 1).is_ok());
    rt.run(&mut world, 1).unwrap();
    assert!(world.results.pop().unwrap().outcome.is_ok());
    assert!(
        env.0.borrow().probes > 0,
        "deadline arbitration must probe before discarding queued readiness"
    );
    assert_eq!(env.0.borrow().drain_calls, 1);
    assert!(rt.close().is_ok());
}
