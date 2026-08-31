//! Commit 后特权 work debt：固定槽、owner hart 与安全点分批推进。

use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::{hart, registry, sync::Spinlock, task::proc::MemoryChangeCompletion};

const HARTS: usize = hart::HART_NUM_LIMIT;
const SLOTS: usize = crate::task::resources::MEMORY_CHANGE_GLOBAL_LIMIT;
const MAX_STEPS_PER_SAFE_POINT: usize = 16;
const MAX_STEPS_PER_DEBT_TURN: usize = 4;

type Debts = work_debt::WorkDebts<Arc<MemoryChangeCompletion>, HARTS, SLOTS>;

static DEBTS: Spinlock<Debts> = Spinlock::new(crate::sync::ranks::WORK_DEBT, Debts::new());
/// 每 owner 的已发布债务数是无锁 Pending 电平；常态安全点不争全局队列锁。
static PENDING: [AtomicUsize; HARTS] = [const { AtomicUsize::new(0) }; HARTS];

/// Commit 前取得的固定槽。Drop 只可能发生在 Publish 前并精确取消 reservation。
pub(crate) struct Reservation(Option<work_debt::Reservation>);

pub(crate) fn reserve() -> Result<Reservation, work_debt::ReserveError> {
    DEBTS
        .lock()
        .reserve()
        .map(|reservation| Reservation(Some(reservation)))
}

impl Reservation {
    /// 最后一个远端确认所在 hart 成为唯一推进 owner。调用点正位于同一个
    /// `drain_current` 安全点，发布后会在 Remote drain 返回时立即观察 Pending，
    /// 无需制造一次冗余 self-IPI；只有预算耗尽后的残债才重新敲门。
    pub(crate) fn publish(mut self, completion: Arc<MemoryChangeCompletion>) {
        let owner = hart::current().slot();
        let reservation = self
            .0
            .take()
            .expect("work debt reservation published twice");
        DEBTS
            .lock()
            .publish(reservation, owner, completion)
            .unwrap_or_else(|_| panic!("reserved work debt slot must publish"));
        PENDING[owner].fetch_add(1, Ordering::Release);
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some(reservation) = self.0.take() {
            assert!(
                DEBTS.lock().cancel(reservation),
                "reserved work debt slot must roll back"
            );
        }
    }
}

/// trap/scheduler 安全点先消费 Remote Call，再按固定预算推进本 hart 的 work debt。
pub(crate) fn drain_current() -> usize {
    let remote = crate::remote_call::drain_current();
    let owner = hart::current().slot();
    if PENDING[owner].load(Ordering::Acquire) == 0 {
        return remote;
    }
    let mut steps = 0;
    while steps < MAX_STEPS_PER_SAFE_POINT {
        let Some(taken) = DEBTS.lock().take(owner) else {
            break;
        };
        let (token, completion) = taken.into_parts();
        let turn = (MAX_STEPS_PER_SAFE_POINT - steps).min(MAX_STEPS_PER_DEBT_TURN);
        let (used, complete) = completion.advance_retire(turn);
        debug_assert!(used > 0 && used <= turn);
        steps += used;
        if complete {
            assert!(DEBTS.lock().finish(token), "taken work debt must finish");
            let previous = PENDING[owner].fetch_sub(1, Ordering::AcqRel);
            assert!(previous > 0, "finished work debt must be pending");
        } else {
            DEBTS
                .lock()
                .requeue(token, completion)
                .unwrap_or_else(|_| panic!("taken work debt must requeue"));
        }
    }
    if has_current() {
        ring_owner(owner);
    }
    remote + steps
}

/// idle 双重检查使用 Pending 电平，避免门铃失败或合并后带债入睡。
pub(crate) fn has_current() -> bool {
    PENDING[hart::current().slot()].load(Ordering::Acquire) != 0
}

fn ring_owner(owner: usize) {
    let failed = registry::try_ipi_slots(1u64 << owner);
    if failed != 0 {
        warn!(
            Task,
            "Deferred-work doorbell failed for hart slot {owner}; work remains pending"
        );
    }
}
