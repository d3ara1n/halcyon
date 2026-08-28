#![no_std]

//! 等待安装与完成权交接的无锁核心。
//!
//! 对象订阅、线程所有权和结果交付留给内核包装层；本 crate 只保证：
//! Installing 期间事件不能取得完成权，arm 后恰有一方完成，outcome 只写一次。

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicU8, Ordering},
};

const INSTALLING: u8 = 0;
const ARMED: u8 = 1;
const FINISHING: u8 = 2;
const DONE: u8 = 3;

const OUTCOME_EMPTY: u8 = 0;
const OUTCOME_WRITING: u8 = 1;
const OUTCOME_READY: u8 = 2;

const TIMEOUT_UNREGISTERED: u64 = 0;
const TIMEOUT_CLOSED: u64 = u64::MAX;

/// WaitContext 的 timeout 注册状态：未登记、稳定 token 或 Closed。
///
/// 队列先产生 token，再尝试发布；完成方统一关闭并取走 token 注销。
/// 到期方只能退休仍由自身 token 表示的注册，因而取消/到期竞争幂等。
pub struct TimeoutRegistration {
    state: core::sync::atomic::AtomicU64,
    /// 完成方在对象锁内关闭时暂存的注销 token；真正触碰 owner queue
    /// 留给锁外 finish 路径。
    pending_cancellation: core::sync::atomic::AtomicU64,
}

impl TimeoutRegistration {
    pub const fn new() -> Self {
        Self {
            state: core::sync::atomic::AtomicU64::new(TIMEOUT_UNREGISTERED),
            pending_cancellation: core::sync::atomic::AtomicU64::new(TIMEOUT_UNREGISTERED),
        }
    }

    /// 发布新登记。false 表示 context 已关闭，调用方必须立即注销 token。
    pub fn publish(&self, token: timer_queue::TimerToken) -> bool {
        self.state
            .compare_exchange(
                TIMEOUT_UNREGISTERED,
                token.raw(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// 关闭登记。若有 live token，将其暂存为待注销项；此操作只碰原子
    /// 状态，允许在对象订阅锁内执行。
    pub fn close(&self) {
        let previous = self.state.swap(TIMEOUT_CLOSED, Ordering::AcqRel);
        if previous != TIMEOUT_UNREGISTERED && previous != TIMEOUT_CLOSED {
            self.pending_cancellation.store(previous, Ordering::Release);
        }
    }

    /// 锁外完成路径取走待注销 token。多次调用幂等。
    pub fn take_cancellation(&self) -> Option<timer_queue::TimerToken> {
        let token = self
            .pending_cancellation
            .swap(TIMEOUT_UNREGISTERED, Ordering::AcqRel);
        (token != TIMEOUT_UNREGISTERED).then(|| timer_queue::TimerToken::from_raw(token))
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
    phase: AtomicU8,
    outcome_state: AtomicU8,
    outcome: UnsafeCell<MaybeUninit<O>>,
}

// SAFETY: outcome 只有赢得 EMPTY→WRITING 的线程写；READY 的 Release/Acquire
// 发布读值。O: Copy + Send 不含需要并发 Drop 的所有权。
unsafe impl<O: Copy + Send> Sync for WaitCore<O> {}
unsafe impl<O: Copy + Send> Send for WaitCore<O> {}

impl<O: Copy> WaitCore<O> {
    pub const fn new() -> Self {
        Self {
            phase: AtomicU8::new(INSTALLING),
            outcome_state: AtomicU8::new(OUTCOME_EMPTY),
            outcome: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    /// 提交唯一 outcome，并在 Armed 时竞争完成权。
    pub fn offer(&self, outcome: O) -> OfferResult {
        if self
            .outcome_state
            .compare_exchange(
                OUTCOME_EMPTY,
                OUTCOME_WRITING,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            return OfferResult::Lost;
        }
        // SAFETY: 本线程独占 OUTCOME_WRITING，且 outcome 槽只写一次。
        unsafe { (*self.outcome.get()).write(outcome) };
        self.outcome_state.store(OUTCOME_READY, Ordering::Release);

        match self.phase.load(Ordering::Acquire) {
            INSTALLING => OfferResult::Deferred,
            ARMED => {
                if self.claim_armed() {
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
        self.outcome_state.load(Ordering::Acquire) == OUTCOME_READY
    }

    /// Installing 安装者发现 outcome 后独占完成权。
    pub fn finish_installing(&self) -> Option<O> {
        if !self.has_outcome() {
            return None;
        }
        if self
            .phase
            .compare_exchange(INSTALLING, FINISHING, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return None;
        }
        Some(self.outcome())
    }

    /// 安装完全部订阅后 arm；闭合 offer 与 arm 的交错。
    pub fn arm(&self) -> ArmResult<O> {
        if self
            .phase
            .compare_exchange(INSTALLING, ARMED, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return ArmResult::ExternalCompleter;
        }

        if self.has_outcome() {
            if self.claim_armed() {
                ArmResult::Complete(self.outcome())
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
        self.phase
            .compare_exchange(FINISHING, DONE, Ordering::Release, Ordering::Relaxed)
            .expect("only the finishing owner may mark a wait done");
    }

    pub fn is_done(&self) -> bool {
        self.phase.load(Ordering::Acquire) == DONE
    }

    /// 已取得完成权的一方读取唯一 outcome。
    pub fn outcome(&self) -> O {
        assert!(self.has_outcome(), "wait outcome is not ready");
        // SAFETY: READY 以 Release 发布初始化，has_outcome 的 Acquire 已同步。
        unsafe { (*self.outcome.get()).assume_init_read() }
    }

    fn claim_armed(&self) -> bool {
        self.phase
            .compare_exchange(ARMED, FINISHING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
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
