//! 真实地址空间、域准入、等待安装、终止债务与 Native 请求的隔离启动组合。

use super::*;
use crate::task::selftest::{Activated, Caller, bound, collect, pump, terminate};
use crate::task::{handle, memory_pool::MemoryPool, proc, process, wait};
use erhino_shared::proc::{
    ProcessDrainResult, ProcessDrainStatus, ProcessExitReason, ProcessFaultCode,
};

fn waiting_close(root: &Arc<MemoryPool>) {
    let mut caller = Caller::new(root);
    let active = Activated::new(&caller.process);
    super::super::create(
        caller.thread(),
        1,
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        caller.output,
    )
    .expect("Waiting Close fixture create failed");
    let handle: Handle = caller.read(caller.output);
    let item = WaitItem::new(handle, ObjectSignals::READABLE, 9);
    caller.put(caller.output + 32, &item);
    super::super::register(
        caller.thread(),
        handle,
        caller.output + 32,
        caller.output + 64,
    )
    .expect("Waiting Close fixture register failed");
    let token: u64 = caller.read(caller.output + 64);
    let object = resolve(caller.thread(), handle, Rights::MANAGE).unwrap();
    concrete(&object)
        .unwrap()
        .state
        .lock()
        .entries
        .get_mut(token)
        .unwrap()
        .operations += 1;
    let operation = Operation {
        object: object.clone(),
        token,
    };
    let identity = concrete(&object)
        .unwrap()
        .state
        .lock()
        .reply
        .as_ref()
        .unwrap()
        .clone();
    let handle::HandleCloseStart::Wait(plan) = handle::close(caller.thread(), handle).unwrap()
    else {
        panic!("nonempty Close did not produce its prepaid wait")
    };
    assert!(
        caller
            .process
            .handles
            .lock()
            .get(handle, Rights::NONE)
            .is_err(),
        "committed Close retained its owner handle"
    );
    drop(active);
    caller.park(plan);
    pump();
    terminate(&caller.process);
    pump();
    wait::selftest::assert_cancelled(&identity);
    assert_eq!(
        caller.process.lifecycle.member_count(),
        0,
        "Waiting kill did not confirm departure"
    );
    assert!(
        !caller.process.lifecycle.is_reapable(),
        "Waiting kill cancelled actor mandatory responsibility"
    );
    drop(operation);
    pump();
    assert!(
        caller.process.lifecycle.is_reapable() && concrete(&object).unwrap().is_finished(),
        "Waiting Close retirement failed to release mandatory responsibility"
    );
    caller.cleanup();
}

fn retiring_target(root: &Arc<MemoryPool>) -> (Arc<Process>, ObjectRef, Operation) {
    let (target, mut objects) = retiring_targets(root, 1);
    let (object, operation) = objects.pop().unwrap();
    (target, object, operation)
}

fn retiring_targets(
    root: &Arc<MemoryPool>,
    count: usize,
) -> (Arc<Process>, Vec<(ObjectRef, Operation)>) {
    let target = bound(root);
    let mut objects = Vec::with_capacity(count);
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let object = set(target.resources.metadata());
        let (cycle, subscription, operation) = prepare(&object, &object);
        cycle
            .install(subscription)
            .expect("continuation target installation failed");
        entries.push(
            handle::entry(
                object.clone(),
                HandleRole::WaitSetOwner,
                Rights::MANAGE | Rights::WAIT,
            )
            .unwrap(),
        );
        objects.push((object, operation));
    }
    let mut table = target.handles.lock();
    let reservation = table
        .reserve(count, handle::transaction_token().unwrap())
        .unwrap();
    table.commit(reservation, entries).unwrap();
    drop(table);
    terminate(&target);
    (target, objects)
}

