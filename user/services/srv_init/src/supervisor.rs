//! init 的长期责任 owner。脚本只借用；阶段失败不销毁运行体、世界或未交付结果。
//! Job 与 Batch 各两槽承接原操作及 services 兜底；Read 保留单一数据面阶段。
//! 六个等待目标来自这些责任及空闲集合，满足 WaitMany 的 ABI 上界。

use super::*;
use alloc::vec::Vec;
use libprocess::{Collector, job_driver::JobDriver};
use libsrv::runtime::{DriveState, TaskFailure};
use rinlib::shared::time::Deadline;

const ROOT_WAIT_TARGETS: usize = 6;
const ROOT_JOB_SLOTS: usize = 2;

fn later(now: u64) -> u64 {
    now.saturating_add(
        DEFAULT_SUPERVISION_POLICY
            .wait_timeout_ms
            .saturating_mul(1_000_000),
    )
}
fn recoverable(error: SystemCallError) -> bool {
    matches!(
        error,
        SystemCallError::ObjectBusy
            | SystemCallError::OutOfMemory
            | SystemCallError::QuotaExceeded
            | SystemCallError::ReachLimit
    )
}
fn process_retry(cause: SupervisionCause) -> bool {
    matches!(cause, SupervisionCause::Stopped | SupervisionCause::Timeout)
        || matches!(cause, SupervisionCause::System(error) if recoverable(error))
}

#[derive(Default)]
struct Disposition {
    retry: Option<u64>,
    held: bool,
}
impl Disposition {
    fn fail(&mut self, error: SystemCallError, now: u64) {
        self.retry = recoverable(error).then(|| later(now));
        self.held = self.retry.is_none();
    }
    fn ready(&mut self, now: u64) -> bool {
        if self.held {
            return false;
        }
        if self.retry.is_some_and(|at| now < at) {
            return false;
        }
        self.retry = None;
        true
    }
}

