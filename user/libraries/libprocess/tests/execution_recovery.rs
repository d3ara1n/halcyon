//! 正式 Runtime 的预付失败路径与 Job 的跨页/嵌套恢复，不使用替代运行体。
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals},
    proc::{
        JOB_ENUMERATE_MAX, JobEnumerateResult, JobMemberKind, JobSnapshot, JobState,
        ProcessDrainResult, ProcessDrainStatus, ProcessSnapshot, ProcessState,
    },
    time::Deadline,
    wait::WaitItem,
    wait_set::ReadyRecord,
};
use libbudget::Budget;
use libexecution::{
    ExecutionResource,
    runtime::{
        Advance, Input, RequestFailure, Requests, Runtime, SourceOps, SourceSet, Step, Task,
    },
};
use libprocess::{
    DEFAULT_SUPERVISION_POLICY, JobCollector, JobCollectorEvent, JobKillCause, JobKillStage,
    JobOperations, SupervisionStage, observation::ObservationResult, supervise::ProcessOperations,
};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

thread_local! {
    // None正常；0持续拒绝；n在第n次申请拒绝一次。只影响当前测试线程。
    static FAIL_ALLOCATION: Cell<Option<usize>> = const { Cell::new(None) };
}
struct TestAllocator;
fn reject_allocation() -> bool {
    FAIL_ALLOCATION
        .try_with(|slot| match slot.get() {
            None => false,
            Some(0) => true,
            Some(1) => {
                slot.set(None);
                true
            }
            Some(n) => {
                slot.set(Some(n - 1));
                false
            }
        })
        .unwrap_or(false)
}
// SAFETY: 未注入失败的申请和全部释放均原样交给System；拒绝仅返回空指针。
unsafe impl GlobalAlloc for TestAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if reject_allocation() {
            std::ptr::null_mut()
        } else {
            // SAFETY: 原样转交有效布局。
            unsafe { System.alloc(layout) }
        }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if reject_allocation() {
            std::ptr::null_mut()
        } else {
            // SAFETY: 原样转交有效布局。
            unsafe { System.alloc_zeroed(layout) }
        }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if reject_allocation() {
            std::ptr::null_mut()
        } else {
            // SAFETY: 指针、旧布局与新大小沿用调用者契约；失败不释放原分配。
            unsafe { System.realloc(ptr, layout, size) }
        }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: 本分配器成功取得的指针全部来自System。
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: TestAllocator = TestAllocator;
struct AllocationFailure;
impl AllocationFailure {
    fn new(nth: usize) -> Self {
        FAIL_ALLOCATION.set(Some(nth));
        Self
    }
}
impl Drop for AllocationFailure {
    fn drop(&mut self) {
        FAIL_ALLOCATION.set(None);
    }
}

struct EmptySources;
impl SourceOps for EmptySources {
    fn register_item(&self, _: WaitItem) -> Result<u64, SystemCallError> {
        unreachable!()
    }
    fn rearm(&self, _: u64) -> Result<u64, SystemCallError> {
        unreachable!()
    }
    fn remove_source(&self, _: u64) -> Result<(), SystemCallError> {
        unreachable!()
    }
    fn receive_into(&self, _: &mut [ReadyRecord]) -> Result<usize, SystemCallError> {
        Err(SystemCallError::ObjectNotAvailable)
    }
    fn wait(&self, _: Deadline) -> Result<(), SystemCallError> {
        unreachable!()
    }
    fn now_ns(&self) -> Result<u64, SystemCallError> {
        Ok(0)
    }
}
impl SourceSet for EmptySources {
    fn close_set(self) -> Result<(), (Self, SystemCallError)> {
        Ok(())
    }
}
#[derive(Debug)]
struct SpawnTask {
    submitted: bool,
}
#[derive(Default)]
struct SpawnWorld {
    refused: Option<SystemCallError>,
    returned: bool,
    advanced: usize,
}
impl Task<SpawnWorld> for SpawnTask {
    type Family = Self;
    fn advance(
        &mut self,
        _: u64,
        world: &mut SpawnWorld,
        requests: &mut Requests<Self>,
        _: &mut Input<'_>,
        _: usize,
    ) -> Result<Advance, SystemCallError> {
        world.advanced += 1;
        if !self.submitted {
            requests
                .spawn(SpawnTask { submitted: false }, 0)
                .map_err(|_| SystemCallError::ReachLimit)?;
            self.submitted = true;
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Complete,
        })
    }
    fn refused(&mut self, world: &mut SpawnWorld, failure: RequestFailure<Self>) {
        if let RequestFailure::Spawn { task, error } = failure {
            world.refused = Some(error);
            world.returned = !task.submitted;
        }
    }
    fn stop(&mut self, _: &mut SpawnWorld) {}
}