fn native(root: &Arc<MemoryPool>, cancelled: bool, faulted: bool) {
    let (mut caller, sibling) = if faulted {
        let (caller, sibling) = Caller::pair(root);
        (caller, Some(sibling))
    } else {
        (Caller::new(root), None)
    };
    let (target, object, operation) = retiring_target(root);
    let control = target.revive_control().unwrap();
    let active = Activated::new(&caller.process);
    handle::install_one(
        caller.thread(),
        handle::entry(
            process::ProcessControl::object_ref(&control),
            HandleRole::ProcessControl,
            Rights::MANAGE | Rights::WAIT,
        )
        .unwrap(),
        caller.output,
        || (),
    )
    .expect("Native fixture control installation failed");
    let control_handle: Handle = caller.read(caller.output);
    let sentinel = [0xa5u8; core::mem::size_of::<ProcessDrainResult>()];
    caller.put(caller.output, &sentinel);
    let process::DrainStart::Wait(plan) =
        process::drain(caller.thread(), control_handle, 8, caller.output).unwrap()
    else {
        panic!("Native fixture bypassed its admitted request")
    };
    let identity = crate::task::request::selftest::identity(&target.drain_executor);
    drop(active);
    caller.park(plan);
    pump();
    wait::selftest::assert_native_parked(&identity, (8, 2));
    crate::task::request::selftest::assert_parked(&target.drain_executor, identity.key(), (8, 2));
    assert_eq!(
        proc::selftest::drain_position(&target),
        (2, true),
        "Native fixture did not stop at its real pending retirement ticket"
    );
    assert!(
        proc::selftest::drain_active(&target),
        "parked Native request released its batch permit"
    );
    assert_eq!(
        crate::task::notify_work::inventory_for_test().3,
        0,
        "parked Native request remained runnable"
    );
    assert_eq!(
        caller.read::<[u8; core::mem::size_of::<ProcessDrainResult>()]>(caller.output),
        sentinel,
        "blocked Native request prematurely wrote a result"
    );
    if cancelled {
        terminate(&caller.process);
        pump();
        wait::selftest::assert_native_done(&identity, true);
        crate::task::request::selftest::assert_idle(&target.drain_executor);
        assert!(
            !proc::selftest::drain_active(&target),
            "Waiting kill retained the Native batch permit"
        );
        assert_eq!(
            caller.process.lifecycle.member_count(),
            0,
            "Native cancellation did not confirm departure"
        );
        assert_eq!(
            caller.read::<[u8; core::mem::size_of::<ProcessDrainResult>()]>(caller.output),
            sentinel,
            "cancelled Native request wrote a partial result"
        );
        drop(operation);
        pump();
    } else {
        if let Some(sibling) = sibling.as_ref() {
            sibling.enter();
            assert!(
                sibling
                    .process
                    .lifecycle
                    .clear_active_if(crate::hart::current().slot(), || true)
            );
            let unmap = proc::memory_unmap(
                sibling.thread(),
                (caller.output & !(proc::PAGE_SIZE - 1)) as u64,
                proc::PAGE_SIZE as u64,
            )
            .expect("parked Native output revocation failed");
            pump();
            drop(unmap);
            assert_eq!(caller.process.space.lock().page_pa(caller.output), None);
            wait::selftest::assert_native_parked(&identity, (8, 2));
            crate::task::request::selftest::assert_parked(
                &target.drain_executor,
                identity.key(),
                (8, 2),
            );
        }
        drop(operation);
        for _ in 0..128 {
            if concrete(&object).unwrap().is_finished() {
                break;
            }
            crate::task::retirement::drain_current(16);
        }
        assert!(
            concrete(&object).unwrap().is_finished(),
            "Native actor did not publish completion"
        );
        let inventory = crate::task::notify_work::inventory_for_test();
        assert_eq!(
            (inventory.2, inventory.3),
            (0, 0),
            "request wake leaked into notification or finish debt"
        );
        assert_eq!(
            crate::task::request::selftest::inventory().1,
            1,
            "Native completion did not wake its request owner"
        );
        assert!(
            !crate::task::retirement::has_current(),
            "Native resumed-turn fixture still had actor work"
        );
        assert_eq!(
            crate::task::notify_work::drain_current(),
            6,
            "Native resume changed its six remaining business work units"
        );
        crate::task::request::selftest::assert_idle(&target.drain_executor);
        assert_eq!(
            crate::task::notify_work::inventory_for_test().3,
            1,
            "completed request did not publish waiter completion"
        );
        pump();
        if faulted {
            // 终止游标与 Native 完成交付均已真实推进，兄弟 Running owner 仍待离场。
            assert_eq!(
                (
                    caller.process.lifecycle.snapshot().1,
                    caller.process.lifecycle.snapshot().2
                ),
                (
                    ProcessExitReason::Fault,
                    ProcessFaultCode::StoreAccess as i64
                ),
                "committed Native output failure did not freeze StoreAccess"
            );
            assert!(
                !proc::selftest::drain_active(&target),
                "faulted completion retained its target batch"
            );
            sibling.unwrap().cleanup();
            pump();
            assert_eq!(
                caller.process.lifecycle.member_count(),
                0,
                "faulted Native retained a lifecycle member"
            );
            wait::selftest::assert_native_done(&identity, false);
            crate::task::request::selftest::assert_idle(&target.drain_executor);
            caller.cleanup();
            collect(&target);
            return;
        }
        wait::selftest::assert_native_done(&identity, false);
        crate::task::request::selftest::assert_idle(&target.drain_executor);
        let result: ProcessDrainResult = caller.read(caller.output);
        assert_eq!(
            result.work_done, 8,
            "Native resume reset its cumulative budget"
        );
        assert_eq!(
            result.status,
            ProcessDrainStatus::More as u32,
            "Native fixture unexpectedly completed its address space"
        );
        assert_eq!(
            proc::selftest::drain_position(&target),
            (2, false),
            "Native resume did not consume its pending retirement ticket"
        );
        assert!(
            !proc::selftest::drain_active(&target),
            "completed Native request retained its batch permit"
        );
        caller.thread = Some(crate::sched::selftest::take(&caller.process));
        assert!(matches!(
            caller.process.lifecycle.enter_running_if(
                caller.thread().member(),
                crate::hart::current().slot(),
                || true
            ),
            crate::task::lifecycle::EnterRunning::Entered
        ));
        let active = Activated::new(&caller.process);
        let process::DrainStart::Wait(plan) =
            process::drain(caller.thread(), control_handle, 1, caller.output).unwrap()
        else {
            panic!("live Core bypassed reusable Native completion")
        };
        drop(active);
        let next = crate::task::request::selftest::identity(&target.drain_executor);
        wait::selftest::assert_stale_cancel(&identity, &next);
        caller.park(plan);
        pump();
        wait::selftest::assert_native_done(&next, false);
        crate::task::request::selftest::assert_idle(&target.drain_executor);
        let result: ProcessDrainResult = caller.read(caller.output);
        assert_eq!(
            result.work_done, 1,
            "second Native epoch lost its independent budget"
        );
        assert!(
            result.status == ProcessDrainStatus::Complete as u32
                || result.status == ProcessDrainStatus::More as u32,
            "second Native epoch returned an invalid status"
        );
        caller.thread = Some(crate::sched::selftest::take(&caller.process));
    }
    assert!(
        concrete(&object).unwrap().is_finished(),
        "Native reply cancellation revoked its target actor"
    );
    caller.cleanup();
    collect(&target);
}