struct JobTask {
    driver: JobDriver,
}
#[derive(Default)]
struct JobWorld {
    done: bool,
    failure: Option<libprocess::JobCollectorError>,
}
impl Task<JobWorld> for JobTask {
    type Family = Self;
    fn advance(
        &mut self,
        _id: u64,
        world: &mut JobWorld,
        requests: &mut Requests<Self>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Result<Advance, SystemCallError> {
        let result = self.driver.advance(requests, input, input.now_ns());
        match &result {
            Ok(advance) if advance.step == Step::Complete => world.done = true,
            Err(_) => world.failure = self.driver.failure(),
            _ => {}
        }
        result
    }
    fn registered(&mut self, _world: &mut JobWorld, kind: SourceKind, source: SourceId) {
        self.driver.registered(kind, source);
    }
    fn unregistered(&mut self, _world: &mut JobWorld, kind: SourceKind, source: SourceId) {
        self.driver.unregistered(kind, source);
    }
    fn refused(&mut self, _world: &mut JobWorld, failure: RequestFailure<Self>) {
        match failure {
            RequestFailure::Source { kind, error } => self.driver.refused(kind, error),
            RequestFailure::Wake { .. } => {}
            RequestFailure::Spawn { .. } => {}
        }
    }
    // Job 任务本身即为收束责任，停止业务不撤销该责任。
    fn stop(&mut self, _world: &mut JobWorld) {}
    fn deadline(&self) -> Deadline {
        self.driver.deadline()
    }
}

struct JobDuty {
    active: bool,
    root: Handle,
    code: i64,
    pending: Option<JobTask>,
    runtime: Option<Runtime<JobTask, WaitSet>>,
    world: JobWorld,
    disposition: Disposition,
    error: Option<SystemCallError>,
    complete: bool,
    failed_task: Option<u64>,
}
struct BatchDuty {
    incoming: Vec<Supervised>,
    one: Option<Supervised>,
    pending: Option<SuperviseTask>,
    runtime: Option<Runtime<SuperviseTask, WaitSet>>,
    world: SupervisorWorld,
    disposition: Disposition,
    error: Option<SystemCallError>,
    capacity: usize,
    admission_retry: Option<u64>,
    result_cursor: usize,
    complete: bool,
}
struct ReadDuty {
    pending: Option<InitTask>,
    runtime: Option<Runtime<InitTask, WaitSet>>,
    world: ReadWorld,
    disposition: Disposition,
    error: Option<SystemCallError>,
    complete: bool,
}

struct Cleanup<T> {
    owner: Option<T>,
    disposition: Disposition,
}
impl<T> Cleanup<T> {
    fn empty() -> Self {
        Self {
            owner: None,
            disposition: Disposition::default(),
        }
    }
    fn step(&mut self, now: u64, close: impl FnOnce(T) -> Result<(), (T, SystemCallError)>) {
        if !self.disposition.ready(now) {
            return;
        }
        if let Some(owner) = self.owner.take()
            && let Err((owner, error)) = close(owner)
        {
            self.owner = Some(owner);
            self.disposition.fail(error, now);
        }
    }
    fn runnable(&self) -> bool {
        self.owner.is_some() && !self.disposition.held && self.disposition.retry.is_none()
    }
}

enum RootCapability {
    Plain(Capability),
    Sender(MailboxSender),
}
impl RootCapability {
    fn handle(&self) -> Handle {
        match self {
            Self::Plain(owner) => owner.as_handle(),
            Self::Sender(owner) => owner.as_handle(),
        }
    }
    fn transferred(self) {
        match self {
            Self::Plain(owner) => {
                owner.into_raw();
            }
            Self::Sender(owner) => {
                owner.into_capability().into_raw();
            }
        }
    }
    fn close(self) -> Result<(), (Self, SystemCallError)> {
        match self {
            Self::Plain(owner) => owner
                .close()
                .map_err(|(owner, error)| (Self::Plain(owner), error)),
            Self::Sender(owner) => owner
                .close()
                .map_err(|(owner, error)| (Self::Sender(owner), error)),
        }
    }
}
struct CapabilityEntry {
    owner: Option<RootCapability>,
    reserved: bool,
    closing: bool,
    disposition: Disposition,
}

pub(super) struct RootSupervisor {
    idle: WaitSet,
    capabilities: Vec<CapabilityEntry>,
    capability_cursor: usize,
    endpoint_cleanup: Cleanup<rinlib::ipc::tunnel::Endpoint>,
    capability_cleanup: Cleanup<Capability>,
    services: Option<Handle>,
    jobs: [Option<JobDuty>; ROOT_JOB_SLOTS],
    batches: [Option<BatchDuty>; 2],
    read: Option<ReadDuty>,
    pub(super) processes: Vec<Supervised>,
    cursor: usize,
    failing: bool,
    reset_attempted: bool,
}

#[derive(Clone, Copy)]
enum Await {
    Job(usize),
    Batch(usize),
    Read,
}

fn can_drive<T>(rt: &Runtime<T, WaitSet>, now: u64) -> bool {
    match rt.drive_state() {
        DriveState::Runnable => true,
        DriveState::Waiting(deadline) => deadline
            .instant()
            .ok()
            .flatten()
            .is_some_and(|at| at <= now),
        DriveState::Drained => false,
    }
}

fn runtime<T>(capacity: usize, sources: usize) -> Result<Runtime<T, WaitSet>, SystemCallError> {
    let bytes = Runtime::<T, WaitSet>::input_budget(sources)?;
    let budget = libsrv::budget::Budget::new(&[capacity, bytes], 1)?;
    let account = budget.account(&[capacity, bytes])?;
    Runtime::new(
        WaitSet::create(sources)?,
        capacity,
        sources,
        libsrv::budget::CoreResource::EXECUTION_SLOTS,
        &account,
    )
}

/// 关闭失败把原运行体放回原槽；该操作不需要申请新的错误承载。
fn close_runtime<T>(slot: &mut Option<Runtime<T, WaitSet>>) -> Result<(), SystemCallError> {
    let Some(runtime) = slot.take() else {
        return Ok(());
    };
    match runtime.close() {
        Ok(()) => Ok(()),
        Err((runtime, error)) => {
            *slot = Some(runtime);
            Err(error)
        }
    }
}

fn isolate<T, W>(runtime: &mut Runtime<T, WaitSet>, failure: TaskFailure, now: u64)
where
    T: Task<W, Family = T>,
{
    if failure.task != 0 {
        let retry = recoverable(failure.error).then(|| later(now));
        runtime
            .defer_failed_task(failure.task, retry)
            .expect("reported failing task must remain owned");
    }
}

impl RootSupervisor {
    /// 必须在创建 services/启动任何子进程之前完成。
    pub(super) fn prepare() -> Result<Self, SystemCallError> {
        Ok(Self {
            idle: WaitSet::create(1)?,
            capabilities: Vec::new(),
            capability_cursor: 0,
            endpoint_cleanup: Cleanup::empty(),
            capability_cleanup: Cleanup::empty(),
            services: None,
            jobs: [None, None],
            batches: [None, None],
            read: None,
            processes: Vec::new(),
            cursor: 0,
            failing: false,
            reset_attempted: false,
        })
    }
    fn reserve_capabilities<const N: usize>(&mut self) -> Result<[usize; N], SystemCallError> {
        self.capabilities
            .try_reserve(N)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(core::array::from_fn(|_| {
            if let Some(index) = self
                .capabilities
                .iter()
                .position(|entry| !entry.reserved && entry.owner.is_none())
            {
                self.capabilities[index].reserved = true;
                index
            } else {
                self.capabilities.push(CapabilityEntry {
                    owner: None,
                    reserved: true,
                    closing: false,
                    disposition: Disposition::default(),
                });
                self.capabilities.len() - 1
            }
        }))
    }
    fn adopt(&mut self, slot: usize, handle: Handle) {
        let entry = &mut self.capabilities[slot];
        assert!(
            entry.reserved && entry.owner.is_none(),
            "capability storage is prepared before acquisition"
        );
        // SAFETY: 仅接收本次正式create/duplicate产生的唯一entry。
        entry.owner = Some(RootCapability::Plain(unsafe {
            Capability::from_raw(handle)
        }));
        entry.closing = false;
        entry.disposition = Disposition::default();
    }
    pub(super) fn create_job(
        &mut self,
        parent: Handle,
        rights: Rights,
    ) -> Result<Handle, SystemCallError> {
        let [slot] = self.reserve_capabilities()?;
        match process::create_job(parent, rights) {
            Ok(handle) => {
                self.adopt(slot, handle);
                Ok(handle)
            }
            Err(error) => {
                self.capabilities[slot].reserved = false;
                Err(error)
            }
        }
    }
    pub(super) fn duplicate(
        &mut self,
        source: Handle,
        rights: Rights,
    ) -> Result<Handle, SystemCallError> {
        let [slot] = self.reserve_capabilities()?;
        match duplicate(source, rights) {
            Ok(handle) => {
                self.adopt(slot, handle);
                Ok(handle)
            }
            Err(error) => {
                self.capabilities[slot].reserved = false;
                Err(error)
            }
        }
    }
    fn create_pair(
        &mut self,
        create: impl FnOnce() -> Result<rinlib::shared::object::HandlePair, SystemCallError>,
    ) -> Result<rinlib::shared::object::HandlePair, SystemCallError> {
        let slots: [usize; 2] = self.reserve_capabilities()?;
        match create() {
            Ok(pair) => {
                self.adopt(slots[0], pair.owner);
                self.adopt(slots[1], pair.peer);
                Ok(pair)
            }
            Err(error) => {
                for slot in slots {
                    self.capabilities[slot].reserved = false;
                }
                Err(error)
            }
        }
    }
    pub(super) fn create_mailbox(
        &mut self,
        owner: Rights,
        sender: Rights,
    ) -> Result<rinlib::shared::object::HandlePair, SystemCallError> {
        let pair = self.create_pair(|| create(owner, sender))?;
        let entry = self
            .capabilities
            .iter_mut()
            .find(|entry| {
                entry
                    .owner
                    .as_ref()
                    .is_some_and(|owner| owner.handle() == pair.peer)
            })
            .expect("new mailbox sender remains owned");
        let RootCapability::Plain(owner) = entry.owner.take().expect("sender remains owned") else {
            unreachable!()
        };
        match MailboxSender::from_capability(owner) {
            Ok((sender, _)) => {
                entry.owner = Some(RootCapability::Sender(sender));
                Ok(pair)
            }
            Err(failure) => {
                entry.owner = Some(RootCapability::Plain(failure.owner));
                Err(failure.error)
            }
        }
    }
    pub(super) fn create_notification(
        &mut self,
        owner: Rights,
        signaler: Rights,
    ) -> Result<rinlib::shared::object::HandlePair, SystemCallError> {
        self.create_pair(|| notification::create(owner, signaler))
    }
    pub(super) fn sender(&self, handle: Handle) -> &MailboxSender {
        self.capabilities
            .iter()
            .find_map(|entry| match &entry.owner {
                Some(RootCapability::Sender(sender)) if sender.as_handle() == handle => {
                    Some(sender)
                }
                _ => None,
            })
            .expect("borrowed sender remains in the root ledger")
    }
    /// 只在内核已经确认消费之后兑现本地owner，失败不调用此入口。
    pub(super) fn transferred(&mut self, handle: Handle) {
        let entry = self
            .capabilities
            .iter_mut()
            .find(|entry| {
                entry
                    .owner
                    .as_ref()
                    .is_some_and(|owner| owner.handle() == handle)
            })
            .expect("transferred capability remains in the root ledger");
        entry
            .owner
            .take()
            .expect("transferred owner exists")
            .transferred();
        entry.reserved = false;
    }

