//! 启动期按真实机制边界排列交错，不依赖时序概率或用户态消费。

use super::*;
use crate::task::Thread;
use crate::task::retirement::RetirementTarget;

pub(crate) mod continuation;

type TicketLedger = crate::work_ledger::DebtLedger<crate::task::handle::PendingClose, 1>;
static TICKETS: TicketLedger = TicketLedger::new(work_debt::TableId::new(9));

fn wake_ticket(token: work_debt::WakeToken) {
    TICKETS.wake_raw(token);
}

fn set(sponsor: &Arc<MetadataSponsor>) -> ObjectRef {
    WaitSet::new(4, sponsor).expect("WaitSet interleaving fixture allocation failed")
}

fn prepare(target: &ObjectRef, source: &ObjectRef) -> (Arc<ArmCycle>, Subscription, Operation) {
    let token = NEXT_TOKEN
        .allocate()
        .expect("WaitSet interleaving token exhausted");
    let item = WaitItem::new(Handle::INVALID, ObjectSignals::READABLE, token);
    let cycle = ArmCycle::new(target, token, item, source.clone(), source.clone())
        .expect("WaitSet interleaving cycle allocation failed");
    let subscription = Subscription::single(
        ObserverSink::Persistent(cycle.clone()),
        source.clone(),
        source.clone(),
        item,
    )
    .expect("WaitSet interleaving subscription allocation failed");
    concrete(target)
        .unwrap()
        .state
        .lock()
        .entries
        .try_insert(
            token,
            Registration {
                completed: None,
                operations: 1,
                retire_next: 0,
                cycle: cycle.clone(),
                removing: false,
                consumed: false,
                queued: None,
                previous: 0,
                next: 0,
            },
        )
        .unwrap_or_else(|_| panic!("WaitSet interleaving registration allocation failed"));
    (
        cycle,
        subscription,
        Operation {
            object: target.clone(),
            token,
        },
    )
}

fn settle() {
    for _ in 0..128 {
        if super::super::notify_work::drain_current() == 0 {
            return;
        }
    }
    panic!("WaitSet interleaving cleanup failed to converge");
}

fn retire(object: &ObjectRef) {
    concrete(object)
        .unwrap()
        .begin(object.clone(), None)
        .expect("WaitSet fixture retirement failed")
        .publish();
    settle();
    assert!(
        concrete(object).unwrap().is_finished(),
        "WaitSet fixture retirement did not finish"
    );
}

fn consume(target: &ObjectRef, token: u64) -> (WaitEpoch, WaitOutcome) {
    let mut state = concrete(target).unwrap().state.lock();
    let snapshot = state
        .entries
        .get(token)
        .unwrap()
        .completed
        .expect("WaitSet fixture completion was not published");
    state.unlink(token);
    state.entries.get_mut(token).unwrap().consumed = true;
    state.publish();
    snapshot
}

fn stale_completion(sponsor: &Arc<MetadataSponsor>) {
    let source = set(sponsor);
    let target = set(sponsor);
    let (cycle, subscription, operation) = prepare(&target, &source);
    cycle
        .install(subscription)
        .expect("WaitSet stale completion installation failed");
    drop(operation);
    let source_set = concrete(&source).unwrap();
    source_set
        .state
        .lock()
        .wait
        .update(ObjectSignals::READABLE, ObjectSignals::READABLE);
    source_set.notify();
    settle();
    let old = consume(&target, cycle.token);
    source_set
        .state
        .lock()
        .wait
        .update(ObjectSignals::READABLE, ObjectSignals::NONE);
    let operation = {
        let mut state = concrete(&target).unwrap().state.lock();
        let entry = state.entries.get_mut(cycle.token).unwrap();
        entry.operations += 1;
        entry.consumed = false;
        Operation {
            object: target.clone(),
            token: cycle.token,
        }
    };
    let rearmed = source
        .rearm_observer(cycle.source_id.load(Ordering::Acquire))
        .expect("WaitSet stale completion rearm failed");
    assert!(
        rearmed.completion.is_none(),
        "empty source completed its next arm"
    );
    drop(operation);
    concrete(&target).unwrap().finish_cycle(&cycle, Some(old));
    assert!(
        concrete(&target)
            .unwrap()
            .state
            .lock()
            .entries
            .get(cycle.token)
            .unwrap()
            .queued
            .is_none(),
        "stale completion published a next-cycle record"
    );
    assert!(
        !cycle.core.is_done(),
        "stale completion completed the next cycle"
    );
    retire(&target);
    retire(&source);
}