fn unpublished(root: &Arc<MemoryPool>) {
    let target = bound(root);
    let object = set(target.resources.metadata());
    let (cycle, subscription, operation) = prepare(&object, &object);
    cycle
        .install(subscription)
        .expect("unpublished target installation failed");
    let entry = handle::entry(
        object.clone(),
        HandleRole::WaitSetOwner,
        Rights::MANAGE | Rights::WAIT,
    )
    .unwrap();
    let mut table = target.handles.lock();
    let reservation = table
        .reserve(1, handle::transaction_token().unwrap())
        .unwrap();
    table.commit(reservation, alloc::vec![entry]).unwrap();
    drop(table);
    let drain =
        crate::deferred_work::reserve_unpublished().expect("unpublished fixture admission failed");
    proc::selftest::rollback_unpublished(target.clone(), drain);
    pump();
    assert_eq!(
        proc::selftest::drain_position(&target),
        (2, true),
        "unpublished fixture did not stop on its pending retirement ticket"
    );
    assert_eq!(
        crate::deferred_work::selftest::inventory()[1].1,
        0,
        "blocked unpublished retirement remained runnable"
    );
    assert_eq!(
        crate::deferred_work::drain_current(),
        0,
        "blocked unpublished retirement polled its dependency"
    );
    let (used, outcome) = proc::selftest::advance_unmanaged(&target, 1);
    assert_eq!(used, 0, "blocked unpublished drain charged false progress");
    assert!(
        matches!(outcome, proc::DrainBatchOutcome::Blocked(_)),
        "blocked unpublished drain lost its typed retirement dependency"
    );
    drop(operation);
    pump();
    assert!(
        concrete(&object).unwrap().is_finished(),
        "unpublished wake revoked target retirement"
    );
    assert_eq!(
        target.lifecycle.snapshot().0,
        erhino_shared::proc::ProcessState::Dead,
        "unpublished completion failed to drain its process"
    );
}

