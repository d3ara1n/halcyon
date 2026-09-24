//! 游标断言只在 Process 类型边界内读取，不公开退休内部状态。

use super::*;

pub(crate) fn drain_position(process: &Process) -> (usize, bool) {
    let state = process.drain_state.lock();
    (
        state.cursor,
        matches!(
            state.pending_close,
            Some(super::super::handle::PendingClose::Retirement(_))
        ),
    )
}

pub(crate) fn drain_active(process: &Process) -> bool {
    process
        .drain_active
        .load(core::sync::atomic::Ordering::Acquire)
}

pub(crate) fn advance_unmanaged(
    process: &Arc<Process>,
    budget: usize,
) -> (usize, DrainBatchOutcome) {
    let _gate = process.drain_gate.lock();
    assert!(
        !process
            .drain_active
            .load(core::sync::atomic::Ordering::Acquire),
        "self-test drain overlapped a managed batch"
    );
    process.drain_batch(budget)
}

pub(crate) fn rollback_unpublished(
    process: Arc<Process>,
    drain: crate::deferred_work::UnpublishedReservation,
) {
    UnpublishedBound::new(process, drain).rollback();
}
