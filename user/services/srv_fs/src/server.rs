//! 文件进程的实际 Runtime 任务族：唯一文件 backing 与 OAL/fs ingress。
//! 启动根、独立 registry 与发布接线由同一改造计划的服务装配工作包承接。

use alloc::boxed::Box;
use erhino_shared::{
    call::SystemCallError,
    object::{ObjectSignals, Rights},
    time::Deadline,
};
use libbudget::{AccountView, Charge};
use libexecution::{
    SourceRegistration, TaskSubmission,
    runtime::{Advance, Input, RequestFailure, Requests, SourceId, SourcePlan, Step, Task},
};
use libfs::{
    FsRights, authority::Issuance, backend::MemoryBackend, grant::GrantTable, provider,
    resource::FsResource,
};
use libkernel::ipc::message::{Mailbox, MessageStorage, ReceiveBuffer};
mod boot;
mod family;
mod lifecycle;
mod services;
pub(super) use boot::{Startup, run};

const MAILBOX_SOURCE: u64 = 1;
const REPLY_SOURCE: u64 = 2;
const WATCH_CONTROL_SOURCE: u64 = 3;
const STREAM_DATA_SOURCE: u64 = 4;

/// 两种协议及名字变更始终操作这一实例；没有 Registry/NodeRef 分流钩子。
struct World {
    pub mailbox: Mailbox,
    pub memory: MemoryBackend,
    pub grants: GrantTable,
    pub route: Mailbox,
    pub management: libfs::management::ManagementTable,
    pub streams: libfs::stream::StreamTable,
    pub pool: libkernel::ipc::capability::Capability,
    pub retirement_task: u64,
    pub account: AccountView<FsResource>,
    pub stop: bool,
    pub service: Option<libservice::provider::State>,
    pub rpc: Option<Box<librpc::dispatcher::Dispatcher>>,
    pub transport_task: u64,
}
impl World {
    fn requests_empty(&self) -> bool {
        self.account.budget_usage(FsResource::Request).0 == 0
            && self.service.as_ref().is_none_or(|state| {
                state
                    .account
                    .budget_usage(libservice::resource::ServiceResource::Request)
                    .0
                    == 0
            })
    }

    fn rpc_mut(&mut self) -> &mut librpc::dispatcher::Dispatcher {
        self.rpc.as_deref_mut().expect("provider transport missing")
    }
    pub fn prepare_root(
        &mut self,
        startup_delivery: bool,
        account: &AccountView<FsResource>,
        execution: &AccountView<libexecution::ExecutionResource>,
    ) -> Result<libfs::grant::PreparedGrant, libfs::grant::GrantIssueFailure> {
        let root = self
            .memory
            .root()
            .expect("file backing root missing")
            .clone();
        self.grants.prepare_issue(
            &self.mailbox,
            root,
            Issuance {
                rights: FsRights::ALL,
                sender_transport: Rights::WRITE
                    | Rights::WAIT
                    | Rights::TRANSIT
                    | Rights::DUPLICATE
                    | if startup_delivery {
                        Rights::GRANT
                    } else {
                        Rights::NONE
                    },
                output_transport: srv_fs::deployment::root_output_transport(),
            },
            account,
            execution,
            0,
        )
    }
}

enum FileTask {
    Ingress(Ingress),
    Request(Box<provider::Request>),
    FileRoot(lifecycle::Root),
    ServiceRoot(libservice::root::Root),
    ServiceIngress(services::Ingress),
    ServiceRequest(Box<libservice::provider::Request>),
    Maintenance(libservice::maintenance::Maintenance),
    Release(lifecycle::Release),
    Retire(lifecycle::Retire),
    Dispatcher(Transport),
}

