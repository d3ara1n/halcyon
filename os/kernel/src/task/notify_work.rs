//! 可等待对象的有界通知债务。
//!
//! 注册时支付一个固定槽；信号发布只把目标对象交给 owner hart，真正的
//! WaitContext offer 与订阅排水在安全点按固定预算推进。对象更新与候选
//! 快照仍由 ObjectWaitState 锁内完成，队列不重新读取 live signals。

use super::{object::ObjectRef, wait::ObserverSink};
use crate::{
    hart, registry,
    work_ledger::{DebtLedger, Reservation as LedgerReservation},
};
/// 上限覆盖固定的 WaitMany 输入槽，超过即在订阅安装前 fail closed。
pub(crate) const SLOTS: usize = 8_192;

type Debts = DebtLedger<ObjectRef, SLOTS>;
const KERNEL_FINISH_END: usize = SLOTS + super::resources::KERNEL_FINISH_LIMIT;
const FINISH_SLOTS: usize = KERNEL_FINISH_END + super::resources::REGISTRATION_FINISH_LIMIT;
type FinishDebts = DebtLedger<ObserverSink, FINISH_SLOTS>;

#[derive(Clone, Copy)]
pub(crate) enum FinishClass {
    Thread,
    Kernel,
    Persistent,
}

static DEBTS: Debts = Debts::new(work_debt::TableId::new(4));
static FINISH_DEBTS: FinishDebts = FinishDebts::new(work_debt::TableId::new(5));

pub(crate) fn inventory_for_test() -> (usize, usize, usize, usize) {
    let owner = hart::current().slot();
    let notifications = DEBTS.available();
    let finishes = FINISH_DEBTS.available();
    (
        notifications,
        finishes,
        DEBTS.pending(owner),
        FINISH_DEBTS.pending(owner),
    )
}

/// 一个已登记订阅未来命中的固定槽；未发布时 Drop 精确取消。
pub(crate) type Reservation = LedgerReservation<ObjectRef, SLOTS>;

pub(crate) enum Completion {
    /// 对象仍有注册项，槽继续由对象持有。
    Held,
    /// 对象已无注册项，归还槽。
    Release(Reservation),
    /// 排水完成与新发布并发；同一槽立即重新发布。
    Reschedule(Reservation),
}

pub(crate) fn reserve() -> Result<Reservation, ()> {
    DEBTS.reserve().map_err(|_| ())
}

/// 完成责任槽在 WaitContext 创建时预付，保证 outcome 获胜后不再申请存储。
pub(crate) type FinishReservation = LedgerReservation<ObserverSink, FINISH_SLOTS>;

pub(crate) fn reserve_finish(class: FinishClass) -> Result<FinishReservation, ()> {
    let range = match class {
        FinishClass::Thread => 0..SLOTS,
        FinishClass::Kernel => SLOTS..KERNEL_FINISH_END,
        FinishClass::Persistent => KERNEL_FINISH_END..FINISH_SLOTS,
    };
    FINISH_DEBTS.reserve_in(range).map_err(|_| ())
}

pub(crate) fn publish_finish(reservation: FinishReservation, context: impl Into<ObserverSink>) {
    reservation.publish_quiet(context.into());
}

/// 由调度安全点消费；每个对象一次最多推进固定数量的 waiter。
pub(crate) fn drain_current() -> usize {
    let owner = hart::current().slot();
    let mut budget = crate::work_ledger::safe_point_budget([
        DEBTS.pending(owner) != 0,
        FINISH_DEBTS.pending(owner) != 0,
        super::request::has_current(),
        super::retirement::has_current(),
    ]);
    while budget.turn(0) != 0 {
        let Some(taken) = DEBTS.take(owner) else {
            break;
        };
        let (token, target) = taken.into_parts();
        let turn = budget.turn(0);
        let mut used = 0;
        let mut complete = false;
        while used < turn {
            let advance = target.advance_waiter();
            if advance.finish() {
                complete = true;
                break;
            }
            used += 1;
        }
        assert!(used <= turn, "notification drain exceeded its debt turn");
        assert!(
            complete || used > 0,
            "incomplete notification drain made no progress"
        );
        // 完成一个已排队但已被清空的对象仍是一个真实工作单位，
        // 否则取消风暴可在单个安全点内连续消费全部槽位。
        budget.charge(0, used.max(1));
        if complete {
            let reservation = token.rearm();
            let completion = target.complete_waiter_drain(reservation);
            match completion {
                Completion::Held => {}
                Completion::Release(reservation) => drop(reservation),
                Completion::Reschedule(reservation) => reservation.publish_quiet(target),
            }
        } else {
            token.requeue(target);
        }
    }
    while budget.turn(1) != 0 {
        let Some(taken) = FINISH_DEBTS.take(owner) else {
            break;
        };
        let (token, context) = taken.into_parts();
        let turn = budget.turn(1);
        let (used, complete) = context.finish_step(turn);
        assert!(
            used <= turn && (used > 0 || complete),
            "wait completion exceeded its debt turn"
        );
        budget.charge(1, used.max(1));
        if complete {
            let reused = if context.reuses_finish() {
                Some(token.rearm())
            } else {
                token.finish();
                None
            };
            context.complete_finish(reused);
        } else {
            token.requeue(context);
        }
    }
    if DEBTS.has_pending(owner) || FINISH_DEBTS.has_pending(owner) {
        let failed = registry::try_ipi_slots(1u64 << owner);
        if failed != 0 {
            warn!(
                Task,
                "Notification work doorbell failed for hart slot {owner}; work remains pending"
            );
        }
    }
    let request_work = super::request::drain_current(budget.remaining(2));
    budget.charge(2, request_work);
    let retirement_work = super::retirement::drain_current(budget.remaining(3));
    budget.charge(3, retirement_work);
    budget.used()
}

pub(crate) fn has_current() -> bool {
    let owner = hart::current().slot();
    DEBTS.pending(owner) != 0
        || FINISH_DEBTS.pending(owner) != 0
        || super::request::has_current()
        || super::retirement::has_current()
}
