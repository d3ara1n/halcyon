//! 请求执行自检辅助；只读观察 executor 的 owner 与批次状态。

use super::DrainExecutor;

pub(crate) fn identity(executor: &DrainExecutor) -> crate::task::wait::WaitIdentity {
    crate::task::wait::selftest::identity(&executor.waiter)
}

pub(crate) fn assert_prepared(executor: &DrainExecutor, key: crate::deferred_work::WaitKey) {
    let state = executor.state.lock();
    assert!(
        state
            .request
            .as_ref()
            .is_some_and(|request| request.wait_key() == key),
        "prepared request executor retained a stale wait identity"
    );
    assert!(
        state.activation.is_some() && state.dependency.is_none() && state.reservation.is_some(),
        "prepared request executor lost activation or prepaid capacity"
    );
}

pub(crate) fn cancel(executor: &DrainExecutor, key: crate::deferred_work::WaitKey) {
    super::WaitOperation::cancel(executor, key);
}

pub(crate) fn assert_parked(
    executor: &DrainExecutor,
    key: crate::deferred_work::WaitKey,
    progress: (usize, usize),
) {
    let state = executor.state.lock();
    let request = state
        .request
        .as_ref()
        .expect("parked request executor lost its batch");
    assert_eq!(
        (request.budget, request.work_done),
        progress,
        "parked request lost its budget or accumulated work"
    );
    assert_eq!(
        request.wait_key(),
        key,
        "parked request retained a stale wait identity"
    );
    assert!(
        state
            .dependency
            .as_ref()
            .is_some_and(|(registered, _)| *registered == key),
        "parked request did not own its current dependency"
    );
    assert!(
        state.reservation.is_none() && state.activation.is_none(),
        "parked request retained unpublished capacity or activation"
    );
}

pub(crate) fn inventory() -> (usize, usize) {
    let owner = crate::hart::current().slot();
    (super::DEBTS.available(), super::DEBTS.pending(owner))
}

pub(crate) fn assert_idle(executor: &DrainExecutor) {
    let state = executor.state.lock();
    assert!(
        state.request.is_none()
            && state.dependency.is_none()
            && state.activation.is_none()
            && state.reservation.is_some(),
        "request executor did not return to its prepaid idle state"
    );
}