fn finalization_waits_for_active_batch(root: &Arc<MemoryPool>) {
    pump();
    let target = bound(root);
    terminate(&target);
    pump();
    assert!(target.lifecycle.is_reapable());
    assert!(
        target.try_acquire_drain(),
        "finalization fixture failed to acquire its managed batch"
    );
    for _ in 0..8192 {
        let (_, outcome) = target.advance_managed_drain(1);
        assert!(
            !matches!(outcome, proc::DrainBatchOutcome::Complete),
            "active batch completed before handing off finalization"
        );
        if target.lifecycle.snapshot().0 == erhino_shared::proc::ProcessState::Dead {
            break;
        }
    }
    assert_eq!(
        target.lifecycle.snapshot().0,
        erhino_shared::proc::ProcessState::Dead,
        "managed drain did not publish Dead"
    );
    assert_eq!(
        crate::deferred_work::selftest::inventory()[3].1,
        1,
        "PublishDead did not publish its independent finalization root"
    );
    assert_eq!(
        crate::deferred_work::drain_current(),
        1,
        "active managed batch did not park finalization in one bounded step"
    );
    assert_eq!(
        crate::deferred_work::selftest::inventory()[3].1,
        0,
        "parked finalization remained runnable"
    );
    target.release_drain();
    assert_eq!(
        crate::deferred_work::selftest::inventory()[3].1,
        1,
        "managed batch release lost the parked finalization wake"
    );
    let weak = Arc::downgrade(&target);
    drop(target);
    pump();
    assert!(
        weak.upgrade().is_none(),
        "finalization debt retained its Process root after Done"
    );
}

fn four_class_deferred_pressure(root: &Arc<MemoryPool>) {
    pump();

    let finalizing = bound(root);
    terminate(&finalizing);
    pump();
    assert!(finalizing.try_acquire_drain());
    for _ in 0..8192 {
        let (_, outcome) = finalizing.advance_managed_drain(1);
        assert!(
            !matches!(outcome, proc::DrainBatchOutcome::Complete),
            "deferred pressure finalization completed before handoff"
        );
        if finalizing.lifecycle.snapshot().0 == erhino_shared::proc::ProcessState::Dead {
            break;
        }
    }
    assert_eq!(
        finalizing.lifecycle.snapshot().0,
        erhino_shared::proc::ProcessState::Dead
    );

    let unpublished = bound(root);
    let unpublished_object = set(unpublished.resources.metadata());
    let entry = handle::entry(
        unpublished_object.clone(),
        HandleRole::WaitSetOwner,
        Rights::MANAGE | Rights::WAIT,
    )
    .unwrap();
    let mut table = unpublished.handles.lock();
    let reservation = table
        .reserve(1, handle::transaction_token().unwrap())
        .unwrap();
    table.commit(reservation, alloc::vec![entry]).unwrap();
    drop(table);
    let drain =
        crate::deferred_work::reserve_unpublished().expect("deferred pressure admission failed");
    proc::selftest::rollback_unpublished(unpublished.clone(), drain);

    let memory_caller = Caller::new(root);
    assert!(
        memory_caller
            .process
            .lifecycle
            .clear_active_if(crate::hart::current().slot(), || true)
    );
    let memory_plan = proc::memory_unmap(
        memory_caller.thread(),
        (memory_caller.output & !(proc::PAGE_SIZE - 1)) as u64,
        proc::PAGE_SIZE as u64,
    )
    .expect("deferred pressure memory change failed");
    for _ in 0..crate::hart::HART_NUM_LIMIT {
        crate::remote_call::drain_current();
        if crate::deferred_work::selftest::inventory()[0].1 != 0 {
            break;
        }
    }

    let mut termination_caller = Caller::new(root);
    let deadline = erhino_shared::time::Deadline::at(crate::clock::now().unwrap().max_deadline_ns);
    let termination_plan = wait::sleep_plan(deadline).unwrap();
    termination_caller.park(termination_plan);
    terminate(&termination_caller.process);

    let before = crate::deferred_work::selftest::inventory();
    assert!(
        before.iter().all(|(_, pending)| *pending > 0),
        "four-class deferred pressure did not admit every runnable class"
    );
    let cursor_before = proc::selftest::drain_position(&unpublished).0;
    let finish_before = crate::task::notify_work::inventory_for_test().3;
    let used = crate::deferred_work::drain_current();
    assert!(
        used <= crate::work_ledger::MAX_STEPS_PER_SAFE_POINT,
        "four-class deferred pressure exceeded its safe-point budget"
    );
    let cursor_after = proc::selftest::drain_position(&unpublished).0;
    let unpublished_work = cursor_after - cursor_before;
    assert!(
        unpublished_work > 0,
        "memory pressure starved unpublished rollback"
    );
    assert!(
        crate::task::notify_work::inventory_for_test().3 > finish_before,
        "earlier deferred classes starved termination cleanup"
    );
    assert!(
        used > unpublished_work + crate::work_ledger::MAX_STEPS_PER_DEBT_TURN + 1,
        "deferred accounting did not prove MemoryChange progress"
    );
    assert_eq!(
        crate::deferred_work::selftest::inventory()[3].1,
        0,
        "earlier deferred classes starved finalization parking"
    );

    finalizing.release_drain();
    drop(memory_plan);
    pump();
    drop(finalizing);
    drop(unpublished);
    drop(unpublished_object);
    memory_caller.cleanup();
    termination_caller.cleanup();
}

