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
type FinalizationDebts = work_debt::WorkDebts<Arc<Process>, HARTS, TERMINATION_SLOTS>;

static DEBTS: Spinlock<Debts> = Spinlock::new(
    crate::sync::ranks::WORK_DEBT,
    Debts::new_with_id(work_debt::TableId::new(1)),
);
static UNPUBLISHED_DEBTS: Spinlock<UnpublishedDebts> = Spinlock::new(
    crate::sync::ranks::WORK_DEBT,
    UnpublishedDebts::new_with_id(work_debt::TableId::new(2)),
);
static TERMINATION_DEBTS: Spinlock<TerminationDebts> = Spinlock::new(
    crate::sync::ranks::WORK_DEBT,
    TerminationDebts::new_with_id(work_debt::TableId::new(3)),
);
static FINALIZATION_DEBTS: Spinlock<FinalizationDebts> = Spinlock::new(
    crate::sync::ranks::WORK_DEBT,
    FinalizationDebts::new_with_id(work_debt::TableId::new(6)),
);
/// 每 owner 的已发布债务数是无锁 Pending 电平；常态安全点不争全局队列锁。
static PENDING: [AtomicUsize; HARTS] = [const { AtomicUsize::new(0) }; HARTS];
static UNPUBLISHED_PENDING: [AtomicUsize; HARTS] = [const { AtomicUsize::new(0) }; HARTS];
static TERMINATION_PENDING: [AtomicUsize; HARTS] = [const { AtomicUsize::new(0) }; HARTS];
static FINALIZATION_PENDING: [AtomicUsize; HARTS] = [const { AtomicUsize::new(0) }; HARTS];

pub(crate) mod selftest;

/// 单一内核债务执行者的依赖；来源条件由来源自身维护，不在此缓存电平。
pub(crate) struct Dependency {
    waiting: Spinlock<Option<WakeAction>>,
}

pub(crate) struct WakeAction {
    token: work_debt::WakeToken,
    publish: fn(work_debt::WakeToken),
    key: Option<WaitKey>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitKey {
    pub(crate) context: usize,
    pub(crate) epoch: u64,
}

impl WakeAction {
    pub(crate) fn unkeyed(token: work_debt::WakeToken, publish: fn(work_debt::WakeToken)) -> Self {
        Self {
            token,
            publish,
            key: None,
        }
    }
    pub(crate) fn new(
        token: work_debt::WakeToken,
        publish: fn(work_debt::WakeToken),
        key: WaitKey,
    ) -> Self {
        Self {
            token,
            publish,
            key: Some(key),
        }
    }

    fn publish(self) {
        (self.publish)(self.token);
    }
}

impl Dependency {
    pub(crate) const fn new() -> Self {
        Self {
            waiting: Spinlock::new(crate::sync::ranks::OBJECT_WAIT, None),
        }
    }

    pub(crate) fn register(&self, action: WakeAction, ready: impl FnOnce() -> bool) {
        let immediate = {
            let mut waiting = self.waiting.lock();
            assert!(waiting.is_none(), "kernel dependency already has a waiter");
            if ready() {
                Some(action)
            } else {
                *waiting = Some(action);
                None
            }
        };
        if let Some(action) = immediate {
            action.publish();
        }
    }

    pub(crate) fn notify(&self) {
        let action = self.waiting.lock().take();
        if let Some(action) = action {
            action.publish();
        }
    }

