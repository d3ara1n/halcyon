//! 等待身份的只读断言在类型边界内访问捕获代次，不向调用方公开 core。

use super::{WaitIdentity, WaitOutcome};

pub(crate) fn identity(context: &alloc::sync::Arc<super::WaitContext>) -> WaitIdentity {
    WaitIdentity::new(context.clone())
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
        !next.core.is_abandoned(next.epoch) && next.request.lock().is_some(),
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
        identity
            .dependency
            .lock()
            .as_ref()
            .is_some_and(|(epoch, _)| *epoch == identity.epoch),
        "native fixture did not register its current epoch dependency"
    );
    assert!(
        identity.finish_reservation.lock().is_none(),
        "parked Native request retained queued finish capacity"
    );
    assert!(
        identity
            .finish_state
            .lock()
            .as_ref()
            .is_some_and(|finish| !finish.delivered && finish.delivery.is_none()),
        "parked Native request already delivered its finish state"
    );
    assert!(
        identity.request.lock().is_some(),
        "native fixture lost captured request during suspension"
    );
    assert!(
        identity.thread.lock().is_some(),
        "native fixture lost its admitted thread during suspension"
    );
    let request = identity.request.lock();
    assert_eq!(
        crate::task::request::selftest::progress(request.as_ref().unwrap()),
        progress,
        "parked Native request lost its original budget or accumulated work"
    );
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
        identity.dependency.lock().is_none(),
        "native fixture retained its dependency"
    );
    assert!(
        identity.request.lock().is_none(),
        "native fixture retained its captured request"
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
