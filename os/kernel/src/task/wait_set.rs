//! 持久 one-shot 观察；来源完成不在来源锁内访问集合，收束共用显式预算。

use super::{
    Thread,
    object::{
        HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState,
        SubscribeResult,
    },
    proc::Process,
    resources::{IpcPermit, MetadataSponsor},
    wait::{ObserverSink, Subscription, WaitIdentity, WaitOutcome, WaitPlan},
};
use crate::{sync::Spinlock, uaccess};
use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    any::Any,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    wait::{WaitItem, WaitReason},
    wait_set::{RECEIVE_MAX, ReadyRecord},
};
use ordered_table::OrderedTable;
use wait_context::{ArmResult, OfferResult, WaitCore, WaitEpoch};

static NEXT_TOKEN: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

pub(crate) mod selftest;

pub(crate) struct ArmCycle {
    core: WaitCore<WaitOutcome>,
    removing: AtomicBool,
    target: Weak<dyn KernelObject>,
    token: u64,
    item: WaitItem,
    source: ObjectRef,
    _authority: ObjectRef,
    source_id: AtomicU64,
    finish: Spinlock<Option<super::notify_work::FinishReservation>>,
    _permit: IpcPermit,
}

impl ArmCycle {
    fn new(
        target: &ObjectRef,
        token: u64,
        item: WaitItem,
        source: ObjectRef,
        authority: ObjectRef,
    ) -> Result<Arc<Self>, SystemCallError> {
        let finish =
            super::notify_work::reserve_finish(super::notify_work::FinishClass::Persistent)
                .map_err(|_| SystemCallError::OutOfMemory)?;
        Arc::try_new(Self {
            core: WaitCore::new(),
            removing: AtomicBool::new(false),
            target: Arc::downgrade(target),
            token,
            item,
            source,
            _authority: authority,
            source_id: AtomicU64::new(0),
            finish: Spinlock::new(crate::sync::ranks::LEAF, Some(finish)),
            _permit: MetadataSponsor::reserve_ipc(
                &concrete(target)?.sponsor,
                super::resources::IpcClass::Registration,
            )?,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    fn install(self: &Arc<Self>, subscription: Subscription) -> Result<(), SystemCallError> {
        let installed = match self.source.subscribe(subscription) {
            SubscribeResult::Ready { outcome, retired } => {
                drop(retired);
                self.offer(outcome);
                Ok(())
            }
            SubscribeResult::Registered(id) => {
                self.source_id.store(id, Ordering::Release);
                Ok(())
            }
            SubscribeResult::ReachLimit(retired) => {
                drop(retired);
                Err(SystemCallError::ReachLimit)
            }
            SubscribeResult::OutOfMemory(retired) => {
                drop(retired);
                Err(SystemCallError::OutOfMemory)
            }
        };
        if let Err(error) = installed {
            self.offer(WaitOutcome::Error(error));
        }
        if self.removing.load(Ordering::Acquire) {
            self.cancel();
        }
        if self.arm() {
            self.clone().publish_finish();
        }
        installed
    }

    pub(crate) fn offer(&self, outcome: WaitOutcome) -> OfferResult {
        self.core.offer(outcome)
    }

    pub(crate) fn restart(&self) -> Result<u64, SystemCallError> {
        if self.removing.load(Ordering::Acquire) {
            return Err(SystemCallError::ObjectClosed);
        }
        if !self.core.is_done() {
            return Err(SystemCallError::ObjectBusy);
        }
        self.core
            .epoch()
            .next()
            .ok_or(SystemCallError::ReachLimit)?;
        // SAFETY: 仅由持有来源锁的 rearm_observer 调用，旧 ready 在完成槽归还后发布。
        if !unsafe { self.core.restart_done() } {
            return Err(SystemCallError::ObjectBusy);
        }
        Ok(self.core.epoch().value())
    }

    pub(crate) fn arm(&self) -> bool {
        matches!(self.core.arm(), ArmResult::Complete(_))
    }

    fn cancel(self: &Arc<Self>) {
        self.removing.store(true, Ordering::Release);
        let id = self.source_id.load(Ordering::Acquire);
        if id == 0 {
            if self.offer(WaitOutcome::Cancelled) == OfferResult::Complete {
                self.clone().publish_finish();
            }
        } else {
            let cancelled = self.source.cancel_observer(id);
            let _ = self
                .source_id
                .compare_exchange(id, 0, Ordering::AcqRel, Ordering::Acquire);
            if let Some(cancelled) = cancelled {
                let super::object::CancelledObservation {
                    sink,
                    result,
                    retired,
                } = cancelled;
                drop(retired);
                if result == OfferResult::Complete {
                    super::wait::finish_offered(sink);
                }
            }
        }
        if self.core.is_done()
            && let Some(target) = self.target.upgrade()
        {
            concrete(&target)
                .expect("WaitSet cancellation target changed kind")
                .finish_cycle(self, None);
        }
    }

    pub(crate) fn publish_finish(self: Arc<Self>) {
        let reservation = self
            .finish
            .lock()
            .take()
            .expect("WaitSet arm completion published twice");
        super::notify_work::publish_finish(reservation, ObserverSink::Persistent(self));
    }

    pub(crate) fn finish_step(&self, _: usize) -> (usize, bool) {
        if self.removing.load(Ordering::Acquire) {
            let source_id = self.source_id.swap(0, Ordering::AcqRel);
            if source_id != 0 {
                self.source.unsubscribe(source_id);
                return (1, false);
            }
        }
        (1, true)
    }

    pub(crate) fn return_finish(&self, reservation: super::notify_work::FinishReservation) {
        let epoch = self.core.epoch();
        let outcome = self.core.outcome_in(epoch);
        assert!(
            self.finish.lock().replace(reservation).is_none(),
            "persistent completion slot returned twice"
        );
        assert!(
            self.core.mark_done_in(epoch),
            "persistent finish changed epoch before Done"
        );
        if let Some(target) = self.target.upgrade() {
            concrete(&target)
                .expect("WaitSet completion target changed kind")
                .finish_cycle(self, Some((epoch, outcome)));
        }
    }
}

struct Registration {
    completed: Option<(WaitEpoch, WaitOutcome)>,
    operations: usize,
    retire_next: u64,
    cycle: Arc<ArmCycle>,
    removing: bool,
    consumed: bool,
    queued: Option<ReadyRecord>,
    previous: u64,
    next: u64,
}

struct SetState {
    wait: ObjectWaitState,
    entries: OrderedTable<Registration>,
    head: u64,
    tail: u64,
    closing: bool,
    retire_head: u64,
    retire_tail: u64,
    actor_active: bool,
    actor: Option<super::retirement::Reservation>,
    reply: Option<WaitIdentity>,
    reply_plan: Option<WaitPlan>,
    reply_committed: bool,
    mandatory: Option<Arc<Process>>,
}

impl SetState {
    fn publish(&mut self) {
        let mut level = ObjectSignals::NONE;
        if self.head != 0 && !self.closing {
            level |= ObjectSignals::READABLE;
        }
        if self.closing {
            level |= ObjectSignals::CLOSED;
        }
        self.wait
            .update(ObjectSignals::READABLE | ObjectSignals::CLOSED, level);
    }

    fn unlink(&mut self, token: u64) {
        let (previous, next, queued) = {
            let entry = self
                .entries
                .get(token)
                .expect("ready registration disappeared");
            (entry.previous, entry.next, entry.queued.is_some())
        };
        if !queued {
            return;
        }
        if previous == 0 {
            self.head = next
        } else {
            self.entries
                .get_mut(previous)
                .expect("ready predecessor disappeared")
                .next = next;
        }
        if next == 0 {
            self.tail = previous
        } else {
            self.entries
                .get_mut(next)
                .expect("ready successor disappeared")
                .previous = previous;
        }
        let entry = self
            .entries
            .get_mut(token)
            .expect("ready registration disappeared");
        entry.previous = 0;
        entry.next = 0;
        entry.queued = None;
    }

    fn begin_remove(&mut self, token: u64) -> Result<Arc<ArmCycle>, SystemCallError> {
        let entry = self
            .entries
            .get_mut(token)
            .ok_or(SystemCallError::ObjectNotFound)?;
        if entry.removing {
            return Err(SystemCallError::ObjectNotFound);
        }
        entry.removing = true;
        let tail = self.retire_tail;
        let cycle = entry.cycle.clone();
        cycle.removing.store(true, Ordering::Release);
        self.unlink(token);
        if tail == 0 {
            self.retire_head = token;
        } else {
            self.entries
                .get_mut(tail)
                .expect("retirement tail disappeared")
                .retire_next = token;
        }
        self.retire_tail = token;
        self.publish();
        Ok(cycle)
    }

    fn enqueue(&mut self, token: u64, record: ReadyRecord) {
        let tail = self.tail;
        if tail == 0 {
            self.head = token
        } else {
            self.entries
                .get_mut(tail)
                .expect("ready tail disappeared")
                .next = token;
        }
        let entry = self
            .entries
            .get_mut(token)
            .expect("ready registration disappeared");
        assert!(entry.queued.is_none(), "WaitSet arm queued twice");
        entry.queued = Some(record);
        entry.previous = tail;
        entry.next = 0;
        self.tail = token;
    }
}

pub struct WaitSet {
    header: ObjectHeader,
    state: Spinlock<SetState>,
    sponsor: Arc<MetadataSponsor>,
    _permit: IpcPermit,
    retirement_progress: crate::deferred_work::Dependency,
    retirement_completion: crate::deferred_work::Dependency,
    retired: AtomicBool,
}

impl WaitSet {
    fn new(limit: usize, sponsor: &Arc<MetadataSponsor>) -> Result<Arc<Self>, SystemCallError> {
        if limit == 0 || limit > handle_table::DEFAULT_HANDLE_LIMIT {
            return Err(SystemCallError::IllegalArgument);
        }
        let actor = super::retirement::reserve()?;
        let metadata = MetadataSponsor::reserve_kernel_wait(sponsor)?;
        let (reply, reply_plan) = super::wait::prepare_kernel(0, metadata)?;
        Arc::try_new(Self {
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            state: Spinlock::new(
                crate::sync::ranks::WAIT_SET,
                SetState {
                    wait: ObjectWaitState::new(ObjectSignals::NONE),
                    entries: OrderedTable::new(limit),
                    head: 0,
                    tail: 0,
                    closing: false,
                    retire_head: 0,
                    retire_tail: 0,
                    actor_active: false,
                    actor: Some(actor),
                    reply: Some(reply),
                    reply_plan: Some(reply_plan),
                    reply_committed: false,
                    mandatory: None,
                },
            ),
            sponsor: sponsor.clone(),
            _permit: MetadataSponsor::reserve_ipc(sponsor, super::resources::IpcClass::Object)?,
            retirement_progress: crate::deferred_work::Dependency::new(),
            retirement_completion: crate::deferred_work::Dependency::new(),
            retired: AtomicBool::new(false),
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    fn notify(&self) {
        let pending = self.state.lock().wait.take_notification();
        if let Some((reservation, target)) = pending {
            reservation.publish(target)
        }
    }

    fn finish_cycle(&self, cycle: &ArmCycle, completion: Option<(WaitEpoch, WaitOutcome)>) {
        {
            let mut state = self.state.lock();
            let closing = state.closing;
            let Some(entry) = state.entries.get_mut(cycle.token) else {
                return;
            };
            if !core::ptr::eq(entry.cycle.as_ref(), cycle) {
                return;
            }
            if let Some((epoch, outcome)) = completion {
                if cycle.core.epoch() != epoch {
                    return;
                }
                entry.completed = Some((epoch, outcome));
            }
            if !closing
                && !entry.removing
                && entry.operations == 0
                && entry.queued.is_none()
                && !entry.consumed
                && entry
                    .completed
                    .is_some_and(|(epoch, _)| epoch == cycle.core.epoch())
            {
                let (epoch, outcome) = entry
                    .completed
                    .expect("ready registration lost completion snapshot");
                let (observed, reason, error) = match outcome {
                    WaitOutcome::Object(result) => (result.observed, result.reason, 0),
                    WaitOutcome::Error(error) => (
                        ObjectSignals::NONE,
                        WaitReason::Signaled as u32,
                        error as u32,
                    ),
                    _ => (ObjectSignals::NONE, WaitReason::Cancelled as u32, 0),
                };
                state.enqueue(
                    cycle.token,
                    ReadyRecord {
                        token: cycle.token,
                        arm_generation: epoch.value(),
                        cookie: cycle.item.cookie,
                        observed,
                        reason,
                        error,
                    },
                );
            }
            state.publish();
        }
        self.retirement_progress.notify();
        self.notify();
    }

    fn remove(&self, object: ObjectRef, token: u64) -> Result<(), SystemCallError> {
        let (cycle, work) = {
            let mut state = self.state.lock();
            if state.closing {
                return Err(SystemCallError::ObjectClosed);
            }
            let cycle = state.begin_remove(token)?;
            let work = Self::start_actor(&mut state);
            (cycle, work)
        };
        cycle.cancel();
        super::retirement::Launch {
            object,
            work,
            reply: None,
        }
        .publish();
        Ok(())
    }

    fn start_actor(state: &mut SetState) -> Option<super::retirement::Reservation> {
        if state.actor_active {
            None
        } else {
            state.actor_active = true;
            Some(state.actor.take().expect("WaitSet lost its prepaid actor"))
        }
    }

    fn finish_operation(&self, token: u64) {
        let cycle = {
            let mut state = self.state.lock();
            let entry = state
                .entries
                .get_mut(token)
                .expect("WaitSet operation retired before completion");
            entry.operations = entry
                .operations
                .checked_sub(1)
                .expect("WaitSet operation completed twice");
            entry.cycle.clone()
        };
        if cycle.core.is_done() {
            self.finish_cycle(&cycle, None);
        }
        self.retirement_progress.notify();
    }
}

struct Operation {
    object: ObjectRef,
    token: u64,
}
impl Drop for Operation {
    fn drop(&mut self) {
        concrete(&self.object)
            .expect("WaitSet operation changed kind")
            .finish_operation(self.token);
    }
}

impl KernelObject for WaitSet {
    fn retirement(&self) -> Option<&dyn super::retirement::RetirementTarget> {
        Some(self)
    }
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::WaitSet
    }
    fn allowed_rights(&self, role: HandleRole) -> Option<Rights> {
        (role == HandleRole::WaitSetOwner)
            .then_some(Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT)
    }
    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals> {
        (role == HandleRole::WaitSetOwner)
            .then_some(ObjectSignals::READABLE | ObjectSignals::CLOSED)
    }
    fn signals(&self) -> ObjectSignals {
        self.state.lock().wait.signals()
    }
    fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
        self.state.lock().wait.subscribe(subscription)
    }
    fn rearm_observer(&self, id: u64) -> Result<super::object::ObserverRearm, SystemCallError> {
        self.state.lock().wait.rearm_observer(id)
    }

    fn cancel_observer(&self, id: u64) -> Option<super::object::CancelledObservation> {
        self.state.lock().wait.cancel_observer(id)
    }

    fn unsubscribe(&self, id: u64) {
        let retired = self.state.lock().wait.unsubscribe(id);
        drop(retired);
    }
    fn close_handle(&self, _: HandleRole, _: &Process, _: bool) {
        unreachable!("WaitSet owner must submit its prepaid retirement")
    }
    fn close_transit(&self, _: HandleRole) {
        unreachable!("WaitSet owner cannot enter messages")
    }
    fn advance_waiter(&self) -> super::object::WaitAdvance {
        self.state.lock().wait.advance_waiter()
    }
    fn complete_waiter_drain(
        &self,
        reservation: super::notify_work::Reservation,
    ) -> super::notify_work::Completion {
        self.state.lock().wait.complete_notification(reservation)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl super::retirement::RetirementTarget for WaitSet {
    fn begin(
        &self,
        object: ObjectRef,
        owner: Option<Arc<Process>>,
    ) -> Result<super::retirement::Launch, SystemCallError> {
        let mut state = self.state.lock();
        if state.closing {
            return Err(SystemCallError::ObjectClosed);
        }
        if let Some(owner) = owner.as_ref() {
            owner
                .lifecycle
                .commit_running(None, true, |_| {
                    state.closing = true;
                    state.publish();
                })
                .map_err(|_| SystemCallError::ObjectClosed)?;
        } else {
            state.closing = true;
            state.publish();
        }
        state.mandatory = owner;
        let reply = if state.mandatory.is_some() {
            state.reply_committed = true;
            let mut plan = state
                .reply_plan
                .take()
                .expect("WaitSet lost its Close reply");
            plan.committed_reply();
            Some(plan)
        } else {
            None
        };
        let work = Self::start_actor(&mut state);
        Ok(super::retirement::Launch {
            object,
            work,
            reply,
        })
    }

    fn step(&self, budget: usize) -> work_debt::StepResult<()> {
        use work_debt::{StepResult, StepState};
        let mut work = 0;
        while work < budget {
            let (cycle, retired) = {
                let mut state = self.state.lock();
                let token = state.retire_head;
                if token != 0 {
                    let entry = state
                        .entries
                        .get(token)
                        .expect("retirement head disappeared");
                    if entry.operations != 0 || !entry.cycle.core.is_done() {
                        return StepResult {
                            work_done: work,
                            state: StepState::Blocked(()),
                        };
                    }
                    if entry.cycle.source_id.load(Ordering::Acquire) != 0 {
                        (Some(entry.cycle.clone()), None)
                    } else {
                        state.retire_head = entry.retire_next;
                        if state.retire_head == 0 {
                            state.retire_tail = 0;
                        }
                        (None, state.entries.remove(token))
                    }
                } else if state.closing {
                    if let Some((&token, _)) = state.entries.next_after(None::<&u64>) {
                        (
                            Some(
                                state
                                    .begin_remove(token)
                                    .expect("close retirement token disappeared"),
                            ),
                            None,
                        )
                    } else {
                        let advance = state.wait.retire_closed_step();
                        drop(state);
                        if advance.finish() {
                            return StepResult {
                                work_done: work,
                                state: StepState::Complete,
                            };
                        }
                        work += 1;
                        continue;
                    }
                } else {
                    return StepResult {
                        work_done: work,
                        state: StepState::Complete,
                    };
                }
            };
            drop(retired);
            if let Some(cycle) = cycle {
                cycle.cancel();
            }
            work += 1;
        }
        StepResult {
            work_done: work,
            state: StepState::Runnable,
        }
    }

    fn register_progress(&self, wake: crate::deferred_work::WakeAction) {
        self.retirement_progress.register(wake, || {
            let state = self.state.lock();
            if state.retire_head == 0 {
                return true;
            }
            let entry = state
                .entries
                .get(state.retire_head)
                .expect("retirement dependency lost head");
            entry.operations == 0 && entry.cycle.core.is_done()
        });
    }

    fn publish_progress(&self) {
        self.notify();
        self.retirement_progress.notify();
    }

    fn finish(
        &self,
        reservation: super::retirement::Reservation,
        object: ObjectRef,
    ) -> Option<super::retirement::RetirementCompletion> {
        let again = {
            let mut state = self.state.lock();
            if state.retire_head != 0
                || (state.closing
                    && (!state.entries.is_empty() || !state.wait.subscriptions_retired()))
            {
                Some(reservation)
            } else {
                state.actor_active = false;
                if state.closing {
                    let reply = state.reply.take();
                    let (reply, unused_reply) = if state.reply_committed {
                        (reply, None)
                    } else {
                        (None, reply)
                    };
                    let plan = state.reply_plan.take();
                    let owner = state.mandatory.take();
                    drop(state);
                    drop(reservation);
                    drop(unused_reply);
                    drop(plan);
                    self.retired.store(true, Ordering::Release);
                    self.retirement_completion.notify();
                    return Some(super::retirement::RetirementCompletion { reply, owner });
                }
                assert!(
                    state.actor.replace(reservation).is_none(),
                    "WaitSet actor returned twice"
                );
                None
            }
        };
        if let Some(work) = again {
            work.publish(object);
        }
        None
    }

    fn completion(&self) -> &crate::deferred_work::Dependency {
        &self.retirement_completion
    }
    fn is_finished(&self) -> bool {
        self.retired.load(Ordering::Acquire)
    }
}

fn resolve(thread: &Thread, handle: Handle, rights: Rights) -> Result<ObjectRef, SystemCallError> {
    let table = thread.process.handles.lock();
    let entry = table
        .get(handle, rights)
        .map_err(super::handle::map_error)?;
    if *entry.role() != HandleRole::WaitSetOwner || entry.object().kind() != ObjectKind::WaitSet {
        return Err(SystemCallError::WrongObjectType);
    }
    Ok(entry.object().clone())
}

pub(crate) fn concrete(object: &ObjectRef) -> Result<&WaitSet, SystemCallError> {
    object
        .as_any()
        .downcast_ref()
        .ok_or(SystemCallError::WrongObjectType)
}

pub fn create(
    thread: &Thread,
    limit: usize,
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let set = WaitSet::new(limit, thread.process.resources.metadata())?;
    let entry = super::handle::entry(set, HandleRole::WaitSetOwner, rights)
        .map_err(super::handle::map_error)?;
    super::handle::install_one(thread, entry, output, || ())
}

pub fn remove(thread: &Thread, handle: Handle, token: u64) -> Result<(), SystemCallError> {
    let object = resolve(thread, handle, Rights::MANAGE)?;
    concrete(&object)?.remove(object.clone(), token)
}

pub fn register(
    thread: &Thread,
    handle: Handle,
    item_ptr: usize,
    output: usize,
) -> Result<(), SystemCallError> {
    let item: WaitItem = {
        let mut space = thread.process.space.lock();
        // SAFETY: 只含整数，下面验证所有业务字段。
        unsafe { uaccess::read_user_value(&mut space, item_ptr) }?
    };
    if item.reserved != 0 || item.signals == ObjectSignals::NONE || !item.signals.is_known() {
        return Err(SystemCallError::IllegalArgument);
    }
    let (set_object, authority, source) = {
        let table = thread.process.handles.lock();
        let set = table
            .get(handle, Rights::MANAGE)
            .map_err(super::handle::map_error)?;
        let item_entry = table
            .get(item.handle, Rights::WAIT)
            .map_err(super::handle::map_error)?;
        let allowed = item_entry
            .object()
            .allowed_signals(*item_entry.role())
            .ok_or(SystemCallError::WrongObjectType)?;
        if item.signals.raw() & !allowed.raw() != 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let authority = item_entry.object().clone();
        let source = authority
            .observation_source()
            .unwrap_or_else(|| authority.clone());
        (set.object().clone(), authority, source)
    };
    let set = concrete(&set_object)?;
    let target = set_object.clone();
    let token = NEXT_TOKEN.allocate().ok_or(SystemCallError::ReachLimit)?;
    let cycle = ArmCycle::new(&target, token, item, source.clone(), authority.clone())?;
    let subscription = Subscription::single(
        ObserverSink::Persistent(cycle.clone()),
        source,
        authority,
        item,
    )?;
    {
        let mut space = thread.process.space.lock();
        space.check_range(output, core::mem::size_of::<u64>(), true)?;
        let mut state = set.state.lock();
        if state.closing {
            return Err(SystemCallError::ObjectClosed);
        }
        let entry = Registration {
            completed: None,
            operations: 1,
            retire_next: 0,
            cycle: cycle.clone(),
            removing: false,
            consumed: false,
            queued: None,
            previous: 0,
            next: 0,
        };
        let prepared = state
            .entries
            .prepare_insert(token, entry)
            .map_err(|error| match error {
                ordered_table::InsertError::Limit(_) => SystemCallError::ReachLimit,
                ordered_table::InsertError::Allocation(_) => SystemCallError::OutOfMemory,
            })?;
        // SAFETY: 地址初检完成，先交付 token 后公开注册。失败无注册副作用。
        unsafe { uaccess::write_user_value(&mut space, output, &token) }?;
        state.entries.insert_prepared(prepared);
    }
    let _operation = Operation {
        object: set_object.clone(),
        token,
    };
    if let Err(error) = cycle.install(subscription) {
        match set.remove(set_object.clone(), token) {
            Ok(()) | Err(SystemCallError::ObjectNotFound) | Err(SystemCallError::ObjectClosed) => {}
            Err(cleanup) => return Err(cleanup),
        }
        return Err(error);
    }
    Ok(())
}

pub fn rearm(thread: &Thread, handle: Handle, token: u64) -> Result<u64, SystemCallError> {
    let object = resolve(thread, handle, Rights::MANAGE)?;
    let set = concrete(&object)?;
    let cycle = {
        let mut state = set.state.lock();
        if state.closing {
            return Err(SystemCallError::ObjectClosed);
        }
        let entry = state
            .entries
            .get_mut(token)
            .ok_or(SystemCallError::ObjectNotFound)?;
        if entry.removing {
            return Err(SystemCallError::ObjectNotFound);
        }
        if !entry.consumed || entry.queued.is_some() || !entry.cycle.core.is_done() {
            return Err(SystemCallError::ObjectBusy);
        }
        entry.consumed = false;
        entry.operations += 1;
        entry.cycle.clone()
    };
    let _operation = Operation {
        object: object.clone(),
        token,
    };
    let id = cycle.source_id.load(Ordering::Acquire);
    let rearmed = match cycle.source.rearm_observer(id) {
        Ok(rearmed) => rearmed,
        Err(error) => {
            let mut state = set.state.lock();
            if let Some(entry) = state.entries.get_mut(token)
                && !entry.removing
            {
                entry.consumed = true
            }
            return Err(error);
        }
    };
    if let Some(completion) = rearmed.completion {
        super::wait::finish_offered(completion)
    }
    Ok(rearmed.generation)
}

pub fn receive(
    thread: &Thread,
    handle: Handle,
    output: usize,
    capacity: usize,
    count_output: usize,
) -> Result<(), SystemCallError> {
    if capacity == 0 || capacity > RECEIVE_MAX {
        return Err(SystemCallError::IllegalArgument);
    }
    let object = resolve(thread, handle, Rights::READ)?;
    let set = concrete(&object)?;
    let mut records = Vec::new();
    records
        .try_reserve_exact(capacity)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    {
        let mut space = thread.process.space.lock();
        let mut state = set.state.lock();
        if state.closing {
            return Err(SystemCallError::ObjectClosed);
        }
        let mut token = state.head;
        while token != 0 && records.len() < capacity {
            let entry = state.entries.get(token).expect("ready token disappeared");
            records.push(entry.queued.expect("ready record disappeared"));
            token = entry.next;
        }
        if records.is_empty() {
            return Err(SystemCallError::ObjectNotAvailable);
        }
        // SAFETY: ReadyRecord 只含固定宽整数且无 padding。
        let bytes = unsafe {
            core::slice::from_raw_parts(
                records.as_ptr().cast::<u8>(),
                core::mem::size_of_val(records.as_slice()),
            )
        };
        uaccess::copy_to_user(&mut space, output, bytes)?;
        // SAFETY: count 为固定宽，所有写回成功后才消费 ready 队列。
        unsafe { uaccess::write_user_value(&mut space, count_output, &(records.len() as u32)) }?;
        for record in &records {
            state.unlink(record.token);
            state
                .entries
                .get_mut(record.token)
                .expect("consumed registration disappeared")
                .consumed = true;
        }
        state.publish();
    }
    set.notify();
    Ok(())
}
