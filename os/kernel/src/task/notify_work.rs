//! 可等待对象的有界通知债务。
//!
//! 注册时支付一个固定槽；信号发布只把目标对象交给 owner hart，真正的
//! WaitContext offer 与订阅排水在安全点按固定预算推进。对象更新与候选
//! 快照仍由 ObjectWaitState 锁内完成，队列不重新读取 live signals。

use core::sync::atomic::{AtomicUsize, Ordering};

use super::{object::ObjectRef, wait::ObserverSink};
use crate::{hart, registry, sync::Spinlock};

const HARTS: usize = hart::HART_NUM_LIMIT;
/// 上限覆盖固定的 WaitMany 输入槽，超过即在订阅安装前 fail closed。
pub(crate) const SLOTS: usize = 8_192;
pub(crate) const MAX_STEPS_PER_SAFE_POINT: usize = 16;
const MAX_STEPS_PER_DEBT_TURN: usize = 4;

type Debts = work_debt::WorkDebts<ObjectRef, HARTS, SLOTS>;
const KERNEL_FINISH_END: usize = SLOTS + super::resources::KERNEL_FINISH_LIMIT;
const FINISH_SLOTS: usize = KERNEL_FINISH_END + super::resources::REGISTRATION_FINISH_LIMIT;
type FinishDebts = work_debt::WorkDebts<ObserverSink, HARTS, FINISH_SLOTS>;

#[derive(Clone, Copy)]
pub(crate) enum FinishClass {
    Thread,
    Kernel,
    Persistent,
}

static DEBTS: Spinlock<Debts> = Spinlock::new(
    crate::sync::ranks::WORK_DEBT,
    Debts::new_with_id(work_debt::TableId::new(4)),
);
static FINISH_DEBTS: Spinlock<FinishDebts> = Spinlock::new(
    crate::sync::ranks::WORK_DEBT,
    FinishDebts::new_with_id(work_debt::TableId::new(5)),
);
static PENDING: [AtomicUsize; HARTS] = [const { AtomicUsize::new(0) }; HARTS];
static FINISH_PENDING: [AtomicUsize; HARTS] = [const { AtomicUsize::new(0) }; HARTS];

pub(crate) fn inventory_for_test() -> (usize, usize, usize, usize) {
    let owner = hart::current().slot();
    let notifications = DEBTS.lock().available();
    let finishes = FINISH_DEBTS.lock().available();
    (
        notifications,
        finishes,
        PENDING[owner].load(Ordering::Acquire),
        FINISH_PENDING[owner].load(Ordering::Acquire),
    )
}

/// 一个已登记订阅未来命中的固定槽；未发布时 Drop 精确取消。
pub(crate) struct Reservation(Option<work_debt::Reservation>);

pub(crate) enum Completion {
    /// 对象仍有注册项，槽继续由对象持有。
    Held,
    /// 对象已无注册项，归还槽。
    Release(Reservation),
    /// 排水完成与新发布并发；同一槽立即重新发布。
    Reschedule(Reservation),
}

pub(crate) fn reserve() -> Result<Reservation, ()> {
    DEBTS
        .lock()
        .reserve()
        .map(|reservation| Reservation(Some(reservation)))
        .map_err(|_| ())
}

impl Reservation {
    pub(crate) fn publish(mut self, target: ObjectRef) {
        let owner = hart::current().slot();
        let reservation = self
            .0
            .take()
            .expect("notification reservation published twice");
        let mut debts = DEBTS.lock();
        debts
            .publish(reservation, owner, target)
            .unwrap_or_else(|_| panic!("reserved notification slot must publish"));
        PENDING[owner].fetch_add(1, Ordering::Release);
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(reservation) = self.0.take() {
            assert!(
                DEBTS.lock().cancel(reservation).is_ok(),
                "reserved notification slot must roll back"
            );
        }
    }
}

/// 完成责任槽在 WaitContext 创建时预付，保证 outcome 获胜后不再申请存储。
pub(crate) struct FinishReservation(Option<work_debt::Reservation>);

pub(crate) fn reserve_finish(class: FinishClass) -> Result<FinishReservation, ()> {
    let range = match class {
        FinishClass::Thread => 0..SLOTS,
        FinishClass::Kernel => SLOTS..KERNEL_FINISH_END,
        FinishClass::Persistent => KERNEL_FINISH_END..FINISH_SLOTS,
    };
    FINISH_DEBTS
        .lock()
        .reserve_in(range)
        .map(|reservation| FinishReservation(Some(reservation)))
        .map_err(|_| ())
}

pub(crate) fn publish_finish(mut reservation: FinishReservation, context: impl Into<ObserverSink>) {
    let owner = hart::current().slot();
    let reservation = reservation
        .0
        .take()
        .expect("finish reservation published twice");
    {
        let mut debts = FINISH_DEBTS.lock();
        debts
            .publish(reservation, owner, context.into())
            .unwrap_or_else(|_| panic!("reserved finish slot must publish"));
        FINISH_PENDING[owner].fetch_add(1, Ordering::Release);
    }
}

fn wake_finish(token: work_debt::WakeToken) {
    let result = {
        let mut debts = FINISH_DEBTS.lock();
        let result = debts
            .wake(token)
            .unwrap_or_else(|_| panic!("finish dependency lost its wake owner"));
        if let work_debt::WakeResult::Runnable { owner } = result {
            FINISH_PENDING[owner].fetch_add(1, Ordering::Release);
        }
        result
    };
    if let work_debt::WakeResult::Runnable { owner } = result
        && registry::try_ipi_slots(1u64 << owner) != 0
    {
        warn!(
            Task,
            "Finish dependency doorbell failed for hart slot {owner}; work remains pending"
        );
    }
}