/// Runtime 槽只拥有运输推进/回调责任与最近期限；唯一 Dispatcher 留在 World。
struct Transport {
    deadline: Deadline,
    stopping: bool,
}
impl Transport {
    fn new() -> Self {
        Self {
            deadline: Deadline::INFINITE,
            stopping: false,
        }
    }
    fn advance(
        &mut self,
        world: &mut World,
        requests: &mut Requests<'_, FileTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        let business_empty = world.requests_empty();
        let rpc = world.rpc_mut();
        if self.stopping && business_empty && rpc.pending_count() == 0 {
            rpc.stop(&mut ());
        }
        let advance = rpc.advance(requests, input, budget)?;
        self.deadline = rpc.deadline();
        if advance.step == Step::Complete {
            // Dispatcher 仅在真实来源全部注销后完成，此时释放共享 receiver/执行存储。
            world.rpc = None;
        }
        Ok(advance)
    }
}
impl Task<World> for FileTask {
    type Family = Self;
    fn advance(
        &mut self,
        _id: u64,
        world: &mut World,
        requests: &mut Requests<'_, Self>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        match self {
            Self::Ingress(task) => task.advance(world, requests, input, budget),
            Self::Request(task) => {
                let mut advance = task.advance(
                    &mut world.memory,
                    &mut world.grants,
                    &world.mailbox,
                    world
                        .rpc
                        .as_deref_mut()
                        .map(|dispatcher| provider::Transport {
                            dispatcher,
                            driver: world.transport_task,
                            waiter: _id,
                        }),
                    provider::StreamHost {
                        table: &mut world.streams,
                        pool: world.pool.as_handle(),
                        config: libfs::stream::Config {
                            offer_window_ns: srv_fs::deployment::OFFER_WINDOW_NS,
                            tunnel_bytes: srv_fs::deployment::STREAM_TUNNEL_BYTES,
                        },
                        driver: _id,
                    },
                    requests,
                    input,
                    budget,
                )?;
                if world.streams.has_wake_work() && requests.wake(world.retirement_task).is_err() {
                    advance.step = Step::Runnable;
                }
                Ok(advance)
            }
            _ => self.advance_extra(_id, world, requests, input, budget),
        }
    }
    fn registered(&mut self, world: &mut World, kind: u64, source: SourceId) {
        match self {
            Self::Ingress(task) => {
                if kind == MAILBOX_SOURCE {
                    task.source.registered(kind, source)
                } else if let Some(reply) = task.refusal.as_mut() {
                    reply.registered(kind, source)
                }
            }
            Self::Request(task) => task.registered(kind, source),
            _ => self.registered_extra(world, kind, source),
        }
    }
    fn unregistered(&mut self, world: &mut World, kind: u64, source: SourceId) {
        match self {
            Self::Ingress(task) => {
                if kind == MAILBOX_SOURCE {
                    task.source.unregistered(source);
                } else if let Some(reply) = task.refusal.as_mut() {
                    reply.unregistered(kind, source);
                }
            }
            Self::Request(task) => task.unregistered(kind, source),
            _ => self.unregistered_extra(world, kind, source),
        }
    }
    fn refused(&mut self, world: &mut World, failure: RequestFailure<Self>) {
        match self {
            Self::Request(task) => task.refused(failure),
            Self::Ingress(task) => match failure {
                RequestFailure::Spawn { submission, .. } => {
                    let Self::Request(request) = submission.into_parts().0 else {
                        unreachable!("file ingress spawned another task kind")
                    };
                    assert!(
                        task.pending.is_none(),
                        "file ingress already owns refused request"
                    );
                    task.refuse(request);
                }
                failure @ RequestFailure::Source {
                    kind: REPLY_SOURCE, ..
                } => {
                    if let Some(reply) = task.refusal.as_mut() {
                        reply.refused(failure);
                    }
                }
                RequestFailure::Source { kind, .. } => {
                    task.source.refused(kind);
                    task.stopping = true;
                    world.stop = true;
                }
                RequestFailure::Wake { .. } => {
                    task.stopping = true;
                    world.stop = true;
                }
            },
            _ => self.refused_extra(world, failure),
        }
    }
    fn stop(&mut self, _world: &mut World) {
        match self {
            Self::Request(task) => task.stop(),
            Self::Ingress(task) => task.stopping = true,
            _ => self.stop_extra(_world),
        }
    }
    fn deadline(&self) -> Deadline {
        match self {
            Self::Request(task) => task.deadline(),
            Self::Ingress(task) => task
                .refusal
                .as_ref()
                .or(task.pending.as_ref())
                .map_or(Deadline::INFINITE, |request| request.deadline()),
            _ => self.deadline_extra(),
        }
    }
}

