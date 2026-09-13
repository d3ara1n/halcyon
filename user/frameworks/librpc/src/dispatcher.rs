//! 多 in-flight RPC；单请求退休不关闭共享 ReplyPort，输出与观察在 Send 前预付。

use core::sync::atomic::{AtomicUsize, Ordering};
use erhino_shared::{
    call::SystemCallError,
    object::{HandleRole, ObjectSignals, Rights},
    time::Deadline,
    wait::WaitItem,
    wait_set::ReadyRecord,
};
use ordered_table::OrderedTable;
use rinlib::ipc::{
    capability::Capability,
    message::{Mailbox, MessageStorage, ReceiveBuffer},
    wait_set::WaitSet,
};
use timer_queue::{TimerQueue, TimerToken};
use crate::{RpcPrefix, exchange::{CallCause, CallError, Reply, Request, cause}};

static ABANDONED: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy)]
enum SourceKind { Writable, Closed }

#[derive(Debug)]
struct SourceBinding {
    txid: u64,
    kind: SourceKind,
    generation: u64,
}

struct PendingCall {
    service: Capability,
    deadline: Deadline,
    protocol: u64,
    request: Option<Request>,
    result: Option<Result<Reply, CallError>>,
    storage: Option<MessageStorage>,
    timer: Option<TimerToken>,
    writable: Option<u64>,
    closed: Option<u64>,
    cleanup_error: Option<SystemCallError>,
    previous: u64,
    next: u64,
}

#[derive(Debug)]
pub struct Completion {
    pub service: Capability,
    pub result: Result<Reply, CallError>,
    pub cleanup_error: Option<SystemCallError>,
}

#[derive(Debug)]
pub struct StartFailure {
    pub service: Capability,
    pub error: CallError,
    pub cleanup_error: Option<SystemCallError>,
}

pub struct Dispatcher<'a> {
    set: &'a WaitSet,
    cookie: u64,
    owner: Mailbox,
    sender: Capability,
    receiver: ReceiveBuffer,
    reply_token: u64,
    reply_generation: u64,
    pending: OrderedTable<PendingCall>,
    sources: OrderedTable<SourceBinding>,
    deadlines: TimerQueue<u64>,
    completed_head: u64,
    completed_tail: u64,
    sealed: bool,
    fault: Option<SystemCallError>,
    limit: usize,
}

