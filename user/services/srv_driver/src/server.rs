//! 驱动管理的单 actor；设备占用仍只由 srv_device 决定。
use alloc::{boxed::Box, rc::Rc, sync::Arc, vec::Vec};
use core::cell::RefCell;
use erhino_shared::{
    call::SystemCallError,
    object::{ObjectSignals, Rights},
    time::Deadline,
};
use libbudget::{AccountView, AllocatorBacked, Budget};
use libdevice::{
    Error,
    identity::{BindingRef, DeviceId},
};
use libdriver::{boot::Catalog, protocol, view::InstanceId};
use libexecution::{
    ExecutionResource, SourceRegistration, TaskSubmission,
    assembly::Assembly,
    runtime::{
        Advance, DriveState, Input, RequestFailure, Requests, Runtime, SourceId, Step, Task,
    },
};
use libkernel::ipc::{
    capability::Capability,
    message::{Mailbox, MailboxSender},
    object,
    wait_set::WaitSet,
};
use libprocess::{context::CurrentProcessInfo, image::Image};
use librpc::{
    Request,
    dispatcher::{Dispatcher, ServiceOwner},
};
use ordered_table::OrderedTable;
mod control;
mod managed;
mod manager;
mod subscription;
mod view;
mod worker;
use control::Ingress;
use managed::Managed;
use manager::Manager;

const DEVICES: usize = 32;
const REQUESTS: usize = 16;
// 一项停止和一项管理状态观察，连同原Request/Outbox来源预付。
const CONTROL_RESERVE: usize = 2;
const PROCESSES: usize = DEVICES;
const TASKS: usize = DEVICES * 3 + PROCESSES + REQUESTS + 4;
// 每个管理请求至多一条Retry只读查询，公开OAL不使用这些RPC席位。
const CALLS: usize = DEVICES * 3 + 1 + REQUESTS;
const RPC_SOURCES: usize = match Dispatcher::source_capacity(CALLS) {
    Ok(n) => n,
    Err(_) => panic!("driver RPC capacity overflow"),
};
const WAIT_SOURCES: usize = DEVICES * 2 + PROCESSES * 2 + REQUESTS + 5 + RPC_SOURCES;
const TRANSPORT: Rights = Rights::from_raw(
    Rights::WRITE.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);
fn advance(step: Step) -> Advance {
    Advance { work_done: 1, step }
}
fn fatal(error: Error) -> ! {
    libkernel::debug!("driver manager failed: {:?}", error);
    // init 保留来源管理与外层 Job；本进程退出绝不证明 DMA 已停止。
    unsafe { libkernel::sys_process_abort() }
}
fn alias(sender: &MailboxSender) -> Result<MailboxSender, Error> {
    let handle = object::duplicate(sender.as_handle(), TRANSPORT)?;
    // SAFETY: duplicate 返回独占的新 entry，typed sender 采用唯一关闭责任。
    MailboxSender::from_capability(unsafe { Capability::from_raw(handle) })
        .map(|(sender, _)| sender)
        .map_err(|failure| failure.error.into())
}
fn deadline() -> Result<Deadline, SystemCallError> {
    libkernel::time::timeout_millis(5_000)
}

