//! 可等待对象的有界通知债务。
//!
//! 注册时支付一个固定槽；信号发布只把目标对象交给 owner hart，真正的
//! WaitContext offer 与订阅排水在安全点按固定预算推进。对象更新与候选
//! 快照仍由 ObjectWaitState 锁内完成，队列不重新读取 live signals。

use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

use super::{object::ObjectRef, wait::WaitContext};
use crate::{hart, registry, sync::Spinlock};

const HARTS: usize = hart::HART_NUM_LIMIT;
/// 上限覆盖固定的 WaitMany 输入槽，超过即在订阅安装前 fail closed。
pub(crate) const SLOTS: usize = 8_192;
const MAX_STEPS_PER_SAFE_POINT: usize = 16;
const MAX_STEPS_PER_DEBT_TURN: usize = 4;

type Debts = work_debt::WorkDebts<ObjectRef, HARTS, SLOTS>;
type FinishDebts = work_debt::WorkDebts<Arc<WaitContext>, HARTS, SLOTS>;

static DEBTS: Spinlock<Debts> = Spinlock::new(crate::sync::ranks::WORK_DEBT, Debts::new());
static FINISH_DEBTS: Spinlock<FinishDebts> =
    Spinlock::new(crate::sync::ranks::WORK_DEBT, FinishDebts::new());
static PENDING: [AtomicUsize; HARTS] = [const { AtomicUsize::new(0) }; HARTS];
static FINISH_PENDING: [AtomicUsize; HARTS] = [const { AtomicUsize::new(0) }; HARTS];

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
        DEBTS
            .lock()
            .publish(reservation, owner, target)
            .unwrap_or_else(|_| panic!("reserved notification slot must publish"));
        PENDING[owner].fetch_add(1, Ordering::Release);
        let failed = registry::try_ipi_slots(1u64 << owner);
        if failed != 0 {
            warn!(
                Task,
                "Notification work doorbell failed for hart slot {owner}; work remains pending"
            );
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(reservation) = self.0.take() {
            assert!(
                DEBTS.lock().cancel(reservation),
                "reserved notification slot must roll back"
            );
        }
    }
}

/// 完成责任槽在 WaitContext 创建时预付，保证 outcome 获胜后不再申请存储。
pub(crate) struct FinishReservation(Option<work_debt::Reservation>);

pub(crate) fn reserve_finish() -> Result<FinishReservation, ()> {
    FINISH_DEBTS
        .lock()
        .reserve()
        .map(|reservation| FinishReservation(Some(reservation)))
        .map_err(|_| ())
}

pub(crate) fn publish_finish(mut reservation: FinishReservation, context: Arc<WaitContext>) {
    let owner = hart::current().slot();
    let reservation = reservation
        .0
        .take()
        .expect("finish reservation published twice");
    FINISH_DEBTS
        .lock()
        .publish(reservation, owner, context)
        .unwrap_or_else(|_| panic!("reserved finish slot must publish"));
    FINISH_PENDING[owner].fetch_add(1, Ordering::Release);
    let failed = registry::try_ipi_slots(1u64 << owner);
    if failed != 0 {
        warn!(
            Task,
            "Notification completion doorbell failed for hart slot {owner}; work remains pending"
        );
    }
}

impl Drop for FinishReservation {
    fn drop(&mut self) {
        if let Some(reservation) = self.0.take() {
            assert!(
                FINISH_DEBTS.lock().cancel(reservation),
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
    let notification_budget = MAX_STEPS_PER_SAFE_POINT - reserve_finish;
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
            let reservation = DEBTS
                .lock()
                .rearm(token)
                .unwrap_or_else(|_| panic!("taken notification slot must rearm"));
            let completion = target.complete_waiter_drain(Reservation(Some(reservation)));
            let previous = PENDING[owner].fetch_sub(1, Ordering::AcqRel);
            assert!(previous > 0, "finished notification slot must be pending");
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
    while steps < MAX_STEPS_PER_SAFE_POINT {
        let Some(taken) = FINISH_DEBTS.lock().take(owner) else {
            break;
        };
        let (token, context) = taken.into_parts();
        let turn = (MAX_STEPS_PER_SAFE_POINT - steps).min(MAX_STEPS_PER_DEBT_TURN);
        let (used, complete) = context.finish_step(turn);
        assert!(
            used > 0 && used <= turn,
            "wait completion exceeded its debt turn"
        );
        steps += used;
        if complete {
            assert!(
                FINISH_DEBTS.lock().finish(token),
                "taken finish slot must finish"
            );
            let previous = FINISH_PENDING[owner].fetch_sub(1, Ordering::AcqRel);
            assert!(previous > 0, "finished completion slot must be pending");
        } else {
            FINISH_DEBTS
                .lock()
                .requeue(token, context)
                .unwrap_or_else(|_| panic!("taken finish slot must requeue"));
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
    steps
}

pub(crate) fn has_current() -> bool {
    let owner = hart::current().slot();
    PENDING[owner].load(Ordering::Acquire) != 0
        || FINISH_PENDING[owner].load(Ordering::Acquire) != 0
}