fn closed_seen(sponsor: &Arc<MetadataSponsor>) {
    let source = set(sponsor);
    let target = set(sponsor);
    let (cycle, subscription, operation) = prepare(&target, &source);
    cycle
        .install(subscription)
        .expect("WaitSet closed-seen installation failed");
    drop(operation);
    let source_set = concrete(&source).unwrap();
    source_set
        .state
        .lock()
        .wait
        .update(ObjectSignals::READABLE, ObjectSignals::READABLE);
    source_set.notify();
    settle();
    consume(&target, cycle.token);
    source_set
        .state
        .lock()
        .wait
        .update(ObjectSignals::READABLE, ObjectSignals::CLOSED);
    source_set.notify();
    let rearmed = source
        .rearm_observer(cycle.source_id.load(Ordering::Acquire))
        .expect("WaitSet closed-seen rearm failed");
    super::super::wait::finish_offered(
        rearmed
            .completion
            .expect("closed source did not complete rearm"),
    );
    // CLOSED 代次已被 Rearm 记入 seen；来源 actor 尚未提交，唯有通知扫描负责摘槽。
    settle();
    assert_eq!(
        source_set.state.lock().wait.active_waiters_for_test(),
        0,
        "closed-seen scan retained a source subscription"
    );
    retire(&target);
    retire(&source);
}

fn late_install(sponsor: &Arc<MetadataSponsor>) {
    let source = set(sponsor);
    let target = set(sponsor);
    let (cycle, subscription, operation) = prepare(&target, &source);
    retire(&source);
    cycle
        .install(subscription)
        .expect("closed source late installation failed");
    drop(operation);
    settle();
    assert_eq!(
        concrete(&source)
            .unwrap()
            .state
            .lock()
            .wait
            .active_waiters_for_test(),
        0,
        "closed source accepted a late persistent slot"
    );
    // 不消费 ready，不 Remove；关闭来源仍不持有新的来源责任。
    assert_eq!(
        concrete(&target)
            .unwrap()
            .state
            .lock()
            .entries
            .get(cycle.token)
            .unwrap()
            .queued
            .expect("late installation did not publish its terminal record")
            .reason,
        WaitReason::Closed as u32,
        "late installation failed to deliver Closed"
    );
    retire(&target);
}

fn retirement_history(sponsor: &Arc<MetadataSponsor>, notification_first: bool) {
    let source = set(sponsor);
    let target = set(sponsor);
    let (cycle, subscription, operation) = prepare(&target, &source);
    cycle
        .install(subscription)
        .expect("retirement history installation failed");
    drop(operation);
    let source_set = concrete(&source).unwrap();
    {
        let mut state = source_set.state.lock();
        state
            .wait
            .update(ObjectSignals::READABLE, ObjectSignals::READABLE);
        state
            .wait
            .update(ObjectSignals::READABLE, ObjectSignals::CLOSED);
    }
    if notification_first {
        source_set.notify();
    } else {
        let advance = source_set.state.lock().wait.retire_closed_step();
        assert!(
            !advance.finish(),
            "retirement history skipped its subscribed slot"
        );
    }
    settle();
    assert_eq!(
        source_set.state.lock().wait.active_waiters_for_test(),
        0,
        "CLOSED history completion retained its source subscription"
    );
    let (_, outcome) = consume(&target, cycle.token);
    let WaitOutcome::Object(result) = outcome else {
        panic!("retirement history lost object outcome")
    };
    assert_eq!(
        result.reason,
        WaitReason::Signaled as u32,
        "retirement replaced earlier activation with Closed"
    );
    assert!(
        result.observed.intersects(ObjectSignals::READABLE),
        "retirement lost earlier Readable snapshot"
    );
    retire(&target);
    retire(&source);
}

