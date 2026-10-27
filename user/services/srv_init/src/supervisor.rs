//! init 的长期责任 owner。脚本只借用；阶段失败不销毁运行体、世界或未交付结果。
//! Job 与 Batch 各两槽承接原操作及 services 兜底；Read 保留单一数据面阶段。
//! 等待目标来自这些责任及空闲集合，由持久来源与 Assembly 统一登记、接收。

use super::*;
use alloc::vec::Vec;
use libbudget::AccountView;
use libexecution::TaskSubmission;
use libexecution::assembly::Assembly;
use libexecution::runtime::{DriveState, TaskFailure};
use libkernel::shared::time::Deadline;
use libprocess::{Collector, job_driver::JobDriver};

mod funding;

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
        requests: &mut Requests<'_, Self>,
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
    funding: Option<AccountView<ExecutionResource>>,
    active: bool,
    root: Handle,
    pending: Option<JobTask>,
    runtime: Option<Runtime<JobTask, WaitSet>>,
    world: JobWorld,
    disposition: Disposition,
    error: Option<SystemCallError>,
    complete: bool,
    failed_task: Option<u64>,
}
struct BatchDuty {
    funding: Option<AccountView<ExecutionResource>>,
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
    funding: Option<AccountView<ExecutionResource>>,
    pending: Vec<InitTask>,
    client_count: usize,
    client_sources: usize,
    rpc_sources: usize,
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
    NotificationOwner(notification::NotificationOwner),
    NotificationSignaler(notification::NotificationSignaler),
}
impl RootCapability {
    fn handle(&self) -> Handle {
        match self {
            Self::Plain(owner) => owner.as_handle(),
            Self::Sender(owner) => owner.as_handle(),
            Self::NotificationOwner(owner) => owner.as_handle(),
            Self::NotificationSignaler(owner) => owner.as_handle(),
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
            Self::NotificationOwner(owner) => {
                owner.into_raw();
            }
            Self::NotificationSignaler(owner) => {
                owner.into_raw();
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
            Self::NotificationOwner(owner) => owner
                .close()
                .map_err(|(owner, error)| (Self::NotificationOwner(owner), error)),
            Self::NotificationSignaler(owner) => owner
                .close()
                .map_err(|(owner, error)| (Self::NotificationSignaler(owner), error)),
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
    pub(super) drivers: Option<super::driver::Manager>,
    pub(super) devices: Option<super::device::Manager>,
    pub(super) device_observer: Option<super::device_supervision::Observer>,
    assembly: Assembly<WaitSet>,
    capabilities: Vec<CapabilityEntry>,
    capability_cursor: usize,
    endpoint_cleanup: Cleanup<librunnel::MappedEndpoint>,
    endpoint_create_cleanup: Cleanup<libkernel::ipc::tunnel::Endpoint>,
    capability_cleanup: Cleanup<Capability>,
    services: Option<Handle>,
    jobs: [Option<JobDuty>; ROOT_JOB_SLOTS],
    batches: [Option<BatchDuty>; 2],
    read: Option<ReadDuty>,
    pub(super) processes: Vec<Supervised>,
    cursor: usize,
    failing: bool,
    reset_attempted: bool,
    reset: Option<Handle>,
}

#[derive(Clone, Copy)]
enum Await {
    Job(usize),
    Batch(usize),
    Read,
    ClientSnapshot,
    ClientCycle(u64),
    CopyProgress(usize, u64),
    CopySent(usize, bool),
    RpcParked,
    RpcReply,
    LateReply,
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

fn fold_wait<T>(
    rt: Option<&Runtime<T, WaitSet>>,
    disposition: &Disposition,
    complete: bool,
) -> (bool, Option<u64>) {
    if disposition.held || disposition.retry.is_some() || complete {
        return (false, disposition.retry);
    }
    match rt.map(Runtime::drive_state) {
        Some(DriveState::Runnable | DriveState::Drained) => (true, None),
        Some(DriveState::Waiting(deadline)) => (
            false,
            deadline.instant().expect("runtime deadline validated"),
        ),
        None => (false, None),
    }
}

/// duty 在实际提交出生前准备一次付款；失败重试继续借用原 view。
fn prepare_funding<T>(
    slot: &mut Option<AccountView<ExecutionResource>>,
    capacity: usize,
    sources: usize,
) -> Result<AccountView<ExecutionResource>, SystemCallError> {
    if let Some(funding) = slot {
        return Ok(funding.clone());
    }
    let bytes = Runtime::<T, WaitSet>::fixed_input_budget(sources)?
        .checked_add(Runtime::<T, WaitSet>::source_input_budget(sources)?)
        .ok_or(SystemCallError::ReachLimit)?;
    let budget = Budget::new(&[capacity, bytes], 1)?;
    let account = budget.account(&[capacity, bytes])?;
    let account = account.view::<ExecutionResource>(&[
        budget.slot(0).ok_or(SystemCallError::IllegalArgument)?,
        budget.slot(1).ok_or(SystemCallError::IllegalArgument)?,
    ])?;
    *slot = Some(account.clone());
    Ok(account)
}
fn runtime<T>(
    assembly: &Assembly<WaitSet>,
    capacity: usize,
    sources: usize,
    funding: &AccountView<ExecutionResource>,
) -> Result<Runtime<T, WaitSet>, SystemCallError> {
    Runtime::new(assembly.bind()?, capacity, sources, funding)
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
    pub(super) fn prepare(reset: &mut Option<Handle>) -> Result<Self, SystemCallError> {
        let bytes = Assembly::<WaitSet>::input_budget()?;
        let budget = Budget::new(&[0, bytes], 1)?;
        let account = budget
            .account(&[0, bytes])?
            .view::<ExecutionResource>(&[budget.slot(0).unwrap(), budget.slot(1).unwrap()])?;
        // query/members 的 ABI 表示为 u32；只取表示上界，运行体额度承担实际政策。
        let set = WaitSet::create(u32::MAX as usize)?;
        let assembly = Assembly::new(set, &account).map_err(|(set, error)| {
            drop(set);
            error
        })?;
        Ok(Self {
            drivers: None,
            devices: None,
            device_observer: None,
            assembly,
            capabilities: Vec::new(),
            capability_cursor: 0,
            endpoint_cleanup: Cleanup::empty(),
            endpoint_create_cleanup: Cleanup::empty(),
            capability_cleanup: Cleanup::empty(),
            services: None,
            jobs: [None, None],
            batches: [None, None],
            read: None,
            processes: Vec::new(),
            cursor: 0,
            failing: false,
            reset_attempted: false,
            reset: reset.take(),
        })
    }

    pub(super) fn reset_authority(&self) -> Option<Handle> {
        self.reset
    }
    pub(super) fn take_reset(&mut self) -> Handle {
        self.reset
            .take()
            .expect("root supervisor reset authority missing")
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
        pool: Handle,
        rights: Rights,
    ) -> Result<Handle, SystemCallError> {
        let [slot] = self.reserve_capabilities()?;
        match process::create_job(parent, pool, rights) {
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
    pub(super) fn derive_pool(
        &mut self,
        parent: Handle,
        budget: libkernel::shared::memory_pool::MemoryPoolBudget,
        rights: Rights,
    ) -> Result<Handle, SystemCallError> {
        let [slot] = self.reserve_capabilities()?;
        match libkernel::memory_pool::derive(parent, budget, rights) {
            Ok(child) => {
                let child = child.into_handle();
                self.adopt(slot, child);
                Ok(child)
            }
            Err(error) => {
                self.capabilities[slot].reserved = false;
                Err(error)
            }
        }
    }
    fn create_pair(
        &mut self,
        create: impl FnOnce() -> Result<libkernel::shared::object::HandlePair, SystemCallError>,
    ) -> Result<libkernel::shared::object::HandlePair, SystemCallError> {
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
    pub(super) fn create_mailbox_owner(
        &mut self,
        rights: Rights,
    ) -> Result<Handle, SystemCallError> {
        let [slot] = self.reserve_capabilities()?;
        match Mailbox::create(rights) {
            Ok(mailbox) => {
                let owner = mailbox.into_raw();
                self.adopt(slot, owner);
                Ok(owner)
            }
            Err(error) => {
                self.capabilities[slot].reserved = false;
                Err(error)
            }
        }
    }

    pub(super) fn create_mailbox(
        &mut self,
        owner: Rights,
        sender: Rights,
    ) -> Result<libkernel::shared::object::HandlePair, SystemCallError> {
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
    ) -> Result<libkernel::shared::object::HandlePair, SystemCallError> {
        let slots: [usize; 2] = self.reserve_capabilities()?;
        match notification::create(owner, signaler) {
            Ok(pair) => {
                let handles = libkernel::shared::object::HandlePair::new(
                    pair.owner.as_handle(),
                    pair.peer.as_handle(),
                );
                self.capabilities[slots[0]].reserved = false;
                self.capabilities[slots[0]].owner =
                    Some(RootCapability::NotificationOwner(pair.owner));
                self.capabilities[slots[1]].reserved = false;
                self.capabilities[slots[1]].owner =
                    Some(RootCapability::NotificationSignaler(pair.peer));
                Ok(handles)
            }
            Err(error) => {
                for slot in slots {
                    self.capabilities[slot].reserved = false;
                }
                Err(error)
            }
        }
    }
    pub(super) fn notification_owner(&self, handle: Handle) -> &notification::NotificationOwner {
        self.capabilities
            .iter()
            .find_map(|entry| match &entry.owner {
                Some(RootCapability::NotificationOwner(owner)) if owner.as_handle() == handle => {
                    Some(owner)
                }
                _ => None,
            })
            .expect("notification owner remains in the root ledger")
    }
    pub(super) fn notification_signaler(
        &self,
        handle: Handle,
    ) -> &notification::NotificationSignaler {
        self.capabilities
            .iter()
            .find_map(|entry| match &entry.owner {
                Some(RootCapability::NotificationSignaler(owner))
                    if owner.as_handle() == handle =>
                {
                    Some(owner)
                }
                _ => None,
            })
            .expect("notification signaler remains in the root ledger")
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
        let index = self.begin_job(services, None);
        let duty = self.jobs[index]
            .as_mut()
            .expect("standby duty remains owned");
        duty.active = false;
        // 服务尚未启动时完成根 frame、运行体和任务准入；兜底激活不重新分配承载。
        Self::prepare_job(duty, &self.assembly)
    }
    pub(super) fn reserve_process(&mut self) -> Result<(), SystemCallError> {
        self.processes
            .try_reserve(1)
            .map_err(|_| SystemCallError::OutOfMemory)
    }
    pub(super) fn track_process(&mut self, target: Supervised) {
        self.processes.push(target);
    }

    fn begin_job(&mut self, root: Handle, machine: Option<JobCollector>) -> usize {
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
            funding: None,
            active: true,
            root,
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

    pub(super) fn collect_job(&mut self, root: Handle) -> Result<(), &'static str> {
        let index = self.begin_job(root, None);
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
        endpoint: librunnel::MappedEndpoint,
        invitation: libkernel::ipc::invitation::Invitation,
    ) {
        assert!(
            self.endpoint_cleanup.owner.is_none(),
            "stream setup has one endpoint"
        );
        self.endpoint_cleanup.owner = Some(endpoint);
        self.retain_capability(invitation.into_capability());
    }
    pub(super) fn retain_failed_create(
        &mut self,
        endpoint: Option<libkernel::ipc::tunnel::Endpoint>,
        invitation: Option<libkernel::ipc::invitation::Invitation>,
    ) {
        assert!(
            self.endpoint_cleanup.owner.is_none() && self.endpoint_create_cleanup.owner.is_none(),
            "stream setup has one endpoint"
        );
        self.endpoint_create_cleanup.owner = endpoint;
        if let Some(invitation) = invitation {
            self.retain_capability(invitation.into_capability());
        }
    }

    fn begin_batch(&mut self, incoming: Vec<Supervised>, one: Option<Supervised>) -> usize {
        let index = self
            .batches
            .iter()
            .position(Option::is_none)
            .expect("batch slots cover the original batch and service handoff");
        let capacity = incoming.len() + usize::from(one.is_some());
        self.batches[index] = Some(BatchDuty {
            funding: None,
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
    pub(super) fn verify_failure_isolation(
        &mut self,
        parent: Handle,
        completed: [u64; 2],
    ) -> Result<(), &'static str> {
        let child = self
            .create_job(parent, crate::root_memory_pool(), JOB_FULL_RIGHTS)
            .map_err(|_| "root isolation Job create failed")?;
        let index = self.begin_job(child, None);
        let duty = self.jobs[index]
            .as_mut()
            .expect("isolation duty remains owned");
        let mut machine = JobCollector::new(child, DEFAULT_SUPERVISION_POLICY)
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
        self.verify_batch_handoff(completed)?;
        let now = libkernel::time::snapshot()
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
        self.collect_job(child)?;
        self.close_control(child)?;
        debug!("root independent runtime failure isolation passed");
        Ok(())
    }

    /// 使用正式运行体验证部分准入、失败结果和健康成员的独立推进。
    pub(super) fn verify_batch_handoff(&mut self, completed: [u64; 2]) -> Result<(), &'static str> {
        if completed[0] == completed[1]
            || completed
                .iter()
                .any(|pid| !self.processes.iter().any(|target| target.pid == *pid))
        {
            return Err("supervision handoff requires two distinct retained service controls");
        }
        let mut incoming = Vec::new();
        incoming
            .try_reserve_exact(2)
            .map_err(|_| "handoff input preparation failed")?;
        let mut results = Vec::new();
        results
            .try_reserve_exact(2)
            .map_err(|_| "handoff output preparation failed")?;
        let mut test_funding = None;
        let test_view = prepare_funding::<SuperviseTask>(&mut test_funding, 1, 2)
            .map_err(|_| "handoff funding preparation failed")?;
        let test_runtime = runtime(&self.assembly, 1, 2, &test_view)
            .map_err(|_| "handoff runtime preparation failed")?;
        // 具体业务owner已经授权收集；不依赖ledger尾部顺序，也不接管仍持硬件责任的管理者。
        for pid in completed {
            let index = self
                .processes
                .iter()
                .position(|target| target.pid == pid)
                .expect("handoff control was verified");
            incoming.push(self.processes.swap_remove(index));
        }
        let index = self.begin_batch(incoming, None);
        let duty = self.batches[index]
            .as_mut()
            .expect("handoff batch remains owned");
        duty.runtime = Some(test_runtime);
        duty.funding = test_funding;
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
            funding: None,
            pending: alloc::vec![InitTask::StreamRead(StreamReadTask {
                source: None,
                arm_failed: None,
                stopping: false,
                removing: false,
            })],
            client_count: 1,
            client_sources: 2,
            rpc_sources: 0,
            runtime: None,
            world: ReadWorld {
                tunnel: Some(tunnel),
                buffer,
                filled: 0,
                failed: false,
                ..ReadWorld::default()
            },
            disposition: Disposition::default(),
            error: None,
            complete: false,
        });
    }
    fn start_client(
        &mut self,
        pending: InitTask,
        inflight: usize,
        client_sources: usize,
    ) -> Result<(), &'static str> {
        let mut clients = Vec::new();
        clients
            .try_reserve_exact(1)
            .map_err(|_| "client preparation failed")?;
        clients.push(pending);
        self.start_clients(
            clients,
            inflight,
            client_sources,
            ReadWorld::default(),
            None,
        )
    }
    fn start_clients(
        &mut self,
        pending: Vec<InitTask>,
        inflight: usize,
        client_sources: usize,
        mut world: ReadWorld,
        mut funding: Option<AccountView<ExecutionResource>>,
    ) -> Result<(), &'static str> {
        assert!(self.read.is_none(), "client duty must not be overwritten");
        let rpc_sources = librpc::dispatcher::Dispatcher::source_capacity(inflight)
            .map_err(|_| "client source capacity invalid")?;
        let client_count = pending.len();
        let sources = rpc_sources
            .checked_add(
                client_sources
                    .checked_mul(client_count)
                    .ok_or("client source capacity overflow")?,
            )
            .ok_or("client source capacity overflow")?;
        let view = prepare_funding::<InitTask>(&mut funding, client_count + 1, sources)
            .map_err(|_| "client funding preparation failed")?;
        let rpc = librpc::dispatcher::Dispatcher::new(inflight, &view)
            .map_err(|_| "client transport preparation failed")?;
        world
            .client_tasks
            .try_reserve_exact(client_count)
            .map_err(|_| "client identity storage failed")?;
        world.rpc = Some(rpc);
        self.read = Some(ReadDuty {
            funding,
            pending,
            client_count,
            client_sources,
            rpc_sources,
            runtime: None,
            world,
            disposition: Disposition::default(),
            error: None,
            complete: false,
        });
        Ok(())
    }
    pub(super) fn start_copies(
        &mut self,
        copies: Vec<libfs::client::copy::CopyTask>,
    ) -> Result<(), &'static str> {
        use libfs::client::copy::CopyTask;
        let count = copies.len();
        if count == 0 {
            return Err("Copy batch is empty");
        }
        let mut world = ReadWorld::default();
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(count)
            .map_err(|_| "Copy task storage failed")?;
        world
            .copies
            .try_reserve_exact(count)
            .map_err(|_| "Copy observation storage failed")?;
        for (slot, task) in copies.into_iter().enumerate() {
            world.copies.push(crate::CopyObservation {
                outcome: None,
                progress: task.progress(),
                done: false,
                hold: false,
            });
            pending.push(InitTask::Copy {
                task: alloc::boxed::Box::try_new(task)
                    .map_err(|_| "Copy task preparation failed")?,
                slot,
            });
        }
        self.start_clients(pending, count, CopyTask::source_capacity(), world, None)
    }
    pub(super) fn await_copy_progress(
        &mut self,
        slot: usize,
        transported: u64,
    ) -> Result<(), &'static str> {
        loop {
            self.turn_and_wait(Some(Await::CopyProgress(slot, transported)))
                .map_err(|_| "Copy progress wait failed")?;
            if self
                .read
                .as_ref()
                .is_some_and(|d| d.error.is_some() || d.world.failed || d.complete)
            {
                return Err("Copy duty stopped before progress");
            }
            let world = self.read_world();
            if matches!(world.copies[slot].progress, libfs::client::copy::Progress::Stream(p) if p.transported >= transported)
            {
                return Ok(());
            }
            if world.copies[slot].outcome.is_some() {
                return Err("Copy ended before requested progress");
            }
        }
    }
    pub(super) fn await_copy_sent(
        &mut self,
        slot: usize,
        creation: bool,
    ) -> Result<(), &'static str> {
        loop {
            self.turn_and_wait(Some(Await::CopySent(slot, creation)))
                .map_err(|_| "Copy send wait failed")?;
            if self
                .read
                .as_ref()
                .is_some_and(|d| d.error.is_some() || d.world.failed || d.complete)
            {
                return Err("Copy duty stopped before send");
            }
            let duty = self.read.as_mut().expect("Copy duty remains owned");
            if let Some(&id) = duty.world.client_tasks.get(slot)
                && let Some(InitTask::Copy { task, .. }) =
                    duty.runtime.as_mut().and_then(|rt| rt.get_task_mut(id))
                && task.call_phase(
                    duty.world
                        .rpc
                        .as_ref()
                        .expect("Copy transport remains owned"),
                ) == Some(librpc::CallPhase::Sent)
                && (!creation || task.creation() == libfs::client::copy::Creation::Unknown)
            {
                return Ok(());
            }
            if duty.world.copies[slot].outcome.is_some() {
                return Err("Copy ended before send window");
            }
        }
    }
    pub(super) fn set_copy_hold(&mut self, slot: usize, hold: bool) -> Result<(), &'static str> {
        let duty = self.read.as_mut().expect("Copy duty remains owned");
        duty.world.copies[slot].hold = hold;
        if let Some(&id) = duty.world.client_tasks.get(slot) {
            duty.runtime
                .as_mut()
                .expect("Copy runtime remains owned")
                .wake(id)
                .map_err(|_| "Copy continuation wake failed")?;
        }
        Ok(())
    }
    pub(super) fn await_copy_late_reply(&mut self) -> Result<(), &'static str> {
        loop {
            self.turn_and_wait(Some(Await::LateReply))
                .map_err(|_| "Copy late reply wait failed")?;
            let duty = self.read.as_ref().expect("Copy duty remains owned");
            if duty.error.is_some() || duty.world.failed || duty.complete {
                return Err("Copy duty stopped before late reply");
            }
            if duty.world.ignored_rpc_replies != 0 {
                return Ok(());
            }
            if duty.world.copies.iter().all(|copy| copy.outcome.is_some()) {
                return Err("Copy ended without observed late reply");
            }
        }
    }
    pub(super) fn cancel_copy(&mut self, slot: usize) -> Result<(), &'static str> {
        let duty = self.read.as_mut().expect("Copy duty remains owned");
        let id = duty.world.client_tasks[slot];
        let runtime = duty.runtime.as_mut().expect("Copy runtime remains owned");
        match runtime
            .get_task_mut(id)
            .expect("Copy task remains admitted")
        {
            InitTask::Copy { task, .. } => task.cancel(
                duty.world
                    .rpc
                    .as_mut()
                    .expect("Copy transport remains owned"),
            ),
            _ => unreachable!("Copy task changed family role"),
        }
        duty.world.copies[slot].hold = false;
        runtime
            .wake(id)
            .map_err(|_| "Copy cancellation wake failed")
    }
    pub(super) fn seal_copy_transport(&mut self) -> Result<(), &'static str> {
        let duty = self.read.as_mut().expect("Copy duty remains owned");
        duty.world.rpc_sealed = true;
        duty.world.rpc_parked = false;
        duty.world
            .rpc
            .as_mut()
            .expect("Copy transport remains owned")
            .stop(&mut ());
        duty.runtime
            .as_mut()
            .expect("Copy runtime remains owned")
            .wake(
                duty.world
                    .rpc_task
                    .expect("Copy transport remains admitted"),
            )
            .map_err(|_| "Copy transport stop wake failed")?;
        loop {
            self.turn_and_wait(Some(Await::RpcParked))
                .map_err(|_| "Copy transport retirement wait failed")?;
            if self
                .read
                .as_ref()
                .is_some_and(|d| d.error.is_some() || d.world.failed || d.complete)
            {
                return Err("Copy transport stopped before retirement barrier");
            }
            if self.read_world().rpc_parked {
                return Ok(());
            }
        }
    }
    pub(super) fn start_fs_watch(
        &mut self,
        watch: libfs::client::Watch,
        deadline: Deadline,
    ) -> Result<(), &'static str> {
        use librpc::subscription::ConsumerTask;
        let task = alloc::boxed::Box::try_new(ConsumerTask::new(watch, deadline))
            .map_err(|_| "file consumer preparation failed")?;
        self.start_client(
            InitTask::FsWatch(task),
            ConsumerTask::<libfs::client::Watch>::inflight_capacity(),
            ConsumerTask::<libfs::client::Watch>::source_capacity(),
        )
    }
    pub(super) fn start_service_watch(
        &mut self,
        watch: libservice::client::Watch,
        deadline: Deadline,
    ) -> Result<(), &'static str> {
        use librpc::subscription::ConsumerTask;
        let task = alloc::boxed::Box::try_new(ConsumerTask::new(watch, deadline))
            .map_err(|_| "service consumer preparation failed")?;
        self.start_client(
            InitTask::ServiceWatch(task),
            ConsumerTask::<libservice::client::Watch>::inflight_capacity(),
            ConsumerTask::<libservice::client::Watch>::source_capacity(),
        )
    }
    pub(super) fn await_client_snapshot(&mut self) -> Result<(), &'static str> {
        loop {
            self.turn_and_wait(Some(Await::ClientSnapshot))
                .map_err(|_| "client supervision wait failed")?;
            let duty = self.read.as_ref().expect("client duty remains owned");
            if duty.error.is_some() || duty.world.failed || duty.complete {
                return Err("client subscription stopped before snapshot");
            }
            if duty.world.fs_snapshot.is_some() || duty.world.service_snapshot.is_some() {
                assert!(
                    duty.world.source_detached,
                    "watch source remained attached during recheck"
                );
                return Ok(());
            }
        }
    }
    pub(super) fn acknowledge_client_snapshot(&mut self) -> Result<(), &'static str> {
        let duty = self.read.as_mut().expect("client duty remains owned");
        let id = duty
            .world
            .client_tasks
            .first()
            .copied()
            .expect("client task remains admitted");
        let runtime = duty.runtime.as_mut().expect("client runtime remains owned");
        match runtime.get_task_mut(id).expect("client task remains owned") {
            InitTask::FsWatch(task) => task.acknowledge_read(),
            InitTask::ServiceWatch(task) => task.acknowledge_read(),
            _ => unreachable!("client task changed family role"),
        }
        .map_err(|_| "client recheck acknowledgement failed")?;
        duty.world.fs_snapshot = None;
        duty.world.service_snapshot = None;
        runtime
            .wake(id)
            .map_err(|_| "client continuation wake failed")
    }
    pub(super) fn await_client_cycle(&mut self, cycle: u64) -> Result<(), &'static str> {
        loop {
            self.turn_and_wait(Some(Await::ClientCycle(cycle)))
                .map_err(|_| "client cycle wait failed")?;
            let duty = self.read.as_ref().expect("client duty remains owned");
            if duty.error.is_some() || duty.world.failed || duty.complete {
                return Err("client subscription stopped before cycle");
            }
            if duty.world.client_cycles >= cycle && duty.world.client_armed {
                return Ok(());
            }
        }
    }
    pub(super) fn stop_client(&mut self) -> Result<(), &'static str> {
        let duty = self.read.as_mut().expect("client duty remains owned");
        let id = duty
            .world
            .client_tasks
            .first()
            .copied()
            .expect("client task remains admitted");
        let rt = duty.runtime.as_mut().expect("client runtime remains owned");
        match rt.get_task_mut(id).expect("client task remains owned") {
            InitTask::FsWatch(task) => task.stop(),
            InitTask::ServiceWatch(task) => task.stop(),
            _ => unreachable!("client task changed family role"),
        }
        rt.wake(id).map_err(|_| "client stop wake failed")?;
        self.await_read()?;
        self.close_read()
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

    fn prepare_job(
        duty: &mut JobDuty,
        assembly: &Assembly<WaitSet>,
    ) -> Result<(), SystemCallError> {
        let funding = prepare_funding::<JobTask>(&mut duty.funding, 1, 1)?;
        if duty.pending.is_none() && duty.runtime.is_none() {
            duty.pending = Some(JobTask {
                driver: JobDriver::new(JobCollector::new(duty.root, DEFAULT_SUPERVISION_POLICY)?),
            });
        }
        if duty.runtime.is_none() {
            duty.runtime = Some(runtime(assembly, 1, 1, &funding)?);
        }
        let rt = duty.runtime.as_mut().expect("job runtime remains owned");
        if let Some(task) = duty.pending.take()
            && let Err(failure) = rt.spawn(TaskSubmission::new(task, 1, funding))
        {
            duty.pending = Some(failure.submission.into_parts().0);
            return Err(failure.error);
        }
        Ok(())
    }

    fn step_job(
        duty: &mut JobDuty,
        now: u64,
        failing: bool,
        assembly: &Assembly<WaitSet>,
    ) -> Result<(), SystemCallError> {
        if !duty.active || duty.complete || !duty.disposition.ready(now) {
            return Ok(());
        }
        Self::prepare_job(duty, assembly)?;
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

    fn step_batch(
        duty: &mut BatchDuty,
        now: u64,
        failing: bool,
        assembly: &Assembly<WaitSet>,
    ) -> Result<(), SystemCallError> {
        if duty.complete || !duty.disposition.ready(now) {
            return Ok(());
        }
        let funding =
            prepare_funding::<SuperviseTask>(&mut duty.funding, duty.capacity, duty.capacity)?;
        if duty.runtime.is_none() {
            duty.world
                .results
                .try_reserve_exact(duty.capacity)
                .map_err(|_| SystemCallError::OutOfMemory)?;
            duty.runtime = Some(runtime(assembly, duty.capacity, duty.capacity, &funding)?);
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
            match rt.spawn(TaskSubmission::new(task, 1, funding)) {
                Ok(_) => duty.admission_retry = None,
                Err(failure) => {
                    duty.pending = Some(failure.submission.into_parts().0);
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
                    "pid {} supervised: queries={}, state={}, reason={}, detail0={}",
                    done.pid,
                    done.progress.query_attempts,
                    done.snapshot.state,
                    done.snapshot.cause,
                    done.snapshot.detail.word0
                );
                duty.result_cursor += 1;
            } else {
                close_runtime(&mut duty.runtime)?;
                duty.complete = true;
            }
        }

        Ok(())
    }

    fn abandon_clients(duty: &mut ReadDuty) -> Result<(), SystemCallError> {
        for pending in &mut duty.pending {
            match pending {
                InitTask::FsWatch(task) => task.abandon_unadmitted()?,
                InitTask::ServiceWatch(task) => task.abandon_unadmitted()?,
                InitTask::Copy { task, slot } => {
                    task.abandon_unadmitted()?;
                    duty.world.copies[*slot].done = true;
                    duty.world.copies[*slot].outcome = task.outcome();
                }
                _ => unreachable!("unadmitted client changed family role"),
            }
        }
        duty.pending.clear();
        duty.world.client_quiescent = duty.world.copies.iter().all(|copy| copy.done);
        Ok(())
    }
    fn step_read(
        duty: &mut ReadDuty,
        now: u64,
        failing: bool,
        assembly: &Assembly<WaitSet>,
    ) -> Result<(), SystemCallError> {
        if !duty.disposition.ready(now) {
            return Ok(());
        }
        if duty.complete {
            return Ok(());
        }
        if failing && duty.runtime.is_none() && duty.world.rpc.is_some() {
            Self::abandon_clients(duty)?;
            // 尚无在途调用和来源；原生角色关闭即完成这一未准入运行体的收束。
            drop(duty.world.rpc.take());
            duty.world.client_quiescent = true;
            duty.world.rpc_complete = true;
            duty.complete = true;
            return Ok(());
        }
        if duty.world.rpc.is_none() && duty.world.buffer.is_empty() && !failing {
            duty.world
                .buffer
                .try_reserve_exact(STREAM_LEN + 1)
                .map_err(|_| SystemCallError::OutOfMemory)?;
            duty.world.buffer.resize(STREAM_LEN + 1, 0);
        }
        if duty.world.client_tasks.capacity() < duty.client_count {
            duty.world
                .client_tasks
                .try_reserve_exact(duty.client_count)
                .map_err(|_| SystemCallError::OutOfMemory)?;
        }
        let sources = duty
            .rpc_sources
            .checked_add(
                duty.client_sources
                    .checked_mul(duty.client_count)
                    .ok_or(SystemCallError::ReachLimit)?,
            )
            .ok_or(SystemCallError::ReachLimit)?;
        let funding = prepare_funding::<InitTask>(
            &mut duty.funding,
            duty.client_count + usize::from(duty.world.rpc.is_some()),
            sources,
        )?;
        if duty.runtime.is_none() {
            duty.runtime = Some(runtime(
                assembly,
                duty.client_count + usize::from(duty.world.rpc.is_some()),
                sources,
                &funding,
            )?);
        }
        if failing && !duty.pending.is_empty() && duty.world.rpc.is_some() {
            Self::abandon_clients(duty)?;
            duty.world
                .rpc
                .as_mut()
                .expect("client transport remains owned")
                .stop(&mut ());
        }
        let rt = duty.runtime.as_mut().expect("read runtime remains owned");
        if duty.world.rpc.is_some() && duty.world.rpc_task.is_none() {
            duty.world.rpc_task = Some(
                rt.spawn(TaskSubmission::new(
                    InitTask::Rpc {
                        deadline: Deadline::INFINITE,
                    },
                    duty.rpc_sources,
                    funding.clone(),
                ))
                .map_err(|failure| failure.error)?,
            );
        }
        if !duty.pending.is_empty() {
            let task = duty.pending.remove(0);
            match rt.spawn(TaskSubmission::new(task, duty.client_sources, funding)) {
                Ok(id) => {
                    duty.world.client_tasks.push(id);
                }
                Err(failure) => {
                    duty.pending.insert(0, failure.submission.into_parts().0);
                    return Err(failure.error);
                }
            }
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
        for &record in self.assembly.receive(64)? {
            for duty in self.jobs.iter_mut().flatten() {
                if let Some(rt) = duty.runtime.as_mut() {
                    rt.feed(record);
                }
            }
            for duty in self.batches.iter_mut().flatten() {
                if let Some(rt) = duty.runtime.as_mut() {
                    rt.feed(record);
                }
            }
            if let Some(rt) = self.read.as_mut().and_then(|duty| duty.runtime.as_mut()) {
                rt.feed(record);
            }
        }
        let now = libkernel::time::snapshot()?.now_ns;
        // 五个运行体槽持续轮转；每轮各至多一个工作单位。
        for _ in 0..5 {
            let index = self.cursor;
            self.cursor = (self.cursor + 1) % 5;
            match index {
                0 | 1 => {
                    if let Some(duty) = &mut self.jobs[index]
                        && let Err(error) = Self::step_job(duty, now, self.failing, &self.assembly)
                    {
                        duty.error = Some(error);
                        duty.disposition.fail(error, now);
                    }
                }
                2 | 3 => {
                    if let Some(duty) = &mut self.batches[index - 2]
                        && let Err(error) =
                            Self::step_batch(duty, now, self.failing, &self.assembly)
                    {
                        duty.error = Some(error);
                        duty.disposition.fail(error, now);
                    }
                }
                _ => {
                    if let Some(duty) = &mut self.read
                        && let Err(error) = Self::step_read(duty, now, self.failing, &self.assembly)
                    {
                        duty.error = Some(error);
                        duty.disposition.fail(error, now);
                    }
                }
            }
        }
        self.endpoint_cleanup
            .step(now, librunnel::MappedEndpoint::close);
        self.endpoint_create_cleanup
            .step(now, libkernel::ipc::tunnel::Endpoint::close);
        self.capability_cleanup.step(now, Capability::close);
        if self.failing {
            for duty in &mut self.jobs {
                if duty.as_ref().is_some_and(|duty| duty.complete) {
                    *duty = None;
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
            && self.endpoint_create_cleanup.owner.is_none()
            && self.capability_cleanup.owner.is_none()
            && self.capabilities.iter().all(|entry| entry.owner.is_none())
        {
            self.reset_attempted = true;
            debug!("failure-path services collection completed");
            if let Some(reset) = self.reset.take() {
                match system::reset(reset, ResetAction::Shutdown, ResetReason::SystemFailure) {
                    Ok(never) => match never {},
                    Err(error) => debug!("failure shutdown rejected: {:?}", error),
                }
            }
        }
        let mut deadline = None;
        let mut runnable = self.endpoint_cleanup.runnable()
            || self.endpoint_create_cleanup.runnable()
            || self.capability_cleanup.runnable();
        for at in [
            self.endpoint_cleanup.disposition.retry,
            self.endpoint_create_cleanup.disposition.retry,
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
        let waits = [
            self.jobs[0]
                .as_ref()
                .filter(|duty| duty.active)
                .map(|d| fold_wait(d.runtime.as_ref(), &d.disposition, d.complete)),
            self.jobs[1]
                .as_ref()
                .filter(|duty| duty.active)
                .map(|d| fold_wait(d.runtime.as_ref(), &d.disposition, d.complete)),
            self.batches[0]
                .as_ref()
                .map(|d| fold_wait(d.runtime.as_ref(), &d.disposition, d.complete)),
            self.batches[1]
                .as_ref()
                .map(|d| fold_wait(d.runtime.as_ref(), &d.disposition, d.complete)),
            self.read
                .as_ref()
                .map(|d| fold_wait(d.runtime.as_ref(), &d.disposition, d.complete)),
        ];
        for (ready, at) in waits.into_iter().flatten() {
            runnable |= ready;
            if let Some(at) = at {
                deadline = Some(deadline.map_or(at, |old: u64| old.min(at)));
            }
        }
        for at in self
            .batches
            .iter()
            .flatten()
            .filter_map(|duty| duty.admission_retry)
        {
            deadline = Some(deadline.map_or(at, |old: u64| old.min(at)));
        }
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
            Some(Await::ClientSnapshot) => self.read.as_ref().is_some_and(|d| {
                d.complete
                    || d.error.is_some()
                    || d.world.failed
                    || d.world.fs_snapshot.is_some()
                    || d.world.service_snapshot.is_some()
            }),
            Some(Await::ClientCycle(cycle)) => self.read.as_ref().is_some_and(|d| {
                d.complete
                    || d.error.is_some()
                    || d.world.failed
                    || (d.world.client_cycles >= cycle && d.world.client_armed)
            }),
            Some(Await::CopyProgress(slot, count)) => self.read.as_ref().is_some_and(|d| {
                d.complete || d.error.is_some() || d.world.copies[slot].outcome.is_some()
                    || matches!(d.world.copies[slot].progress, libfs::client::copy::Progress::Stream(p) if p.transported >= count)
            }),
            Some(Await::CopySent(slot, creation)) => self.read.as_mut().is_some_and(|d| {
                d.complete || d.error.is_some() || d.world.copies[slot].outcome.is_some()
                || d.world.client_tasks.get(slot).copied().is_some_and(|id| {
                    matches!(d.runtime.as_mut().and_then(|rt| rt.get_task_mut(id)), Some(InitTask::Copy {task,..})
                        if task.call_phase(d.world.rpc.as_ref().expect("Copy transport remains owned")) == Some(librpc::CallPhase::Sent)
                        && (!creation || task.creation() == libfs::client::copy::Creation::Unknown))
                })
            }),
            Some(Await::RpcParked) => self.read.as_ref().is_some_and(|d| {
                d.complete || d.error.is_some() || (d.world.rpc_sealed && d.world.rpc_parked)
            }),
            Some(Await::RpcReply) => self.read.as_ref().is_some_and(|d| {
                d.complete || d.error.is_some() || d.world.failed
                    || d.world.rpc.as_ref().is_some_and(librpc::dispatcher::Dispatcher::has_completed)
            }),
            Some(Await::LateReply) => self.read.as_ref().is_some_and(|d| {
                d.complete || d.error.is_some() || d.world.ignored_rpc_replies != 0 || d.world.copies.iter().all(|copy| copy.outcome.is_some())
            }),
            None => false,
        };
        if runnable || outcome {
            return Ok(());
        }
        self.assembly
            .wait(deadline.map_or(Deadline::INFINITE, Deadline::at))?;
        Ok(())
    }

    pub(super) fn serve_forever(mut self) -> ! {
        if let Some(observer) = self.device_observer.as_ref() {
            observer.fail("main device assembly failure");
        }
        if let Some(driver) = self.drivers.as_ref() {
            driver.fault_seal();
        }
        self.failing = true;
        if !self.processes.is_empty() {
            let incoming = core::mem::take(&mut self.processes);
            self.begin_batch(incoming, None);
        }
        if let Some(services) = self.services {
            self.begin_job(services, None);
        }
        debug!("init: persistent root supervisor owns all pending duties");
        loop {
            if let Err(error) = self.turn_and_wait(None) {
                debug!("root supervisor infrastructure failure: {:?}", error);
                if let Ok(now) = libkernel::time::snapshot() {
                    let _ = libkernel::time::sleep_until(Deadline::at(later(now.now_ns)));
                }
            }
        }
    }
}