fn four_class_control_pressure(root: &Arc<MemoryPool>) {
    pump();
    let mut caller = Caller::new(root);
    let target = bound(root);
    terminate(&target);
    pump();
    let control = target.revive_control().unwrap();
    let active = Activated::new(&caller.process);
    handle::install_one(
        caller.thread(),
        handle::entry(
            process::ProcessControl::object_ref(&control),
            HandleRole::ProcessControl,
            Rights::MANAGE | Rights::WAIT,
        )
        .unwrap(),
        caller.output,
        || (),
    )
    .unwrap();
    let control_handle: Handle = caller.read(caller.output);
    let process::DrainStart::Wait(plan) =
        process::drain(caller.thread(), control_handle, 1, caller.output).unwrap()
    else {
        panic!("control pressure request bypassed its admitted executor")
    };
    drop(active);
    caller.park(plan);

    let sponsor = caller.process.resources.metadata();
    let source = set(sponsor);
    let backlog = crate::work_ledger::MAX_STEPS_PER_SAFE_POINT * 4;
    let notification_target: ObjectRef =
        WaitSet::new(backlog, sponsor).expect("four-class notification target failed");
    let mut cycles = Vec::with_capacity(backlog);
    for _ in 0..backlog {
        let (cycle, subscription, operation) = prepare(&notification_target, &source);
        cycle.install(subscription).unwrap();
        drop(operation);
        cycles.push(cycle);
    }
    concrete(&source)
        .unwrap()
        .state
        .lock()
        .wait
        .update(ObjectSignals::READABLE, ObjectSignals::READABLE);
    concrete(&source).unwrap().notify();

    let finish_source = set(sponsor);
    let finish_target = set(sponsor);
    let (finish, subscription, operation) = prepare(&finish_target, &finish_source);
    finish.install(subscription).unwrap();
    drop(operation);
    let finish_set = concrete(&finish_source).unwrap();
    finish_set
        .state
        .lock()
        .wait
        .update(ObjectSignals::READABLE, ObjectSignals::READABLE);
    finish_set.notify();
    assert!(
        !finish_source.advance_waiter().finish(),
        "four-class finish notification completed too early"
    );

    let actor_source = set(sponsor);
    let actor: ObjectRef =
        WaitSet::new(backlog, sponsor).expect("four-class retirement target failed");
    let mut actor_cycles = Vec::with_capacity(backlog);
    for _ in 0..backlog {
        let (cycle, subscription, operation) = prepare(&actor, &actor_source);
        cycle.install(subscription).unwrap();
        drop(operation);
        actor_cycles.push(cycle);
    }
    concrete(&actor)
        .unwrap()
        .begin(actor.clone(), None)
        .unwrap()
        .publish();

    let inventory = crate::task::notify_work::inventory_for_test();
    assert!(
        inventory.2 > 0
            && inventory.3 > 0
            && crate::task::request::selftest::inventory().1 > 0
            && crate::task::retirement::has_current(),
        "four-class control pressure did not admit every runnable class"
    );
    let used = crate::task::notify_work::drain_current();
    assert_eq!(
        used,
        crate::work_ledger::MAX_STEPS_PER_SAFE_POINT,
        "four-class control pressure did not consume its bounded safe point"
    );
    assert!(
        cycles.iter().any(|cycle| cycle.core.has_outcome()),
        "four-class pressure starved notification work"
    );
    assert!(
        finish.core.is_done(),
        "four-class pressure starved finish work"
    );
    crate::task::request::selftest::assert_idle(&target.drain_executor);
    let result: ProcessDrainResult = caller.read(caller.output);
    assert_eq!(
        result.work_done, 1,
        "four-class pressure starved the request executor"
    );
    assert!(
        actor_cycles
            .iter()
            .any(|cycle| cycle.removing.load(Ordering::Acquire)),
        "four-class pressure starved retirement work"
    );

    pump();
    assert!(concrete(&actor).unwrap().is_finished());
    caller.thread = Some(crate::sched::selftest::take(&caller.process));
    caller.cleanup();
    collect(&target);
    retire(&notification_target);
    retire(&source);
    retire(&finish_target);
    retire(&finish_source);
    retire(&actor_source);
}

