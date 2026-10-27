//! 原Runtime组合HID激活、USB消费及键盘功能；不持物理资源或中央占用。
use alloc::{boxed::Box, rc::Rc, sync::Arc, vec::Vec};
use core::cell::RefCell;
use erhino_shared::{call::SystemCallError, object::ObjectSignals, time::Deadline};
use libbudget::{AccountView, Budget};
use libdevice::{Error, protocol as device};
use libdriver::{
    protocol,
    worker::{ActivationSlot, SLOT_SOURCES, TRANSPORT, WorkerControl},
};
use libexecution::{
    ExecutionResource, SourceRegistration, TaskSubmission,
    assembly::Assembly,
    runtime::{Advance, DriveState, Input, Requests, Runtime, SourceId, SourcePlan, Step, Task},
};
use libkernel::ipc::{
    capability::Capability,
    message::{Mailbox, MailboxSender, MessageStorage, ReceiveBuffer},
    object,
    packet::Packet,
    wait_set::WaitSet,
};
use librpc::dispatcher::{Dispatcher, ServiceOwner};
mod engine;
mod keyboard;
mod request;
use engine::Engine;
const REQUESTS: usize = 8;
// 原管理者Stop与中央Provision停止各有一条独立控制通道；普通客户端不占这两份额度。
const CONTROL_REQUESTS: usize = 1;
const ROOT_TASKS: usize = 2;
const INSTANCES: usize = 32;
const INSTANCE_SOURCES: usize = SLOT_SOURCES + keyboard::WATCHES + 1;
const ACTIVE: usize = 2;
const REQUEST_SOURCES: usize = 2;
const CALLS: usize = ACTIVE * 3;
const RPC_SOURCES: usize = match Dispatcher::source_capacity(CALLS) {
    Ok(n) => n,
    Err(_) => panic!("HID RPC capacity overflow"),
};
const TASKS: usize = REQUESTS + CONTROL_REQUESTS + ROOT_TASKS + INSTANCES * 2;
const ROOT_SOURCES: usize = RPC_SOURCES + 1;
const SOURCES: usize = ROOT_SOURCES
    + (REQUESTS + CONTROL_REQUESTS) * REQUEST_SOURCES
    + INSTANCES * (INSTANCE_SOURCES + 1);
fn step(step: Step) -> Advance {
    Advance { work_done: 1, step }
}
fn operation_end() -> Result<Deadline, SystemCallError> {
    libkernel::time::timeout_millis(5_000)
}
fn fatal(error: Error) -> ! {
    libkernel::debug!(
        "HID provider failed: {:?}; hardware responsibility retained by outer owner",
        error
    );
    // SAFETY: 显式失败进入 init 的受信供应链故障终局；退出不被解释为硬件停止。
    unsafe { libkernel::sys_process_abort() }
}
struct Instance {
    slot: ActivationSlot,
    keys: keyboard::Keys,
    task: u64,
}
type InstanceRef = Rc<RefCell<Instance>>;
struct World {
    control: WorkerControl,
    instances: Vec<InstanceRef>,
    instance_funding: AccountView<ExecutionResource>,
    funding: AccountView<ExecutionResource>,
    requests: AccountView<ExecutionResource>,
    management: AccountView<ExecutionResource>,
    rpc: Dispatcher,
    transport: u64,
    rpc_dirty: bool,
    _pool: Capability,
}
impl World {
    fn context(&self, context: u64) -> Option<InstanceRef> {
        self.instances
            .iter()
            .find(|instance| {
                let state = instance.borrow();
                state.slot.is_function_context(context)
                    || state.slot.is_observation_context(context)
                    || state.keys.has_context(context)
            })
            .cloned()
    }
    fn associate(
        &mut self,
        reference: libdevice::identity::BindingRef,
        requests: &mut Requests<'_, Family>,
    ) -> Result<InstanceRef, Error> {
        if let Some(instance) = self
            .instances
            .iter()
            .find(|i| i.borrow().slot.reference() == reference)
        {
            return Ok(instance.clone());
        }
        if self.instances.len() == INSTANCES {
            return Err(Error::Quota);
        }
        let slot = ActivationSlot::new(&self.control, reference)?;
        let instance = Rc::try_new(RefCell::new(Instance {
            slot,
            keys: keyboard::Keys::new(),
            task: 0,
        }))
        .map_err(|_| Error::Resource)?;
        let task = Engine::new(instance.clone())?;
        requests
            .spawn(TaskSubmission::new(
                Family::Engine(task),
                INSTANCE_SOURCES,
                self.instance_funding.clone(),
            ))
            .map_err(|_| Error::Quota)?;
        self.instances.push(instance.clone());
        Ok(instance)
    }
}

