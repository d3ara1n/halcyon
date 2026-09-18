//! 等待身份的只读断言在类型边界内访问捕获代次，不向调用方公开 core。

use super::{WaitIdentity, WaitOutcome};

pub(crate) fn identity(context: &alloc::sync::Arc<super::WaitContext>) -> WaitIdentity {
    WaitIdentity::new(context.clone())
}

pub(crate) fn cancel_then_start(plan: &super::WaitPlan) -> WaitIdentity {
    let identity = plan
        .prepared
        .as_ref()
        .expect("request plan did not retain its prepared wait identity")
        .clone();
    let operation = plan
        .operation
        .as_ref()
        .expect("request plan did not retain its operation");
    assert_ne!(
        identity.abandon(),
        wait_context::OfferResult::Lost,
        "prepared request cancellation lost its epoch"
    );
    crate::task::request::WaitOperation::start(&**operation, identity.key());
    identity
}

pub(crate) fn assert_stale_cancel(old: &WaitIdentity, next: &WaitIdentity) {
    assert_eq!(
        next.core.epoch(),
        next.epoch,
        "new Native identity did not capture the current epoch"
    );
    assert!(
        alloc::sync::Arc::ptr_eq(&old.context, &next.context),
        "Native fixture did not reuse its prepaid waiter"
    );
    assert_eq!(
        old.epoch.next(),
        Some(next.epoch),
        "Native reuse did not advance exactly one epoch"
    );
    assert_eq!(
        old.abandon(),
        wait_context::OfferResult::Lost,
        "old cancellation reached a new Native epoch"
    );
    assert!(
        !next.core.is_abandoned(next.epoch),
        "old cancellation retired the new captured request"
    );
}

pub(crate) fn assert_native_parked(identity: &WaitIdentity, progress: (usize, usize)) {
    assert_eq!(
        identity.core.epoch(),
        identity.epoch,
        "parked Native identity refers to a stale epoch"
    );
    assert!(
        !identity.core.is_done() && !identity.core.is_abandoned(identity.epoch),
        "native fixture did not retain its finishing epoch"
    );

    assert!(
        identity.finish_reservation.lock().is_some(),
        "parked Native request lost its prepaid finish capacity"
    );
    assert!(
        identity.finish_state.lock().is_none(),
        "parked Native request entered waiter completion before its result existed"
    );
    let _ = progress;
}

pub(crate) fn assert_native_done(identity: &WaitIdentity, abandoned: bool) {
    assert_eq!(
        identity.core.epoch(),
        identity.epoch,
        "completed Native identity refers to a stale epoch"
    );
    assert!(
        identity.core.is_done(),
        "native fixture did not complete its epoch"
    );
    assert!(
        identity.finish_state.lock().is_none(),
        "completed Native epoch retained its finish state"
    );
    assert_eq!(
        identity.core.is_abandoned(identity.epoch),
        abandoned,
        "native fixture cancellation mismatch"
    );

    assert!(
        identity.thread.lock().is_none(),
        "native fixture retained its admitted thread"
    );
    assert!(
        identity.finish_reservation.lock().is_some(),
        "native fixture did not return reusable capacity"
    );
}

pub(crate) fn assert_cancelled(identity: &WaitIdentity) {
    assert_eq!(
        identity.core.epoch(),
        identity.epoch,
        "cancelled identity refers to a stale epoch"
    );
    assert!(
        identity.core.is_abandoned(identity.epoch) && identity.core.is_done(),
        "discarded committed reply did not complete its cancellation"
    );
    assert!(
        matches!(
            identity.core.outcome_in(identity.epoch),
            WaitOutcome::Abandoned
        ),
        "discarded committed reply retained a success outcome"
    );
}

pub(crate) fn time_install(root: &alloc::sync::Arc<crate::task::memory_pool::MemoryPool>) {
    use crate::task::selftest::{Caller, pump, terminate};
    use erhino_shared::time::Deadline;
    pump();
    let pool = root.snapshot();
    let metadata = crate::task::resources::admission_usage();
    let control = crate::task::notify_work::inventory_for_test();
    let timers = crate::sched::selftest::timer_count();
    for cancelled in [false, true] {
        let mut caller = Caller::new(root);
        let deadline = if cancelled {
            Deadline::at(crate::clock::now().unwrap().max_deadline_ns)
        } else {
            Deadline::at(0)
        };
        let mut plan = super::sleep_plan(deadline).unwrap();
        let context = super::WaitContext::new(plan.action, 0, None, false).unwrap();
        let identity = WaitIdentity::new(context.clone());
        plan.prepared = Some(identity.clone());
        caller.park(plan);
        pump();
        if cancelled {
            assert!(
                !context.core.is_done(),
                "future Sleep completed before cancellation"
            );
            assert_eq!(crate::sched::selftest::timer_count(), timers + 1);
            terminate(&caller.process);
            pump();
            assert_cancelled(&identity);
            assert_eq!(caller.process.lifecycle.member_count(), 0);
        } else {
            assert!(context.core.is_done());
            assert!(
                matches!(
                    context.core.outcome_in(identity.epoch),
                    WaitOutcome::Timeout
                ),
                "expired captured tick did not win during wait installation"
            );
            let thread = crate::sched::selftest::take(&caller.process);
            // SAFETY: fixture 独占已完成 owner，没有用户执行点在访问 frame。
            assert_eq!(unsafe { &*thread.frame_ptr() }.x[10], 0);
            assert_eq!(unsafe { &*thread.frame_ptr() }.sepc, 4);
            caller.thread = Some(thread);
        }
        assert_eq!(
            crate::sched::selftest::timer_count(),
            timers,
            "Sleep retained a timer registration"
        );
        assert!(context.timeout_registration.take_cancellation().is_none());
        caller.cleanup();
    }
    pump();
    assert_eq!(root.snapshot(), pool);
    assert_eq!(crate::task::resources::admission_usage(), metadata);
    assert_eq!(crate::task::notify_work::inventory_for_test(), control);
    assert_eq!(crate::sched::selftest::timer_count(), timers);
    info!(
        Task,
        "Time wait installation checks passed: expired captured tick, registered timer cancellation, owner and timer refund"
    );
}