#[test]
fn prepaid_gate_returns_spawn_owner_and_retires_without_fresh_allocation() {
    let bytes = Runtime::<SpawnTask, EmptySources>::input_budget(1).unwrap();
    let budget = Budget::new(&[2, bytes], 1).unwrap();
    let account = budget.account(&[2, bytes]).unwrap();
    let account = account
        .view(&[budget.slot(0).unwrap(), budget.slot(1).unwrap()])
        .unwrap();
    let mut runtime = Runtime::new(EmptySources, 2, 1, &account).unwrap();
    runtime.spawn(SpawnTask { submitted: false }, 0).unwrap();
    let mut world = SpawnWorld::default();
    let mut error = None;
    {
        let _failure = AllocationFailure::new(0);
        for _ in 0..80 {
            if runtime.is_empty() {
                break;
            }
            if let Err(failure) = runtime.turn(&mut world, 1) {
                error = Some(failure);
                break;
            }
        }
    }
    assert_eq!(error, None);
    assert_eq!(world.refused, Some(SystemCallError::OutOfMemory));
    assert!(world.returned);
    assert_eq!(world.advanced, 2);
    assert!(runtime.is_empty());
    assert_eq!(account.usage(ExecutionResource::Task).0, 0);
    assert!(runtime.close().is_ok());
    assert_eq!(account.usage(ExecutionResource::InputBytes).0, 0);
}