fn terminal_deferred(sponsor: &Arc<MetadataSponsor>) {
    let source = set(sponsor);
    let target = set(sponsor);
    let (cycle, subscription, operation) = prepare(&target, &source);
    let SubscribeResult::Registered(id) = source.subscribe(subscription) else {
        panic!("Deferred fixture failed to install its unarmed source slot")
    };
    cycle.source_id.store(id, Ordering::Release);
    let source_set = concrete(&source).unwrap();
    source_set
        .state
        .lock()
        .wait
        .update(ObjectSignals::NONE, ObjectSignals::CLOSED);
    source_set.notify();
    settle();
    assert_eq!(
        source_set.state.lock().wait.active_waiters_for_test(),
        0,
        "terminal Deferred offer retained its source subscription"
    );
    assert!(
        !cycle.core.is_done(),
        "Deferred source offer completed an unarmed cycle"
    );
    assert!(
        cycle.arm(),
        "terminal Deferred outcome was not latched for arm"
    );
    cycle.clone().publish_finish();
    drop(operation);
    settle();
    let (_, outcome) = consume(&target, cycle.token);
    let WaitOutcome::Object(result) = outcome else {
        panic!("Deferred terminal outcome disappeared")
    };
    assert_eq!(result.reason, WaitReason::Closed as u32);
    retire(&target);
    retire(&source);
}

fn registered_operation(sponsor: &Arc<MetadataSponsor>) {
    let source = set(sponsor);
    let target = set(sponsor);
    let (cycle, subscription, operation) = prepare(&target, &source);
    concrete(&target)
        .unwrap()
        .begin(target.clone(), None)
        .unwrap()
        .publish();
    settle();
    assert!(
        !concrete(&target).unwrap().is_finished(),
        "close retired an installing operation"
    );
    cycle
        .install(subscription)
        .expect("closing target installation cleanup failed");
    settle();
    assert!(
        concrete(&target)
            .unwrap()
            .state
            .lock()
            .entries
            .contains_key(cycle.token),
        "close removed an operation before its responsibility returned"
    );
    drop(operation);
    settle();
    assert!(
        concrete(&target).unwrap().is_finished(),
        "operation return failed to wake retirement"
    );
    retire(&source);
}

fn rearm_retirement(sponsor: &Arc<MetadataSponsor>, reset_first: bool, closing: bool) {
    let source = set(sponsor);
    let target = set(sponsor);
    let (cycle, subscription, operation) = prepare(&target, &source);
    cycle
        .install(subscription)
        .expect("rearm retirement installation failed");
    drop(operation);
    let source_set = concrete(&source).unwrap();
    source_set
        .state
        .lock()
        .wait
        .update(ObjectSignals::READABLE, ObjectSignals::READABLE);
    source_set.notify();
    settle();
    consume(&target, cycle.token);
    source_set
        .state
        .lock()
        .wait
        .update(ObjectSignals::READABLE, ObjectSignals::NONE);
    let operation = {
        let mut state = concrete(&target).unwrap().state.lock();
        let entry = state.entries.get_mut(cycle.token).unwrap();
        entry.operations += 1;
        entry.consumed = false;
        Operation {
            object: target.clone(),
            token: cycle.token,
        }
    };
    if reset_first {
        assert!(
            source
                .rearm_observer(cycle.source_id.load(Ordering::Acquire))
                .expect("rearm retirement source reset failed")
                .completion
                .is_none()
        );
    }
    if closing {
        concrete(&target)
            .unwrap()
            .begin(target.clone(), None)
            .unwrap()
            .publish();
    } else {
        concrete(&target)
            .unwrap()
            .remove(target.clone(), cycle.token)
            .unwrap();
    }
    settle();
    assert!(
        concrete(&target)
            .unwrap()
            .state
            .lock()
            .entries
            .contains_key(cycle.token),
        "retirement removed an in-flight rearm operation"
    );
    if !reset_first {
        assert!(
            matches!(
                source.rearm_observer(cycle.source_id.load(Ordering::Acquire)),
                Err(SystemCallError::ObjectNotFound) | Err(SystemCallError::ObjectClosed)
            ),
            "removed source accepted a late rearm"
        );
    }
    drop(operation);
    settle();
    assert!(
        !concrete(&target)
            .unwrap()
            .state
            .lock()
            .entries
            .contains_key(cycle.token),
        "rearm operation return did not wake its actor"
    );
    if !closing {
        retire(&target);
    }
    retire(&source);
}

