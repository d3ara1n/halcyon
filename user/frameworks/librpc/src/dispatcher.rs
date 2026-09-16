//! 多 in-flight RPC 的协议状态与 Runtime 任务。
//!
//! Runtime 拥有 WaitSet、来源登记、期限唤醒和退休；本模块只拥有
//! PendingCall、txid 路由、回复存储及其运输 owner。

use erhino_shared::{
    call::SystemCallError,
    object::{ObjectSignals, Rights},
    time::Deadline,
};
use libsrv::runtime::{
    Advance, Input, RequestFailure, Requests, SourceEvent, SourceId, SourcePlan, Step,
};
use ordered_table::OrderedTable;
use rinlib::ipc::{
    capability::Capability,
    message::{Mailbox, MailboxSender, MessageStorage, ReceiveBuffer},
};
use timer_queue::{TimerQueue, TimerToken};

use crate::{
    RpcPrefix,
    exchange::{CallCause, CallError, Reply, Request, cause},
    outbound::{OutboundStage, Sweep},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceRole {
    Writable,
    Closed,
}

#[derive(Debug, Clone, Copy)]
struct SourceBinding {
    txid: u64,
    role: SourceRole,
}

struct SourceState {
    key: u64,
    source: Option<SourceId>,
    requested: bool,
    removing: bool,
    rearm_needed: bool,
}

struct PendingCall {
    service: MailboxSender,
    deadline: Deadline,
    protocol: u64,
    request: Option<Request>,
    result: Option<Result<Reply, CallError>>,
    storage: Option<MessageStorage>,
    timer: Option<TimerToken>,
    writable: SourceState,
    closed: SourceState,
    stage: OutboundStage,
    queued: bool,
    waiter: Option<u64>,
    wake_queued: bool,
    previous: u64,
    next: u64,
}

#[derive(Debug)]
pub struct Completion {
    pub service: Capability,
    pub result: Result<Reply, CallError>,
}

#[derive(Debug)]
pub struct StartFailure {
    pub service: Capability,
    pub error: CallError,
}

/// 多 in-flight RPC 的协议任务。
///
/// 外层通过 `Runtime::get_task_mut` 提交调用并以 `Runtime::wake` 唤醒任务；
/// 完成结果经 [`Self::take`] 或 [`Self::pop_completed`] 取回。
pub struct Dispatcher {
    owner: Mailbox,
    sender: MailboxSender,
    receiver: ReceiveBuffer,
    reply_key: u64,
    reply_source: Option<SourceId>,
    reply_requested: bool,
    reply_removing: bool,
    pending: OrderedTable<PendingCall>,
    bindings: OrderedTable<SourceBinding>,
    deadlines: TimerQueue<u64>,
    completed_head: u64,
    completed_tail: u64,
    registrations: Sweep,
    sends: Sweep,
    removals: Sweep,
    stopping: Sweep,
    retiring: Sweep,
    wake_pending: bool,
    phase: u8,
    reply_rearm_needed: bool,
    next_source_key: u64,
    sealed: bool,
    fault: Option<SystemCallError>,
}

impl Dispatcher {
    pub fn new(limit: usize) -> Result<Self, SystemCallError> {
        if limit == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let source_limit = limit.checked_mul(2).ok_or(SystemCallError::ReachLimit)?;
        let receiver = ReceiveBuffer::new()?;
        let owner = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)?;
        let minted = owner.mint(
            0,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )?;
        Ok(Self {
            owner,
            sender: minted.sender,
            receiver,
            reply_key: 1,
            reply_source: None,
            reply_requested: false,
            reply_removing: false,
            pending: OrderedTable::new(limit),
            bindings: OrderedTable::new(source_limit),
            deadlines: TimerQueue::new(0),
            completed_head: 0,
            completed_tail: 0,
            registrations: Sweep::default(),
            sends: Sweep::default(),
            removals: Sweep::default(),
            stopping: Sweep::default(),
            retiring: Sweep::default(),
            wake_pending: false,
            phase: 0,
            reply_rearm_needed: false,
            next_source_key: 2,
            sealed: false,
            fault: None,
        })
    }

    pub fn begin(
        &mut self,
        service: Capability,
        deadline: Deadline,
        request: Request,
    ) -> Result<u64, StartFailure> {
        self.begin_for(service, deadline, request, None)
    }

    /// 提交调用并在完成进入 FIFO 后请求唤醒指定 Runtime 任务。
    pub fn begin_for(
        &mut self,
        service: Capability,
        deadline: Deadline,
        mut request: Request,
        waiter: Option<u64>,
    ) -> Result<u64, StartFailure> {
        let early = (|| {
            if self.sealed {
                return Err(CallCause::Shutdown);
            }
            request.new_attempt().map_err(cause)?;
            if rinlib::time::expired(deadline).map_err(cause)? {
                return Err(CallCause::Timeout);
            }
            Ok(())
        })();
        if let Err(cause) = early {
            return Err(StartFailure {
                service,
                error: CallError::unsent(cause, request),
            });
        }

        let (service, description) = match MailboxSender::from_capability(service) {
            Ok(adopted) => adopted,
            Err(failure) => {
                return Err(StartFailure {
                    service: failure.owner,
                    error: CallError::unsent(cause(failure.error), request),
                });
            }
        };
        if !description.rights.contains(Rights::WRITE | Rights::WAIT) {
            return Err(StartFailure {
                service: service.into_capability(),
                error: CallError::unsent(CallCause::System(SystemCallError::RightsDenied), request),
            });
        }
        if let Err(error) = request.attach_reply(&self.sender) {
            return Err(StartFailure {
                service: service.into_capability(),
                error: CallError::unsent(cause(error), request),
            });
        }
        let storage = match MessageStorage::new() {
            Ok(storage) => storage,
            Err(error) => {
                return Err(StartFailure {
                    service: service.into_capability(),
                    error: CallError::unsent(cause(error), request),
                });
            }
        };
        let expires_at = match deadline.instant() {
            Ok(expires_at) => expires_at,
            Err(_) => {
                return Err(StartFailure {
                    service: service.into_capability(),
                    error: CallError::unsent(
                        CallCause::System(SystemCallError::IllegalArgument),
                        request,
                    ),
                });
            }
        };
        let closed_key = match self.allocate_source_key() {
            Ok(key) => key,
            Err(error) => {
                return Err(StartFailure {
                    service: service.into_capability(),
                    error: CallError::unsent(cause(error), request),
                });
            }
        };
        let writable_key = match self.allocate_source_key() {
            Ok(key) => key,
            Err(error) => {
                return Err(StartFailure {
                    service: service.into_capability(),
                    error: CallError::unsent(cause(error), request),
                });
            }
        };

        let txid = request.txid();
        let mut pending = PendingCall {
            service,
            deadline,
            protocol: request.protocol(),
            request: Some(request),
            result: None,
            storage: Some(storage),
            timer: None,
            writable: SourceState {
                key: writable_key,
                source: None,
                requested: false,
                removing: false,
                rearm_needed: false,
            },
            closed: SourceState {
                key: closed_key,
                source: None,
                requested: false,
                removing: false,
                rearm_needed: false,
            },
            stage: OutboundStage::Ready,
            queued: false,
            waiter,
            wake_queued: false,
            previous: 0,
            next: 0,
        };
        if let Some(at) = expires_at {
            pending.timer = match self.deadlines.try_register(at, txid) {
                Ok(timer) => Some(timer),
                Err(_) => {
                    return self
                        .start_failure(pending, CallCause::System(SystemCallError::OutOfMemory));
                }
            };
        }
        let prepared = match self.pending.prepare_insert(txid, pending) {
            Ok(prepared) => prepared,
            Err(error) => {
                let (error, pending) = match error {
                    ordered_table::InsertError::Limit(value) => {
                        (SystemCallError::ReachLimit, value)
                    }
                    ordered_table::InsertError::Allocation(value) => {
                        (SystemCallError::OutOfMemory, value)
                    }
                };
                return self.start_failure(pending, cause(error));
            }
        };
        if let Err(error) = self.bindings.try_insert(
            closed_key,
            SourceBinding {
                txid,
                role: SourceRole::Closed,
            },
        ) {
            return self.start_failure(
                prepared.into_value(),
                cause(match error {
                    ordered_table::InsertError::Limit(_) => SystemCallError::ReachLimit,
                    ordered_table::InsertError::Allocation(_) => SystemCallError::OutOfMemory,
                }),
            );
        }
        if let Err(error) = self.bindings.try_insert(
            writable_key,
            SourceBinding {
                txid,
                role: SourceRole::Writable,
            },
        ) {
            self.bindings.remove(closed_key);
            return self.start_failure(
                prepared.into_value(),
                cause(match error {
                    ordered_table::InsertError::Limit(_) => SystemCallError::ReachLimit,
                    ordered_table::InsertError::Allocation(_) => SystemCallError::OutOfMemory,
                }),
            );
        }
        self.pending.insert_prepared(prepared);
        self.registrations.request();
        self.sends.request();
        Ok(txid)
    }

    fn start_failure(
        &mut self,
        mut pending: PendingCall,
        cause: CallCause,
    ) -> Result<u64, StartFailure> {
        if let Some(timer) = pending.timer.take() {
            self.deadlines.cancel(timer);
        }
        let request = pending
            .request
            .take()
            .expect("unsent call lost its request");
        Err(StartFailure {
            service: pending.service.into_capability(),
            error: CallError::unsent(cause, request),
        })
    }

    fn allocate_source_key(&mut self) -> Result<u64, SystemCallError> {
        let key = self.next_source_key;
        self.next_source_key = key.checked_add(1).ok_or(SystemCallError::ReachLimit)?;
        Ok(key)
    }

    fn clean(pending: &PendingCall) -> bool {
        [(&pending.writable), (&pending.closed)]
            .into_iter()
            .all(|source| source.source.is_none() && !source.requested && !source.removing)
    }

    fn queue_completion(&mut self, txid: u64) {
        let previous = self.completed_tail;
        let should_queue = self.pending.get(txid).is_some_and(|pending| {
            pending.result.is_some() && !pending.queued && Self::clean(pending)
        });
        if !should_queue {
            return;
        }
        {
            let pending = self
                .pending
                .get_mut(txid)
                .expect("completed RPC disappeared");
            pending.queued = true;
            pending.previous = previous;
        }
        if previous == 0 {
            self.completed_head = txid;
        } else {
            self.pending
                .get_mut(previous)
                .expect("RPC completion tail disappeared")
                .next = txid;
        }
        self.completed_tail = txid;
    }

    fn wake_completion<F>(&mut self, requests: &mut Requests<F>) {
        self.wake_pending = false;
        let mut txid = self.completed_head;
        while txid != 0 {
            let next = self.pending.get(txid).map_or(0, |pending| pending.next);
            let Some(pending) = self.pending.get_mut(txid) else {
                break;
            };
            if let Some(waiter) = pending.waiter
                && !pending.wake_queued
            {
                if requests.wake(waiter).is_err() {
                    self.wake_pending = true;
                    break;
                }
                pending.wake_queued = true;
            }
            txid = next;
        }
    }

    fn complete(&mut self, txid: u64, result: Result<Reply, CallCause>) {
        let Some(pending) = self.pending.get_mut(txid) else {
            return;
        };
        if pending.result.is_some() {
            return;
        }
        assert!(pending.stage.finish(), "RPC completed twice");
        let result = match result {
            Ok(reply) => Ok(reply),
            Err(cause) => Err(match pending.request.take() {
                Some(request) => CallError::unsent(cause, request),
                None => CallError::sent(cause),
            }),
        };
        if let Some(timer) = pending.timer.take() {
            self.deadlines.cancel(timer);
        }
        pending.storage = None;
        pending.result = Some(result);
        self.removals.request();
        self.retiring.request();
        self.queue_completion(txid);
    }

    fn progress_send(&mut self, txid: u64) {
        let result = {
            let Some(pending) = self.pending.get_mut(txid) else {
                return;
            };
            let Some(request) = pending.request.as_mut() else {
                return;
            };
            if pending.result.is_some() {
                return;
            }
            request.try_send(&pending.service, pending.deadline)
        };
        match result {
            Ok(()) => {
                let pending = self
                    .pending
                    .get_mut(txid)
                    .expect("published RPC disappeared");
                assert!(pending.stage.sent(), "RPC published from invalid stage");
                pending.request = None;
                self.removals.request();
            }
            Err(SystemCallError::MailboxFull) => {
                assert!(
                    self.pending
                        .get_mut(txid)
                        .expect("blocked RPC disappeared")
                        .stage
                        .blocked(),
                    "RPC blocked from invalid stage"
                );
                if let Some(pending) = self.pending.get_mut(txid) {
                    pending.writable.rearm_needed = true;
                }
                self.registrations.request();
            }
            Err(error) => self.complete(txid, Err(cause(error))),
        }
    }

    fn receive_replies(&mut self, budget: usize) -> Result<usize, SystemCallError> {
        let mut used = 0;
        while used < budget {
            match self.receiver.receive(self.owner.as_handle()) {
                Ok(()) => {}
                Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => break,
                Err(error) => {
                    self.fault = Some(error);
                    self.begin_stop();
                    return Err(error);
                }
            }
            used += 1;
            let accepted = if let Ok(prefix) = RpcPrefix::decode(self.receiver.payload()) {
                if let Some(pending) = self.pending.get_mut(prefix.txid)
                    && pending.request.is_none()
                    && pending.result.is_none()
                {
                    let storage = pending
                        .storage
                        .take()
                        .expect("sent RPC lacks prepaid reply storage");
                    Some((
                        prefix.txid,
                        storage
                            .take(&mut self.receiver)
                            .map_err(cause)
                            .and_then(|message| {
                                Reply::accept(
                                    message,
                                    pending.protocol,
                                    prefix.txid,
                                    pending.deadline,
                                )
                            }),
                    ))
                } else {
                    None
                }
            } else {
                None
            };
            if let Some((txid, result)) = accepted {
                self.complete(txid, result);
            }
            self.receiver.discard();
        }
        Ok(used)
    }

    fn declare_source<F>(
        requests: &mut Requests<F>,
        state: &mut SourceState,
        plan: SourcePlan,
    ) -> bool {
        if state.removing {
            return false;
        }
        if let Some(source) = state.source {
            if state.rearm_needed && requests.rearm(source).is_ok() {
                state.rearm_needed = false;
                return true;
            }
            return false;
        }
        if state.requested {
            return false;
        }
        if requests.arm_source(plan, state.key).is_ok() {
            state.requested = true;
            true
        } else {
            false
        }
    }

    fn declare_remove<F>(requests: &mut Requests<F>, state: &mut SourceState) -> bool {
        let Some(source) = state.source else {
            return false;
        };
        if state.removing {
            return false;
        }
        if requests.remove(source).is_ok() {
            state.removing = true;
            true
        } else {
            false
        }
    }

    fn next_candidate(pending: &OrderedTable<PendingCall>, sweep: &mut Sweep) -> Option<u64> {
        let next = if sweep.active {
            pending
                .next_after((sweep.cursor != 0).then_some(&sweep.cursor))
                .map(|(&txid, _)| txid)
        } else {
            None
        };
        sweep.candidate(next)
    }

    fn progress_sends(&mut self) {
        let Some(txid) = Self::next_candidate(&self.pending, &mut self.sends) else {
            return;
        };
        let ready = self.pending.get(txid).is_some_and(|pending| {
            pending.stage.can_attempt()
                && pending.closed.source.is_some()
                && pending.writable.source.is_some()
                && self.reply_source.is_some()
        });
        if ready && !self.sealed {
            self.progress_send(txid);
        }
    }

    fn declare_registrations<F>(&mut self, requests: &mut Requests<F>) {
        if self.sealed {
            self.registrations = Sweep::default();
            return;
        }
        if self.reply_source.is_none() && !self.reply_requested && !self.reply_removing {
            if requests
                .arm_source(
                    SourcePlan::new(
                        self.owner.as_handle(),
                        ObjectSignals::READABLE | ObjectSignals::CLOSED,
                    ),
                    self.reply_key,
                )
                .is_ok()
            {
                self.reply_requested = true;
            } else {
                self.registrations.request();
            }
            return;
        }
        if self.reply_rearm_needed {
            if let Some(source) = self.reply_source
                && requests.rearm(source).is_ok()
            {
                self.reply_rearm_needed = false;
            } else {
                self.registrations.request();
            }
            return;
        }
        let Some(txid) = Self::next_candidate(&self.pending, &mut self.registrations) else {
            return;
        };
        let pending = self
            .pending
            .get_mut(txid)
            .expect("registration candidate disappeared");
        if pending.result.is_some() {
            return;
        }
        let declared_closed = Self::declare_source(
            requests,
            &mut pending.closed,
            SourcePlan::new(pending.service.as_handle(), ObjectSignals::CLOSED),
        );
        let declared_writable = pending.request.is_some()
            && Self::declare_source(
                requests,
                &mut pending.writable,
                SourcePlan::new(
                    pending.service.as_handle(),
                    ObjectSignals::WRITABLE | ObjectSignals::CLOSED,
                ),
            );
        let incomplete = pending.result.is_none()
            && (pending.closed.source.is_none()
                || (pending.request.is_some() && pending.writable.source.is_none()));
        if declared_closed || declared_writable || incomplete {
            self.registrations.request();
        }
    }

    fn declare_removals<F>(&mut self, requests: &mut Requests<F>) {
        if self.sealed
            && !self.reply_removing
            && let Some(source) = self.reply_source
        {
            if requests.remove(source).is_ok() {
                self.reply_removing = true;
            }
            return;
        }
        if let Some(txid) = Self::next_candidate(&self.pending, &mut self.removals) {
            let pending = self
                .pending
                .get_mut(txid)
                .expect("removal candidate disappeared");
            let remove_writable = (pending.request.is_none() || pending.result.is_some())
                && Self::declare_remove(requests, &mut pending.writable);
            let remove_closed =
                pending.result.is_some() && Self::declare_remove(requests, &mut pending.closed);
            let needs_retry = pending.result.is_some() && !Self::clean(pending);
            if remove_writable || remove_closed {
                self.removals.request();
            }
            if pending.result.is_some() {
                self.queue_completion(txid);
            }
            if needs_retry {
                self.removals.request();
            }
        }
    }

    fn process_event(&mut self, event: SourceEvent) -> Result<usize, SystemCallError> {
        if event.kind == self.reply_key {
            if event.error != 0 {
                self.fault = Some(
                    SystemCallError::from_u32(event.error)
                        .unwrap_or(SystemCallError::InternalError),
                );
                self.begin_stop();
                return Ok(1);
            }
            if event.observed.intersects(ObjectSignals::CLOSED) {
                self.fault = Some(SystemCallError::ObjectClosed);
                self.begin_stop();
                return Ok(1);
            }
            let used = self.receive_replies(1)?;
            self.reply_rearm_needed = true;
            return Ok(used.max(1));
        }
        let Some(binding) = self.bindings.get(event.kind).copied() else {
            return Ok(1);
        };
        if event.error != 0 {
            self.complete(
                binding.txid,
                Err(CallCause::System(
                    SystemCallError::from_u32(event.error)
                        .unwrap_or(SystemCallError::InternalError),
                )),
            );
            return Ok(1);
        }
        if event.observed.intersects(ObjectSignals::CLOSED) || binding.role == SourceRole::Closed {
            self.complete(binding.txid, Err(CallCause::ServiceClosed));
            return Ok(1);
        }
        if binding.role == SourceRole::Writable {
            if let Some(pending) = self.pending.get_mut(binding.txid) {
                let _ = pending.stage.writable();
            }
            self.sends.request();
        }
        Ok(1)
    }

    fn begin_stop(&mut self) {
        if !self.sealed {
            self.sealed = true;
            self.stopping.request();
            self.removals.request();
            self.retiring.request();
        }
    }

    fn finish_one(&mut self) {
        let failure = self.fault.map_or(CallCause::Shutdown, cause);
        if let Some(txid) = Self::next_candidate(&self.pending, &mut self.stopping)
            && self
                .pending
                .get(txid)
                .is_some_and(|pending| pending.result.is_none())
        {
            self.complete(txid, Err(failure));
        }
    }

    fn retire_abandoned(&mut self) {
        if !self.sealed {
            self.retiring = Sweep::default();
            return;
        }
        if let Some(txid) = Self::next_candidate(&self.pending, &mut self.retiring) {
            drop(self.take(txid));
        }
    }

    fn has_immediate_work(&self) -> bool {
        self.registrations.active
            || self.sends.active
            || self.removals.active
            || self.stopping.active
            || self.retiring.active
            || self.wake_pending
            || (!self.sealed
                && (self.reply_rearm_needed
                    || (self.reply_source.is_none() && !self.reply_requested)))
    }

    /// 仅终结本地等待；已投递请求不会被服务端撤回。
    pub fn cancel(&mut self, txid: u64) -> bool {
        if !self
            .pending
            .get(txid)
            .is_some_and(|pending| pending.result.is_none())
        {
            return false;
        }
        self.complete(txid, Err(CallCause::Shutdown));
        true
    }

    pub fn has_completed(&self) -> bool {
        self.completed_head != 0
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    pub fn take(&mut self, txid: u64) -> Option<Completion> {
        if !self.pending.get(txid).is_some_and(|pending| pending.queued) {
            return None;
        }
        let mut pending = self.pending.remove(txid)?;
        self.bindings.remove(pending.writable.key);
        self.bindings.remove(pending.closed.key);
        if pending.previous == 0 {
            self.completed_head = pending.next;
        } else {
            self.pending
                .get_mut(pending.previous)
                .expect("RPC completion predecessor disappeared")
                .next = pending.next;
        }
        if pending.next == 0 {
            self.completed_tail = pending.previous;
        } else {
            self.pending
                .get_mut(pending.next)
                .expect("RPC completion successor disappeared")
                .previous = pending.previous;
        }
        Some(Completion {
            service: pending.service.into_capability(),
            result: pending
                .result
                .take()
                .expect("RPC completion lost its result"),
        })
    }

    pub fn pop_completed(&mut self) -> Option<(u64, Completion)> {
        let txid = self.completed_head;
        if txid == 0 {
            return None;
        }
        self.take(txid).map(|completion| (txid, completion))
    }
}

impl Dispatcher {
    pub fn advance<F>(
        &mut self,
        requests: &mut Requests<F>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if budget == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        for _ in 0..budget {
            let phase = self.phase;
            self.phase = (self.phase + 1) % 7;
            match phase {
                0 => {
                    if let Some(event) = input.pull() {
                        self.process_event(event)?;
                    }
                }
                1 => {
                    let timed_out = input.take_timeout();
                    if let Some((_, txid)) = self.deadlines.pop_expired(input.now_ns()) {
                        if let Some(pending) = self.pending.get_mut(txid) {
                            pending.timer = None;
                        }
                        self.complete(txid, Err(CallCause::Timeout));
                    } else if timed_out {
                        // Runtime 的期限与 Dispatcher 的内部堆共享同一绝对时钟。
                        // 若当前轮尚未弹出对应项，保留输入交给 Runtime 重新投递。
                        self.phase = 1;
                    }
                }
                2 => self.declare_registrations(requests),
                3 => self.progress_sends(),
                4 => self.declare_removals(requests),
                5 => self.finish_one(),
                _ => self.retire_abandoned(),
            }
        }
        self.wake_completion(requests);

        let complete = self.sealed
            && self.pending.is_empty()
            && self.bindings.is_empty()
            && self.reply_source.is_none()
            && !self.reply_requested
            && !self.reply_removing;
        Ok(Advance {
            work_done: budget,
            step: if complete {
                Step::Complete
            } else if input.has_pending() || self.has_immediate_work() {
                Step::Runnable
            } else {
                Step::Parked
            },
        })
    }

    pub fn refused<W, F>(&mut self, _world: &mut W, failure: RequestFailure<F>) {
        match failure {
            RequestFailure::Source { kind, error } => {
                if kind == self.reply_key {
                    self.reply_requested = false;
                    self.fault = Some(error);
                    self.begin_stop();
                    return;
                }
                let Some(binding) = self.bindings.get(kind).copied() else {
                    return;
                };
                if let Some(pending) = self.pending.get_mut(binding.txid) {
                    match binding.role {
                        SourceRole::Writable => pending.writable.requested = false,
                        SourceRole::Closed => pending.closed.requested = false,
                    }
                }
                self.complete(binding.txid, Err(CallCause::System(error)));
            }
            RequestFailure::Wake { task, .. } => {
                let mut txid = self.completed_head;
                while txid != 0 {
                    let Some(pending) = self.pending.get_mut(txid) else {
                        break;
                    };
                    let next = pending.next;
                    if pending.waiter == Some(task) {
                        pending.waiter = None;
                        pending.wake_queued = false;
                    }
                    txid = next;
                }
                self.retiring.request();
            }
            RequestFailure::Spawn { .. } => {}
        }
    }

    pub fn registered<W>(&mut self, _world: &mut W, kind: u64, source: SourceId) {
        if kind == self.reply_key {
            self.reply_requested = false;
            self.reply_source = Some(source);
            return;
        }
        let Some(binding) = self.bindings.get(kind).copied() else {
            return;
        };
        if let Some(pending) = self.pending.get_mut(binding.txid) {
            let state = match binding.role {
                SourceRole::Writable => &mut pending.writable,
                SourceRole::Closed => &mut pending.closed,
            };
            state.requested = false;
            state.source = Some(source);
        }
        self.registrations.request();
        self.sends.request();
        self.removals.request();
    }

    pub fn unregistered<W>(&mut self, _world: &mut W, kind: u64, _source: SourceId) {
        if kind == self.reply_key {
            self.reply_source = None;
            self.reply_requested = false;
            self.reply_removing = false;
            return;
        }
        let Some(binding) = self.bindings.get(kind).copied() else {
            return;
        };
        if let Some(pending) = self.pending.get_mut(binding.txid) {
            let state = match binding.role {
                SourceRole::Writable => &mut pending.writable,
                SourceRole::Closed => &mut pending.closed,
            };
            state.source = None;
            state.requested = false;
            state.removing = false;
        }
        self.bindings.remove(kind);
        self.queue_completion(binding.txid);
        self.retiring.request();
    }

    pub fn stop<W>(&mut self, _world: &mut W) {
        self.begin_stop();
    }

    pub fn deadline(&self) -> Deadline {
        if self.sealed {
            Deadline::INFINITE
        } else {
            self.deadlines
                .peek_expires_at()
                .map_or(Deadline::INFINITE, Deadline::at)
        }
    }
}
