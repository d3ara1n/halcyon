//! Commit 后特权 work debt：固定槽、owner hart 与安全点分批推进。

use alloc::sync::Arc;

use crate::{
    hart, registry,
    sync::Spinlock,
    task::proc::{MemoryChangeCompletion, Process},
    task::retirement::RetirementTicket,
    work_ledger::{DebtLedger, Reservation as LedgerReservation},
};

const SLOTS: usize = crate::task::resources::MEMORY_CHANGE_GLOBAL_LIMIT;
/// 当前唯一生产者是 boot 的 spawn_from_elf；正式 launcher 并发接入时须
/// 重新按真实来源上界建立独立 admission，不借用内存事务容量。
const UNPUBLISHED_SLOTS: usize = 1;
/// 每个 admitted Process 在出生时支付一槽；容量与 sponsor 真值相同。
const TERMINATION_SLOTS: usize = crate::task::resources::PROCESS_GLOBAL_LIMIT;

type Debts = DebtLedger<Arc<MemoryChangeCompletion>, SLOTS>;
type UnpublishedDebts = DebtLedger<Arc<Process>, UNPUBLISHED_SLOTS>;

pub(crate) struct TerminationWork {
    process: Arc<Process>,
    cursor: usize,
    slots: usize,
}

type TerminationDebts = DebtLedger<TerminationWork, TERMINATION_SLOTS>;
type FinalizationDebts = DebtLedger<Arc<Process>, TERMINATION_SLOTS>;

static DEBTS: Debts = Debts::new(work_debt::TableId::new(1));
static UNPUBLISHED_DEBTS: UnpublishedDebts = UnpublishedDebts::new(work_debt::TableId::new(2));
static TERMINATION_DEBTS: TerminationDebts = TerminationDebts::new(work_debt::TableId::new(3));
static FINALIZATION_DEBTS: FinalizationDebts = FinalizationDebts::new(work_debt::TableId::new(6));

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WaitKey {
    pub(crate) context: usize,
    pub(crate) epoch: u64,
}

trait ProcessDependency {
    fn register_process(&self, action: WakeAction);
}

impl ProcessDependency for Dependency {
    fn register_process(&self, action: WakeAction) {
        self.register(action, || false);
    }
}

impl ProcessDependency for RetirementTicket {
    fn register_process(&self, action: WakeAction) {
        self.register(action, || false);
    }
}

fn park_process_debt<const SLOTS: usize, S: ProcessDependency>(
    token: crate::work_ledger::Token<Arc<Process>, SLOTS>,
    process: Arc<Process>,
    dependency: &S,
    publish: fn(work_debt::WakeToken),
) {
    let wake = token
        .arm_wake()
        .expect("blocked process debt must own its wake");
    dependency.register_process(WakeAction::unkeyed(wake.into_raw(), publish));
    token.park(process);
}