fn native_uninstalled(root: &Arc<MemoryPool>) {
    let caller = Caller::new(root);
    let (target, _object, operation) = retiring_target(root);
    let control = target.revive_control().unwrap();
    let active = Activated::new(&caller.process);
    handle::install_one(
        caller.thread(),
        handle::entry(
            process::ProcessControl::object_ref(&control),
            HandleRole::ProcessControl,
            Rights::MANAGE | Rights::WAIT,
        )
        .unwrap(),
        caller.output,
        || (),
    )
    .expect("uninstalled Native control installation failed");
    let handle: Handle = caller.read(caller.output);
    let sentinel = [0x5au8; core::mem::size_of::<ProcessDrainResult>()];
    caller.put(caller.output, &sentinel);
    let process::DrainStart::Wait(plan) =
        process::drain(caller.thread(), handle, 8, caller.output).unwrap()
    else {
        panic!("uninstalled Native fixture bypassed its admitted request")
    };
    drop(active);
    let identity = wait::selftest::cancel_then_start(&plan);
    drop(plan);
    pump();
    wait::selftest::assert_native_done(&identity, true);
    assert!(
        !proc::selftest::drain_active(&target),
        "cancel-before-start Native request retained its batch permit"
    );
    crate::task::request::selftest::assert_idle(&target.drain_executor);
    assert_eq!(
        proc::selftest::drain_position(&target),
        (1, false),
        "cancel-before-start Native request advanced target resources"
    );
    assert_eq!(
        caller.read::<[u8; core::mem::size_of::<ProcessDrainResult>()]>(caller.output),
        sentinel,
        "cancel-before-start Native request wrote a result"
    );
    drop(operation);
    pump();
    caller.cleanup();
    collect(&target);
}

