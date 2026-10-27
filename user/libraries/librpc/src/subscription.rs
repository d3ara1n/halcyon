//! 带序号确认的 RPC 订阅客户端；领域协议解释快照，本任务持有通知与调用寿命。
use crate::{CallCause, CallError, Reply, Request, dispatcher::Dispatcher};
use erhino_shared::{call::SystemCallError, object::ObjectSignals, time::Deadline};
use libexecution::{
    SourceRegistration,
    runtime::{Advance, Input, RequestFailure, Requests, SourceId, SourcePlan, Step},
};
use libkernel::ipc::{message::MailboxSender, notification::NotificationObserver};

const NOTIFICATION: u64 = 0;
#[derive(Clone, Copy)]
pub enum Command {
    Query,
    Ack(u64),
}
/// 领域快照供会话推进使用的只读投影，不解释事件位或观察对象。
pub struct SnapshotState {
    pub sequence: u64,
    pub pending: bool,
    pub terminal: bool,
}

/// 具体订阅拥有线上协议与快照解释；不选择运行体或来源付款。
/// Query/Ack 不携带业务能力，合法响应也不交付能力。
pub trait Protocol: Sized {
    type Snapshot: Copy;
    type Status: Copy;

    fn into_parts(self) -> (MailboxSender, NotificationObserver, Self::Snapshot);
    fn request(command: Command, deadline: Deadline) -> Result<Request, Failure<Self::Status>>;
    fn response(
        reply: &Reply,
        command: Command,
        deadline: Deadline,
    ) -> Result<Self::Snapshot, Failure<Self::Status>>;
    fn observation(snapshot: &Self::Snapshot) -> SnapshotState;
}

#[derive(Clone, Copy)]
enum Phase<I> {
    Listening,
    Removing,
    Query,
    Reading(I),
    Ack(I),
    InFlight(Command, u64),
    Retiring,
}
#[derive(Debug, Clone, Copy)]
pub enum Failure<S> {
    System(SystemCallError),
    Transport(CallCause),
    Status(S),
    Protocol,
}

