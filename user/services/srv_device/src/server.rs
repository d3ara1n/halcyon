//! 实际服务的单 actor World、Runtime 任务族与准入配置。
use alloc::boxed::Box;
use erhino_shared::{
    call::SystemCallError,
    message::PAYLOAD_MAX,
    object::{ObjectSignals, Rights},
    time::Deadline,
};
use libbudget::{Budget, Charge, Taxonomy};
use libdevice::{
    Error,
    binding_work::{self, Binding},
    funding::Funding,
    identity::DeviceId,
    registry::{Config, Registry},
    resource::DeviceResource,
};
use libexecution::{
    ExecutionResource, SourceRegistration, TaskSubmission,
    assembly::Assembly,
    runtime::{
        Advance, DriveState, Input, RequestFailure, Requests, Runtime, SourceId, SourcePlan, Step,
        Task,
    },
};
use libkernel::ipc::{
    capability::Capability,
    message::{LifetimeObserver, Mailbox, MailboxSender, MessageStorage, ReceiveBuffer},
    object,
    wait_set::WaitSet,
};
use librpc::dispatcher::Dispatcher;
use ordered_table::{OrderedTable, PreparedEntry};
mod platform;
mod request;
mod source;
mod watch;
use request::Request;
use source::Source;

// 首批管理域的业务容量政策；执行槽及来源/输入存储从这些实际对象上界推导。
const SOURCES: usize = 16;
const DEVICES: usize = 32;
const BINDINGS: usize = 32;
const USES: usize = 64;
const WATCHES: usize = 32;
const FUNCTIONS: usize = 64;
const REQUESTS: usize = 32;
const CONTROL_REQUESTS: usize = 32;
const REQUEST_SOURCES: usize = 3;
const CONTROL_SOURCES: usize = 1;
const INGRESS_SOURCES: usize = 2;
const ROOT_SOURCES: usize = 2;
const RPC_CALLS: usize = BINDINGS + 1;
const RPC_SOURCES: usize = match Dispatcher::source_capacity(RPC_CALLS) {
    Ok(count) => count,
    Err(_) => panic!("device RPC capacity overflow"),
};
const BUSINESS_TASKS: usize = 4 + SOURCES + DEVICES + BINDINGS + USES + WATCHES + REQUESTS;
const TASKS: usize = BUSINESS_TASKS + CONTROL_REQUESTS;
const BUSINESS_SOURCES: usize =
    REQUEST_SOURCES * (BUSINESS_TASKS - 4) + INGRESS_SOURCES + ROOT_SOURCES + RPC_SOURCES;
