//! 持真实 Taken token 排列业务 Complete 与队列交回的两侧窗口。

use super::*;

pub(crate) fn inventory() -> (usize, usize) {
    (
        DEBTS.lock().available(),
        PENDING[hart::current().slot()].load(Ordering::Acquire),
    )
}

pub(crate) fn complete_window(target: &ObjectRef, after_return: bool, concurrent: impl FnOnce()) {
    let owner = hart::current().slot();
    let taken = DEBTS
        .lock()
        .take(owner)
        .expect("actor completion fixture was not runnable");
    let (token, object) = taken.into_parts();
    assert!(
        Arc::ptr_eq(target, &object),
        "actor completion fixture took unrelated work"
    );
    let backend = object.retirement().unwrap();
    let result = backend.step(4);
    assert!(
        matches!(result.state, work_debt::StepState::Complete),
        "actor completion fixture did not reach Complete"
    );
    if after_return {
        let reservation = return_slot(owner, token);
        assert!(!has_current(), "returned actor remained runnable");
        concurrent();
        backend.finish(reservation, object.clone());
    } else {
        concurrent();
        backend.finish(return_slot(owner, token), object.clone());
    }
    assert!(
        has_current(),
        "concurrent work failed to republish the returned actor slot"
    );
}