impl<'a> Dispatcher<'a> {
    pub fn new(set: &'a WaitSet, limit: usize, cookie: u64) -> Result<Self, SystemCallError> {
        if limit == 0 { return Err(SystemCallError::IllegalArgument) }
        let source_limit = limit.checked_mul(2).ok_or(SystemCallError::ReachLimit)?;
        let receiver = ReceiveBuffer::new()?;
        let owner = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)?;
        let minted = owner.mint(0, Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT)?;
        let reply_token = owner.register(set, cookie)?;
        Ok(Self { set, cookie, owner, sender: minted.sender, receiver, reply_token, reply_generation: 1,
            pending: OrderedTable::new(limit), sources: OrderedTable::new(source_limit),
            deadlines: TimerQueue::new(0), completed_head: 0, completed_tail: 0, sealed: false, fault: None, limit })
    }

    #[expect(clippy::result_large_err, reason = "准入失败原样返还服务授权及请求拥有者，错误路径不分配")]
    pub fn begin(&mut self, service: Capability, deadline: Deadline, mut request: Request) -> Result<u64, StartFailure> {
        let early = (|| {
            if self.sealed { return Err(CallCause::Shutdown) }
            request.new_attempt().map_err(cause)?;
            if rinlib::time::expired(deadline).map_err(cause)? { return Err(CallCause::Timeout) }
            let description = service.description().map_err(cause)?;
            if description.role != HandleRole::MailboxSender as u32 { return Err(CallCause::System(SystemCallError::WrongObjectType)) }
            if !description.rights.contains(Rights::WRITE | Rights::WAIT) { return Err(CallCause::System(SystemCallError::RightsDenied)) }
            request.attach_reply(&self.sender).map_err(cause)
        })();
        if let Err(cause) = early {
            return Err(StartFailure { service, error: CallError::unsent(cause, request), cleanup_error: None });
        }
        let txid = request.txid();
        let pending = PendingCall { service, deadline, protocol: request.protocol(), request: Some(request),
            result: None, storage: None, timer: None, writable: None, closed: None, cleanup_error: None, previous: 0, next: 0 };
        let mut prepared = match self.pending.prepare_insert(txid, pending) {
            Ok(prepared) => prepared,
            Err(error) => {
                let (error, pending) = match error {
                    ordered_table::InsertError::Limit(value) => (SystemCallError::ReachLimit, value),
                    ordered_table::InsertError::Allocation(value) => (SystemCallError::OutOfMemory, value),
                };
                return Err(Self::start_failure(pending, cause(error)));
            }
        };
        let setup = (|| {
            let pending = prepared.value_mut();
            pending.storage = Some(MessageStorage::new()?);
            if let Some(at) = deadline.instant().map_err(|_| SystemCallError::IllegalArgument)? {
                pending.timer = Some(self.deadlines.try_register(at, txid).map_err(|_| SystemCallError::OutOfMemory)?);
            }
            pending.closed = Some(self.register_source(pending.service.as_handle(), ObjectSignals::CLOSED, txid, SourceKind::Closed)?);
            pending.writable = Some(self.register_source(pending.service.as_handle(), ObjectSignals::WRITABLE | ObjectSignals::CLOSED, txid, SourceKind::Writable)?);
            Ok(())
        })();
        if let Err(error) = setup {
            let mut pending = prepared.into_value();
            self.retire_sources(&mut pending);
            return Err(Self::start_failure(pending, cause(error)));
        }
        self.pending.insert_prepared(prepared);
        self.progress_send(txid);
        Ok(txid)
    }

    fn start_failure(mut pending: PendingCall, cause: CallCause) -> StartFailure {
        let request = pending.request.take().expect("unsent call lost its request");
        StartFailure { service: pending.service, error: CallError::unsent(cause, request), cleanup_error: pending.cleanup_error }
    }

    fn register_source(&mut self, handle: erhino_shared::object::Handle, signals: ObjectSignals, txid: u64, kind: SourceKind) -> Result<u64, SystemCallError> {
        let token = self.set.register(WaitItem::new(handle, signals, self.cookie))?;
        let entry = SourceBinding { txid, kind, generation: 1 };
        let inserted = self.sources.try_insert(token, entry).map_err(|error| match error {
            ordered_table::InsertError::Limit(_) => SystemCallError::ReachLimit,
            ordered_table::InsertError::Allocation(_) => SystemCallError::OutOfMemory,
        });
        if let Err(error) = inserted { let _ = self.set.remove(token); return Err(error) }
        Ok(token)
    }

    fn unregister(set: &WaitSet, sources: &mut OrderedTable<SourceBinding>, token: u64) -> Option<SystemCallError> {
        sources.remove(token);
        match set.remove(token) {
            Ok(()) | Err(SystemCallError::ObjectNotFound) => None,
            Err(error) => Some(error),
        }
    }

    fn retire_sources(&mut self, pending: &mut PendingCall) {
        if let Some(timer) = pending.timer.take() { self.deadlines.cancel(timer); }
        for token in [pending.writable.take(), pending.closed.take()].into_iter().flatten() {
            let error = Self::unregister(self.set, &mut self.sources, token);
            if pending.cleanup_error.is_none() { pending.cleanup_error = error; }
        }
    }

    fn complete(&mut self, txid: u64, result: Result<Reply, CallCause>) {
        let Some(pending) = self.pending.get_mut(txid) else { return };
        if pending.result.is_some() { return }
        let result = match result {
            Ok(reply) => Ok(reply),
            Err(cause) => Err(match pending.request.take() {
                Some(request) => CallError::unsent(cause, request),
                None => CallError::sent(cause),
            }),
        };
        if let Some(timer) = pending.timer.take() { self.deadlines.cancel(timer); }
        for token in [pending.writable.take(), pending.closed.take()].into_iter().flatten() {
            let error = Self::unregister(self.set, &mut self.sources, token);
            if pending.cleanup_error.is_none() { pending.cleanup_error = error; }
        }
        pending.storage = None;
        pending.result = Some(result);
        pending.previous = self.completed_tail;
        let previous = self.completed_tail;
        if previous == 0 { self.completed_head = txid }
        else { self.pending.get_mut(previous).expect("RPC completion tail disappeared").next = txid; }
        self.completed_tail = txid;
    }

    fn progress_send(&mut self, txid: u64) {
        let send = {
            let Some(pending) = self.pending.get_mut(txid) else { return };
            let Some(request) = pending.request.as_mut() else { return };
            if pending.result.is_some() { return }
            request.try_send(&pending.service, pending.deadline)
        };
        match send {
            Ok(()) => {
                let pending = self.pending.get_mut(txid).expect("published RPC disappeared");
                pending.request = None;
                if let Some(token) = pending.writable.take() {
                    let error = Self::unregister(self.set, &mut self.sources, token);
                    if pending.cleanup_error.is_none() { pending.cleanup_error = error }
                }
            }
            Err(SystemCallError::MailboxFull) => (),
            Err(error) => self.complete(txid, Err(cause(error))),
        }
    }

    pub fn on_ready(&mut self, record: ReadyRecord, budget: usize) -> Result<usize, SystemCallError> {
        if budget == 0 { return Err(SystemCallError::IllegalArgument) }
        if self.fault.is_some() { return Ok(0) }
        if record.token == self.reply_token {
            if record.arm_generation != self.reply_generation { return Ok(0) }
            return self.receive_replies(budget);
        }
        let Some(binding) = self.sources.get(record.token) else { return Ok(0) };
        if binding.generation != record.arm_generation { return Ok(0) }
        let txid = binding.txid;
        let kind = binding.kind;
        if record.error != 0 {
            self.complete(txid, Err(CallCause::System(SystemCallError::InternalError)));
        } else if record.observed.intersects(ObjectSignals::CLOSED) {
            self.complete(txid, Err(CallCause::ServiceClosed));
        } else if matches!(kind, SourceKind::Writable) {
            self.progress_send(txid);
            if let Some(binding) = self.sources.get_mut(record.token) {
                match self.set.rearm(record.token) {
                    Ok(generation) => binding.generation = generation,
                    Err(error) => self.complete(txid, Err(cause(error))),
                }
            }
        }
        Ok(1)
    }

    fn receive_replies(&mut self, budget: usize) -> Result<usize, SystemCallError> {
        let mut used = 0;
        while used < budget {
            match self.receiver.receive(self.owner.as_handle()) {
                Ok(()) => (),
                Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => break,
                Err(error) => { self.fault = Some(error); self.sealed = true; return Err(error) }
            }
            used += 1;
            let prefix = RpcPrefix::decode(self.receiver.payload());
            if let Ok(prefix) = prefix
                && let Some(pending) = self.pending.get_mut(prefix.txid)
                && pending.request.is_none() && pending.result.is_none()
            {
                let storage = pending.storage.take().expect("sent RPC lacks prepaid reply storage");
                let result = storage.take(&mut self.receiver).map_err(cause)
                    .and_then(|message| Reply::accept(message, pending.protocol, prefix.txid, pending.deadline));
                self.complete(prefix.txid, result);
            }
            self.receiver.discard();
        }
        match self.set.rearm(self.reply_token) {
            Ok(generation) => self.reply_generation = generation,
            Err(error) => { self.fault = Some(error); self.sealed = true; return Err(error) }
        }
        Ok(used)
    }

    pub fn expire(&mut self, budget: usize) -> Result<usize, SystemCallError> {
        let now = rinlib::time::snapshot()?.now_ns;
        let mut used = 0;
        while used < budget {
            let Some((_, txid)) = self.deadlines.pop_expired(now) else { break };
            if let Some(pending) = self.pending.get_mut(txid) { pending.timer = None; }
            self.complete(txid, Err(CallCause::Timeout));
            used += 1;
        }
        Ok(used)
    }

    pub fn next_deadline(&self) -> Deadline {
        self.deadlines.peek_expires_at().map_or(Deadline::INFINITE, Deadline::at)
    }
    pub fn has_completed(&self) -> bool { self.completed_head != 0 }
    pub fn pending_count(&self) -> usize { self.pending.len() }

    pub fn take(&mut self, txid: u64) -> Option<Completion> {
        self.pending.get(txid)?.result.as_ref()?;
        let mut pending = self.pending.remove(txid).expect("completed RPC disappeared");
        if pending.previous == 0 { self.completed_head = pending.next }
        else { self.pending.get_mut(pending.previous).expect("RPC completion predecessor disappeared").next = pending.next; }
        if pending.next == 0 { self.completed_tail = pending.previous }
        else { self.pending.get_mut(pending.next).expect("RPC completion successor disappeared").previous = pending.previous; }
        Some(Completion { service: pending.service, result: pending.result.take().expect("RPC completion lost its result"), cleanup_error: pending.cleanup_error })
    }

    pub fn pop_completed(&mut self) -> Option<(u64, Completion)> {
        let txid = self.completed_head;
        if txid == 0 { return None }
        self.take(txid).map(|completion| (txid, completion))
    }

    /// 调用者明确放弃剩余结果；每轮只退休有限调用，服务端请求不会被撤回。
    pub fn shutdown_step(&mut self, budget: usize) -> usize {
        self.sealed = true;
        let mut used = 0;
        while used < budget {
            let mut ids = [0];
            if self.pending.scan_visible(|_| true, 0, &mut ids).0 == 0 { break }
            let txid = ids[0];
            if self.pending.get(txid).expect("RPC shutdown entry disappeared").result.is_none() {
                let error = self.fault.map_or(CallCause::Shutdown, cause);
                self.complete(txid, Err(error));
            }
            drop(self.take(txid));
            used += 1;
        }
        used
    }
}

impl Drop for Dispatcher<'_> {
    fn drop(&mut self) {
        let _ = self.set.remove(self.reply_token);
        if !self.pending.is_empty() {
            // 异常放弃不在控制循环中无界析构；entry 留在 HandleTable 由 ProcessDrain 接管。
            let pending = core::mem::replace(&mut self.pending, OrderedTable::new(self.limit));
            core::mem::forget(pending);
            let _ = ABANDONED.try_update(Ordering::AcqRel, Ordering::Acquire, |count| Some(count.saturating_add(1)));
        }
    }
}

pub fn abandoned_count() -> usize { ABANDONED.load(Ordering::Acquire) }