const PROCESS_BASE: u64 = 100_000;
#[derive(Debug, Default)]
struct State {
    members: BTreeMap<u64, Vec<u64>>,
    children: BTreeMap<u64, Vec<u64>>,
    closed_processes: BTreeSet<u64>,
    closed_jobs: BTreeSet<u64>,
    drains: BTreeMap<u64, usize>,
    derives: BTreeMap<u64, usize>,
    enumerated: Vec<(u64, JobMemberKind, u64)>,
    stalls: usize,
    page_failure: bool,
    process_close_failure: bool,
    child_close_failure: bool,
    child_page_failure: bool,
    missing_process: Option<u64>,
    missing_child: Option<u64>,
    malformed: bool,
    child_page_ready: bool,
}
#[derive(Clone, Debug)]
struct Environment(Rc<RefCell<State>>);
impl ProcessOperations for Environment {
    fn probe(
        &self,
        _: Handle,
        signals: ObjectSignals,
    ) -> Result<ObservationResult, SystemCallError> {
        Ok(ObservationResult::Ready(signals))
    }
    fn drain(&self, control: Handle, _: u32) -> Result<ProcessDrainResult, SystemCallError> {
        let pid = control.raw() - PROCESS_BASE;
        *self.0.borrow_mut().drains.entry(pid).or_default() += 1;
        Ok(ProcessDrainResult {
            work_done: 3,
            status: ProcessDrainStatus::Complete as u32,
            reserved: 0,
        })
    }
    fn query(&self, control: Handle) -> Result<ProcessSnapshot, SystemCallError> {
        Ok(ProcessSnapshot {
            pid: control.raw() - PROCESS_BASE,
            parent_pid: 0,
            state: ProcessState::Dead as u32,
            reason: 0,
            code: 0,
            reserved: 0,
        })
    }
    fn close(&self, control: Handle) -> Result<(), SystemCallError> {
        let mut state = self.0.borrow_mut();
        if control.raw() >= PROCESS_BASE {
            let pid = control.raw() - PROCESS_BASE;
            if pid == 1000 && state.process_close_failure {
                state.process_close_failure = false;
                return Err(SystemCallError::ObjectBusy);
            }
            assert!(
                state.closed_processes.insert(pid),
                "process control closed twice"
            );
            for members in state.members.values_mut() {
                members.retain(|id| *id != pid);
            }
        } else {
            let jid = control.raw();
            if jid == 10 && state.child_close_failure {
                assert!(
                    state.closed_jobs.contains(&900),
                    "nested child must finish first"
                );
                state.child_close_failure = false;
                return Err(SystemCallError::ObjectBusy);
            }
            assert!(state.closed_jobs.insert(jid), "child control closed twice");
            for children in state.children.values_mut() {
                children.retain(|id| *id != jid);
            }
        }
        Ok(())
    }
}
impl JobOperations for Environment {
    fn seal_job(&self, _: Handle) -> Result<(), SystemCallError> {
        Ok(())
    }
    fn enumerate_job(
        &self,
        job: Handle,
        kind: JobMemberKind,
        cursor: u64,
        output: &mut [u64],
    ) -> Result<JobEnumerateResult, SystemCallError> {
        assert_eq!(output.len(), JOB_ENUMERATE_MAX);
        let mut state = self.0.borrow_mut();
        state.enumerated.push((job.raw(), kind, cursor));
        if state.malformed {
            state.malformed = false;
            return Ok(JobEnumerateResult {
                actual: 0,
                more: 1,
                next_cursor: cursor + 1,
            });
        }
        if job.raw() == 1 && kind == JobMemberKind::MemberProcesses {
            if state.stalls != 0 {
                state.stalls -= 1;
                return Ok(JobEnumerateResult {
                    actual: 0,
                    more: 1,
                    next_cursor: cursor,
                });
            }
            if cursor != 0 && state.page_failure {
                assert_eq!(
                    state.closed_processes.len(),
                    JOB_ENUMERATE_MAX - 1,
                    "prior page must be collected before requesting another page"
                );
                state.page_failure = false;
                output[0] = u64::MAX;
                return Err(SystemCallError::ObjectBusy);
            }
        }
        if job.raw() == 1 && kind == JobMemberKind::ChildJobs {
            state.child_page_ready = true;
            if cursor != 0 && state.child_page_failure {
                assert_eq!(state.closed_jobs.len(), JOB_ENUMERATE_MAX);
                state.child_page_failure = false;
                return Err(SystemCallError::ObjectBusy);
            }
        }
        let values = if kind == JobMemberKind::MemberProcesses {
            &state.members
        } else {
            &state.children
        };
        let mut ids = values
            .get(&job.raw())
            .into_iter()
            .flatten()
            .copied()
            .filter(|id| *id > cursor);
        let mut actual = 0;
        for value in output.iter_mut() {
            let Some(id) = ids.next() else {
                break;
            };
            *value = id;
            actual += 1;
        }
        Ok(JobEnumerateResult {
            actual: actual as u32,
            more: u32::from(ids.next().is_some()),
            next_cursor: if actual == 0 {
                cursor
            } else {
                output[actual - 1]
            },
        })
    }
    fn derive_job(
        &self,
        job: Handle,
        kind: JobMemberKind,
        id: u64,
        _: erhino_shared::object::Rights,
    ) -> Result<Handle, SystemCallError> {
        let mut state = self.0.borrow_mut();
        if kind == JobMemberKind::MemberProcesses {
            if state.missing_process == Some(id) {
                state
                    .members
                    .get_mut(&job.raw())
                    .unwrap()
                    .retain(|pid| *pid != id);
                return Err(SystemCallError::ObjectNotFound);
            }
            Ok(Handle::from_raw(PROCESS_BASE + id))
        } else {
            if state.missing_child == Some(id) {
                state
                    .children
                    .get_mut(&job.raw())
                    .unwrap()
                    .retain(|jid| *jid != id);
                return Err(SystemCallError::ObjectNotFound);
            }
            *state.derives.entry(id).or_default() += 1;
            Ok(Handle::from_raw(id))
        }
    }
    fn kill(&self, _: Handle, _: i64) -> Result<(), SystemCallError> {
        Ok(())
    }
    fn query_job(&self, job: Handle) -> Result<JobSnapshot, SystemCallError> {
        let state = self.0.borrow();
        assert!(state.members.get(&job.raw()).is_none_or(Vec::is_empty));
        assert!(state.children.get(&job.raw()).is_none_or(Vec::is_empty));
        Ok(JobSnapshot {
            jid: job.raw(),
            parent_jid: 0,
            state: JobState::Dead as u32,
            live_processes: 0,
            live_children: 0,
            reserved: 0,
            reserved2: 0,
        })
    }
}

