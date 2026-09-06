//! 可等待对象的有界通知债务。
//!
//! 注册时支付一个固定槽；信号发布只把目标对象交给 owner hart，真正的
//! WaitContext offer 与订阅排水在安全点按固定预算推进。对象更新与候选
//! 快照仍由 ObjectWaitState 锁内完成，队列不重新读取 live signals。

use core::sync::atomic::{AtomicUsize, Ordering};

use super::object::ObjectRef;
use crate::{hart, registry, sync::Spinlock};

const HARTS: usize = hart::HART_NUM_LIMIT;
/// 上限覆盖固定的 WaitMany 输入槽，超过即在订阅安装前 fail closed。
pub(crate) const SLOTS: usize = 8_192;
const MAX_STEPS_PER_SAFE_POINT: usize = 16;
const MAX_STEPS_PER_DEBT_TURN: usize = 4;

type Debts = work_debt::WorkDebts<ObjectRef, HARTS, SLOTS>;

static DEBTS: Spinlock<Debts> = Spinlock::new(crate::sync::ranks::WORK_DEBT, Debts::new());
static PENDING: [AtomicUsize; HARTS] = [const { AtomicUsize::new(0) }; HARTS];

/// 一个已登记订阅未来命中的固定槽；未发布时 Drop 精确取消。
pub(crate) struct Reservation(Option<work_debt::Reservation>);

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

/// 由调度安全点消费；每个对象一次最多推进固定数量的 waiter。
pub(crate) fn drain_current() -> usize {
    let owner = hart::current().slot();
    if PENDING[owner].load(Ordering::Acquire) == 0 {
        return 0;
    }
    let mut steps = 0;
    while steps < MAX_STEPS_PER_SAFE_POINT {
        let Some(taken) = DEBTS.lock().take(owner) else {
            break;
        };
        let (token, target) = taken.into_parts();
        let turn = (MAX_STEPS_PER_SAFE_POINT - steps).min(MAX_STEPS_PER_DEBT_TURN);
        let (used, complete) = target.drain_waiters(turn);
        steps += used;
        if complete {
            assert!(
                DEBTS.lock().finish(token),
                "taken notification slot must finish"
            );
            target.complete_waiter_drain();
            let previous = PENDING[owner].fetch_sub(1, Ordering::AcqRel);
            assert!(previous > 0, "finished notification slot must be pending");
        } else {
            DEBTS
                .lock()
                .requeue(token, target)
                .unwrap_or_else(|_| panic!("taken notification slot must requeue"));
        }
    }
    if DEBTS.lock().has_pending(owner) {
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
    PENDING[hart::current().slot()].load(Ordering::Acquire) != 0
}
