//! 隔离启动夹具的真实全局债务槽与当前 hart runnable 库存。

use super::*;

pub(crate) fn inventory() -> [(usize, usize); 4] {
    let owner = hart::current().slot();
    let memory = DEBTS.available();
    let unpublished = UNPUBLISHED_DEBTS.available();
    let termination = TERMINATION_DEBTS.available();
    let finalization = FINALIZATION_DEBTS.available();
    [
        (memory, DEBTS.pending(owner)),
        (unpublished, UNPUBLISHED_DEBTS.pending(owner)),
        (termination, TERMINATION_DEBTS.pending(owner)),
        (finalization, FINALIZATION_DEBTS.pending(owner)),
    ]
}