fn completed_cycle(target: &ObjectRef, source: &ObjectRef) -> Arc<ArmCycle> {
    let (cycle, subscription, operation) = prepare(target, source);
    cycle
        .install(subscription)
        .expect("completed cycle fixture installation failed");
    drop(operation);
    let source_set = concrete(source).unwrap();
    source_set
        .state
        .lock()
        .wait
        .update(ObjectSignals::READABLE, ObjectSignals::READABLE);
    source_set.notify();
    settle();
    consume(target, cycle.token);
    source_set
        .state
        .lock()
        .wait
        .update(ObjectSignals::READABLE, ObjectSignals::NONE);
    cycle
}

fn actor_window(sponsor: &Arc<MetadataSponsor>, after_return: bool, closing: bool) {
    let source = set(sponsor);
    let target = set(sponsor);
    let old = completed_cycle(&target, &source);
    concrete(&target)
        .unwrap()
        .remove(target.clone(), old.token)
        .unwrap();
    super::super::retirement::selftest::complete_window(&target, after_return, || {
        let (cycle, subscription, operation) = prepare(&target, &source);
        cycle
            .install(subscription)
            .expect("actor completion concurrent installation failed");
        drop(operation);
        if closing {
            concrete(&target)
                .unwrap()
                .begin(target.clone(), None)
                .unwrap()
                .publish();
        } else {
            concrete(&target)
                .unwrap()
                .remove(target.clone(), cycle.token)
                .unwrap();
        }
    });
    settle();
    assert!(
        concrete(&target).unwrap().state.lock().entries.is_empty(),
        "republished actor lost concurrent registrations"
    );
    if closing {
        assert!(
            concrete(&target).unwrap().is_finished(),
            "republished close did not finish"
        );
    } else {
        retire(&target);
    }
    retire(&source);
}

fn caller() -> (Arc<Process>, Vec<Arc<Thread>>) {
    use erhino_shared::proc::ThreadStartContext;
    let process = Arc::new(
        Process::new(
            0,
            0,
            Weak::new(),
            super::super::resources::ProcessResources::try_new()
                .expect("Close caller sponsor failed"),
        )
        .expect("Close caller process failed"),
    );
    assert!(process.lifecycle.enter_building_op());
    process
        .lifecycle
        .attach_member(|_, member| {
            let thread = Thread::new_thread(
                member,
                &process,
                ThreadStartContext {
                    entry: 0,
                    stack_pointer: 0,
                    arg1: 0,
                    arg2: 0,
                },
            )
            .map_err(|_| super::super::lifecycle::AttachFault::Oom)?;
            Arc::try_new(thread).map_err(|_| super::super::lifecycle::AttachFault::Oom)
        })
        .expect("Close caller member failed");
    let mut threads = Vec::with_capacity(1);
    process
        .lifecycle
        .begin_running(1, &mut threads)
        .expect("Close caller start failed");
    (process, threads)
}

fn abandoned_close(sponsor: &Arc<MetadataSponsor>) {
    use erhino_shared::proc::ProcessExitReason;
    let source = set(sponsor);
    let target = set(sponsor);
    let (cycle, subscription, operation) = prepare(&target, &source);
    cycle
        .install(subscription)
        .expect("abandoned Close source installation failed");
    let (process, mut threads) = caller();
    let thread = threads.pop().unwrap();
    let member = thread.member();
    drop(thread);
    let reply_identity = concrete(&target)
        .unwrap()
        .state
        .lock()
        .reply
        .as_ref()
        .unwrap()
        .clone();
    let reply = concrete(&target)
        .unwrap()
        .begin(target.clone(), Some(process.clone()))
        .expect("Close caller commit failed")
        .publish()
        .expect("Close caller lost prepaid reply");
    let todo = process
        .lifecycle
        .request_termination(ProcessExitReason::Killed, 7, None);
    assert!(
        !todo.reapable,
        "Close caller became reapable before departure"
    );
    let (_, reapable) = process.lifecycle.thread_departed(member, None);
    assert!(
        !reapable,
        "Close mandatory responsibility did not delay caller reapability"
    );
    drop(reply);
    settle();
    super::super::wait::selftest::assert_cancelled(&reply_identity);
    assert!(
        !process.lifecycle.is_reapable(),
        "reply cancellation released actor mandatory responsibility"
    );
    assert!(
        !concrete(&target).unwrap().is_finished(),
        "Close bypassed outstanding installation responsibility"
    );
    drop(operation);
    settle();
    assert!(
        concrete(&target).unwrap().is_finished(),
        "abandoned reply cancelled committed retirement"
    );
    assert!(
        process.lifecycle.is_reapable(),
        "retirement did not release caller mandatory responsibility"
    );
    retire(&source);
}