/// 宿主把本任务与既有 Dispatcher 放在同一 Runtime；本类型不创建运行体。
/// snapshot 暂存的序号在消费者采用后才可 acknowledge_read；初始和 Ack 快照同样交付。
pub struct ConsumerTask<P: Protocol> {
    control: Option<MailboxSender>,
    observer: Option<NotificationObserver>,
    source: SourceRegistration,
    phase: Phase<P::Snapshot>,
    deadline: Deadline,
    pending: bool,
    stopping: bool,
    failure: Option<Failure<P::Status>>,
    cycles: u64,
}
impl<P: Protocol> ConsumerTask<P> {
    pub const fn source_capacity() -> usize {
        1
    }
    pub const fn inflight_capacity() -> usize {
        1
    }
    pub fn new(subscription: P, deadline: Deadline) -> Self {
        let (control, observer, snapshot) = subscription.into_parts();
        Self {
            control: Some(control),
            observer: Some(observer),
            source: SourceRegistration::new(),
            phase: Phase::Reading(snapshot),
            deadline,
            pending: false,
            stopping: false,
            failure: None,
            cycles: 0,
        }
    }
    pub fn snapshot(&self) -> Option<P::Snapshot> {
        match self.phase {
            Phase::Reading(info) => Some(info),
            _ => None,
        }
    }
    pub fn acknowledge_read(&mut self) -> Result<(), SystemCallError> {
        let Phase::Reading(info) = self.phase else {
            return Err(SystemCallError::ObjectBusy);
        };
        self.phase = Phase::Ack(info);
        Ok(())
    }
    pub fn cycles(&self) -> u64 {
        self.cycles
    }
    pub fn source_is_detached(&self) -> bool {
        self.source.is_empty()
    }
    pub fn armed(&self) -> bool {
        matches!(self.phase, Phase::Listening) && !self.pending && self.source.source().is_some()
    }
    pub fn failure(&self) -> Option<Failure<P::Status>> {
        self.failure
    }
    pub fn deadline(&self) -> Deadline {
        if matches!(self.phase, Phase::Retiring) {
            Deadline::INFINITE
        } else {
            self.deadline
        }
    }
    fn fail(&mut self, failure: Failure<P::Status>) {
        self.failure.get_or_insert(failure);
        self.stopping = true;
    }
    fn fail_call(&mut self, error: CallError) {
        self.fail(Failure::Transport(error.cause));
        // Dispatcher 交付完成前已实际注销来源；未发送 Packet 的能力也由
        // typed owner 必成关闭，不把本地表项关闭另存为待重试业务责任。
    }
    fn restore(&mut self, service: crate::dispatcher::ServiceOwner) {
        // 本任务只提交 Owned，唯一 txid 的完成原样返还同一角色。
        self.control = Some(
            service
                .into_owned()
                .expect("owned Watch call returned shared service"),
        );
    }
    fn start<F>(
        &mut self,
        command: Command,
        task: u64,
        rpc_task: u64,
        rpc: &mut Dispatcher,
        requests: &mut Requests<'_, F>,
    ) {
        let request = match P::request(command, self.deadline) {
            Ok(request) => request,
            Err(failure) => {
                self.fail(failure);
                return;
            }
        };
        let Some(control) = self.control.take() else {
            self.fail(Failure::Protocol);
            return;
        };
        match rpc.begin_for(
            crate::dispatcher::ServiceOwner::Owned(control),
            self.deadline,
            request,
            Some(task),
            requests.task_funding(),
        ) {
            Ok(txid) => {
                self.phase = Phase::InFlight(command, txid);
                // 即使请求队列暂满，原在途责任仍保留；下一次 advance 重试唤醒。
                let _ = requests.wake(rpc_task);
            }
            Err(failed) => {
                self.restore(failed.service);
                self.fail_call(failed.error);
            }
        }
    }
    pub fn advance<F>(
        &mut self,
        task: u64,
        rpc_task: u64,
        rpc: &mut Dispatcher,
        requests: &mut Requests<'_, F>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Advance {
        while let Some(event) = input.pull() {
            if event.kind == NOTIFICATION && self.source.source() == Some(event.source) {
                if event.observed.intersects(ObjectSignals::CLOSED) {
                    self.fail(Failure::Transport(CallCause::ServiceClosed));
                } else if event.observed.intersects(ObjectSignals::READABLE) {
                    self.pending = true;
                }
            }
        }
        if input.take_timeout() && !matches!(self.phase, Phase::Retiring) {
            self.fail(Failure::Transport(CallCause::Timeout));
        }
        if let Phase::InFlight(command, txid) = self.phase {
            if self.stopping {
                rpc.cancel(txid);
            }
            // 活性不依赖一次 wake 成功；失败仍由当前已准入任务重试。
            if requests.wake(rpc_task).is_err() {
                return step(Step::Runnable);
            }
            let Some(completion) = rpc.take(txid) else {
                return step(Step::Parked);
            };
            self.restore(completion.service);
            match completion.result {
                Err(error) => self.fail_call(error),
                Ok(reply) => match P::response(&reply, command, self.deadline) {
                    Ok(info) => match command {
                        Command::Query => self.phase = Phase::Reading(info),
                        Command::Ack(_) => {
                            self.cycles = self.cycles.saturating_add(1);
                            let state = P::observation(&info);
                            self.pending = state.pending;
                            self.phase = if state.pending {
                                Phase::Reading(info)
                            } else if state.terminal {
                                Phase::Retiring
                            } else {
                                Phase::Listening
                            };
                        }
                    },
                    Err(failure) => self.fail(failure),
                },
            }
            if self.stopping {
                self.phase = Phase::Retiring;
            }
            return step(Step::Runnable);
        }
        if self.stopping {
            self.phase = Phase::Retiring;
        }
        match self.phase {
            Phase::Listening => {
                if self.source.is_empty() {
                    let Some(observer) = self.observer.as_ref() else {
                        self.fail(Failure::Protocol);
                        return step(Step::Runnable);
                    };
                    if let Err(error) = self.source.install(
                        requests,
                        SourcePlan::new(
                            observer.as_handle(),
                            ObjectSignals::READABLE | ObjectSignals::CLOSED,
                        ),
                        NOTIFICATION,
                    ) {
                        self.fail(Failure::System(error));
                    }
                    return step(Step::Runnable);
                }
                if self.source.source().is_some() && self.pending {
                    self.pending = false;
                    self.phase = Phase::Removing;
                    return step(Step::Runnable);
                }
                step(Step::Parked)
            }
            Phase::Removing => {
                if !self.source.is_empty() {
                    if self.source.remove(requests).is_err() {
                        return step(Step::Runnable);
                    }
                    return step(
                        if self.source.is_removing() || self.source.is_installing() {
                            Step::Parked
                        } else {
                            Step::Runnable
                        },
                    );
                }
                self.phase = Phase::Query;
                step(Step::Runnable)
            }
            Phase::Query => {
                self.start(Command::Query, task, rpc_task, rpc, requests);
                step(Step::Runnable)
            }
            Phase::Reading(_) => step(Step::Parked),
            Phase::Ack(info) => {
                self.start(
                    Command::Ack(P::observation(&info).sequence),
                    task,
                    rpc_task,
                    rpc,
                    requests,
                );
                step(Step::Runnable)
            }
            Phase::Retiring => {
                if !self.source.is_empty() {
                    if self.source.remove(requests).is_err() {
                        return step(Step::Runnable);
                    }
                    return step(
                        if self.source.is_removing() || self.source.is_installing() {
                            Step::Parked
                        } else {
                            Step::Runnable
                        },
                    );
                }
                // 来源回执先于表项关闭；两者均为独占非映射 typed owner。
                drop(self.control.take());
                drop(self.observer.take());
                step(Step::Complete)
            }
            Phase::InFlight(_, _) => {
                unreachable!("in-flight subscription was handled before local progression")
            }
        }
    }
    pub fn registered(&mut self, kind: u64, source: SourceId) {
        if kind == NOTIFICATION {
            self.source.registered(kind, source);
        }
    }
    pub fn unregistered(&mut self, source: SourceId) {
        self.source.unregistered(source);
    }
    pub fn refused<F>(&mut self, failure: RequestFailure<F>) {
        match failure {
            RequestFailure::Source { kind, error } => {
                self.source.refused(kind);
                self.fail(Failure::System(error));
            }
            RequestFailure::Wake { error, .. } => self.fail(Failure::System(error)),
            RequestFailure::Spawn { .. } => {}
        }
    }
    pub fn stop(&mut self) {
        self.stopping = true;
    }
    /// 任务尚未准入时只关闭原角色，不发送 RPC，也不制造来源责任。
    pub fn abandon_unadmitted(&mut self) -> Result<(), SystemCallError> {
        if !self.source.is_empty()
            || !matches!(
                self.phase,
                Phase::Reading(_) | Phase::Listening | Phase::Retiring
            )
        {
            return Err(SystemCallError::ObjectBusy);
        }
        self.stopping = true;
        self.phase = Phase::Retiring;
        drop(self.control.take());
        drop(self.observer.take());
        Ok(())
    }
}
fn step(step: Step) -> Advance {
    Advance { work_done: 1, step }
}
