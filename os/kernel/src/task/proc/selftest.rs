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