#[derive(Clone)]
struct Run {
    device: DeviceId,
    reference: BindingRef,
    implementation: usize,
    task: u64,
    process: Option<InstanceId>,
    released: bool,
    stop_requested: bool,
    /// 首个选择失败原因；正常停止不产生fallback资格，后续清理不覆盖。
    failure: Option<Error>,
    phase: protocol::Phase,
    binding_watch: u64,
    activation_watch: u64,
    binding_changed: bool,
    activation_changed: bool,
    activation_status: Option<protocol::Snapshot>,
    watchers_stopping: bool,
}
impl Run {
    fn fail(&mut self, error: Error) {
        if !self.stop_requested {
            self.failure.get_or_insert(error);
        }
        self.stop_requested = true;
    }
    fn settled(&self) -> bool {
        // task=0/Terminal只在Managed已Released并Detach，或Claim明确未提交后发布。
        self.phase == protocol::Phase::Terminal
            && self.task == 0
            && self.process.is_none()
            && self.binding_watch == 0
            && self.activation_watch == 0
    }
    fn can_fallback(&self) -> bool {
        self.released
            && self.settled()
            && self.failure.is_some_and(|error| {
                !matches!(
                    error,
                    Error::Quota | Error::Resource | Error::Busy | Error::NotReady
                )
            })
    }
}
struct World<'a> {
    mailbox: Mailbox,
    observer_mailbox: Option<Mailbox>,
    incarnation: u64,
    control_context: u64,
    observer_context: u64,
    observer_ingress: u64,
    process: CurrentProcessInfo,
    pool: Capability,
    job: Capability,
    claim: Arc<MailboxSender>,
    catalog: Catalog<'a>,
    images: Vec<Image>,
    runs: OrderedTable<Rc<RefCell<Run>>, ()>,
    processes: OrderedTable<Rc<RefCell<worker::Record>>, ()>,
    next_process: u64,
    funding: AccountView<ExecutionResource>,
    rpc: Dispatcher,
    transport_task: u64,
    manager_task: u64,
    rpc_dirty: bool,
    discovery_dirty: bool,
    stopping: bool,
    ready: bool,
    finished: bool,
    outstanding_requests: usize,
}
impl World<'_> {
    fn call(
        &mut self,
        waiter: u64,
        service: Arc<MailboxSender>,
        request: Request,
        deadline: Deadline,
    ) -> Result<u64, librpc::CallError> {
        let result = self
            .rpc
            .begin_for(
                ServiceOwner::Shared(service),
                deadline,
                request,
                Some(waiter),
                &self.funding,
            )
            .map_err(|failure| failure.error)?;
        self.rpc_dirty = true;
        Ok(result)
    }
}
impl libprocess::SuperviseSink for World<'_> {
    fn supervised(&mut self, result: libprocess::SuperviseResult) {
        let done = result.outcome.unwrap_or_else(|_| fatal(Error::Unavailable));
        libkernel::debug!("driver worker collected: pid={}", done.pid);
        let mut cursor = None;
        while let Some((serial, record)) = self.processes.next_after(cursor.as_ref()) {
            cursor = Some(*serial);
            if record.borrow().pid == Some(done.pid) {
                record.borrow_mut().collected = true;
                return;
            }
        }
        fatal(Error::Invalid);
    }
}
enum Family {
    Manager(Manager),
    Managed(Box<Managed>),
    Process(Box<worker::ProcessTask>),
    Ingress(Ingress),
    Control(Box<control::Request>),
    Subscription(subscription::Subscription),
    Transport { deadline: Deadline, stopping: bool },
}
impl Task<World<'_>> for Family {
    type Family = Self;
    fn advance(
        &mut self,
        id: u64,
        world: &mut World<'_>,
        requests: &mut Requests<'_, Self>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        match self {
            Self::Manager(task) => task.advance(id, world, requests, input),
            Self::Managed(task) => task.advance(id, world, requests, input),
            Self::Process(task) => task.advance(id, world, requests, input),
            Self::Ingress(task) => task.advance(world, requests, input),
            Self::Control(task) => task.advance(id, world, requests, input),
            Self::Subscription(task) => task.advance(id, world, requests, input),
            Self::Transport { deadline, stopping } => {
                if *stopping && world.rpc.pending_count() == 0 {
                    world.rpc.stop(&mut ());
                }
                let result = world.rpc.advance(requests, input, budget)?;
                *deadline = world.rpc.deadline();
                Ok(result)
            }
        }
    }
    fn registered(&mut self, world: &mut World<'_>, kind: u64, source: SourceId) {
        match self {
            Self::Manager(task) => task.registered(kind, source),
            Self::Managed(_) => {}
            Self::Process(task) => task.registered(world, kind, source),
            Self::Ingress(task) => task.registered(kind, source),
            Self::Control(task) => task.registered(kind, source),
            Self::Subscription(task) => task.registered(kind, source),
            Self::Transport { .. } => world.rpc.registered(&mut (), kind, source),
        }
    }
    fn unregistered(&mut self, world: &mut World<'_>, kind: u64, source: SourceId) {
        match self {
            Self::Manager(task) => task.unregistered(source),
            Self::Managed(_) => {}
            Self::Process(task) => task.unregistered(world, kind, source),
            Self::Ingress(task) => task.unregistered(kind, source),
            Self::Control(task) => task.unregistered(kind, source),
            Self::Subscription(task) => task.unregistered(source),
            Self::Transport { .. } => world.rpc.unregistered(&mut (), kind, source),
        }
    }
    fn refused(&mut self, world: &mut World<'_>, failure: RequestFailure<Self>) {
        if let Self::Transport { .. } = self {
            world.rpc.refused(&mut (), failure);
        } else if matches!(self, Self::Manager(_) | Self::Managed(_) | Self::Process(_))
            && matches!(&failure,RequestFailure::Wake{task,error:SystemCallError::ObjectNotFound} if *task!=world.transport_task)
        {
            // 唯一来源为原Run/Process或固定Ingress中已准入且不可复用的维护通知；目标已退休无额外效果。
        } else {
            fatal(Error::Quota);
        }
    }
    fn stop(&mut self, world: &mut World<'_>) {
        world.stopping = true;
        match self {
            Self::Manager(task) => task.stop(),
            Self::Managed(task) => task.stop(),
            Self::Process(_) => {}
            Self::Ingress(task) => task.stop(),
            Self::Control(task) => task.stop(),
            Self::Subscription(task) => task.stop(),
            Self::Transport { stopping, .. } => *stopping = true,
        }
    }
    fn deadline(&self) -> Deadline {
        match self {
            Self::Manager(task) => task.deadline(),
            Self::Managed(task) => task.deadline(),
            Self::Process(task) => task.deadline(),
            Self::Ingress(task) => task.deadline(),
            Self::Control(task) => task.deadline(),
            Self::Subscription(task) => task.deadline(),
            Self::Transport { deadline, .. } => *deadline,
        }
    }
}

