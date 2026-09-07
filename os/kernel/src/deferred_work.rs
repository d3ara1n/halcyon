//! Commit 后特权 work debt：固定槽、owner hart 与安全点分批推进。

use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::{
    hart, registry,
    sync::Spinlock,
    task::proc::{MemoryChangeCompletion, Process},
};

const HARTS: usize = hart::HART_NUM_LIMIT;
const SLOTS: usize = crate::task::resources::MEMORY_CHANGE_GLOBAL_LIMIT;
/// 每个 admitted Process 在出生时支付一槽；容量与 sponsor 真值相同。
const TERMINATION_SLOTS: usize = crate::task::resources::PROCESS_GLOBAL_LIMIT;
const MAX_STEPS_PER_SAFE_POINT: usize = 16;
const MAX_STEPS_PER_DEBT_TURN: usize = 4;

type Debts = work_debt::WorkDebts<Arc<MemoryChangeCompletion>, HARTS, SLOTS>;
type UnpublishedDebts = work_debt::WorkDebts<Arc<Process>, HARTS, SLOTS>;

struct TerminationWork {
    process: Arc<Process>,
    cursor: usize,
    slots: usize,
}

type TerminationDebts = work_debt::WorkDebts<TerminationWork, HARTS, TERMINATION_SLOTS>;

static DEBTS: Spinlock<Debts> = Spinlock::new(crate::sync::ranks::WORK_DEBT, Debts::new());
static UNPUBLISHED_DEBTS: Spinlock<UnpublishedDebts> =
    Spinlock::new(crate::sync::ranks::WORK_DEBT, UnpublishedDebts::new());
static TERMINATION_DEBTS: Spinlock<TerminationDebts> =
    Spinlock::new(crate::sync::ranks::WORK_DEBT, TerminationDebts::new());
/// 每 owner 的已发布债务数是无锁 Pending 电平；常态安全点不争全局队列锁。
static PENDING: [AtomicUsize; HARTS] = [const { AtomicUsize::new(0) }; HARTS];
static UNPUBLISHED_PENDING: [AtomicUsize; HARTS] = [const { AtomicUsize::new(0) }; HARTS];
static TERMINATION_PENDING: [AtomicUsize; HARTS] = [const { AtomicUsize::new(0) }; HARTS];

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

pub(crate) struct UnpublishedReservation(Option<work_debt::Reservation>);

pub(crate) fn reserve_unpublished() -> Result<UnpublishedReservation, ()> {
    UNPUBLISHED_DEBTS
        .lock()
        .reserve()
        .map(|reservation| UnpublishedReservation(Some(reservation)))
        .map_err(|_| ())
}

impl UnpublishedReservation {
    pub(crate) fn publish(mut self, process: Arc<Process>) {
        let owner = hart::current().slot();
        let reservation = self
            .0
            .take()
            .expect("unpublished reservation published twice");
        UNPUBLISHED_DEBTS
            .lock()
            .publish(reservation, owner, process)
            .unwrap_or_else(|_| panic!("reserved unpublished slot must publish"));
        UNPUBLISHED_PENDING[owner].fetch_add(1, Ordering::Release);
        let failed = registry::try_ipi_slots(1u64 << owner);
        if failed != 0 {
            warn!(
                Task,
                "Unpublished drain doorbell failed for hart slot {owner}; work remains pending"
            );
        }
    }
}

impl Drop for UnpublishedReservation {
    fn drop(&mut self) {
        if let Some(reservation) = self.0.take() {
            assert!(
                UNPUBLISHED_DEBTS.lock().cancel(reservation),
                "reserved unpublished slot must roll back"
            );
        }
    }
}

pub(crate) struct TerminationReservation(Option<work_debt::Reservation>);

pub(crate) fn reserve_termination() -> Result<TerminationReservation, ()> {
    TERMINATION_DEBTS
        .lock()
        .reserve()
        .map(|reservation| TerminationReservation(Some(reservation)))
        .map_err(|_| ())
}

impl TerminationReservation {
    pub(crate) fn publish(mut self, process: Arc<Process>, slots: usize) {
        assert!(
            slots != 0,
            "empty termination cleanup must not be published"
        );
        let owner = hart::current().slot();
        let reservation = self
            .0
            .take()
            .expect("termination reservation published twice");
        TERMINATION_DEBTS
            .lock()
            .publish(
                reservation,
                owner,
                TerminationWork {
                    process,
                    cursor: 0,
                    slots,
                },
            )
            .unwrap_or_else(|_| panic!("reserved termination slot must publish"));
        TERMINATION_PENDING[owner].fetch_add(1, Ordering::Release);
        ring_owner(owner);
    }
}

