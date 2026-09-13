#![no_std]

//! 等待安装与完成权交接的无锁核心。
//!
//! 对象订阅、线程所有权和结果交付留给内核包装层；本 crate 只保证：
//! Installing 期间事件不能取得完成权，arm 后恰有一方完成，outcome 只写一次。

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicU64, Ordering},
};

const INSTALLING: u8 = 0;
const ARMED: u8 = 1;
const FINISHING: u8 = 2;
const DONE: u8 = 3;

const OUTCOME_EMPTY: u8 = 0;
const OUTCOME_WRITING: u8 = 1;
const OUTCOME_READY: u8 = 2;
const PHASE_MASK: u64 = 3;
const ABANDONED_FLAG: u64 = 4;
const EPOCH_SHIFT: u32 = 3;

/// 等待轮次身份；范围由原子状态字的标记位布局推导。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitEpoch(u64);

impl WaitEpoch {
    pub const fn value(self) -> u64 {
        self.0
    }

    pub const fn next(self) -> Option<Self> {
        if self.0 == u64::MAX >> EPOCH_SHIFT {
            None
        } else {
            Some(Self(self.0 + 1))
        }
    }
}

const fn phase_word(epoch: WaitEpoch, phase: u8) -> u64 {
    (epoch.0 << EPOCH_SHIFT) | phase as u64
}

const fn outcome_word(epoch: WaitEpoch, state: u8) -> u64 {
    (epoch.0 << 2) | state as u64
}

const TIMEOUT_UNREGISTERED: u64 = 0;
const TIMEOUT_CLOSED: u64 = !timer_queue::TimerToken::RAW_MASK;

/// WaitContext 的 timeout 注册状态：未登记、稳定 token 或 Closed。
///
/// 队列先产生 token，再尝试发布；完成方统一关闭并取走 token 注销。
/// 到期方只能退休仍由自身 token 表示的注册，因而取消/到期竞争幂等。
pub struct TimeoutRegistration {
    state: core::sync::atomic::AtomicU64,
}

impl TimeoutRegistration {
    pub const fn new() -> Self {
        Self {
            state: core::sync::atomic::AtomicU64::new(TIMEOUT_UNREGISTERED),
        }
    }