fn native_stale_parked(root: &Arc<MemoryPool>) {
    let mut caller = Caller::new(root);
    let (target, mut objects) = retiring_targets(root, 2);
    let control = target.revive_control().unwrap();
    {
        let _active = Activated::new(&caller.process);
        handle::install_one(
            caller.thread(),
            handle::entry(
                process::ProcessControl::object_ref(&control),
                HandleRole::ProcessControl,
                Rights::MANAGE | Rights::WAIT,
            )
            .unwrap(),
            caller.output,
            || (),
        )
        .unwrap();
    }
    let handle: Handle = caller.read(caller.output);
    let plan = {
        let _active = Activated::new(&caller.process);
        let process::DrainStart::Wait(plan) =
            process::drain(caller.thread(), handle, 3, caller.output).unwrap()
        else {
            panic!("first parked reuse round bypassed Native wait")
        };
        plan
    };
    let old = crate::task::request::selftest::identity(&target.drain_executor);
    caller.park(plan);
    pump();
    wait::selftest::assert_native_parked(&old, (3, 2));
    crate::task::request::selftest::assert_parked(&target.drain_executor, old.key(), (3, 2));
    let (first, operation) = objects.remove(0);
    drop(operation);
    pump();
    wait::selftest::assert_native_done(&old, false);
    crate::task::request::selftest::assert_idle(&target.drain_executor);
    assert!(concrete(&first).unwrap().is_finished());
    let result: ProcessDrainResult = caller.read(caller.output);
    assert_eq!(
        (result.work_done, result.status),
        (3, ProcessDrainStatus::More as u32)
    );
    caller.thread = Some(crate::sched::selftest::take(&caller.process));
    caller.enter();
    let plan = {
        let _active = Activated::new(&caller.process);
        let process::DrainStart::Wait(plan) =
            process::drain(caller.thread(), handle, 8, caller.output).unwrap()
        else {
            panic!("second parked reuse round bypassed Native wait")
        };
        plan
    };
    let next = crate::task::request::selftest::identity(&target.drain_executor);
    crate::task::request::selftest::cancel(&target.drain_executor, old.key());
    crate::task::request::selftest::assert_prepared(&target.drain_executor, next.key());
    caller.park(plan);
    pump();
    wait::selftest::assert_native_parked(&next, (8, 2));
    crate::task::request::selftest::assert_parked(&target.drain_executor, next.key(), (8, 2));
    wait::selftest::assert_stale_cancel(&old, &next);
    wait::selftest::assert_native_parked(&next, (8, 2));
    crate::task::request::selftest::assert_parked(&target.drain_executor, next.key(), (8, 2));
    assert_eq!(
        crate::task::notify_work::inventory_for_test().3,
        0,
        "old cancellation woke a new parked dependency"
    );
    let (second, operation) = objects.pop().unwrap();
    assert!(!concrete(&second).unwrap().is_finished());
    drop(operation);
    pump();
    wait::selftest::assert_native_done(&next, false);
    crate::task::request::selftest::assert_idle(&target.drain_executor);
    assert!(concrete(&second).unwrap().is_finished());
    let result: ProcessDrainResult = caller.read(caller.output);
    assert_eq!(
        result.work_done, 8,
        "new parked round lost its budget after stale cancellation"
    );
    caller.thread = Some(crate::sched::selftest::take(&caller.process));
    caller.cleanup();
    collect(&target);
}

pub(crate) fn run(root: &Arc<MemoryPool>) {
    pump();
    let pool = root.snapshot();
    let metadata = crate::task::resources::admission_usage();
    let control = crate::task::notify_work::inventory_for_test();
    let deferred = crate::deferred_work::selftest::inventory();
    assert_eq!(
        deferred[1].0, 1,
        "unpublished debt capacity must match its boot-only producer"
    );
    let requests = crate::task::request::selftest::inventory();
    let actors = crate::task::retirement::selftest::inventory();
    waiting_close(root);
    native(root, false, false);
    native(root, true, false);
    native(root, false, true);
    native_uninstalled(root);
    native_stale_parked(root);
    unpublished(root);
    finalization_waits_for_active_batch(root);
    four_class_deferred_pressure(root);
    four_class_control_pressure(root);
    pump();
    assert_eq!(
        root.snapshot(),
        pool,
        "continuation fixtures did not refund their funded Pool"
    );
    assert_eq!(
        crate::task::resources::admission_usage(),
        metadata,
        "continuation fixtures did not refund metadata"
    );
    assert_eq!(
        crate::task::notify_work::inventory_for_test(),
        control,
        "continuation fixtures retained control slots"
    );
    assert_eq!(
        crate::deferred_work::selftest::inventory(),
        deferred,
        "continuation fixtures retained deferred slots"
    );
    assert_eq!(
        crate::task::request::selftest::inventory(),
        requests,
        "continuation fixtures retained request slots"
    );
    assert_eq!(
        crate::task::retirement::selftest::inventory(),
        actors,
        "continuation fixtures retained actor slots"
    );
    info!(
        Task,
        "Waiting continuation checks passed: Close kill, Native parked resume and kill, committed-output StoreAccess, unpublished blocked wake, active-batch finalization handoff, four-class deferred and control pressure, cumulative budget, reused parked epoch and stale cancel, complete refund"
    );
}