const WAIT_SOURCES: usize = BUSINESS_SOURCES + CONTROL_REQUESTS * CONTROL_SOURCES;
const ROLES: usize = DEVICES + BINDINGS + USES + WATCHES;
const POLICY: binding_work::Policy = binding_work::Policy {
    prepare_ns: 5_000_000_000,
    stop_ns: 20_000_000_000,
    retry_ns: 1_000_000,
};
pub(super) const TRANSPORT: Rights = Rights::from_raw(
    Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
pub(super) const ENTRY_RIGHTS: Rights = Rights::from_raw(
    Rights::WRITE.raw() | Rights::WAIT.raw() | Rights::DUPLICATE.raw() | Rights::TRANSIT.raw(),
);
pub(super) fn step(next: Step) -> Advance {
    Advance {
        work_done: 1,
        step: next,
    }
}
pub(super) fn fatal(error: Error) -> ! {
    libkernel::debug!("device manager failed: {:?}", error);
    // 不可恢复时由 init 的监督接管；不把退出当作硬件停止证明。
    unsafe { libkernel::sys_process_abort() }
}
pub(super) fn alias(sender: &MailboxSender) -> Result<Capability, Error> {
    alias_with(sender, TRANSPORT)
}
pub(super) fn alias_with(sender: &MailboxSender, rights: Rights) -> Result<Capability, Error> {
    let handle = object::duplicate(sender.as_handle(), rights)?;
    // SAFETY: duplicate 刚返回本进程的新 entry，关闭责任只移交给这个 owner。
    Ok(unsafe { Capability::from_raw(handle) })
}

pub(super) struct Observed {
    owner: Capability,
    source: SourceRegistration,
    closed: bool,
    kind: u64,
}
impl Observed {
    fn is_pending(&self) -> bool {
        self.source.is_installing() || self.source.is_removing()
    }
    fn new(owner: Capability, kind: u64) -> Self {
        Self {
            owner,
            source: SourceRegistration::new(),
            closed: false,
            kind,
        }
    }
    fn lifetime(owner: LifetimeObserver, kind: u64) -> Self {
        Self::new(owner.into_capability(), kind)
    }
    fn observe(&mut self, event: libexecution::runtime::SourceEvent) {
        if event.kind == self.kind && event.observed.intersects(ObjectSignals::CLOSED) {
            self.closed = true;
        }
    }
    fn drive(&mut self, requests: &mut Requests<'_, Family>) -> Result<bool, SystemCallError> {
        if self.closed {
            self.source.remove(requests)?;
            Ok(self.source.is_empty())
        } else {
            if self.source.is_empty() {
                self.source.install(
                    requests,
                    SourcePlan::new(self.owner.as_handle(), ObjectSignals::CLOSED),
                    self.kind,
                )?;
            }
            Ok(self.source.source().is_some())
        }
    }
    fn registered(&mut self, kind: u64, id: SourceId) {
        if kind == self.kind {
            self.source.registered(kind, id);
        }
    }
    fn unregistered(&mut self, id: SourceId) {
        self.source.unregistered(id);
    }
}

pub(super) enum RoleKind {
    Device {
        id: DeviceId,
    },
    Binding(Binding),
    Use(Binding),
    Watch {
        signal: watch::Signal,
        binding: Option<Binding>,
    },
}
pub(super) struct Role {
    kind: RoleKind,
    sender: Option<MailboxSender>,
    funding: Funding,
    _bytes: Charge,
    _unit: Option<Charge>,
}
pub(super) struct World {
    mailbox: Mailbox,
    registry: binding_work::Backing,
    roles: OrderedTable<Role, ()>,
    funding: Funding,
    management: Funding,
    rpc: Dispatcher,
    transport_task: u64,
    maintenance_task: u64,
    dirty: bool,
    finished: bool,
    platform: Option<platform::Adapter>,
    platform_ready: bool,
    platform_task: u64,
    root_task: u64,
}
impl World {
    fn role(&self, context: u64) -> Result<&Role, Error> {
        self.roles.get(context).ok_or(Error::Permission)
    }
    fn prepare_role(
        &self,
        context: u64,
        kind: RoleKind,
        sender: MailboxSender,
        funding: Funding,
    ) -> Result<PreparedEntry<Role, ()>, Error> {
        let unit = if matches!(&kind, RoleKind::Use(_)) {
            Some(funding.domain().acquire(DeviceResource::Use, 1)?)
        } else {
            None
        };
        let bytes = funding.domain().acquire(
            DeviceResource::Bytes,
            PreparedEntry::<Role, ()>::allocation_bytes(),
        )?;
        self.roles
            .prepare_insert(
                context,
                Role {
                    kind,
                    sender: Some(sender),
                    funding,
                    _bytes: bytes,
                    _unit: unit,
                },
                &libbudget::AllocatorBacked,
            )
            .map_err(|e| match e {
                ordered_table::InsertError::Limit(_) => Error::Quota,
                ordered_table::InsertError::Storage(e, _) => libbudget::allocator_error(e).into(),
            })
    }
    fn existing_device(&self, id: DeviceId) -> Option<&Role> {
        let mut cursor = None;
        while let Some((key, role)) = self.roles.next_after(cursor.as_ref()) {
            if matches!(role.kind, RoleKind::Device {id: existing,..} if existing == id) {
                return Some(role);
            }
            cursor = Some(*key);
        }
        None
    }
    fn existing_binding(&self, reference: libdevice::identity::BindingRef) -> Option<&Role> {
        let mut cursor = None;
        while let Some((key, role)) = self.roles.next_after(cursor.as_ref()) {
            if let RoleKind::Binding(binding) = &role.kind
                && binding
                    .snapshot(&self.registry)
                    .is_ok_and(|snapshot| snapshot.reference == reference)
            {
                return Some(role);
            }
            cursor = Some(*key);
        }
        None
    }
}

enum Family {
    Ingress(Ingress),
    Request(Box<Request>),
    Source(Source),
    Transport(Transport),
    Maintenance(Maintenance),
    Platform(platform::PlatformTask),
}
impl Task<World> for Family {
    type Family = Self;
    fn advance(
        &mut self,
        id: u64,
        world: &mut World,
        requests: &mut Requests<'_, Self>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        match self {
            Self::Ingress(task) => task.advance(world, requests, input),
            Self::Request(task) => task.advance(id, world, requests, input),
            Self::Source(task) => task.advance(id, world, requests, input),
            Self::Transport(task) => {
                if task.stopping && world.rpc.pending_count() == 0 {
                    world.rpc.stop(&mut ());
                }
                let result = world.rpc.advance(requests, input, budget)?;
                task.deadline = world.rpc.deadline();
                Ok(result)
            }
            Self::Maintenance(task) => task.advance(world, requests, input),
            Self::Platform(task) => task.advance(id, world, requests, input),
        }
    }
    fn registered(&mut self, world: &mut World, kind: u64, id: SourceId) {
        match self {
            Self::Ingress(task) => {
                if kind == 1 {
                    task.source.registered(kind, id);
                } else if let Some(refusal) = task.refusal.as_mut() {
                    refusal.registered(kind, id);
                }
            }
            Self::Request(task) => task.registered(kind, id),
            Self::Source(task) => task.registered(kind, id),
            Self::Transport(_) => world.rpc.registered(&mut (), kind, id),
            Self::Maintenance(_) => {}
            Self::Platform(task) => task.registered(kind, id),
        }
    }
    fn unregistered(&mut self, world: &mut World, kind: u64, id: SourceId) {
        match self {
            Self::Ingress(task) => {
                if kind == 1 {
                    task.source.unregistered(id);
                } else if let Some(refusal) = task.refusal.as_mut() {
                    refusal.unregistered(kind, id);
                }
            }
            Self::Request(task) => task.unregistered(kind, id),
            Self::Source(task) => task.unregistered(id),
            Self::Transport(_) => world.rpc.unregistered(&mut (), kind, id),
            Self::Maintenance(_) => {}
            Self::Platform(task) => task.unregistered(id),
        }
    }
    fn refused(&mut self, world: &mut World, failure: RequestFailure<Self>) {
        if !matches!(self, Self::Transport(_))
            && matches!(failure, RequestFailure::Wake { task, error: SystemCallError::ObjectNotFound } if task != world.transport_task)
        {
            // 所有通知目标来自已提交Registry/Binding/Runtime记录，task id不复用。
            // Wake在Gate中时原owner可安全Complete；通知已退休owner不产生新清理责任。
            // 失败任务仍由Runtime持有，不会落入此分支；运输owner及对运输任务的Wake
            // 保留原失败处理，其它登记/准入/唤醒错误继续报故障。
            return;
        }
        match self {
            Self::Transport(_) => world.rpc.refused(&mut (), failure),
            Self::Ingress(task) => match failure {
                RequestFailure::Spawn { submission, .. } => {
                    let Family::Request(mut request) = submission.into_parts().0 else {
                        unreachable!("device ingress task kind changed")
                    };
                    request.reject(Error::Quota);
                    task.refusal = Some(request);
                }
                _ => fatal(Error::Resource),
            },
            // 任务仍留在当前进程；不可恢复错误使进程失败，不能伪造资源安全完成。
            _ => {
                match &failure {
                    RequestFailure::Wake { task, error } => libkernel::debug!(
                        "device task wake refused: target={}, error={:?}",
                        task,
                        error
                    ),
                    RequestFailure::Source { kind, error } => {
                        libkernel::debug!("device source refused: kind={}, error={:?}", kind, error)
                    }
                    RequestFailure::Spawn { error, .. } => {
                        libkernel::debug!("device task admission refused: error={:?}", error)
                    }
                }
                fatal(Error::Unavailable)
            }
        }
    }
    fn stop(&mut self, world: &mut World) {
        match self {
            Self::Ingress(task) => task.stopping = true,
            Self::Transport(task) => task.stopping = true,
            Self::Maintenance(task) => task.stopping = true,
            Self::Request(task) => task.stop(world),
            Self::Source(task) => task.stop(world),
            Self::Platform(task) => task.stop(),
        }
    }
    fn deadline(&self) -> Deadline {
        match self {
            Self::Request(task) => task.deadline(),
            Self::Source(task) => task.deadline(),
            Self::Transport(task) => task.deadline,
            Self::Ingress(task) => task
                .refusal
                .as_ref()
                .map_or(Deadline::INFINITE, |r| r.deadline()),
            Self::Maintenance(_) => Deadline::INFINITE,
            Self::Platform(_) => Deadline::INFINITE,
        }
    }
}
struct Transport {
    deadline: Deadline,
    stopping: bool,
}
struct Maintenance {
    cursor: u64,
    end: u64,
    phase: u8,
    active: bool,
    stopping: bool,
}
impl Maintenance {
    fn advance(
        &mut self,
        world: &mut World,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        while input.pull().is_some() {}
        input.take_timeout();
        if self.stopping {
            return Ok(step(Step::Complete));
        }
        if !self.active {
            if !world.dirty {
                return Ok(step(Step::Parked));
            }
            world.dirty = false;
            self.active = true;
            self.phase = 0;
            self.cursor = 0;
            self.end = world.registry.scan_end();
        }
        if self.phase == 0 {
            if let Some((cursor, task)) = world
                .registry
                .maintenance(self.cursor, self.end)
                .unwrap_or_else(|error| fatal(error))
            {
                if let Some(task) = task {
                    requests.wake(task)?;
                }
                self.cursor = cursor;
                return Ok(step(Step::Runnable));
            }
            self.phase = 1;
            self.cursor = 0;
        }
        if self.phase == 1 {
            if let Some((cursor, task)) = world.registry.source_tasks(self.cursor) {
                requests.wake(task)?;
                self.cursor = cursor;
                return Ok(step(Step::Runnable));
            }
            self.phase = 2;
            self.cursor = 0;
        }
        if let Some((cursor, task)) = world.registry.watch_tasks(self.cursor) {
            requests.wake(task)?;
            self.cursor = cursor;
            return Ok(step(Step::Runnable));
        }
        self.active = false;
        Ok(step(if world.dirty {
            Step::Runnable
        } else {
            Step::Parked
        }))
    }
}
struct Ingress {
    source: SourceRegistration,
    buffer: ReceiveBuffer,
    storage: Option<MessageStorage>,
    pending: Option<Box<Request>>,
    refusal: Option<Box<Request>>,
    stopping: bool,
    _bytes: Charge,
}
impl Ingress {
    fn new(funding: &Funding) -> Result<Self, SystemCallError> {
        let bytes = funding.domain().acquire(
            DeviceResource::Bytes,
            core::mem::size_of::<Self>() + PAYLOAD_MAX * 2,
        )?;
        Ok(Self {
            source: SourceRegistration::new(),
            buffer: ReceiveBuffer::new()?,
            storage: Some(MessageStorage::new()?),
            pending: None,
            refusal: None,
            stopping: false,
            _bytes: bytes,
        })
    }
    fn advance(
        &mut self,
        world: &mut World,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        let mut ready = false;
        while let Some(event) = input.pull() {
            if event.kind == 1 {
                ready = true;
                if event.observed.intersects(ObjectSignals::CLOSED) {
                    self.stopping = true;
                }
            } else if let Some(refusal) = self.refusal.as_mut() {
                refusal.observe(event);
            }
        }
        if input.take_timeout()
            && let Some(refusal) = self.refusal.as_mut()
        {
            refusal.timed_out();
        }
        if self.stopping {
            self.pending = None;
            if let Some(refusal) = self.refusal.as_mut() {
                refusal.stop(world);
            }
        }
        if self.stopping || self.refusal.is_some() {
            self.source.remove(requests)?;
        }
        if let Some(refusal) = self.refusal.as_mut() {
            let result = refusal.advance(0, world, requests, input)?;
            if result.step == Step::Complete {
                self.refusal = None;
            }
            return Ok(step(Step::Runnable));
        }
        if self.stopping {
            return Ok(step(if self.source.is_empty() {
                Step::Complete
            } else {
                Step::Runnable
            }));
        }
        if let Some(task) = self.pending.take() {
            let funding = task.funding().execution().clone();
            let sources = task.max_sources();
            if let Err(submission) =
                requests.spawn(TaskSubmission::new(Family::Request(task), sources, funding))
            {
                let Family::Request(mut task) = submission.into_parts().0 else {
                    unreachable!("device ingress task kind changed")
                };
                task.reject(Error::Quota);
                self.refusal = Some(task);
            }
            return Ok(step(Step::Runnable));
        }
        if self.source.is_empty() {
            self.source.install(
                requests,
                SourcePlan::new(
                    world.mailbox.as_handle(),
                    ObjectSignals::READABLE | ObjectSignals::CLOSED,
                ),
                1,
            )?;
            return Ok(step(Step::Runnable));
        }
        if !ready {
            return Ok(step(Step::Parked));
        }
        if self.storage.is_none() {
            self.storage = Some(MessageStorage::new()?);
        }
        match self.buffer.receive(world.mailbox.as_handle()) {
            Ok(()) => {}
            Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => {
                return Ok(step(Step::Parked));
            }
            Err(e) => return Err(e),
        }
        let message = self
            .storage
            .take()
            .expect("device receive storage missing")
            .take(&mut self.buffer)?;
        if let Ok(request) = Request::accept(message, world) {
            self.pending = Some(Box::try_new(request).map_err(|_| SystemCallError::OutOfMemory)?);
        }
        Ok(step(Step::Runnable))
    }
}