struct Ingress {
    management: bool,
    source: SourceRegistration,
    buffer: ReceiveBuffer,
    storage: Option<MessageStorage>,
    pending: Option<Box<provider::Request>>,
    refusal: Option<Box<provider::Request>>,
    stopping: bool,
    _source_charge: Charge,
    _bytes_charge: Charge,
}
impl Ingress {
    fn management(account: &AccountView<FsResource>) -> Result<Self, SystemCallError> {
        let mut ingress = Self::new(account)?;
        ingress.management = true;
        Ok(ingress)
    }
    /// 一个 mailbox 来源，加满载拒绝回复沿既有 Outbox 使用的一个来源。
    pub const MAX_SOURCES: usize = 2;
    fn refuse(&mut self, mut request: Box<provider::Request>) {
        // 未成功进入 Runtime 的请求没有登记来源、发送 Packet 或编码业务结果。
        request
            .reject(libfs::protocol::Status::Quota)
            .expect("trusted request error exceeds prepared response");
        assert!(self.refusal.is_none(), "file ingress already owns refusal");
        self.refusal = Some(request);
    }
    pub fn new(account: &AccountView<FsResource>) -> Result<Self, SystemCallError> {
        let source_charge = account.acquire(FsResource::WaitSource, 1)?;
        let bytes_charge = account.acquire(
            FsResource::Bytes,
            erhino_shared::message::PAYLOAD_MAX
                .checked_mul(2)
                .and_then(|n| n.checked_add(core::mem::size_of::<Self>()))
                .ok_or(SystemCallError::OutOfMemory)?,
        )?;
        Ok(Self {
            management: false,
            source: SourceRegistration::new(),
            buffer: ReceiveBuffer::new()?,
            storage: Some(MessageStorage::new()?),
            pending: None,
            refusal: None,
            stopping: false,
            _source_charge: source_charge,
            _bytes_charge: bytes_charge,
        })
    }
    fn advance(
        &mut self,
        world: &mut World,
        requests: &mut Requests<'_, FileTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if budget == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let mut ready = false;
        while let Some(event) = input.pull() {
            if let Some(reply) = self.refusal.as_mut() {
                reply.observe(event);
            }
            if event.kind == MAILBOX_SOURCE {
                if event.observed.intersects(ObjectSignals::CLOSED) {
                    self.stopping = true;
                    world.stop = true;
                } else {
                    ready = true;
                }
            }
        }
        if input.take_timeout() {
            self.pending = None;
            if let Some(reply) = self.refusal.as_mut() {
                reply.timed_out();
            }
        }
        if self.pending.as_ref().is_some_and(|request| {
            request
                .deadline()
                .instant()
                .is_ok_and(|at| at.is_some_and(|at| at <= input.now_ns()))
        }) {
            self.pending = None;
        }
        if self.stopping {
            // pending 尚未进入 Runtime，Outbox 未登记，直接释放不丢提交责任。
            self.pending = None;
            if let Some(reply) = self.refusal.as_mut() {
                reply.stop();
            }
            if self.source.source().is_some() && !self.source.is_removing() {
                self.source.remove(requests)?;
            }
            if let Some(reply) = self.refusal.as_mut() {
                let advance = reply.advance(
                    &mut world.memory,
                    &mut world.grants,
                    &world.mailbox,
                    None,
                    provider::StreamHost {
                        table: &mut world.streams,
                        pool: world.pool.as_handle(),
                        config: libfs::stream::Config {
                            offer_window_ns: srv_fs::deployment::OFFER_WINDOW_NS,
                            tunnel_bytes: srv_fs::deployment::STREAM_TUNNEL_BYTES,
                        },
                        driver: 0,
                    },
                    requests,
                    input,
                    budget,
                )?;
                if !reply.reply_complete() {
                    return Ok(advance);
                }
                self.refusal = None;
            }
            return Ok(Advance {
                work_done: 1,
                step: if self.source.is_empty() {
                    Step::Complete
                } else {
                    Step::Runnable
                },
            });
        }
        if let Some(reply) = self.refusal.as_mut() {
            // 拒绝回复等待期间暂停 mailbox READABLE，避免满载队列持续唤醒自旋。
            if self.source.source().is_some() && !self.source.is_removing() {
                self.source.remove(requests)?;
            }
            let advance = reply.advance(
                &mut world.memory,
                &mut world.grants,
                &world.mailbox,
                None,
                provider::StreamHost {
                    table: &mut world.streams,
                    pool: world.pool.as_handle(),
                    config: libfs::stream::Config {
                        offer_window_ns: srv_fs::deployment::OFFER_WINDOW_NS,
                        tunnel_bytes: srv_fs::deployment::STREAM_TUNNEL_BYTES,
                    },
                    driver: 0,
                },
                requests,
                input,
                budget,
            )?;
            if !reply.reply_complete() {
                return Ok(advance);
            }
            self.refusal = None;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if let Some(task) = self.pending.take() {
            let funding = task.execution_funding().clone();
            if let Err(submission) = requests.spawn(TaskSubmission::new(
                FileTask::Request(task),
                provider::Request::MAX_SOURCES,
                funding,
            )) {
                let FileTask::Request(task) = submission.into_parts().0 else {
                    unreachable!("spawn returned different file task")
                };
                self.refuse(task);
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if self.source.source().is_none() && !self.source.is_installing() {
            self.source.install(
                requests,
                SourcePlan::new(
                    if self.management {
                        world.route.as_handle()
                    } else {
                        world.mailbox.as_handle()
                    },
                    ObjectSignals::READABLE | ObjectSignals::CLOSED,
                ),
                MAILBOX_SOURCE,
            )?;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if !ready {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        // 接收前持有完整可用的 MessageStorage；成功 Receive 后不会为 owner 分配。
        if self.storage.is_none() {
            self.storage = Some(MessageStorage::new()?);
        }
        match self.buffer.receive(if self.management {
            world.route.as_handle()
        } else {
            world.mailbox.as_handle()
        }) {
            Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            Err(error) => return Err(error),
            Ok(()) => {}
        }
        let storage = self
            .storage
            .take()
            .expect("prepared receive storage missing");
        let message = storage.take(&mut self.buffer)?;
        match provider::Request::accept(
            message,
            &world.grants,
            &world.management,
            &world.memory,
            &world.streams,
            &world.account,
            requests.task_funding(),
            REPLY_SOURCE,
            WATCH_CONTROL_SOURCE,
            STREAM_DATA_SOURCE,
        ) {
            Ok(request) => {
                let task = Box::try_new(request).map_err(|_| SystemCallError::OutOfMemory)?;
                self.pending = Some(task);
            }
            // 所有拒收值持有完整消息/回复/Delivery；Drop 沿真实 RPC owner 清理。
            Err(rejected) => drop(rejected),
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Runnable,
        })
    }
}