impl Drop for TerminationReservation {
    fn drop(&mut self) {
        if let Some(reservation) = self.0.take() {
            assert!(
                TERMINATION_DEBTS.lock().cancel(reservation),
                "reserved termination slot must roll back"
            );
        }
    }
}

/// trap/scheduler 安全点先消费 Remote Call，再按固定预算推进本 hart 的 work debt。
pub(crate) fn drain_current() -> usize {
    let remote = crate::remote_call::drain_current();
    let owner = hart::current().slot();
    let mut steps = 0;
    let reserve_unpublished = usize::from(UNPUBLISHED_PENDING[owner].load(Ordering::Acquire) != 0);
    let reserve_termination = usize::from(TERMINATION_PENDING[owner].load(Ordering::Acquire) != 0);
    let memory_budget = MAX_STEPS_PER_SAFE_POINT - reserve_unpublished - reserve_termination;
    while steps < memory_budget {
        let Some(taken) = DEBTS.lock().take(owner) else {
            break;
        };
        let (token, completion) = taken.into_parts();
        let turn = (memory_budget - steps).min(MAX_STEPS_PER_DEBT_TURN);
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
    let unpublished_budget = MAX_STEPS_PER_SAFE_POINT - reserve_termination;
    while steps < unpublished_budget {
        let Some(taken) = UNPUBLISHED_DEBTS.lock().take(owner) else {
            break;
        };
        let (token, process) = taken.into_parts();
        let turn = (unpublished_budget - steps).min(MAX_STEPS_PER_DEBT_TURN);
        let blocked = !process.lifecycle.is_reapable();
        let (used, complete) = if blocked {
            // termination debt 拥有成员清理；轮询一次屏障计一个单位，随后
            // 把本安全点剩余预算让给该队列。
            (1, false)
        } else {
            let _gate = process.drain_gate.lock();
            process.drain_batch(turn)
        };
        assert!(
            used > 0 && used <= turn,
            "unpublished drain exceeded its debt turn"
        );
        steps += used;
        if complete {
            assert!(
                UNPUBLISHED_DEBTS.lock().finish(token),
                "taken unpublished slot must finish"
            );
            let previous = UNPUBLISHED_PENDING[owner].fetch_sub(1, Ordering::AcqRel);
            assert!(previous > 0, "finished unpublished slot must be pending");
            drop(process);
        } else {
            UNPUBLISHED_DEBTS
                .lock()
                .requeue(token, process)
                .unwrap_or_else(|_| panic!("taken unpublished slot must requeue"));
            if blocked {
                break;
            }
        }
    }
    while steps < MAX_STEPS_PER_SAFE_POINT {
        let Some(taken) = TERMINATION_DEBTS.lock().take(owner) else {
            break;
        };
        let (token, mut work) = taken.into_parts();
        let turn = (MAX_STEPS_PER_SAFE_POINT - steps).min(MAX_STEPS_PER_DEBT_TURN);
        let (used, complete) = crate::task::process::advance_termination_cleanup(
            &work.process,
            &mut work.cursor,
            work.slots,
            turn,
        );
        assert!(
            used > 0 && used <= turn,
            "termination cleanup exceeded its debt turn"
        );
        steps += used;
        if complete {
            assert!(
                TERMINATION_DEBTS.lock().finish(token),
                "taken termination slot must finish"
            );
            let previous = TERMINATION_PENDING[owner].fetch_sub(1, Ordering::AcqRel);
            assert!(previous > 0, "finished termination slot must be pending");
            drop(work.process);
        } else {
            TERMINATION_DEBTS
                .lock()
                .requeue(token, work)
                .unwrap_or_else(|_| panic!("taken termination slot must requeue"));
        }
    }
    if has_current() {
        ring_owner(owner);
    }
    remote + steps
}

/// idle 双重检查使用 Pending 电平，避免门铃失败或合并后带债入睡。
pub(crate) fn has_current() -> bool {
    let owner = hart::current().slot();
    PENDING[owner].load(Ordering::Acquire) != 0
        || UNPUBLISHED_PENDING[owner].load(Ordering::Acquire) != 0
        || TERMINATION_PENDING[owner].load(Ordering::Acquire) != 0
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