pub(super) fn run(
    bootstrap: MailboxSender,
    _pool: Capability,
    platform: libpci::platform::Platform,
) {
    let input = Runtime::<Family, WaitSet>::fixed_input_budget(WAIT_SOURCES)
        .and_then(|fixed| {
            Runtime::<Family, WaitSet>::source_input_budget(WAIT_SOURCES).and_then(|sources| {
                Assembly::<WaitSet>::input_budget().and_then(|assembly| {
                    fixed
                        .checked_add(sources)
                        .and_then(|n| n.checked_add(assembly))
                        .ok_or(SystemCallError::ReachLimit)
                })
            })
        })
        .expect("device execution input budget overflow");
    // 记录/消息预算为管理者的明确资源政策；内核 Pool 另对真实页和内核元数据收费。
    let limits = [
        DEVICES,
        BINDINGS,
        2 * 1024 * 1024,
        SOURCES,
        USES,
        REQUESTS,
        CONTROL_REQUESTS,
        CONTROL_REQUESTS * (core::mem::size_of::<Request>() + PAYLOAD_MAX * 2),
        WATCHES,
        FUNCTIONS,
        TASKS,
        input,
    ];
    let budget = Budget::new(&limits, 2).expect("device budget failed");
    let control_input =
        Runtime::<Family, WaitSet>::source_input_budget(CONTROL_REQUESTS * CONTROL_SOURCES)
            .expect("device control input budget overflow");
    let mut business_limits = limits;
    business_limits[DeviceResource::ControlRequest.slot()] = 0;
    business_limits[DeviceResource::ControlBytes.slot()] = 0;
    business_limits[DeviceResource::COUNT + ExecutionResource::Task.slot()] = BUSINESS_TASKS;
    business_limits[DeviceResource::COUNT + ExecutionResource::InputBytes.slot()] =
        input - control_input;
    let mut control_limits = [0; DeviceResource::COUNT + ExecutionResource::COUNT];
    control_limits[DeviceResource::ControlRequest.slot()] = CONTROL_REQUESTS;
    control_limits[DeviceResource::ControlBytes.slot()] =
        limits[DeviceResource::ControlBytes.slot()];
    control_limits[DeviceResource::COUNT + ExecutionResource::Task.slot()] = CONTROL_REQUESTS;
    control_limits[DeviceResource::COUNT + ExecutionResource::InputBytes.slot()] = control_input;
    let payer = budget
        .account(&business_limits)
        .expect("device account failed");
    let manager = budget
        .account(&control_limits)
        .expect("device control account failed");
    let domain_slots: [_; DeviceResource::COUNT] =
        core::array::from_fn(|n| budget.slot(n).expect("device domain slot missing"));
    let exec_slots: [_; ExecutionResource::COUNT] = core::array::from_fn(|n| {
        budget
            .slot(DeviceResource::COUNT + n)
            .expect("device execution slot missing")
    });
    let funding = Funding::new(
        payer
            .view(&domain_slots)
            .expect("device domain funding invalid"),
        payer
            .view(&exec_slots)
            .expect("device execution funding invalid"),
    )
    .expect("device funding mismatch");
    let management = Funding::new(
        manager
            .view(&domain_slots)
            .expect("device control domain funding invalid"),
        manager
            .view(&exec_slots)
            .expect("device control execution funding invalid"),
    )
    .expect("device control funding mismatch");
    let mailbox = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)
        .expect("device mailbox failed");
    let incarnation = object::query(mailbox.as_handle())
        .expect("device mailbox description failed")
        .object_id;
    let mut world = World {
        mailbox,
        registry: Registry::new(
            incarnation,
            Config {
                sources: SOURCES,
                devices: DEVICES,
                watches: WATCHES,
                functions: FUNCTIONS,
            },
        )
        .expect("device registry failed"),
        roles: OrderedTable::new(ROLES),
        funding: funding.clone(),
        management,
        rpc: Dispatcher::new(RPC_CALLS, funding.execution()).expect("device dispatcher failed"),
        transport_task: 0,
        maintenance_task: 0,
        dirty: false,
        finished: false,
        platform: None,
        platform_ready: false,
        platform_task: 0,
        root_task: 0,
    };
    let set = WaitSet::create(WAIT_SOURCES).expect("device wait set failed");
    let mut assembly = Assembly::new(set, funding.execution())
        .unwrap_or_else(|(_, e)| panic!("device assembly failed: {e:?}"));
    let mut runtime = Runtime::<Family, WaitSet>::new(
        assembly.bind().expect("device assembly bind failed"),
        TASKS,
        WAIT_SOURCES,
        funding.execution(),
    )
    .expect("device runtime failed");
    let spawn = |runtime: &mut Runtime<Family, WaitSet>, task, max_sources| {
        runtime
            .spawn(TaskSubmission::new(
                task,
                max_sources,
                funding.execution().clone(),
            ))
            .unwrap_or_else(|f| panic!("device task admission failed: {:?}", f.error))
    };
    world.transport_task = spawn(
        &mut runtime,
        Family::Transport(Transport {
            deadline: Deadline::INFINITE,
            stopping: false,
        }),
        RPC_SOURCES,
    );
    world.maintenance_task = spawn(
        &mut runtime,
        Family::Maintenance(Maintenance {
            cursor: 0,
            end: 0,
            phase: 0,
            active: false,
            stopping: false,
        }),
        0,
    );
    spawn(
        &mut runtime,
        Family::Ingress(Ingress::new(&funding).expect("device ingress failed")),
        INGRESS_SOURCES,
    );
    let root = Source::root(&world, bootstrap).unwrap_or_else(|error| fatal(error));
    let provider = world
        .mailbox
        .mint(0, TRANSPORT)
        .expect("platform Provision entry failed");
    let adapter = platform::Adapter::new(
        platform,
        provider
            .sender
            .description()
            .expect("platform Provision identity missing")
            .object_id,
        funding.clone(),
    )
    .unwrap_or_else(|error| fatal(error));
    let platform_task = if adapter.has_host() {
        Some(
            platform::PlatformTask::new(
                root.registration_entry()
                    .unwrap_or_else(|error| fatal(error)),
                provider.sender,
            )
            .unwrap_or_else(|error| fatal(error)),
        )
    } else {
        world.platform_ready = true;
        None
    };
    world.platform = Some(adapter);
    world.root_task = spawn(&mut runtime, Family::Source(root), ROOT_SOURCES);
    if let Some(task) = platform_task {
        world.platform_task = spawn(&mut runtime, Family::Platform(task), 1);
    }
    let mut stopping = false;
    loop {
        for &record in assembly.receive(64).expect("device receive sources failed") {
            runtime.feed(record);
        }
        if world.finished && !stopping {
            runtime.seal();
            stopping = true;
        }
        let result = if stopping {
            runtime.shutdown_turn(&mut world, 1)
        } else {
            runtime.turn(&mut world, 1)
        };
        if let Err(failure) = result {
            libkernel::debug!(
                "device task failed: id={}, error={:?}",
                failure.task,
                failure.error
            );
            fatal(Error::Unavailable);
        }
        if world.registry.take_discovery_change() {
            world.dirty = true;
        }
        if world.dirty && !stopping {
            runtime
                .wake(world.maintenance_task)
                .unwrap_or_else(|_| fatal(Error::Unavailable));
            if world.platform_task != 0 {
                runtime
                    .wake(world.platform_task)
                    .unwrap_or_else(|_| fatal(Error::Unavailable));
            }
        }
        match runtime.drive_state() {
            DriveState::Drained => break,
            DriveState::Runnable => {}
            DriveState::Waiting(deadline) => assembly.wait(deadline).expect("device wait failed"),
        }
    }
    if !world.registry.is_empty() || !world.roles.is_empty() {
        fatal(Error::Busy);
    }
    runtime
        .close()
        .unwrap_or_else(|(_, e)| panic!("device runtime close failed: {e:?}"));
    assembly
        .close()
        .unwrap_or_else(|(_, e)| panic!("device assembly close failed: {e:?}"));
    drop(world);
    assert!(
        budget.is_empty(),
        "device service budget retained after shutdown"
    );
    libkernel::debug!("device manager stopped; control records retired");
}