enum Family {
    Engine(Engine),
    Binding(Box<libdriver::binding::BindingTask>, InstanceRef),
    Ingress(Box<Ingress>),
    Request(Box<request::Request>),
    Transport { deadline: Deadline },
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
            Self::Engine(task) => task.advance(id, world, requests, input),
            Self::Binding(task, instance) => {
                let mut state = instance.borrow_mut();
                let engine = state.task;
                Ok(task
                    .advance(
                        &mut state.slot,
                        id,
                        engine,
                        world.transport,
                        &mut world.rpc,
                        requests,
                        input,
                    )
                    .unwrap_or_else(|error| fatal(error)))
            }
            Self::Ingress(task) => task.advance(world, requests, input),
            Self::Request(task) => task.advance(id, world, requests, input),
            Self::Transport { deadline } => {
                let result = world.rpc.advance(requests, input, budget)?;
                *deadline = world.rpc.deadline();
                Ok(result)
            }
        }
    }
    fn registered(&mut self, world: &mut World, kind: u64, id: SourceId) {
        match self {
            Self::Engine(task) => task.registered(kind, id),
            Self::Binding(task, _) => task.registered(kind, id),
            Self::Ingress(t) => t.source.registered(kind, id),
            Self::Request(t) => t.registered(kind, id),
            Self::Transport { .. } => world.rpc.registered(&mut (), kind, id),
        }
    }
    fn unregistered(&mut self, world: &mut World, kind: u64, id: SourceId) {
        match self {
            Self::Engine(task) => task.unregistered(id),
            Self::Binding(task, _) => task.unregistered(id),
            Self::Ingress(t) => {
                t.source.unregistered(id);
            }
            Self::Request(t) => t.unregistered(kind, id),
            Self::Transport { .. } => world.rpc.unregistered(&mut (), kind, id),
        }
    }
    fn refused(&mut self, world: &mut World, failure: libexecution::runtime::RequestFailure<Self>) {
        if let (
            Self::Request(task),
            libexecution::runtime::RequestFailure::Wake {
                task: target,
                error: SystemCallError::ObjectNotFound,
            },
        ) = (&self, &failure)
            && task.notification_target(*target)
        {
            // 已准入不可复用的实例任务可在最后Delivery退休后先完成；迟到通知不产生完成事实。
            return;
        }
        if let Self::Binding(task, _) = self {
            task.refused(failure).unwrap_or_else(|error| fatal(error));
        } else if matches!(self, Self::Transport { .. }) {
            world.rpc.refused(&mut (), failure);
        } else if matches!(self, Self::Ingress(_)) {
            match failure {
                libexecution::runtime::RequestFailure::Spawn { submission, .. } => drop(submission),
                _ => fatal(Error::Quota),
            }
        } else {
            fatal(Error::Quota);
        }
    }
    fn stop(&mut self, _: &mut World) {
        fatal(Error::Unavailable);
    }
    fn deadline(&self) -> Deadline {
        match self {
            Self::Engine(t) => t.deadline(),
            Self::Binding(t, _) => t.deadline(),
            Self::Request(t) => t.deadline(),
            Self::Transport { deadline } => *deadline,
            _ => Deadline::INFINITE,
        }
    }
}
struct Ingress {
    source: SourceRegistration,
    buffer: ReceiveBuffer,
    storage: Option<MessageStorage>,
}
impl Ingress {
    fn advance(
        &mut self,
        world: &mut World,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        let mut readable = false;
        while let Some(event) = input.pull() {
            if event.kind == 1 {
                readable = true;
            }
        }
        input.take_timeout();
        if self.source.is_empty() {
            self.source.install(
                requests,
                SourcePlan::new(
                    world.control.mailbox.as_handle(),
                    ObjectSignals::READABLE | ObjectSignals::CLOSED,
                ),
                1,
            )?;
            return Ok(step(Step::Runnable));
        }
        if self.source.is_installing() {
            return Ok(step(Step::Runnable));
        }
        if !readable {
            return Ok(step(Step::Parked));
        }
        if self.storage.is_none() {
            self.storage = Some(MessageStorage::new()?);
        }
        match self.buffer.receive(world.control.mailbox.as_handle()) {
            Ok(()) => {}
            Err(SystemCallError::ObjectBusy | SystemCallError::ObjectNotAvailable) => {
                return Ok(step(Step::Parked));
            }
            Err(e) => return Err(e),
        }
        let message = self
            .storage
            .take()
            .expect("HID ingress storage missing")
            .take(&mut self.buffer)?;
        if let Ok(task) = request::Request::accept(message, world) {
            let funding = if task.control_request() {
                world.management.clone()
            } else {
                world.requests.clone()
            };
            let task = Box::try_new(task).map_err(|_| SystemCallError::OutOfMemory)?;
            // 预付账户是唯一准入计数；拒绝的未执行请求关闭原输入/回复，不产生设备效果。
            let _ = requests.spawn(TaskSubmission::new(
                Family::Request(task),
                REQUEST_SOURCES,
                funding,
            ));
        }
        Ok(step(Step::Runnable))
    }
}
pub(super) fn run(bootstrap: MailboxSender, pool: Capability, capacity: u32) {
    let fixed =
        Runtime::<Family, WaitSet>::fixed_input_budget(SOURCES).expect("HID fixed inputs invalid");
    let root_input = fixed
        .checked_add(
            Runtime::<Family, WaitSet>::source_input_budget(ROOT_SOURCES)
                .expect("HID root sources invalid"),
        )
        .and_then(|bytes| {
            bytes.checked_add(
                Assembly::<WaitSet>::input_budget().expect("HID assembly inputs invalid"),
            )
        })
        .expect("HID root inputs overflow");
    let request_input = Runtime::<Family, WaitSet>::source_input_budget(REQUESTS * REQUEST_SOURCES)
        .expect("HID request inputs invalid");
    let control_input =
        Runtime::<Family, WaitSet>::source_input_budget(CONTROL_REQUESTS * REQUEST_SOURCES)
            .expect("HID control inputs invalid");
    let instance_input =
        Runtime::<Family, WaitSet>::source_input_budget(INSTANCES * (INSTANCE_SOURCES + 1))
            .expect("HID instance inputs invalid");
    let input = root_input
        .checked_add(request_input)
        .and_then(|n| n.checked_add(control_input))
        .and_then(|n| n.checked_add(instance_input))
        .expect("HID inputs overflow");
    let budget = Budget::new(&[TASKS, input], 4).expect("HID budget failed");
    let view = |tasks, bytes| {
        budget
            .account(&[tasks, bytes])
            .expect("HID account failed")
            .view::<ExecutionResource>(&[
                budget.slot(0).expect("HID task slot missing"),
                budget.slot(1).expect("HID input slot missing"),
            ])
            .expect("HID funding failed")
    };
    let funding = view(ROOT_TASKS, root_input);
    let request_funding = view(REQUESTS, request_input);
    let management_funding = view(CONTROL_REQUESTS, control_input);
    let (state, control) =
        WorkerControl::new(libusb::descriptor::BOOT_KEYBOARD_IDENTITY, false, capacity)
            .unwrap_or_else(|e| fatal(e));
    let mut instances = Vec::new();
    instances
        .try_reserve_exact(INSTANCES)
        .unwrap_or_else(|_| fatal(Error::Resource));
    let mut world = World {
        control: state,
        instances,
        instance_funding: view(INSTANCES * 2, instance_input),
        funding: funding.clone(),
        rpc: Dispatcher::new(CALLS, &funding).expect("HID dispatcher failed"),
        transport: 0,
        rpc_dirty: false,
        requests: request_funding,
        management: management_funding,
        _pool: pool,
    };
    let mut packet = Packet::new(
        protocol::BOOT_READY,
        &object::query(world.control.mailbox.as_handle())
            .expect("HID mailbox identity failed")
            .object_id
            .to_le_bytes(),
    )
    .expect("HID bootstrap packet failed");
    packet
        .push(control.into_capability(), TRANSPORT)
        .map_err(|f| f.error)
        .expect("HID bootstrap grant failed");
    packet
        .try_send(&bootstrap, Deadline::INFINITE)
        .map_err(|f| f.error)
        .expect("HID bootstrap send failed");
    drop(bootstrap);
    let set = WaitSet::create(SOURCES).expect("HID WaitSet failed");
    let mut assembly =
        Assembly::new(set, &funding).unwrap_or_else(|(_, e)| panic!("HID Assembly failed: {e:?}"));
    let mut runtime = Runtime::<Family, WaitSet>::new(
        assembly.bind().expect("HID bind failed"),
        TASKS,
        SOURCES,
        &funding,
    )
    .expect("HID Runtime failed");
    world.transport = runtime
        .spawn(TaskSubmission::new(
            Family::Transport {
                deadline: Deadline::INFINITE,
            },
            RPC_SOURCES,
            funding.clone(),
        ))
        .unwrap_or_else(|f| panic!("HID transport failed: {:?}", f.error));
    runtime
        .spawn(TaskSubmission::new(
            Family::Ingress(
                Box::try_new(Ingress {
                    source: SourceRegistration::new(),
                    buffer: ReceiveBuffer::new().expect("HID receive buffer failed"),
                    storage: Some(MessageStorage::new().expect("HID receive storage failed")),
                })
                .expect("HID ingress allocation failed"),
            ),
            1,
            funding.clone(),
        ))
        .unwrap_or_else(|f| panic!("HID ingress failed: {:?}", f.error));
    // 父管理者只在原 Binding Released 后收集本进程；Terminal 后仍服务软件清理入口。
    loop {
        for &event in assembly.receive(64).expect("HID source receive failed") {
            runtime.feed(event);
        }
        if let Err(failure) = runtime.turn(&mut world, 1) {
            libkernel::debug!(
                "HID task failed: task={}, error={:?}",
                failure.task,
                failure.error
            );
            fatal(Error::Unavailable);
        }
        if core::mem::take(&mut world.rpc_dirty) {
            runtime
                .wake(world.transport)
                .unwrap_or_else(|_| fatal(Error::Unavailable));
        }
        match runtime.drive_state() {
            DriveState::Runnable => {}
            DriveState::Waiting(deadline) => assembly.wait(deadline).expect("HID wait failed"),
            DriveState::Drained => fatal(Error::Unavailable),
        }
    }
}