    /// 真实创建后下一操作失败，两个owner仍由根保存并显式兑现。
    pub(super) fn verify_startup_capture(&mut self) -> Result<(), &'static str> {
        let baseline = self
            .capabilities
            .iter()
            .filter(|entry| entry.owner.is_some())
            .count();
        let pair = self
            .create_mailbox(
                Rights::READ | Rights::WAIT | Rights::MANAGE,
                Rights::WRITE | Rights::WAIT,
            )
            .map_err(|_| "startup ownership fixture create failed")?;
        assert_eq!(
            self.duplicate(pair.peer, Rights::WRITE),
            Err(SystemCallError::RightsDenied)
        );
        assert_eq!(
            self.capabilities
                .iter()
                .filter(|entry| entry.owner.is_some())
                .count(),
            baseline + 2
        );
        let control = self
            .duplicate(
                self.services.expect("services root is prepared"),
                Rights::MANAGE | Rights::TRANSIT,
            )
            .map_err(|_| "startup ownership fixture duplicate failed")?;
        self.close_control(pair.owner)?;
        // SAFETY: 源entry仍由根持有；关闭的接收端使本次发送在提交前失败。
        assert_eq!(
            unsafe {
                send_raw(
                    pair.peer,
                    0,
                    &[],
                    &[HandleMove {
                        handle: control,
                        rights: Rights::MANAGE,
                    }],
                )
            },
            Err(SystemCallError::ObjectClosed)
        );
        // 发送失败不兑现移交；成功close同时证明内核仍保留源entry。
        self.close_control(control)?;
        self.close_control(pair.peer)?;
        assert_eq!(
            self.capabilities
                .iter()
                .filter(|entry| entry.owner.is_some())
                .count(),
            baseline
        );
        debug!("root startup ownership rollback passed");
        Ok(())
    }