#[test]
fn multiple_pages_preserve_progress_through_stalls_missing_entries_and_nested_close_failure() {
    let count = JOB_ENUMERATE_MAX as u64 + 2;
    let state = Rc::new(RefCell::new(State {
        members: BTreeMap::from([
            (1, (1000..1000 + count).collect()),
            (10, vec![2000]),
            (900, vec![3000]),
        ]),
        children: BTreeMap::from([(1, (10..10 + count).collect()), (10, vec![900])]),
        stalls: 3,
        page_failure: true,
        process_close_failure: true,
        child_close_failure: true,
        child_page_failure: true,
        missing_process: Some(1001),
        missing_child: Some(11),
        ..State::default()
    }));
    let policy = libprocess::SupervisionPolicy {
        enumerate_stalls: 2,
        ..DEFAULT_SUPERVISION_POLICY
    };
    let mut collector =
        JobCollector::new_with_ops(Handle::from_raw(1), 0, policy, Environment(state.clone()))
            .unwrap();
    let mut stages = Vec::new();
    let mut done = false;
    for _ in 0..20_000 {
        match collector.step(0) {
            Ok(JobCollectorEvent::Progress) => {}
            Ok(JobCollectorEvent::Observe(request)) => collector
                .observe(request, ObservationResult::Ready(request.signals), 0)
                .unwrap(),
            Ok(JobCollectorEvent::RetryAt(_)) => panic!("fixture has no timed retry"),
            Ok(JobCollectorEvent::Done) => {
                done = true;
                break;
            }
            Err(failure) => {
                stages.push(failure.stage);
                if failure.cause == JobKillCause::Timeout {
                    collector.replenish(policy);
                } else if failure.stage == JobKillStage::EnumerateMembers {
                    assert_eq!(
                        failure.progress.members_collected as usize,
                        JOB_ENUMERATE_MAX - 1
                    );
                }
            }
        }
    }
    assert!(done);
    assert_eq!(
        stages,
        [
            JobKillStage::EnumerateMembers,
            JobKillStage::CollectMember(SupervisionStage::Close),
            JobKillStage::EnumerateMembers,
            JobKillStage::CloseChild,
            JobKillStage::EnumerateChildren
        ]
    );
    let state = state.borrow();
    assert_eq!(collector.progress().members_collected as u64, count + 1);
    assert_eq!(collector.progress().process_work as u64, (count + 1) * 3);
    assert_eq!(collector.progress().children_collected as u64, count);
    assert!(state.drains.values().all(|n| *n == 1));
    assert!(state.derives.values().all(|n| *n == 1));
    assert_eq!(
        state
            .enumerated
            .iter()
            .filter(|(j, k, c)| *j == 1
                && *k == JobMemberKind::MemberProcesses
                && *c == 1000 + JOB_ENUMERATE_MAX as u64 - 1)
            .count(),
        2
    );
    assert!(
        state
            .enumerated
            .iter()
            .any(|(j, k, c)| *j == 1 && *k == JobMemberKind::ChildJobs && *c != 0)
    );
}

#[test]
fn zero_progress_cannot_advance_past_an_unpublished_entry() {
    let state = Rc::new(RefCell::new(State {
        malformed: true,
        ..State::default()
    }));
    let mut collector = JobCollector::new_with_ops(
        Handle::from_raw(1),
        0,
        DEFAULT_SUPERVISION_POLICY,
        Environment(state.clone()),
    )
    .unwrap();
    collector.step(0).unwrap();
    let error = collector.step(0).unwrap_err();
    assert_eq!(
        error.cause,
        JobKillCause::System(SystemCallError::InternalError)
    );
    collector.step(0).unwrap();
    assert_eq!(
        state
            .borrow()
            .enumerated
            .iter()
            .map(|(_, _, cursor)| *cursor)
            .collect::<Vec<_>>(),
        [0, 0]
    );
}

#[test]
fn stack_and_child_page_allocation_fail_before_deriving_authority() {
    for allocation in [1, 2] {
        let state = Rc::new(RefCell::new(State {
            children: BTreeMap::from([(1, vec![10])]),
            ..State::default()
        }));
        let mut collector = JobCollector::new_with_ops(
            Handle::from_raw(1),
            0,
            DEFAULT_SUPERVISION_POLICY,
            Environment(state.clone()),
        )
        .unwrap();
        while !state.borrow().child_page_ready {
            collector.step(0).unwrap();
        }
        let outcome = {
            let _failure = AllocationFailure::new(allocation);
            collector.step(0)
        };
        let error = outcome.unwrap_err();
        assert_eq!(error.stage, JobKillStage::DeriveChild);
        assert_eq!(
            error.cause,
            JobKillCause::System(SystemCallError::OutOfMemory)
        );
        assert!(state.borrow().derives.is_empty());
        collector.step(0).unwrap();
        assert_eq!(state.borrow().derives.get(&10), Some(&1));
    }
}