impl WakeAction {
    pub(crate) fn unkeyed(token: work_debt::WakeToken, publish: fn(work_debt::WakeToken)) -> Self {
        Self {
            token,
            publish,
            key: None,
        }
    }
    pub(crate) fn keyed(
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

    /// 只取消带本轮 epoch 的等待；unkeyed 依赖没有取消入口，只能由
    /// 其来源条件变化后调用 `notify` 唤醒，避免把无 key 的依赖静默当成
    /// 可取消请求。
    pub(crate) fn cancel_keyed(&self, key: WaitKey) {
        let action = {
            let mut waiting = self.waiting.lock();
            if let Some(action) = waiting.as_ref() {
                debug_assert!(
                    action.key.is_some(),
                    "keyed dependency cancellation cannot target an unkeyed waiter"
                );
            }
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
    UNPUBLISHED_DEBTS.wake_raw(token);
}

fn wake_finalization(token: work_debt::WakeToken) {
    FINALIZATION_DEBTS.wake_raw(token);
}

/// 每个 Process 出生时预付；解除 Job 归属前转交独立的终段拥有根。
pub(crate) type FinalizationReservation = LedgerReservation<Arc<Process>, TERMINATION_SLOTS>;

pub(crate) fn reserve_finalization() -> Result<FinalizationReservation, ()> {
    FINALIZATION_DEBTS.reserve().map_err(|_| ())
}

/// Commit 前取得的固定槽。Drop 只可能发生在 Publish 前并精确取消 reservation。
pub(crate) type Reservation = LedgerReservation<Arc<MemoryChangeCompletion>, SLOTS>;

pub(crate) fn reserve() -> Result<Reservation, work_debt::ReserveError> {
    DEBTS.reserve()
}

/// 最后一个远端确认所在 hart 成为唯一推进 owner。调用点正位于同一个
/// `drain_current` 安全点，发布后会在 Remote drain 返回时立即观察 Pending，
/// 无需制造一次冗余 self-IPI；只有预算耗尽后的残债才重新敲门。
pub(crate) fn publish_memory(reservation: Reservation, completion: Arc<MemoryChangeCompletion>) {
    reservation.publish_quiet(completion);
}

pub(crate) type UnpublishedReservation = LedgerReservation<Arc<Process>, UNPUBLISHED_SLOTS>;

pub(crate) fn reserve_unpublished() -> Result<UnpublishedReservation, ()> {
    UNPUBLISHED_DEBTS.reserve().map_err(|_| ())
}

pub(crate) type TerminationReservation = LedgerReservation<TerminationWork, TERMINATION_SLOTS>;

pub(crate) fn reserve_termination() -> Result<TerminationReservation, ()> {
    TERMINATION_DEBTS.reserve().map_err(|_| ())
}

pub(crate) fn publish_termination(
    reservation: TerminationReservation,
    process: Arc<Process>,
    slots: usize,
) {
    assert!(
        slots != 0,
        "empty termination cleanup must not be published"
    );
    reservation.publish(TerminationWork {
        process,
        cursor: 0,
        slots,
    });
}

/// trap/scheduler 安全点先消费 Remote Call，再按固定预算推进本 hart 的 work debt。
pub(crate) fn drain_current() -> usize {
    let remote = crate::remote_call::drain_current();
    let owner = hart::current().slot();
    let mut budget = crate::work_ledger::safe_point_budget([
        DEBTS.pending(owner) != 0,
        UNPUBLISHED_DEBTS.pending(owner) != 0,
        TERMINATION_DEBTS.pending(owner) != 0,
        FINALIZATION_DEBTS.pending(owner) != 0,
    ]);
    while budget.turn(0) != 0 {
        let Some(taken) = DEBTS.take(owner) else {
            break;
        };
        let (token, completion) = taken.into_parts();
        let turn = budget.turn(0);
        let advance = completion.advance_retire(turn);
        debug_assert!(advance.work_done > 0 && advance.work_done <= turn);
        budget.charge(0, advance.work_done);
        if let Some(completion) = advance.completion {
            completion.deliver();
        }
        if advance.complete {
            token.finish();
        } else {
            token.requeue(completion);
        }
    }
    while budget.turn(1) != 0 {
        let Some(taken) = UNPUBLISHED_DEBTS.take(owner) else {
            break;
        };
        let (token, process) = taken.into_parts();
        let turn = budget.turn(1);
        if !process.lifecycle.is_reapable() {
            let wake = token
                .arm_wake()
                .expect("unpublished work must own its dependency");
            process.reapable_dependency.register(
                WakeAction::unkeyed(wake.into_raw(), wake_unpublished),
                || process.lifecycle.is_reapable(),
            );
            token.park(process);
            // 登记依赖是固定工作；不是把屏障轮询记作清理进度。
            budget.charge(1, 1);
            continue;
        }
        let (used, outcome) = process.advance_unpublished_drain(turn);
        assert!(used <= turn, "unpublished drain exceeded its debt turn");
        match outcome {
            super::task::proc::DrainBatchOutcome::Blocked(dependency) => {
                park_process_debt(token, process, &dependency, wake_unpublished);
                budget.charge(1, 1);
            }
            super::task::proc::DrainBatchOutcome::Complete => {
                budget.charge(1, used);
                token.finish();
                drop(process);
            }
            super::task::proc::DrainBatchOutcome::More => {
                assert!(used > 0, "unpublished drain made no progress");
                budget.charge(1, used);
                token.requeue(process);
            }
        }
    }
    while budget.turn(2) != 0 {
        let Some(taken) = TERMINATION_DEBTS.take(owner) else {
            break;
        };
        let (token, mut work) = taken.into_parts();
        let turn = budget.turn(2);
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
        budget.charge(2, used);
        if complete {
            token.finish();
            drop(work.process);
        } else {
            token.requeue(work);
        }
    }
    while budget.turn(3) != 0 {
        let Some(taken) = FINALIZATION_DEBTS.take(owner) else {
            break;
        };
        let (token, process) = taken.into_parts();
        let turn = budget.turn(3);
        let advance = process.advance_finalization_drain(turn);
        if let Some((used, outcome)) = advance {
            assert!(used <= turn, "finalization debt exceeded its debt turn");
            match outcome {
                super::task::proc::DrainBatchOutcome::Blocked(dependency) => {
                    park_process_debt(token, process, &dependency, wake_finalization);
                    budget.charge(3, 1);
                }
                super::task::proc::DrainBatchOutcome::Complete => {
                    budget.charge(3, used);
                    token.finish();
                    drop(process);
                }
                super::task::proc::DrainBatchOutcome::More => {
                    assert!(used > 0, "finalization drain made no progress");
                    budget.charge(3, used);
                    token.requeue(process);
                }
            }
        } else {
            let wake = token
                .arm_wake()
                .expect("finalization debt must own its dependency");
            process.finalization_dependency.register(
                WakeAction::unkeyed(wake.into_raw(), wake_finalization),
                || process.finalization_can_advance(),
            );
            token.park(process);
            budget.charge(3, 1);
        }
    }
    if has_current() {
        ring_owner(owner);
    }
    remote + budget.used()
}

/// idle 双重检查使用 Pending 电平，避免门铃失败或合并后带债入睡。
pub(crate) fn has_current() -> bool {
    let owner = hart::current().slot();
    DEBTS.pending(owner) != 0
        || UNPUBLISHED_DEBTS.pending(owner) != 0
        || TERMINATION_DEBTS.pending(owner) != 0
        || FINALIZATION_DEBTS.pending(owner) != 0
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