fn ticket_completion(sponsor: &Arc<MetadataSponsor>, early: bool) {
    use crate::task::handle::PendingClose;
    let source = set(sponsor);
    let target = set(sponsor);
    let (cycle, subscription, operation) = prepare(&target, &source);
    cycle
        .install(subscription)
        .expect("retirement ticket installation failed");
    concrete(&target)
        .unwrap()
        .begin(target.clone(), None)
        .unwrap()
        .publish();
    settle();
    let process = Process::new(
        0,
        0,
        Weak::new(),
        super::super::resources::ProcessResources::try_new().expect("ticket owner sponsor failed"),
    )
    .expect("ticket owner process failed");
    let reservation = TICKETS.reserve().expect("ticket fixture admission failed");
    reservation.publish_quiet(PendingClose::Retirement(
        super::super::retirement::RetirementTicket::new(target.clone()),
    ));
    let owner = crate::hart::current().slot();
    let (token, ticket) = TICKETS.take(owner).unwrap().into_parts();
    let (used, ticket) = ticket.advance(&process, 1);
    assert_eq!(
        used, 0,
        "pending retirement ticket charged work while blocked"
    );
    let ticket = ticket.expect("pending retirement ticket completed too early");
    let PendingClose::Retirement(dependency) = &ticket else {
        panic!("ticket fixture lost its retirement dependency")
    };
    let dependency = dependency.clone();
    let wake = token.arm_wake().unwrap();
    dependency.register(
        crate::deferred_work::WakeAction::unkeyed(wake.into_raw(), wake_ticket),
        || false,
    );
    let mut operation = Some(operation);
    if early {
        drop(operation.take());
        settle();
    }
    let parked = token.park(ticket);
    assert_eq!(
        parked,
        if early {
            work_debt::ParkResult::Runnable
        } else {
            work_debt::ParkResult::Parked
        },
        "ticket completion lost its early/late wake boundary"
    );
    if !early {
        assert_eq!(
            TICKETS.pending(owner),
            0,
            "blocked ticket remained runnable"
        );
        drop(operation.take());
        settle();
    }
    let (token, ticket) = TICKETS
        .take(owner)
        .expect("retirement completion failed to wake its ticket")
        .into_parts();
    let (used, ticket) = ticket.advance(&process, 1);
    assert_eq!(
        used, 1,
        "completed ticket charged an unexpected cursor budget"
    );
    assert!(
        ticket.is_none(),
        "completed actor did not release its pending ticket"
    );
    token.finish();
    assert_eq!(
        TICKETS.available(),
        1,
        "ticket fixture retained its slot or wake"
    );
    retire(&source);
}

fn actor_root(sponsor: &Arc<MetadataSponsor>) {
    let source = set(sponsor);
    let target = set(sponsor);
    let (cycle, subscription, operation) = prepare(&target, &source);
    cycle
        .install(subscription)
        .expect("actor root source installation failed");
    drop(operation);
    concrete(&target)
        .unwrap()
        .begin(target.clone(), None)
        .unwrap()
        .publish();
    let weak = Arc::downgrade(&target);
    drop(cycle);
    drop(target);
    assert_eq!(
        weak.strong_count(),
        1,
        "retirement fixture was not solely queue-owned"
    );
    settle();
    assert!(
        weak.upgrade().is_none(),
        "completed actor retained its last object root"
    );
    retire(&source);
}