pub(super) fn run(
    process: CurrentProcessInfo,
    bootstrap: MailboxSender,
    pool: Capability,
    job: Capability,
    claim: MailboxSender,
    catalog: Catalog<'_>,
    images: Vec<Image>,
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
        .expect("driver input budget overflow");
    let budget = Budget::new(&[TASKS, input], 1).expect("driver budget failed");
    let account = budget
        .account(&[TASKS, input])
        .expect("driver account failed");
    let funding = account
        .view(&[
            budget.slot(0).expect("driver task slot missing"),
            budget.slot(1).expect("driver input slot missing"),
        ])
        .expect("driver funding failed");
    let mailbox = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)
        .expect("driver mailbox failed");
    let control = mailbox
        .mint(0, TRANSPORT)
        .expect("driver control entry failed");
    let control_info = control
        .sender
        .description()
        .expect("driver control identity failed");
    let observer_mailbox = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)
        .expect("driver observer mailbox failed");
    let observer = observer_mailbox
        .mint(0, TRANSPORT)
        .expect("driver observer entry failed");
    let observer_context = observer
        .sender
        .description()
        .expect("driver observer identity failed")
        .object_id;
    let observer_handle = observer_mailbox.as_handle();
    let claim = Arc::try_new(claim).expect("driver ClaimSource storage failed");
    let mut world = World {
        mailbox,
        observer_mailbox: Some(observer_mailbox),
        incarnation: control_info.related_object_id,
        control_context: control_info.object_id,
        observer_context,
        observer_ingress: 0,
        process,
        pool,
        job,
        claim,
        catalog,
        images,
        runs: OrderedTable::new(DEVICES),
        processes: OrderedTable::new(PROCESSES),
        next_process: 0,
        funding: funding.clone(),
        rpc: Dispatcher::new(CALLS, &funding).expect("driver dispatcher failed"),
        transport_task: 0,
        manager_task: 0,
        rpc_dirty: false,
        discovery_dirty: true,
        stopping: false,
        ready: false,
        finished: false,
        outstanding_requests: 0,
    };
    let set = WaitSet::create(WAIT_SOURCES).expect("driver WaitSet failed");
    let mut assembly = Assembly::new(set, &funding)
        .unwrap_or_else(|(_, error)| panic!("driver assembly failed: {error:?}"));
    let mut runtime = Runtime::<Family, WaitSet>::new(
        assembly.bind().expect("driver assembly bind failed"),
        TASKS,
        WAIT_SOURCES,
        &funding,
    )
    .expect("driver Runtime failed");
    let spawn = |runtime: &mut Runtime<Family, WaitSet>, task, sources| {
        runtime
            .spawn(TaskSubmission::new(task, sources, funding.clone()))
            .unwrap_or_else(|failure| panic!("driver task admission failed: {:?}", failure.error))
    };
    world.transport_task = spawn(
        &mut runtime,
        Family::Transport {
            deadline: Deadline::INFINITE,
            stopping: false,
        },
        RPC_SOURCES,
    );
    world.manager_task = spawn(
        &mut runtime,
        Family::Manager(Manager::new(bootstrap, control.sender, observer.sender)),
        1,
    );
    spawn(
        &mut runtime,
        Family::Ingress(
            Ingress::new(world.mailbox.as_handle(), false).expect("driver ingress failed"),
        ),
        2,
    );
    world.observer_ingress = spawn(
        &mut runtime,
        Family::Ingress(
            Ingress::new(observer_handle, true).expect("driver observer ingress failed"),
        ),
        2,
    );
    let mut stopping = false;
    loop {
        for &event in assembly.receive(64).expect("driver source receive failed") {
            runtime.feed(event);
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
                "driver task failed: id={}, error={:?}",
                failure.task,
                failure.error
            );
            fatal(Error::Unavailable);
        }
        if core::mem::take(&mut world.rpc_dirty) {
            runtime
                .wake(world.transport_task)
                .unwrap_or_else(|_| fatal(Error::Unavailable));
        }
        match runtime.drive_state() {
            DriveState::Drained => break,
            DriveState::Runnable => {}
            DriveState::Waiting(deadline) => {
                assembly.wait(deadline).expect("driver source wait failed")
            }
        }
    }
    if !world.runs.is_empty() {
        fatal(Error::Busy);
    }
    runtime
        .close()
        .unwrap_or_else(|(_, error)| panic!("driver runtime close failed: {error:?}"));
    assembly
        .close()
        .unwrap_or_else(|(_, error)| panic!("driver assembly close failed: {error:?}"));
    drop(world);
    assert!(
        budget.is_empty(),
        "driver manager retained execution budget"
    );
    libkernel::debug!("driver manager stopped; bindings and worker processes collected");
}