    /// 发布新登记。false 表示 context 已关闭，调用方必须立即注销 token。
    pub fn publish(&self, token: timer_queue::TimerToken) -> bool {
        if token.raw() == 0 || token.raw() & TIMEOUT_CLOSED != 0 {
            return false;
        }
        self.state
            .compare_exchange(
                TIMEOUT_UNREGISTERED,
                token.raw(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// 关闭位和待注销 token 同字发布，允许在对象订阅锁内执行。
    pub fn close(&self) {
        self.state.fetch_or(TIMEOUT_CLOSED, Ordering::AcqRel);
    }

    /// 锁外完成路径取走待注销 token。多次调用幂等。
    pub fn take_cancellation(&self) -> Option<timer_queue::TimerToken> {
        let mut current = self.state.load(Ordering::Acquire);
        loop {
            if current & TIMEOUT_CLOSED == 0 || current == TIMEOUT_CLOSED {
                return None;
            }
            match self.state.compare_exchange_weak(
                current,
                TIMEOUT_CLOSED,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(timer_queue::TimerToken::from_raw(current & !TIMEOUT_CLOSED)),
                Err(changed) => current = changed,
            }
        }
    }

    /// timer queue 弹出到期项后的退休仲裁；true 表示本调用方可提交
    /// Timeout outcome，false 表示它已被另一完成路径关闭或取代。
    pub fn retire(&self, token: timer_queue::TimerToken) -> bool {
        self.state
            .compare_exchange(
                token.raw(),
                TIMEOUT_CLOSED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }
}

impl Default for TimeoutRegistration {
    fn default() -> Self {
        Self::new()
    }
}

/// 外部提交 outcome 的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfferResult {
    /// 已有另一 outcome 或另一方已取得完成权。
    Lost,
    /// outcome 已记录，但 Installing 期间只能由安装者完成。
    Deferred,
    /// 调用方取得完成权，必须完成清理并调用 [`WaitCore::mark_done`]。
    Complete,
}

/// 安装者 arm 后的结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmResult<O> {
    /// 无待决 outcome，Context 已 Armed。
    Armed,
    /// 安装者取得完成权及唯一 outcome。
    Complete(O),
    /// 并发 offer 已取得完成权；安装者不得再触达完成资源。
    ExternalCompleter,
}

/// 只写一次 outcome + Installing/Armed 完成权交接。
pub struct WaitCore<O: Copy> {
    phase: AtomicU64,
    outcome_state: AtomicU64,
    outcome: UnsafeCell<MaybeUninit<O>>,
}

// SAFETY: outcome 只有赢得 EMPTY→WRITING 的线程写；READY 的 Release/Acquire
// 发布读值。O: Copy + Send 不含需要并发 Drop 的所有权。
unsafe impl<O: Copy + Send> Sync for WaitCore<O> {}
unsafe impl<O: Copy + Send> Send for WaitCore<O> {}

impl<O: Copy> WaitCore<O> {
    pub const fn new() -> Self {
        Self {
            phase: AtomicU64::new(phase_word(WaitEpoch(1), INSTALLING)),
            outcome_state: AtomicU64::new(outcome_word(WaitEpoch(1), OUTCOME_EMPTY)),
            outcome: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// 提交唯一 outcome，并在 Armed 时竞争完成权。
    pub fn offer(&self, outcome: O) -> OfferResult {
        self.offer_in(self.epoch(), outcome)
    }

    pub fn epoch(&self) -> WaitEpoch {
        WaitEpoch(self.phase.load(Ordering::Acquire) >> EPOCH_SHIFT)
    }

    /// 身份和 outcome 写权在同一个 CAS 中验证，旧轮次不能写入新槽。
    pub fn offer_in(&self, epoch: WaitEpoch, outcome: O) -> OfferResult {
        let phase = self.phase.load(Ordering::Acquire);
        if phase >> EPOCH_SHIFT != epoch.0 || phase & PHASE_MASK == DONE as u64 {
            return OfferResult::Lost;
        }
        if self
            .outcome_state
            .compare_exchange(
                outcome_word(epoch, OUTCOME_EMPTY),
                outcome_word(epoch, OUTCOME_WRITING),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return OfferResult::Lost;
        }
        // SAFETY: 本线程独占 OUTCOME_WRITING，且 outcome 槽只写一次。
        unsafe { (*self.outcome.get()).write(outcome) };
        self.outcome_state
            .store(outcome_word(epoch, OUTCOME_READY), Ordering::Release);

        let phase = self.phase.load(Ordering::Acquire);
        if phase >> EPOCH_SHIFT != epoch.0 {
            return OfferResult::Lost;
        }
        match (phase & PHASE_MASK) as u8 {
            INSTALLING => OfferResult::Deferred,
            ARMED => {
                if self.claim_armed(epoch) {
                    OfferResult::Complete
                } else {
                    OfferResult::Lost
                }
            }
            FINISHING | DONE => OfferResult::Lost,
            _ => unreachable!("invalid wait phase"),
        }
    }

    pub fn has_outcome(&self) -> bool {
        self.has_outcome_in(self.epoch())
    }

    fn has_outcome_in(&self, epoch: WaitEpoch) -> bool {
        self.outcome_state.load(Ordering::Acquire) == outcome_word(epoch, OUTCOME_READY)
    }

    /// 放弃请求独立于 outcome；已进入 FINISHING 仍可登记取消。
    pub fn abandon(&self, epoch: WaitEpoch) -> bool {
        let mut current = self.phase.load(Ordering::Acquire);
        loop {
            if current >> EPOCH_SHIFT != epoch.0
                || current & PHASE_MASK == DONE as u64
                || current & ABANDONED_FLAG != 0
            {
                return false;
            }
            match self.phase.compare_exchange_weak(
                current,
                current | ABANDONED_FLAG,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(changed) => current = changed,
            }
        }
    }

    pub fn is_abandoned(&self, epoch: WaitEpoch) -> bool {
        let current = self.phase.load(Ordering::Acquire);
        current >> EPOCH_SHIFT == epoch.0 && current & ABANDONED_FLAG != 0
    }

    /// Installing 安装者发现 outcome 后独占完成权。
    pub fn finish_installing(&self) -> Option<O> {
        self.finish_installing_in(self.epoch())
    }

    pub fn finish_installing_in(&self, epoch: WaitEpoch) -> Option<O> {
        if !self.has_outcome_in(epoch) {
            return None;
        }
        let mut current = self.phase.load(Ordering::Acquire);
        loop {
            if current >> EPOCH_SHIFT != epoch.0 || current & PHASE_MASK != INSTALLING as u64 {
                return None;
            }
            match self.phase.compare_exchange_weak(
                current,
                (current & !PHASE_MASK) | FINISHING as u64,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(self.outcome_in(epoch)),
                Err(changed) => current = changed,
            }
        }
    }

    /// 安装完全部订阅后 arm；闭合 offer 与 arm 的交错。
    pub fn arm(&self) -> ArmResult<O> {
        self.arm_in(self.epoch())
    }

    pub fn arm_in(&self, epoch: WaitEpoch) -> ArmResult<O> {
        let mut current = self.phase.load(Ordering::Acquire);
        loop {
            if current >> EPOCH_SHIFT != epoch.0 || current & PHASE_MASK != INSTALLING as u64 {
                return ArmResult::ExternalCompleter;
            }
            match self.phase.compare_exchange_weak(
                current,
                (current & !PHASE_MASK) | ARMED as u64,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(changed) => current = changed,
            }
        }

        if self.has_outcome_in(epoch) {
            if self.claim_armed(epoch) {
                ArmResult::Complete(self.outcome_in(epoch))
            } else {
                ArmResult::ExternalCompleter
            }
        } else {
            // outcome 可能在 has_outcome 之后才发布。offer 观察到 ARMED 后会
            // 自己竞争完成权；因此此处返回 Armed 不会丢完成。
            ArmResult::Armed
        }
    }

    /// 完成者在资源清理和结果交付结束后发布终态。
    pub fn mark_done(&self) {
        assert!(
            self.mark_done_in(self.epoch()),
            "only the finishing owner may mark a wait done"
        );
    }

    pub fn mark_done_in(&self, epoch: WaitEpoch) -> bool {
        let mut current = self.phase.load(Ordering::Acquire);
        loop {
            if current >> EPOCH_SHIFT != epoch.0 || current & PHASE_MASK != FINISHING as u64 {
                return false;
            }
            match self.phase.compare_exchange_weak(
                current,
                (current & !PHASE_MASK) | DONE as u64,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(changed) => current = changed,
            }
        }
    }

    pub fn is_done(&self) -> bool {
        self.phase.load(Ordering::Acquire) & PHASE_MASK == DONE as u64
    }

    /// # Safety
    /// 调用者须排除当前轮次的 offer/arm/restart，并确认上一轮完成者不再读取 outcome。
    /// 持旧 WaitEpoch 的 offer/abandon 可以并发，不能用隐式当前身份跨轮回调。
    /// 持久观察用来源锁隔离发布者，先归还完成槽再交付 ready 后才允许调用。
    pub unsafe fn restart_done(&self) -> bool {
        if !self.is_done() {
            return false;
        }
        let Some(epoch) = self.epoch().next() else {
            return false;
        };
        self.outcome_state
            .store(outcome_word(epoch, OUTCOME_EMPTY), Ordering::Relaxed);
        self.phase
            .store(phase_word(epoch, INSTALLING), Ordering::Release);
        true
    }

    /// 已取得完成权的一方读取唯一 outcome。
    pub fn outcome(&self) -> O {
        self.outcome_in(self.epoch())
    }

    /// 仅完成拥有者读取本轮 outcome；重置仍须等待所有旧读取结束。
    pub fn outcome_in(&self, epoch: WaitEpoch) -> O {
        assert!(self.has_outcome_in(epoch), "wait outcome is not ready");
        // SAFETY: READY 以 Release 发布初始化，has_outcome 的 Acquire 已同步。
        unsafe { (*self.outcome.get()).assume_init_read() }
    }

    fn claim_armed(&self, epoch: WaitEpoch) -> bool {
        let mut current = self.phase.load(Ordering::Acquire);
        loop {
            if current >> EPOCH_SHIFT != epoch.0 || current & PHASE_MASK != ARMED as u64 {
                return false;
            }
            match self.phase.compare_exchange_weak(
                current,
                (current & !PHASE_MASK) | FINISHING as u64,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(changed) => current = changed,
            }
        }
    }
}

impl<O: Copy> Default for WaitCore<O> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use std::{
        sync::{Arc, Barrier},
        thread,
    };

    use super::*;

    #[test]
    fn timeout_registration_closes_and_retires_idempotently() {
        let token = timer_queue::TimerToken::from_raw(0x1000);
        let registration = TimeoutRegistration::new();
        assert!(registration.publish(token));
        registration.close();
        assert_eq!(registration.take_cancellation(), Some(token));
        registration.close();
        assert_eq!(registration.take_cancellation(), None);
        assert!(!registration.retire(token));

        let registration = TimeoutRegistration::new();
        assert!(registration.publish(token));
        assert!(registration.retire(token));
        registration.close();
        assert_eq!(registration.take_cancellation(), None);
    }

    #[test]
    fn simultaneous_close_and_take_cancellation_have_one_owner() {
        for _ in 0..128 {
            let token = timer_queue::TimerToken::from_raw(0x1000);
            let registration = Arc::new(TimeoutRegistration::new());
            assert!(registration.publish(token));
            let barrier = Arc::new(Barrier::new(3));
            let workers: std::vec::Vec<_> = (0..3)
                .map(|_| {
                    let registration = registration.clone();
                    let barrier = barrier.clone();
                    thread::spawn(move || {
                        barrier.wait();
                        registration.close();
                        registration.take_cancellation()
                    })
                })
                .collect();
            let tokens: std::vec::Vec<_> = workers
                .into_iter()
                .filter_map(|worker| worker.join().unwrap())
                .collect();
            assert_eq!(tokens, [token]);
            assert!(registration.take_cancellation().is_none());
        }
    }

    #[test]
    fn close_and_expiry_transfer_exactly_one_token() {
        for _ in 0..128 {
            let token = timer_queue::TimerToken::from_raw(0x1000);
            let registration = Arc::new(TimeoutRegistration::new());
            assert!(registration.publish(token));
            let other = registration.clone();
            let expiry = thread::spawn(move || other.retire(token));
            registration.close();
            let cancelled = registration.take_cancellation();
            let expired = expiry.join().unwrap();
            assert_eq!(usize::from(expired) + usize::from(cancelled.is_some()), 1);
            if let Some(cancelled) = cancelled {
                assert_eq!(cancelled, token);
            }
        }
    }

    #[test]
    fn abandon_and_arm_allow_an_outcome_writer_to_finish_later() {
        let core = WaitCore::new();
        let epoch = core.epoch();
        assert!(
            core.outcome_state
                .compare_exchange(
                    outcome_word(epoch, OUTCOME_EMPTY),
                    outcome_word(epoch, OUTCOME_WRITING),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                )
                .is_ok()
        );
        assert!(core.abandon(epoch));
        assert_eq!(core.offer_in(epoch, 2), OfferResult::Lost);
        assert!(core.finish_installing_in(epoch).is_none());
        assert!(matches!(core.arm_in(epoch), ArmResult::Armed));
        // SAFETY: 本测试独占此前通过 CAS 取得的 outcome 写权。
        unsafe { (*core.outcome.get()).write(1) };
        core.outcome_state
            .store(outcome_word(epoch, OUTCOME_READY), Ordering::Release);
        assert!(core.claim_armed(epoch));
        assert!(core.is_abandoned(epoch));
        assert_eq!(core.outcome(), 1);
        assert!(core.mark_done_in(epoch));
    }

    #[test]
    fn quiescent_done_core_restarts_without_reusing_an_outcome() {
        let core = WaitCore::new();
        for value in 1..128 {
            // SAFETY: 单线程且上一轮已完成所有读取，模拟来源锁内的静止重置。
            if value != 1 {
                assert!(unsafe { core.restart_done() });
            }
            assert_eq!(core.offer(value), OfferResult::Deferred);
            assert!(matches!(core.arm(), ArmResult::Complete(found) if found == value));
            assert_eq!(core.outcome(), value);
            core.mark_done();
            assert_eq!(core.offer(value + 1), OfferResult::Lost);
        }
    }

    #[test]
    fn stale_epoch_cannot_offer_arm_cancel_or_complete_a_reused_core() {
        let core = WaitCore::new();
        let old = core.epoch();
        assert_eq!(core.offer_in(old, 7), OfferResult::Deferred);
        assert!(matches!(core.arm_in(old), ArmResult::Complete(7)));
        assert!(core.mark_done_in(old));
        // SAFETY: 上一轮所有读取已结束；仅在重置后重放旧身份。
        assert!(unsafe { core.restart_done() });
        let current = core.epoch();
        assert_ne!(old, current);
        assert_eq!(core.offer_in(old, 99), OfferResult::Lost);
        assert!(!core.abandon(old));
        assert!(!core.mark_done_in(old));
        assert!(matches!(core.arm_in(old), ArmResult::ExternalCompleter));
        assert!(!core.has_outcome());
        assert!(!core.is_abandoned(current));
        assert_eq!(core.offer_in(current, 8), OfferResult::Deferred);
        assert!(matches!(core.arm_in(current), ArmResult::Complete(8)));
        assert!(core.mark_done_in(current));
    }

    #[test]
    fn finishing_abandon_is_independent_of_the_chosen_outcome() {
        let core = WaitCore::new();
        let epoch = core.epoch();
        assert!(matches!(core.arm_in(epoch), ArmResult::Armed));
        assert_eq!(core.offer_in(epoch, 42), OfferResult::Complete);
        assert_eq!(core.offer_in(epoch, 99), OfferResult::Lost);
        assert!(core.abandon(epoch));
        assert!(!core.abandon(epoch));
        assert!(core.is_abandoned(epoch));
        assert_eq!(core.outcome(), 42);
        assert!(core.mark_done_in(epoch));
        assert!(!core.abandon(epoch));
        // SAFETY: 没有并发当前轮次事件或 outcome 读取。
        assert!(unsafe { core.restart_done() });
        assert!(!core.is_abandoned(core.epoch()));
    }

    #[test]
    fn installing_abandon_survives_arm_and_completion() {
        let core = WaitCore::new();
        let epoch = core.epoch();
        assert!(core.abandon(epoch));
        assert_eq!(core.offer_in(epoch, 1), OfferResult::Deferred);
        assert!(matches!(core.arm_in(epoch), ArmResult::Complete(1)));
        assert!(core.is_abandoned(epoch));
        assert!(core.mark_done_in(epoch));
    }

    #[test]
    fn epoch_exhaustion_retains_done_without_wraparound() {
        let epoch = WaitEpoch(u64::MAX >> EPOCH_SHIFT);
        let core: WaitCore<u64> = WaitCore {
            phase: AtomicU64::new(phase_word(epoch, DONE)),
            outcome_state: AtomicU64::new(outcome_word(epoch, OUTCOME_READY)),
            outcome: UnsafeCell::new(MaybeUninit::new(5)),
        };
        assert!(epoch.next().is_none());
        // SAFETY: 静止的终态，无并发事件。
        assert!(!unsafe { core.restart_done() });
        assert_eq!(core.epoch(), epoch);
        assert!(core.is_done());
        assert_eq!(core.outcome(), 5);
    }

    #[test]
    fn stale_callbacks_are_rejected_while_new_epochs_restart() {
        let core = Arc::new(WaitCore::new());
        let stale = core.epoch();
        assert_eq!(core.offer(0u64), OfferResult::Deferred);
        assert!(matches!(core.arm(), ArmResult::Complete(0)));
        core.mark_done();
        // SAFETY: 初始轮次已静止，下面的并发者仅持旧身份。
        assert!(unsafe { core.restart_done() });
        let other = core.clone();
        let callbacks = thread::spawn(move || {
            for _ in 0..10_000 {
                assert_eq!(other.offer_in(stale, u64::MAX), OfferResult::Lost);
                assert!(!other.abandon(stale));
                assert!(!other.mark_done_in(stale));
                assert!(matches!(other.arm_in(stale), ArmResult::ExternalCompleter));
            }
        });
        for value in 1..1_000 {
            let epoch = core.epoch();
            assert_eq!(core.offer_in(epoch, value), OfferResult::Deferred);
            assert!(matches!(core.arm_in(epoch), ArmResult::Complete(found) if found == value));
            assert!(!core.is_abandoned(epoch));
            assert!(core.mark_done_in(epoch));
            // SAFETY: 唯一当前轮次执行者已完成；并发回调只引用旧代次。
            assert!(unsafe { core.restart_done() });
        }
        callbacks.join().unwrap();
    }

    #[test]
    fn close_before_publish_rejects_and_requires_immediate_unregistration() {
        let token = timer_queue::TimerToken::from_raw(0x1000);
        let registration = TimeoutRegistration::new();
        registration.close();
        assert!(!registration.publish(token));
    }

    #[test]
    fn timeout_cancel_and_expiry_have_one_retirement_owner() {
        let token = timer_queue::TimerToken::from_raw(0x1000);
        for _ in 0..2_000 {
            let registration = Arc::new(TimeoutRegistration::new());
            assert!(registration.publish(token));
            let barrier = Arc::new(Barrier::new(3));
            let cancelled = {
                let registration = registration.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    registration.close();
                })
            };
            let expired = {
                let registration = registration.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    registration.retire(token)
                })
            };
            barrier.wait();
            cancelled.join().unwrap();
            let expired = expired.join().unwrap();
            assert_eq!(registration.take_cancellation().is_some(), !expired);
        }
    }

    #[test]
    fn timeout_queue_entries_disappear_for_object_abandon_and_timeout_completion() {
        let mut queue = timer_queue::TimerQueue::new(1);

        // 对象完成关闭登记；完成者在 context 释放前移除队列强引用。
        let object = queue.try_register(100, ()).unwrap();
        let registration = TimeoutRegistration::new();
        assert!(registration.publish(object));
        registration.close();
        assert_eq!(
            queue.cancel(registration.take_cancellation().unwrap()),
            Some(())
        );
        assert_eq!(queue.len(), 0);

        // 终止/Abandoned 走同一关闭与注销路径。
        let abandoned = queue.try_register(100, ()).unwrap();
        let registration = TimeoutRegistration::new();
        assert!(registration.publish(abandoned));
        registration.close();
        assert_eq!(
            queue.cancel(registration.take_cancellation().unwrap()),
            Some(())
        );
        assert_eq!(queue.len(), 0);

        // Timeout 在竞争 outcome 前已从队列移除条目。
        let timeout = queue.try_register(10, ()).unwrap();
        let registration = TimeoutRegistration::new();
        assert!(registration.publish(timeout));
        let (popped, ()) = queue.pop_expired(10).unwrap();
        assert_eq!(popped, timeout);
        assert!(registration.retire(popped));
        assert_eq!(queue.len(), 0);
    }

    #[test]
    fn installing_offer_is_deferred_to_installer() {
        let core = WaitCore::new();
        assert_eq!(core.offer(7), OfferResult::Deferred);
        assert_eq!(core.finish_installing(), Some(7));
        core.mark_done();
        assert!(core.is_done());
    }

    #[test]
    fn arm_without_outcome_leaves_external_completion_enabled() {
        let core = WaitCore::new();
        assert_eq!(core.arm(), ArmResult::Armed);
        assert_eq!(core.offer(9), OfferResult::Complete);
        assert_eq!(core.outcome(), 9);
        core.mark_done();
    }

    #[test]
    fn only_one_concurrent_offer_wins() {
        let core = Arc::new(WaitCore::new());
        assert_eq!(core.arm(), ArmResult::Armed);
        let barrier = Arc::new(Barrier::new(9));
        let mut joins = std::vec::Vec::new();
        for value in 0..8 {
            let core = core.clone();
            let barrier = barrier.clone();
            joins.push(thread::spawn(move || {
                barrier.wait();
                (value, core.offer(value))
            }));
        }
        barrier.wait();
        let results: std::vec::Vec<_> = joins.into_iter().map(|j| j.join().unwrap()).collect();
        assert_eq!(
            results
                .iter()
                .filter(|(_, r)| *r == OfferResult::Complete)
                .count(),
            1
        );
        assert!(
            results
                .iter()
                .any(|(v, r)| { *r == OfferResult::Complete && *v == core.outcome() })
        );
        core.mark_done();
    }

    #[test]
    fn offer_racing_arm_always_has_a_completion_owner() {
        for value in 0..2_000u32 {
            let core = Arc::new(WaitCore::new());
            let barrier = Arc::new(Barrier::new(2));
            let offered = {
                let core = core.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    core.offer(value)
                })
            };
            barrier.wait();
            let armed = core.arm();
            let offered = offered.join().unwrap();
            let external = offered == OfferResult::Complete;
            let installer = matches!(armed, ArmResult::Complete(v) if v == value);
            let deferred = offered == OfferResult::Deferred;
            assert!(external || installer || deferred);
            if deferred {
                // offer 观察 Installing 时，arm 必须取得该 outcome，或在极窄
                // 交错中已由 offer 的 Armed 分支取得完成权。
                assert!(installer || matches!(armed, ArmResult::ExternalCompleter));
            }
            assert_eq!(core.outcome(), value);
            core.mark_done();
        }
    }
}
