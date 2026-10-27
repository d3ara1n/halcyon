//! 一个现有 Runtime 组合 PCI 激活、供应和 RPC；不复制中央设备占用。
use alloc::{boxed::Box, sync::Arc};
use erhino_shared::{
    call::SystemCallError,
    object::{ObjectSignals, Rights},
    time::Deadline,
};
use libbudget::{AccountView, Budget};
use libdevice::{Error, protocol as device};
use libdriver::{
    protocol,
    worker::{ActivationSlot, SLOT_SOURCES, TRANSPORT, WorkerControl, alias},
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
mod request;
mod supply;
use engine::Engine;
const REQUESTS: usize = 8;
// 原管理者Stop与中央Provision停止各有一条独立控制通道；普通客户端不占这两份额度。
const CONTROL_REQUESTS: usize = 2;
const ROOT_TASKS: usize = 3;
const REQUEST_SOURCES: usize = 2;
const CALLS: usize = REQUESTS + 1;
const RPC_SOURCES: usize = match Dispatcher::source_capacity(CALLS) {
    Ok(n) => n,
    Err(_) => panic!("PCI RPC capacity overflow"),
};
const TASKS: usize = REQUESTS + CONTROL_REQUESTS + ROOT_TASKS;
const ROOT_SOURCES: usize = RPC_SOURCES + 1 + SLOT_SOURCES;
const SOURCES: usize = ROOT_SOURCES + (REQUESTS + CONTROL_REQUESTS) * REQUEST_SOURCES;
fn step(step: Step) -> Advance {
    Advance { work_done: 1, step }
}
fn fatal(error: Error) -> ! {
    libkernel::debug!(
        "PCI provider failed: {:?}; hardware responsibility retained by outer owner",
        error
    );
    // SAFETY: 显式失败进入 init 的受信供应链故障终局；退出不被解释为硬件停止。
    unsafe { libkernel::sys_process_abort() }
}
struct World {
    control: WorkerControl,
    slot: Option<ActivationSlot>,
    host: supply::Host,
    funding: AccountView<ExecutionResource>,
    requests: AccountView<ExecutionResource>,
    management: AccountView<ExecutionResource>,
    rpc: Dispatcher,
    transport: u64,
    engine: u64,
    rpc_dirty: bool,
    _pool: Capability,
}
impl World {
    fn changed(&mut self) -> Result<(), SystemCallError> {
        if let Some(slot) = self.slot.as_ref() {
            slot.changed().unwrap_or_else(|e| fatal(e));
        }
        Ok(())
    }
}
enum Family {
    Engine(Engine),
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
            Self::Engine(_) => {
                if let Some(slot) = world.slot.as_mut() {
                    slot.registered(kind, id);
                }
            }
            Self::Ingress(t) => t.source.registered(kind, id),
            Self::Request(t) => t.registered(kind, id),
            Self::Transport { .. } => world.rpc.registered(&mut (), kind, id),
        }
    }
    fn unregistered(&mut self, world: &mut World, kind: u64, id: SourceId) {
        match self {
            Self::Engine(_) => {
                if let Some(slot) = world.slot.as_mut() {
                    slot.unregistered(id);
                }
            }
            Self::Ingress(t) => {
                t.source.unregistered(id);
            }
            Self::Request(t) => t.unregistered(kind, id),
            Self::Transport { .. } => world.rpc.unregistered(&mut (), kind, id),
        }
    }
    fn refused(&mut self, world: &mut World, failure: libexecution::runtime::RequestFailure<Self>) {
        if matches!(self, Self::Transport { .. }) {
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
            .expect("PCI ingress storage missing")
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
pub(super) fn run(bootstrap: MailboxSender, pool: Capability) {
    let fixed =
        Runtime::<Family, WaitSet>::fixed_input_budget(SOURCES).expect("PCI fixed inputs invalid");
    let root_input = fixed
        .checked_add(
            Runtime::<Family, WaitSet>::source_input_budget(ROOT_SOURCES)
                .expect("PCI root sources invalid"),
        )
        .and_then(|bytes| {
            bytes.checked_add(
                Assembly::<WaitSet>::input_budget().expect("PCI assembly inputs invalid"),
            )
        })
        .expect("PCI root inputs overflow");
    let request_input = Runtime::<Family, WaitSet>::source_input_budget(REQUESTS * REQUEST_SOURCES)
        .expect("PCI request inputs invalid");
    let control_input =
        Runtime::<Family, WaitSet>::source_input_budget(CONTROL_REQUESTS * REQUEST_SOURCES)
            .expect("PCI control inputs invalid");
    let input = root_input
        .checked_add(request_input)
        .and_then(|n| n.checked_add(control_input))
        .expect("PCI inputs overflow");
    let budget = Budget::new(&[TASKS, input], 3).expect("PCI budget failed");
    let view = |tasks, bytes| {
        budget
            .account(&[tasks, bytes])
            .expect("PCI account failed")
            .view::<ExecutionResource>(&[
                budget.slot(0).expect("PCI task slot missing"),
                budget.slot(1).expect("PCI input slot missing"),
            ])
            .expect("PCI funding failed")
    };
    let funding = view(ROOT_TASKS, root_input);
    let request_funding = view(REQUESTS, request_input);
    let management_funding = view(CONTROL_REQUESTS, control_input);
    let (state, control) =
        WorkerControl::new(libpci::host::HOST_IDENTITY, true, 1).unwrap_or_else(|e| fatal(e));
    let host = supply::Host::new().unwrap_or_else(|e| fatal(e));
    let mut world = World {
        control: state,
        slot: None,
        host,
        funding: funding.clone(),
        rpc: Dispatcher::new(CALLS, &funding).expect("PCI dispatcher failed"),
        transport: 0,
        engine: 0,
        rpc_dirty: false,
        requests: request_funding,
        management: management_funding,
        _pool: pool,
    };
    let mut packet = Packet::new(
        protocol::BOOT_READY,
        &object::query(world.control.mailbox.as_handle())
            .expect("PCI mailbox identity failed")
            .object_id
            .to_le_bytes(),
    )
    .expect("PCI bootstrap packet failed");
    packet
        .push(control.into_capability(), TRANSPORT)
        .map_err(|f| f.error)
        .expect("PCI bootstrap grant failed");
    packet
        .try_send(&bootstrap, Deadline::INFINITE)
        .map_err(|f| f.error)
        .expect("PCI bootstrap send failed");
    drop(bootstrap);
    let set = WaitSet::create(SOURCES).expect("PCI WaitSet failed");
    let mut assembly =
        Assembly::new(set, &funding).unwrap_or_else(|(_, e)| panic!("PCI Assembly failed: {e:?}"));
    let mut runtime = Runtime::<Family, WaitSet>::new(
        assembly.bind().expect("PCI bind failed"),
        TASKS,
        SOURCES,
        &funding,
    )
    .expect("PCI Runtime failed");
    world.transport = runtime
        .spawn(TaskSubmission::new(
            Family::Transport {
                deadline: Deadline::INFINITE,
            },
            RPC_SOURCES,
            funding.clone(),
        ))
        .unwrap_or_else(|f| panic!("PCI transport failed: {:?}", f.error));
    world.engine = runtime
        .spawn(TaskSubmission::new(
            Family::Engine(Engine::new()),
            SLOT_SOURCES,
            funding.clone(),
        ))
        .unwrap_or_else(|f| panic!("PCI engine failed: {:?}", f.error));
    runtime
        .spawn(TaskSubmission::new(
            Family::Ingress(
                Box::try_new(Ingress {
                    source: SourceRegistration::new(),
                    buffer: ReceiveBuffer::new().expect("PCI receive buffer failed"),
                    storage: Some(MessageStorage::new().expect("PCI receive storage failed")),
                })
                .expect("PCI ingress allocation failed"),
            ),
            1,
            funding.clone(),
        ))
        .unwrap_or_else(|f| panic!("PCI ingress failed: {:?}", f.error));
    // 父管理者只在原 Binding Released 后收集本进程；Terminal 后仍服务软件清理入口。
    loop {
        for &event in assembly.receive(64).expect("PCI source receive failed") {
            runtime.feed(event);
        }
        if let Err(failure) = runtime.turn(&mut world, 1) {
            libkernel::debug!(
                "PCI task failed: task={}, error={:?}",
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
            DriveState::Waiting(deadline) => assembly.wait(deadline).expect("PCI wait failed"),
            DriveState::Drained => fatal(Error::Unavailable),
        }
    }
}