    pub(crate) fn cancel(&self, key: WaitKey) {
        let action = {
            let mut waiting = self.waiting.lock();
            if waiting
                .as_ref()
                .is_some_and(|action| action.key == Some(key))
            {
                waiting.take()
            } else {
                None
            }
        };
        if let Some(action) = action {
            action.publish();
        }
    }
}

fn wake_unpublished(token: work_debt::WakeToken) {
    let result = {
        let mut debts = UNPUBLISHED_DEBTS.lock();
        let result = debts
            .wake(token)
            .unwrap_or_else(|_| panic!("unpublished dependency wake must remain owned"));
        if let work_debt::WakeResult::Runnable { owner } = result {
            UNPUBLISHED_PENDING[owner].fetch_add(1, Ordering::Release);
        }
        result
    };
    if let work_debt::WakeResult::Runnable { owner } = result {
        ring_owner(owner);
    }
}

fn wake_finalization(token: work_debt::WakeToken) {
    let result = {
        let mut debts = FINALIZATION_DEBTS.lock();
        let result = debts
            .wake(token)
            .unwrap_or_else(|_| panic!("finalization wake must remain owned"));
        if let work_debt::WakeResult::Runnable { owner } = result {
            FINALIZATION_PENDING[owner].fetch_add(1, Ordering::Release);
        }
        result
    };
    if let work_debt::WakeResult::Runnable { owner } = result {
        ring_owner(owner);
    }
}

/// 每个 Process 出生时预付；解除 Job 归属前转交独立的终段拥有根。
pub(crate) struct FinalizationReservation(Option<work_debt::Reservation>);

pub(crate) fn reserve_finalization() -> Result<FinalizationReservation, ()> {
    FINALIZATION_DEBTS
        .lock()
        .reserve()
        .map(|reservation| FinalizationReservation(Some(reservation)))
        .map_err(|_| ())
}

impl FinalizationReservation {
    pub(crate) fn publish(mut self, process: Arc<Process>) {
        let owner = hart::current().slot();
        let reservation = self
            .0
            .take()
            .expect("finalization reservation published twice");
        {
            let mut debts = FINALIZATION_DEBTS.lock();
            debts
                .publish(reservation, owner, process)
                .unwrap_or_else(|_| panic!("finalization debt must publish"));
            FINALIZATION_PENDING[owner].fetch_add(1, Ordering::Release);
        }
        ring_owner(owner);
    }
}

impl Drop for FinalizationReservation {
    fn drop(&mut self) {
        if let Some(reservation) = self.0.take() {
            assert!(
                FINALIZATION_DEBTS.lock().cancel(reservation).is_ok(),
                "finalization reservation must roll back"
            );
        }
    }
}

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
                DEBTS.lock().cancel(reservation).is_ok(),
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
        {
            let mut debts = UNPUBLISHED_DEBTS.lock();
            debts
                .publish(reservation, owner, process)
                .unwrap_or_else(|_| panic!("reserved unpublished slot must publish"));
            UNPUBLISHED_PENDING[owner].fetch_add(1, Ordering::Release);
        }
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
                UNPUBLISHED_DEBTS.lock().cancel(reservation).is_ok(),
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
                TERMINATION_DEBTS.lock().cancel(reservation).is_ok(),
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
    let reserve_finalization =
        usize::from(FINALIZATION_PENDING[owner].load(Ordering::Acquire) != 0);
    let memory_budget =
        MAX_STEPS_PER_SAFE_POINT - reserve_unpublished - reserve_termination - reserve_finalization;
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
            assert!(
                DEBTS.lock().finish(token).is_ok(),
                "taken work debt must finish"
            );
            let previous = PENDING[owner].fetch_sub(1, Ordering::AcqRel);
            assert!(previous > 0, "finished work debt must be pending");
        } else {
            DEBTS
                .lock()
                .requeue(token, completion)
                .unwrap_or_else(|_| panic!("taken work debt must requeue"));
        }
    }
    let unpublished_budget = MAX_STEPS_PER_SAFE_POINT - reserve_termination - reserve_finalization;
    while steps < unpublished_budget {
        let Some(taken) = UNPUBLISHED_DEBTS.lock().take(owner) else {
            break;
        };
        let (token, process) = taken.into_parts();
        let turn = (unpublished_budget - steps).min(MAX_STEPS_PER_DEBT_TURN);
        if !process.lifecycle.is_reapable() {
            let wake = UNPUBLISHED_DEBTS
                .lock()
                .arm_wake(&token)
                .expect("unpublished work must own its dependency");
            process.reapable_dependency.register(
                WakeAction {
                    token: wake,
                    publish: wake_unpublished,
                    key: None,
                },
                || process.lifecycle.is_reapable(),
            );
            {
                let mut debts = UNPUBLISHED_DEBTS.lock();
                let parked = debts
                    .park(token, process)
                    .unwrap_or_else(|_| panic!("unpublished work must park with its dependency"));
                if parked == work_debt::ParkResult::Parked {
                    let previous = UNPUBLISHED_PENDING[owner].fetch_sub(1, Ordering::AcqRel);
                    assert!(
                        previous > 0,
                        "parked unpublished work must have been runnable"
                    );
                }
            }
            // 登记依赖是固定工作；不是把屏障轮询记作清理进度。
            steps += 1;
            continue;
        }
        let (used, complete) = {
            let _gate = process.drain_gate.lock();
            process.drain_batch(turn)
        };
        assert!(
            used <= turn && (used > 0 || !complete),
            "unpublished drain exceeded its debt turn"
        );
        if used == 0 {
            let dependency = process.drain_dependency();
            let wake = UNPUBLISHED_DEBTS
                .lock()
                .arm_wake(&token)
                .expect("unpublished retirement must own its wake");
            dependency.register(WakeAction::unkeyed(wake, wake_unpublished), || false);
            let mut debts = UNPUBLISHED_DEBTS.lock();
            if debts
                .park(token, process)
                .unwrap_or_else(|_| panic!("unpublished retirement must park"))
                == work_debt::ParkResult::Parked
            {
                assert!(
                    UNPUBLISHED_PENDING[owner].fetch_sub(1, Ordering::AcqRel) > 0,
                    "parked unpublished retirement must be pending"
                );
            }
            steps += 1;
            continue;
        }
        steps += used;
        if complete {
            {
                let mut debts = UNPUBLISHED_DEBTS.lock();
                assert!(
                    debts.finish(token).is_ok(),
                    "taken unpublished slot must finish"
                );
                let previous = UNPUBLISHED_PENDING[owner].fetch_sub(1, Ordering::AcqRel);
                assert!(previous > 0, "finished unpublished slot must be pending");
            }
            drop(process);
        } else {
            UNPUBLISHED_DEBTS
                .lock()
                .requeue(token, process)
                .unwrap_or_else(|_| panic!("taken unpublished slot must requeue"));
        }
    }
    let termination_budget = MAX_STEPS_PER_SAFE_POINT - reserve_finalization;
    while steps < termination_budget {
        let Some(taken) = TERMINATION_DEBTS.lock().take(owner) else {
            break;
        };
        let (token, mut work) = taken.into_parts();
        let turn = (termination_budget - steps).min(MAX_STEPS_PER_DEBT_TURN);
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
                TERMINATION_DEBTS.lock().finish(token).is_ok(),
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
    while steps < MAX_STEPS_PER_SAFE_POINT {
        let Some(taken) = FINALIZATION_DEBTS.lock().take(owner) else {
            break;
        };
        let (token, process) = taken.into_parts();
        let turn = (MAX_STEPS_PER_SAFE_POINT - steps).min(MAX_STEPS_PER_DEBT_TURN);
        let advance = {
            let _gate = process.drain_gate.lock();
            if process.drain_active.load(Ordering::Acquire) {
                None
            } else {
                Some(process.drain_batch(turn))
            }
        };
        if let Some((used, complete)) = advance {
            assert!(
                used <= turn && (used > 0 || complete),
                "finalization debt made no progress"
            );
            steps += used.max(1);
            if complete {
                let mut debts = FINALIZATION_DEBTS.lock();
                assert!(debts.finish(token).is_ok(), "finalization debt must finish");
                let previous = FINALIZATION_PENDING[owner].fetch_sub(1, Ordering::AcqRel);
                assert!(previous > 0, "finalization debt must have been runnable");
                drop(debts);
                drop(process);
            } else {
                FINALIZATION_DEBTS
                    .lock()
                    .requeue(token, process)
                    .unwrap_or_else(|_| panic!("finalization debt must requeue"));
            }
        } else {
            let wake = FINALIZATION_DEBTS
                .lock()
                .arm_wake(&token)
                .expect("finalization debt must own its dependency");
            process.finalization_dependency.register(
                WakeAction {
                    token: wake,
                    publish: wake_finalization,
                    key: None,
                },
                || !process.drain_active.load(Ordering::Acquire),
            );
            let mut debts = FINALIZATION_DEBTS.lock();
            let parked = debts
                .park(token, process)
                .unwrap_or_else(|_| panic!("finalization debt must retain its owner"));
            if parked == work_debt::ParkResult::Parked {
                let previous = FINALIZATION_PENDING[owner].fetch_sub(1, Ordering::AcqRel);
                assert!(previous > 0, "parked finalization must have been runnable");
            }
            steps += 1;
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
        || FINALIZATION_PENDING[owner].load(Ordering::Acquire) != 0
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