    pub(super) fn verify_idle_ownership(&self) -> Result<(), &'static str> {
        if self
            .capabilities
            .iter()
            .filter_map(|entry| entry.owner.as_ref())
            .any(|owner| Some(owner.handle()) != self.services)
        {
            return Err("root still owns an auxiliary capability");
        }
        debug!("root auxiliary capability cleanup passed");
        Ok(())
    }

    pub(super) fn bind_services(&mut self, services: Handle) -> Result<(), SystemCallError> {
        self.services = Some(services);
        let index = self.begin_job(services, 0x1F, None);
        let duty = self.jobs[index]
            .as_mut()
            .expect("standby duty remains owned");
        duty.active = false;
        // 服务尚未启动时完成根 frame、运行体和任务准入；兜底激活不重新分配承载。
        Self::prepare_job(duty)
    }
    pub(super) fn reserve_process(&mut self) -> Result<(), SystemCallError> {
        self.processes
            .try_reserve(1)
            .map_err(|_| SystemCallError::OutOfMemory)
    }
    pub(super) fn track_process(&mut self, target: Supervised) {
        self.processes.push(target);
    }

    fn begin_job(&mut self, root: Handle, code: i64, machine: Option<JobCollector>) -> usize {
        if let Some(index) = self
            .jobs
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|duty| duty.root == root))
        {
            // 仅对未建立机器的新兜底请求去重；原失败机器不能被另一台替代。
            assert!(
                machine.is_none(),
                "an existing job duty cannot replace a transferred machine"
            );
            self.jobs[index]
                .as_mut()
                .expect("existing duty remains owned")
                .active = true;
            return index;
        }
        let index = self
            .jobs
            .iter()
            .position(Option::is_none)
            .expect("root job slots cover the original operation and services escalation");
        self.jobs[index] = Some(JobDuty {
            active: true,
            root,
            code,
            pending: machine.map(|machine| JobTask {
                driver: JobDriver::new(machine),
            }),
            runtime: None,
            world: JobWorld::default(),
            disposition: Disposition::default(),
            error: None,
            complete: false,
            failed_task: None,
        });
        index
    }

    pub(super) fn collect_job(&mut self, root: Handle, code: i64) -> Result<(), &'static str> {
        let index = self.begin_job(root, code, None);
        loop {
            self.turn_and_wait(Some(Await::Job(index)))
                .map_err(|_| "root supervision wait failed")?;
            let duty = self.jobs[index].as_ref().expect("job duty remains owned");
            if duty.error.is_some() {
                return Err("job supervision failed");
            }
            if duty.complete {
                return Ok(());
            }
        }
    }

    /// 脚本只借用handle；关闭失败仍留在原预备槽。
    pub(super) fn close_control(&mut self, control: Handle) -> Result<(), &'static str> {
        let job = self
            .jobs
            .iter()
            .position(|duty| duty.as_ref().is_some_and(|duty| duty.root == control));
        if job.is_some_and(|index| !self.jobs[index].as_ref().is_some_and(|duty| duty.complete)) {
            return Err("Job root still has active supervision");
        }
        let entry = self
            .capabilities
            .iter_mut()
            .find(|entry| {
                entry
                    .owner
                    .as_ref()
                    .is_some_and(|owner| owner.handle() == control)
            })
            .ok_or("control is not in the root ledger")?;
        entry.closing = true;
        match entry
            .owner
            .take()
            .expect("closing owner remains held")
            .close()
        {
            Ok(()) => {
                entry.reserved = false;
                entry.closing = false;
            }
            Err((owner, _)) => {
                entry.owner = Some(owner);
                return Err("control close failed");
            }
        }
        if let Some(index) = job {
            self.jobs[index] = None;
        }
        if self.services == Some(control) {
            self.services = None;
        }
        Ok(())
    }

    pub(super) fn retain_capability(&mut self, capability: Capability) {
        assert!(
            self.capability_cleanup.owner.is_none(),
            "stream setup has one in-flight capability"
        );
        self.capability_cleanup.owner = Some(capability);
    }
    pub(super) fn retain_transport(
        &mut self,
        endpoint: rinlib::ipc::tunnel::Endpoint,
        invitation: rinlib::ipc::invitation::Invitation,
    ) {
        assert!(
            self.endpoint_cleanup.owner.is_none(),
            "stream setup has one endpoint"
        );
        self.endpoint_cleanup.owner = Some(endpoint);
        self.retain_capability(invitation.into_capability());
    }

    fn begin_batch(&mut self, incoming: Vec<Supervised>, one: Option<Supervised>) -> usize {
        let index = self
            .batches
            .iter()
            .position(Option::is_none)
            .expect("batch slots cover the original batch and service handoff");
        let capacity = incoming.len() + usize::from(one.is_some());
        self.batches[index] = Some(BatchDuty {
            incoming,
            one,
            pending: None,
            runtime: None,
            world: SupervisorWorld {
                results: Vec::new(),
                failures: 0,
            },
            disposition: Disposition::default(),
            error: None,
            capacity,
            admission_retry: None,
            result_cursor: 0,
            complete: capacity == 0,
        });
        index
    }
    fn await_batch(&mut self, index: usize) -> Result<(), &'static str> {
        loop {
            self.turn_and_wait(Some(Await::Batch(index)))
                .map_err(|_| "root supervision wait failed")?;
            let duty = self.batches[index].as_ref().expect("batch remains owned");
            if duty.error.is_some() {
                return Err("process supervision failed");
            }
            if duty.complete {
                self.batches[index] = None;
                return Ok(());
            }
        }
    }
    pub(super) fn collect_one(&mut self, target: Supervised) -> Result<(), &'static str> {
        let index = self.begin_batch(Vec::new(), Some(target));
        self.await_batch(index)
    }
    pub(super) fn collect_machine(
        &mut self,
        machine: Collector,
    ) -> Result<libprocess::CollectedProcess, &'static str> {
        let index = self.begin_batch(Vec::new(), None);
        let duty = self.batches[index].as_mut().expect("new batch exists");
        duty.capacity = 1;
        duty.complete = false;
        duty.pending = Some(SuperviseTask::from_collector(machine));
        loop {
            self.turn_and_wait(Some(Await::Batch(index)))
                .map_err(|_| "root supervision wait failed")?;
            let duty = self.batches[index].as_ref().expect("batch remains owned");
            if duty.error.is_some() {
                return Err("process supervision failed");
            }
            if duty.complete {
                let result = *duty.world.results[0]
                    .outcome
                    .as_ref()
                    .expect("completed singleton batch succeeded");
                self.batches[index] = None;
                return Ok(result);
            }
        }
    }
    pub(super) fn collect_targets(&mut self, targets: Vec<Supervised>) -> Result<(), &'static str> {
        let index = self.begin_batch(targets, None);
        self.await_batch(index)
    }
    pub(super) fn collect_services(&mut self) -> Result<(), &'static str> {
        let incoming = core::mem::take(&mut self.processes);
        let index = self.begin_batch(incoming, None);
        self.await_batch(index)
    }
    pub(super) fn collect_terminated(&mut self) -> Result<usize, &'static str> {
        let mut count = 0;
        let mut index = 0;
        while index < self.processes.len() {
            let snapshot = process::query(self.processes[index].control)
                .map_err(|_| "service state query failed")?;
            if snapshot.state == ProcessState::Building as u32
                || snapshot.state == ProcessState::Running as u32
            {
                index += 1;
            } else {
                let target = self.processes.swap_remove(index);
                self.collect_one(target)?;
                count += 1;
            }
        }
        Ok(count)
    }

    pub(super) fn collect_process(&mut self, pid: u64) -> Result<(), &'static str> {
        let index = self
            .processes
            .iter()
            .position(|target| target.pid == pid)
            .ok_or("supervised process is not in the root ledger")?;
        let target = self.processes.swap_remove(index);
        self.collect_one(target)
    }

    /// 一台运行体被策略错误停驻时，另一台批次仍完成，并恢复原 Job 机器。
    pub(super) fn verify_failure_isolation(&mut self, parent: Handle) -> Result<(), &'static str> {
        let child = self
            .create_job(parent, JOB_FULL_RIGHTS)
            .map_err(|_| "root isolation Job create failed")?;
        let index = self.begin_job(child, 0x1F, None);
        let duty = self.jobs[index]
            .as_mut()
            .expect("isolation duty remains owned");
        let mut machine = JobCollector::new(child, 0x1F, DEFAULT_SUPERVISION_POLICY)
            .map_err(|_| "root isolation machine preparation failed")?;
        machine.replenish(libprocess::SupervisionPolicy {
            wait_attempts: 0,
            ..DEFAULT_SUPERVISION_POLICY
        });
        duty.pending = Some(JobTask {
            driver: JobDriver::new(machine),
        });
        loop {
            self.turn_and_wait(Some(Await::Job(index)))
                .map_err(|_| "root isolation wait failed")?;
            if self.jobs[index]
                .as_ref()
                .is_some_and(|duty| duty.failed_task.is_some())
            {
                break;
            }
        }
        self.verify_batch_handoff()?;
        let now = rinlib::time::snapshot()
            .map_err(|_| "root isolation clock failed")?
            .now_ns;
        let duty = self.jobs[index]
            .as_mut()
            .expect("held Job duty remains owned");
        let id = duty
            .failed_task
            .take()
            .expect("failed Job task remains owned");
        let rt = duty
            .runtime
            .as_mut()
            .expect("failed Job runtime remains owned");
        rt.get_task_mut(id)
            .expect("held task remains in its runtime")
            .driver
            .replenish(DEFAULT_SUPERVISION_POLICY);
        rt.defer_failed_task(id, Some(now))
            .map_err(|_| "root isolation resume failed")?;
        duty.error = None;
        duty.world.failure = None;
        self.collect_job(child, 0x1F)?;
        self.close_control(child)?;
        debug!("root independent runtime failure isolation passed");
        Ok(())
    }

    /// 使用正式运行体验证部分准入、失败结果和健康成员的独立推进。
    pub(super) fn verify_batch_handoff(&mut self) -> Result<(), &'static str> {
        if self.processes.len() < 2 {
            return Err("supervision handoff requires two launched services");
        }
        let mut incoming = Vec::new();
        incoming
            .try_reserve_exact(2)
            .map_err(|_| "handoff input preparation failed")?;
        let mut results = Vec::new();
        results
            .try_reserve_exact(2)
            .map_err(|_| "handoff output preparation failed")?;
        let test_runtime = runtime(1, 2).map_err(|_| "handoff runtime preparation failed")?;
        incoming.push(self.processes.pop().expect("two controls were verified"));
        incoming.push(self.processes.pop().expect("two controls were verified"));
        let index = self.begin_batch(incoming, None);
        let duty = self.batches[index]
            .as_mut()
            .expect("handoff batch remains owned");
        duty.runtime = Some(test_runtime);
        duty.world.results = results;
        let target = duty.incoming.pop().expect("handoff input remains owned");
        let policy = libprocess::SupervisionPolicy {
            wait_attempts: 0,
            ..DEFAULT_SUPERVISION_POLICY
        };
        duty.pending = Some(SuperviseTask::new(
            SupervisionTarget::new(target.pid, target.control),
            policy,
        ));
        loop {
            self.turn_and_wait(None)
                .map_err(|_| "handoff combined wait failed")?;
            let duty = self.batches[index]
                .as_ref()
                .expect("handoff batch remains owned");
            if duty.world.results.len() == 2 && duty.runtime.as_ref().is_some_and(Runtime::is_empty)
            {
                break;
            }
        }
        let duty = self.batches[index]
            .as_mut()
            .expect("handoff batch remains owned");
        assert_eq!(
            duty.error,
            Some(SystemCallError::QuotaExceeded),
            "partial admission must preserve the first failure"
        );
        let failed = duty
            .world
            .results
            .iter()
            .position(|result| result.outcome.is_err())
            .expect("invalid policy must produce a retained failure");
        assert_eq!(
            duty.world
                .results
                .iter()
                .filter(|result| result.outcome.is_ok())
                .count(),
            1,
            "healthy supervision must progress past partial admission failure"
        );
        let failure = duty
            .world
            .results
            .swap_remove(failed)
            .outcome
            .expect_err("failure remains owned");
        assert_eq!(failure.cause, SupervisionCause::InvalidPolicy);
        duty.world.failures -= 1;
        let mut machine = failure.collector;
        machine.replenish(DEFAULT_SUPERVISION_POLICY);
        duty.pending = Some(SuperviseTask::from_collector(machine));
        duty.error = None;
        duty.disposition = Disposition::default();
        self.await_batch(index)?;
        debug!("root supervision failure isolation passed");
        Ok(())
    }

    /// 先进入根槽，再尝试建立执行存储；任何失败仍由根持有 tunnel 与 buffer。
    pub(super) fn start_read(&mut self, tunnel: blocking::Consumer, buffer: Vec<u8>) {
        assert!(self.read.is_none(), "read duty must not be overwritten");
        self.read = Some(ReadDuty {
            pending: Some(InitTask::StreamRead(StreamReadTask {
                source: None,
                arm_failed: None,
                stopping: false,
                removing: false,
            })),
            runtime: None,
            world: ReadWorld {
                tunnel: Some(tunnel),
                buffer,
                filled: 0,
                failed: false,
            },
            disposition: Disposition::default(),
            error: None,
            complete: false,
        });
    }
    pub(super) fn read_world(&mut self) -> &mut ReadWorld {
        &mut self.read.as_mut().expect("read duty remains owned").world
    }
    pub(super) fn await_read(&mut self) -> Result<(), &'static str> {
        loop {
            self.turn_and_wait(Some(Await::Read))
                .map_err(|_| "root supervision wait failed")?;
            let duty = self.read.as_ref().expect("read duty remains owned");
            if duty.error.is_some() || duty.world.failed {
                return Err("stream read failed");
            }
            if duty.complete {
                return Ok(());
            }
        }
    }
    pub(super) fn close_read(&mut self) -> Result<(), &'static str> {
        let duty = self.read.as_mut().expect("read duty remains owned");
        if let Some(tunnel) = duty.world.tunnel.take()
            && let Err((tunnel, error)) = tunnel.close()
        {
            duty.world.tunnel = Some(tunnel);
            duty.error = Some(SystemCallError::InternalError);
            debug!("root stream close failed: {:?}", error);
            return Err("stream close failed");
        }
        close_runtime(&mut duty.runtime).map_err(|_| "stream runtime close failed")?;
        self.read = None;
        Ok(())
    }

    fn prepare_job(duty: &mut JobDuty) -> Result<(), SystemCallError> {
        if duty.pending.is_none() && duty.runtime.is_none() {
            duty.pending = Some(JobTask {
                driver: JobDriver::new(JobCollector::new(
                    duty.root,
                    duty.code,
                    DEFAULT_SUPERVISION_POLICY,
                )?),
            });
        }
        if duty.runtime.is_none() {
            duty.runtime = Some(runtime(1, 1)?);
        }
        let rt = duty.runtime.as_mut().expect("job runtime remains owned");
        if let Some(task) = duty.pending.take()
            && let Err(failure) = rt.spawn(task, 1)
        {
            duty.pending = Some(failure.task);
            return Err(failure.error);
        }
        Ok(())
    }

    fn step_job(duty: &mut JobDuty, now: u64, failing: bool) -> Result<(), SystemCallError> {
        if !duty.active || duty.complete || !duty.disposition.ready(now) {
            return Ok(());
        }
        Self::prepare_job(duty)?;
        let rt = duty.runtime.as_mut().expect("job runtime remains owned");
        if failing && let Some(id) = duty.failed_task {
            let retry = duty.world.failure.is_some_and(|error| match error.cause {
                libprocess::JobKillCause::Timeout => true,
                libprocess::JobKillCause::Process(cause) => process_retry(cause),
                libprocess::JobKillCause::System(error) => recoverable(error),
                _ => false,
            });
            if retry {
                rt.get_task_mut(id)
                    .expect("failed job task remains owned")
                    .driver
                    .replenish(DEFAULT_SUPERVISION_POLICY);
                rt.defer_failed_task(id, Some(later(now)))?;
                duty.failed_task = None;
            }
        }
        if can_drive(rt, now)
            && let Err(failure) = rt.turn(&mut duty.world, 1)
        {
            isolate::<_, JobWorld>(rt, failure, now);
            if failure.task != 0 {
                duty.failed_task = Some(failure.task);
            }
            duty.error = Some(failure.error);
            if failure.task == 0 {
                duty.disposition.fail(failure.error, now);
            }
        }
        if rt.drive_state() == DriveState::Drained && duty.world.done {
            close_runtime(&mut duty.runtime)?;
            duty.complete = true;
        }
        Ok(())
    }

    fn step_batch(duty: &mut BatchDuty, now: u64, failing: bool) -> Result<(), SystemCallError> {
        if duty.complete || !duty.disposition.ready(now) {
            return Ok(());
        }
        if duty.runtime.is_none() {
            duty.world
                .results
                .try_reserve_exact(duty.capacity)
                .map_err(|_| SystemCallError::OutOfMemory)?;
            duty.runtime = Some(runtime(duty.capacity, duty.capacity)?);
        }
        let rt = duty.runtime.as_mut().expect("batch runtime remains owned");
        if duty.pending.is_none()
            && let Some(target) = duty.one.take().or_else(|| duty.incoming.pop())
        {
            duty.pending = Some(SuperviseTask::new(
                SupervisionTarget::new(target.pid, target.control),
                DEFAULT_SUPERVISION_POLICY,
            ));
        }
        if duty.admission_retry.is_none_or(|at| now >= at)
            && let Some(task) = duty.pending.take()
        {
            match rt.spawn(task, 1) {
                Ok(_) => duty.admission_retry = None,
                Err(failure) => {
                    duty.pending = Some(failure.task);
                    duty.error.get_or_insert(failure.error);
                    duty.admission_retry = Some(later(now));
                }
            }
        }
        if can_drive(rt, now)
            && let Err(failure) = rt.turn(&mut duty.world, 1)
        {
            isolate::<_, SupervisorWorld>(rt, failure, now);
            duty.error = Some(failure.error);
            if failure.task == 0 {
                duty.disposition.fail(failure.error, now);
            }
        }
        if duty.world.failures != 0 {
            duty.error.get_or_insert(SystemCallError::InternalError);
        }
        if rt.drive_state() == DriveState::Drained && duty.pending.is_some() {
            duty.disposition.retry = duty.admission_retry;
        }
        if rt.drive_state() == DriveState::Drained
            && duty.incoming.is_empty()
            && duty.one.is_none()
            && duty.pending.is_none()
        {
            // 每轮只领取/报告一个结果；失败记录保留完整机器。
            if duty.world.failures != 0 {
                if failing {
                    if duty.result_cursor < duty.world.results.len() {
                        let index = duty.result_cursor;
                        duty.result_cursor += 1;
                        if duty.world.results[index]
                            .outcome
                            .as_ref()
                            .is_err_and(|failure| process_retry(failure.cause))
                        {
                            let failure = duty
                                .world
                                .results
                                .swap_remove(index)
                                .outcome
                                .expect_err("selected result retains a collector");
                            duty.world.failures -= 1;
                            let mut machine = failure.collector;
                            machine.replenish(DEFAULT_SUPERVISION_POLICY);
                            duty.pending = Some(SuperviseTask::from_collector(machine));
                            duty.result_cursor = 0;
                            duty.disposition.retry = Some(later(now));
                        }
                    } else {
                        duty.disposition.held = true;
                        debug!(
                            "root retained {} failed Process operation(s)",
                            duty.world.failures
                        );
                    }
                }
            } else if duty.result_cursor < duty.world.results.len() {
                let done = duty.world.results[duty.result_cursor]
                    .outcome
                    .as_ref()
                    .expect("all results succeeded");
                debug!(
                    "pid {} supervised: work={}, state={}, reason={}, code={}",
                    done.pid,
                    done.progress.work_done,
                    done.snapshot.state,
                    done.snapshot.reason,
                    done.snapshot.code
                );
                duty.result_cursor += 1;
            } else {
                close_runtime(&mut duty.runtime)?;
                duty.complete = true;
            }
        }

        Ok(())
    }

    fn step_read(duty: &mut ReadDuty, now: u64, failing: bool) -> Result<(), SystemCallError> {
        if !duty.disposition.ready(now) {
            return Ok(());
        }
        if duty.complete {
            return Ok(());
        }
        if duty.world.buffer.is_empty() && !failing {
            duty.world
                .buffer
                .try_reserve_exact(STREAM_LEN + 1)
                .map_err(|_| SystemCallError::OutOfMemory)?;
            duty.world.buffer.resize(STREAM_LEN + 1, 0);
        }
        if duty.runtime.is_none() {
            duty.runtime = Some(runtime(1, 2)?);
        }
        let rt = duty.runtime.as_mut().expect("read runtime remains owned");
        if let Some(task) = duty.pending.take()
            && let Err(failure) = rt.spawn(task, 2)
        {
            duty.pending = Some(failure.task);
            return Err(failure.error);
        }
        let result = if failing {
            rt.seal();
            rt.shutdown_turn(&mut duty.world, 1)
        } else if can_drive(rt, now) {
            rt.turn(&mut duty.world, 1)
        } else {
            Ok(0)
        };
        if let Err(failure) = result {
            isolate::<_, ReadWorld>(rt, failure, now);
            duty.error = Some(failure.error);
            if failure.task == 0 {
                duty.disposition.fail(failure.error, now);
            }
        }
        if rt.drive_state() == DriveState::Drained {
            close_runtime(&mut duty.runtime)?;
            duty.complete = true;
        }
        Ok(())
    }

    fn turn_and_wait(&mut self, awaited: Option<Await>) -> Result<(), SystemCallError> {
        let now = rinlib::time::snapshot()?.now_ns;
        // 五个运行体槽持续轮转；每轮各至多一个工作单位。
        for _ in 0..5 {
            let index = self.cursor;
            self.cursor = (self.cursor + 1) % 5;
            match index {
                0 | 1 => {
                    if let Some(duty) = &mut self.jobs[index]
                        && let Err(error) = Self::step_job(duty, now, self.failing)
                    {
                        duty.error = Some(error);
                        duty.disposition.fail(error, now);
                    }
                }
                2 | 3 => {
                    if let Some(duty) = &mut self.batches[index - 2]
                        && let Err(error) = Self::step_batch(duty, now, self.failing)
                    {
                        duty.error = Some(error);
                        duty.disposition.fail(error, now);
                    }
                }
                _ => {
                    if let Some(duty) = &mut self.read
                        && let Err(error) = Self::step_read(duty, now, self.failing)
                    {
                        duty.error = Some(error);
                        duty.disposition.fail(error, now);
                    }
                }
            }
        }
        self.endpoint_cleanup
            .step(now, rinlib::ipc::tunnel::Endpoint::close);
        self.capability_cleanup.step(now, Capability::close);
        if self.failing {
            for index in 0..ROOT_JOB_SLOTS {
                if self.jobs[index].as_ref().is_some_and(|duty| duty.complete) {
                    self.jobs[index] = None;
                }
            }
            for duty in &mut self.batches {
                if duty.as_ref().is_some_and(|duty| duty.complete) {
                    *duty = None;
                }
            }
        }
        if self.failing
            && self
                .read
                .as_ref()
                .is_some_and(|d| d.complete && !d.disposition.held)
            && self.close_read().is_err()
            && let Some(duty) = &mut self.read
        {
            duty.disposition.held = true;
        }
        if !self.capabilities.is_empty() {
            let index = self.capability_cursor % self.capabilities.len();
            self.capability_cursor = (index + 1) % self.capabilities.len();
            let entry = &mut self.capabilities[index];
            let borrowed = entry.owner.as_ref().is_some_and(|owner| {
                self.jobs
                    .iter()
                    .flatten()
                    .any(|duty| duty.root == owner.handle())
            });
            if (entry.closing || self.failing)
                && !borrowed
                && entry.disposition.ready(now)
                && let Some(owner) = entry.owner.take()
            {
                let handle = owner.handle();
                match owner.close() {
                    Ok(()) => {
                        entry.reserved = false;
                        entry.closing = false;
                        if self.services == Some(handle) {
                            self.services = None;
                        }
                    }
                    Err((owner, error)) => {
                        entry.owner = Some(owner);
                        entry.disposition.fail(error, now);
                    }
                }
            }
        }
        if self.failing
            && !self.reset_attempted
            && self.jobs.iter().all(Option::is_none)
            && self.batches.iter().all(Option::is_none)
            && self.read.is_none()
            && self.processes.is_empty()
            && self.endpoint_cleanup.owner.is_none()
            && self.capability_cleanup.owner.is_none()
            && self.capabilities.iter().all(|entry| entry.owner.is_none())
        {
            self.reset_attempted = true;
            debug!("failure-path services collection completed");
            if let Some(reset) = env::startup_handle(initial::SYSTEM_RESET) {
                match system::reset(reset, ResetAction::Shutdown, ResetReason::SystemFailure) {
                    Ok(never) => match never {},
                    Err(error) => debug!("failure shutdown rejected: {:?}", error),
                }
            }
        }
        let mut items =
            [WaitItem::new(self.idle.handle(), ObjectSignals::READABLE, 0); ROOT_WAIT_TARGETS];
        let mut count = 1;
        let mut deadline = None;
        let mut runnable = self.endpoint_cleanup.runnable() || self.capability_cleanup.runnable();
        for at in [
            self.endpoint_cleanup.disposition.retry,
            self.capability_cleanup.disposition.retry,
        ]
        .into_iter()
        .flatten()
        {
            deadline = Some(deadline.map_or(at, |old: u64| old.min(at)));
        }
        for entry in &self.capabilities {
            if !(entry.closing || self.failing) || entry.owner.is_none() {
                continue;
            }
            if let Some(at) = entry.disposition.retry {
                deadline = Some(deadline.map_or(at, |old: u64| old.min(at)));
            }
            let borrowed = entry.owner.as_ref().is_some_and(|owner| {
                self.jobs
                    .iter()
                    .flatten()
                    .any(|duty| duty.root == owner.handle())
            });
            if !borrowed && !entry.disposition.held && entry.disposition.retry.is_none() {
                runnable = true;
            }
        }
        macro_rules! waiting {
            ($duty:expr, $cookie:expr) => {
                if let Some(duty) = $duty {
                    if let Some(at) = duty.disposition.retry {
                        deadline = Some(deadline.map_or(at, |old: u64| old.min(at)));
                    }
                    if !duty.disposition.held
                        && duty.disposition.retry.is_none()
                        && !duty.complete
                        && let Some(rt) = &duty.runtime
                    {
                        match rt.drive_state() {
                            DriveState::Runnable => runnable = true,
                            DriveState::Waiting(at) => {
                                if let Some(at) = at.instant().expect("runtime deadline validated")
                                {
                                    deadline = Some(deadline.map_or(at, |old: u64| old.min(at)));
                                }
                                items[count] = rt.wait_item($cookie);
                                count += 1;
                            }
                            DriveState::Drained => runnable = true,
                        }
                    }
                }
            };
        }
        waiting!(self.jobs[0].as_ref().filter(|duty| duty.active), 1);
        waiting!(self.jobs[1].as_ref().filter(|duty| duty.active), 2);
        for duty in self.batches.iter().flatten() {
            if let Some(at) = duty.admission_retry {
                deadline = Some(deadline.map_or(at, |old: u64| old.min(at)));
            }
        }
        waiting!(self.batches[0].as_ref(), 3);
        waiting!(self.batches[1].as_ref(), 4);
        waiting!(self.read.as_ref(), 5);
        // 只向正在等待的脚本阶段返回结果；其他已完成槽不妨碍组合停驻。
        let outcome = match awaited {
            Some(Await::Job(index)) => self.jobs[index]
                .as_ref()
                .is_some_and(|d| d.complete || d.error.is_some()),
            Some(Await::Batch(index)) => self.batches[index]
                .as_ref()
                .is_some_and(|d| d.complete || d.error.is_some()),
            Some(Await::Read) => self
                .read
                .as_ref()
                .is_some_and(|d| d.complete || d.error.is_some() || d.world.failed),
            None => false,
        };
        if runnable || outcome {
            return Ok(());
        }
        let result = rinlib::ipc::wait::wait_until(
            &items[..count],
            deadline.map_or(Deadline::INFINITE, Deadline::at),
        )?;
        match result.cookie {
            1 | 2 => {
                if let Some(rt) = self.jobs[result.cookie as usize - 1]
                    .as_mut()
                    .and_then(|d| d.runtime.as_mut())
                {
                    rt.notified();
                }
            }
            3 | 4 => {
                if let Some(rt) = self.batches[result.cookie as usize - 3]
                    .as_mut()
                    .and_then(|d| d.runtime.as_mut())
                {
                    rt.notified();
                }
            }
            5 => {
                if let Some(rt) = self.read.as_mut().and_then(|d| d.runtime.as_mut()) {
                    rt.notified();
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn serve_forever(self) -> ! {
        self.serve(true)
    }
    pub(super) fn idle_forever(self) -> ! {
        self.serve(false)
    }

    fn serve(mut self, failing: bool) -> ! {
        self.failing = failing;
        if !self.processes.is_empty() {
            let incoming = core::mem::take(&mut self.processes);
            self.begin_batch(incoming, None);
        }
        if failing && let Some(services) = self.services {
            self.begin_job(services, 0x1F, None);
        }
        debug!("init: persistent root supervisor owns all pending duties");
        loop {
            if let Err(error) = self.turn_and_wait(None) {
                debug!("root supervisor infrastructure failure: {:?}", error);
                if let Ok(now) = rinlib::time::snapshot() {
                    let _ = rinlib::time::sleep_until(Deadline::at(later(now.now_ns)));
                }
            }
        }
    }
}
