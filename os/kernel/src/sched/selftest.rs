//! Ready 发布之前的隔离启动自检从正式域队列取回已完成线程。

use super::*;

pub(crate) fn timer_count() -> usize {
    timers().lock().len()
}

pub(crate) fn take(process: &Arc<crate::task::proc::Process>) -> AdmittedThread {
    let thread = process
        .domain()
        .pick()
        .expect("completed fixture did not enter its ready domain");
    assert!(
        Arc::ptr_eq(process, &thread.process),
        "fixture picked an unrelated thread"
    );
    assert!(
        !process.domain().has_ready(),
        "fixture left another ready owner"
    );
    thread
}