fn constructor_pressure(sponsor: &Arc<MetadataSponsor>) {
    use crate::task::{notify_work, retirement};
    let mut actors = Vec::new();
    actors
        .try_reserve_exact(retirement::selftest::inventory().0)
        .expect("actor pressure storage failed");
    while let Ok(slot) = retirement::reserve() {
        actors.push(slot);
    }
    let before = super::super::resources::admission_usage();
    let slots = notify_work::inventory_for_test();
    assert!(
        matches!(WaitSet::new(1, sponsor), Err(SystemCallError::OutOfMemory)),
        "actor exhaustion did not reject WaitSet construction"
    );
    assert_eq!(
        super::super::resources::admission_usage(),
        before,
        "actor pressure leaked constructor admission"
    );
    assert_eq!(
        notify_work::inventory_for_test(),
        slots,
        "actor pressure leaked constructor finish capacity"
    );
    drop(actors);

    let actor_slots = retirement::selftest::inventory();
    let mut finishes = Vec::new();
    finishes
        .try_reserve_exact(super::super::resources::KERNEL_FINISH_LIMIT)
        .expect("kernel finish pressure storage failed");
    while let Ok(slot) = notify_work::reserve_finish(notify_work::FinishClass::Kernel) {
        finishes.push(slot);
    }
    assert!(
        matches!(WaitSet::new(1, sponsor), Err(SystemCallError::OutOfMemory)),
        "kernel finish exhaustion did not reject WaitSet construction"
    );
    assert_eq!(
        super::super::resources::admission_usage(),
        before,
        "finish pressure leaked constructor admission"
    );
    assert_eq!(
        retirement::selftest::inventory(),
        actor_slots,
        "finish pressure leaked prepaid actor slot"
    );
    drop(finishes);

    let slots = notify_work::inventory_for_test();
    let objects = super::super::resources::exhaust_objects_for_test(sponsor);
    let pressured = super::super::resources::admission_usage();
    assert!(
        matches!(WaitSet::new(1, sponsor), Err(SystemCallError::ReachLimit)),
        "object exhaustion did not reject WaitSet construction"
    );
    assert_eq!(
        super::super::resources::admission_usage(),
        pressured,
        "late constructor failure leaked admission"
    );
    assert_eq!(
        notify_work::inventory_for_test(),
        slots,
        "late constructor failure leaked finish slot"
    );
    assert_eq!(
        retirement::selftest::inventory(),
        actor_slots,
        "late constructor failure leaked actor slot"
    );
    drop(objects);
    assert_eq!(
        super::super::resources::admission_usage(),
        before,
        "constructor pressure did not refund admission"
    );
}