impl Drop for FinishReservation {
    fn drop(&mut self) {
        if let Some(reservation) = self.0.take() {
            assert!(
                FINISH_DEBTS.lock().cancel(reservation).is_ok(),
                "reserved finish slot must roll back"
            );
        }
    }
}

/// 由调度安全点消费；每个对象一次最多推进固定数量的 waiter。
pub(crate) fn drain_current() -> usize {
    let owner = hart::current().slot();
    let mut steps = 0;
    let reserve_finish = usize::from(FINISH_PENDING[owner].load(Ordering::Acquire) != 0);
    let reserve_retirement = usize::from(super::retirement::has_current());
    let notification_budget = MAX_STEPS_PER_SAFE_POINT - reserve_finish - reserve_retirement;
    while steps < notification_budget {
        let Some(taken) = DEBTS.lock().take(owner) else {
            break;
        };
        let (token, target) = taken.into_parts();
        let turn = (notification_budget - steps).min(MAX_STEPS_PER_DEBT_TURN);
        let (used, complete) = target.drain_waiters(turn);
        assert!(used <= turn, "notification drain exceeded its debt turn");
        assert!(
            complete || used > 0,
            "incomplete notification drain made no progress"
        );
        // 完成一个已排队但已被清空的对象仍是一个真实工作单位，
        // 否则取消风暴可在单个安全点内连续消费全部槽位。
        steps += used.max(1);
        if complete {
            let reservation = {
                let mut debts = DEBTS.lock();
                let slot = debts
                    .rearm(token)
                    .unwrap_or_else(|_| panic!("taken notification slot must rearm"));
                assert!(
                    PENDING[owner].fetch_sub(1, Ordering::AcqRel) > 0,
                    "finished notification slot must be pending"
                );
                Reservation(Some(slot))
            };
            let completion = target.complete_waiter_drain(reservation);
            match completion {
                Completion::Held => {}
                Completion::Release(reservation) => drop(reservation),
                Completion::Reschedule(reservation) => reservation.publish(target),
            }
        } else {
            DEBTS
                .lock()
                .requeue(token, target)
                .unwrap_or_else(|_| panic!("taken notification slot must requeue"));
        }
    }
    let finish_budget = MAX_STEPS_PER_SAFE_POINT - reserve_retirement;
    while steps < finish_budget {
        let Some(taken) = FINISH_DEBTS.lock().take(owner) else {
            break;
        };
        let (token, context) = taken.into_parts();
        let turn = (finish_budget - steps).min(MAX_STEPS_PER_DEBT_TURN);
        let advance = context.finish_step(turn);
        let used = advance.work_done;
        assert!(
            used <= turn && (used > 0 || !matches!(advance.state, work_debt::StepState::Runnable)),
            "wait completion exceeded its debt turn"
        );
        steps += used.max(1);
        match advance.state {
            work_debt::StepState::Complete => {
                let reused = {
                    let mut debts = FINISH_DEBTS.lock();
                    let reused = if context.reuses_finish() {
                        Some(FinishReservation(Some(debts.rearm(token).unwrap_or_else(
                            |_| panic!("reusable finish slot must rearm"),
                        ))))
                    } else {
                        assert!(debts.finish(token).is_ok(), "taken finish slot must finish");
                        None
                    };
                    let previous = FINISH_PENDING[owner].fetch_sub(1, Ordering::AcqRel);
                    assert!(previous > 0, "finished completion slot must be pending");
                    reused
                };
                context.complete_finish(reused);
            }
            work_debt::StepState::Runnable => {
                FINISH_DEBTS
                    .lock()
                    .requeue(token, context)
                    .unwrap_or_else(|_| panic!("taken finish slot must requeue"));
            }
            work_debt::StepState::Blocked(dependency) => {
                let wake = FINISH_DEBTS
                    .lock()
                    .arm_wake(&token)
                    .expect("blocked finish must own its wake ticket");
                let action =
                    crate::deferred_work::WakeAction::new(wake, wake_finish, context.wait_key());
                context.register_dependency(dependency, action);
                let mut debts = FINISH_DEBTS.lock();
                let parked = debts
                    .park(token, context)
                    .unwrap_or_else(|_| panic!("blocked finish must retain its payload"));
                if parked == work_debt::ParkResult::Parked {
                    let previous = FINISH_PENDING[owner].fetch_sub(1, Ordering::AcqRel);
                    assert!(previous > 0, "parked finish must have been runnable");
                }
            }
        }
    }
    if DEBTS.lock().has_pending(owner) || FINISH_DEBTS.lock().has_pending(owner) {
        let failed = registry::try_ipi_slots(1u64 << owner);
        if failed != 0 {
            warn!(
                Task,
                "Notification work doorbell failed for hart slot {owner}; work remains pending"
            );
        }
    }
    steps += super::retirement::drain_current(MAX_STEPS_PER_SAFE_POINT - steps);
    steps
}

pub(crate) fn has_current() -> bool {
    let owner = hart::current().slot();
    PENDING[owner].load(Ordering::Acquire) != 0
        || FINISH_PENDING[owner].load(Ordering::Acquire) != 0
        || super::retirement::has_current()
}
