//! 隔离启动夹具的真实全局债务槽与当前 hart runnable 库存。

use super::*;

pub(crate) fn inventory() -> [(usize, usize); 4] {
    let owner = hart::current().slot();
    let memory = DEBTS.lock().available();
    let unpublished = UNPUBLISHED_DEBTS.lock().available();
    let termination = TERMINATION_DEBTS.lock().available();
    let finalization = FINALIZATION_DEBTS.lock().available();
    [
        (memory, PENDING[owner].load(Ordering::Acquire)),
        (
            unpublished,
            UNPUBLISHED_PENDING[owner].load(Ordering::Acquire),
        ),
        (
            termination,
            TERMINATION_PENDING[owner].load(Ordering::Acquire),
        ),
        (
            finalization,
            FINALIZATION_PENDING[owner].load(Ordering::Acquire),
        ),
    ]
}