fn control_pressure(sponsor: &Arc<MetadataSponsor>, nonempty_actor: bool) {
    use crate::task::notify_work;
    let source = set(sponsor);
    let backlog = crate::work_ledger::MAX_STEPS_PER_SAFE_POINT * 4;
    let target: ObjectRef = WaitSet::new(backlog, sponsor).expect("control pressure target failed");
    let mut cycles = Vec::with_capacity(backlog);
    for _ in 0..backlog {
        let (cycle, subscription, operation) = prepare(&target, &source);
        cycle
            .install(subscription)
            .expect("control pressure installation failed");
        drop(operation);
        cycles.push(cycle);
    }
    let finish_source = set(sponsor);
    let finish_target = set(sponsor);
    let (finish, subscription, operation) = prepare(&finish_target, &finish_source);
    finish
        .install(subscription)
        .expect("control pressure finish installation failed");
    drop(operation);
    let finish_set = concrete(&finish_source).unwrap();
    finish_set
        .state
        .lock()
        .wait
        .update(ObjectSignals::READABLE, ObjectSignals::READABLE);
    finish_set.notify();
    // 来源实际 offer 获得完成权，留下已排队 finish；通知槽仍由正式 runner 收束。
    let advance = finish_source.advance_waiter();
    assert!(!advance.finish(), "waiter notification finished too early");
    assert!(
        finish.core.has_outcome() && finish.finish.lock().is_none(),
        "control pressure did not publish its prepaid finish responsibility"
    );
    assert!(
        !finish.core.is_done(),
        "control pressure finish completed before the safe point"
    );
    let actor_source = set(sponsor);
    let actor: ObjectRef = WaitSet::new(backlog, sponsor).expect("control pressure actor failed");
    let mut actor_cycles = Vec::new();
    if nonempty_actor {
        for _ in 0..backlog {
            let (cycle, subscription, operation) = prepare(&actor, &actor_source);
            cycle.install(subscription).unwrap();
            drop(operation);
            actor_cycles.push(cycle);
        }
    }
    concrete(&actor)
        .unwrap()
        .begin(actor.clone(), None)
        .unwrap()
        .publish();
    concrete(&source)
        .unwrap()
        .state
        .lock()
        .wait
        .update(ObjectSignals::READABLE, ObjectSignals::READABLE);
    concrete(&source).unwrap().notify();
    let before = notify_work::inventory_for_test();
    assert!(
        before.2 > 0 && before.3 > 0 && crate::task::retirement::has_current(),
        "control pressure did not admit all three runnable classes"
    );
    let used = notify_work::drain_current();
    assert!(
        used <= crate::work_ledger::MAX_STEPS_PER_SAFE_POINT,
        "control pressure exceeded the shared safe-point budget"
    );
    assert!(
        cycles.iter().any(|cycle| cycle.core.has_outcome()),
        "notification backlog made no progress"
    );
    assert!(
        finish.core.is_done(),
        "notification backlog starved already-pending finish"
    );
    if nonempty_actor {
        assert!(
            actor_cycles
                .iter()
                .any(|cycle| cycle.removing.load(Ordering::Acquire)),
            "notification backlog starved nonempty actor cancellation"
        );
        assert!(
            !concrete(&actor).unwrap().is_finished(),
            "one turn consumed an entire nonempty actor backlog"
        );
    } else {
        assert!(
            concrete(&actor).unwrap().is_finished(),
            "notification backlog starved already-pending actor"
        );
    }
    assert!(
        notify_work::inventory_for_test().2 > 0,
        "control pressure failed to retain a notification backlog"
    );
    assert!(
        cycles.iter().any(|cycle| !cycle.core.has_outcome()),
        "control pressure consumed its entire observed backlog"
    );
    settle();
    assert!(
        concrete(&actor).unwrap().is_finished(),
        "control pressure failed to finish its actor backlog"
    );
    assert!(
        actor_cycles.iter().all(|cycle| cycle.core.is_done()),
        "control pressure retained a cancelled actor cycle"
    );
    for cycle in &cycles {
        assert!(
            cycle.core.is_done(),
            "control pressure failed to complete its notification backlog"
        );
        assert!(
            matches!(cycle.core.outcome_in(cycle.core.epoch()), WaitOutcome::Object(result)
            if result.reason == WaitReason::Signaled as u32 && result.observed.intersects(ObjectSignals::READABLE)),
            "control pressure lost its signaled completion"
        );
    }
    retire(&target);
    retire(&source);
    retire(&finish_target);
    retire(&finish_source);
    retire(&actor_source);
}

pub(crate) fn run(sponsor: &Arc<MetadataSponsor>) {
    let before = super::super::resources::admission_usage();
    let work_before = super::super::notify_work::inventory_for_test();
    let actor_before = super::super::retirement::selftest::inventory();
    stale_completion(sponsor);
    closed_seen(sponsor);
    late_install(sponsor);
    super::super::object::check_history_for_test();
    retirement_history(sponsor, false);
    retirement_history(sponsor, true);
    terminal_deferred(sponsor);
    registered_operation(sponsor);
    for reset_first in [false, true] {
        for closing in [false, true] {
            rearm_retirement(sponsor, reset_first, closing);
        }
    }
    for after_return in [false, true] {
        for closing in [false, true] {
            actor_window(sponsor, after_return, closing);
        }
    }
    abandoned_close(sponsor);
    ticket_completion(sponsor, false);
    ticket_completion(sponsor, true);
    actor_root(sponsor);
    constructor_pressure(sponsor);
    control_pressure(sponsor, false);
    control_pressure(sponsor, true);
    settle();
    assert_eq!(
        super::super::resources::admission_usage(),
        before,
        "WaitSet interleaving fixtures did not refund their admission"
    );
    assert_eq!(
        super::super::notify_work::inventory_for_test(),
        work_before,
        "WaitSet fixtures did not refund notification and finish slots"
    );
    assert_eq!(
        super::super::retirement::selftest::inventory(),
        actor_before,
        "WaitSet fixtures did not refund actor slots"
    );
    info!(
        Task,
        "WaitSet deterministic interleaving checks passed: stale completion, closed seen, late install, retirement history, operation retirement, rearm retirement 4/4, actor completion 4/4, abandoned Close, ticket wake 2/2, sole actor root, constructor pressure 3/3, three-class control pressure, fixed-slot refund"
    );
}
