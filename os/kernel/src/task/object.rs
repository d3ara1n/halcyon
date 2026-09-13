//! 内核对象共同部分：身份、类型、Handle role 与对象锁内等待状态。

use alloc::sync::Arc;
use core::any::Any;

use erhino_shared::object::{ObjectSignals, Rights};

use super::{
    notify_work,
    proc::Process,
    wait::{Subscription, WaitOutcome},
};

/// 仅用于诊断和内核内部关联的对象身份；不是用户凭据。
pub type Koid = u64;

static NEXT_KOID: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

/// 内核对象身份的唯一铸造口。KernelObject 经 [`ObjectHeader`] 取得；需要类型化
/// 身份的对象核心（Pool 的 `PoolId`、MemoryObject 的 `ObjectId`）直接从这里取，
/// 因此全系统只有一个对象身份序列。
pub fn try_mint_koid() -> Option<Koid> {
    NEXT_KOID.allocate()
}

/// 单对象订阅额度；使协作式信号发布路径有明确工作上界。
pub const OBJECT_WAIT_LIMIT: usize = 1024;

pub use erhino_shared::object::{HandleRole, ObjectKind};

/// 只含稳定身份的共同头；对象状态和订阅必须与类型数据共用一把对象锁。
pub struct ObjectHeader {
    koid: Koid,
}

impl ObjectHeader {
    /// 用户可触达对象使用的 fallible identity 铸造口。
    pub fn try_new() -> Option<Self> {
        try_mint_koid().map(|koid| Self { koid })
    }

    pub fn new() -> Self {
        Self::try_new().expect("kernel object identity exhausted")
    }

    pub const fn koid(&self) -> Koid {
        self.koid
    }
}

const SIGNAL_BITS: &[ObjectSignals] = ObjectSignals::BITS;

#[derive(Clone, Copy)]
struct SignalEpoch {
    generation: u64,
    serial: u64,
    snapshot: ObjectSignals,
}

impl SignalEpoch {
    const EMPTY: Self = Self {
        generation: 0,
        serial: 0,
        snapshot: ObjectSignals::NONE,
    };
}

struct RegisteredSubscription {
    id: u64,
    subscription: Subscription,
    seen: [u64; SIGNAL_BITS.len()],
}

pub(crate) enum WaitAdvance {
    Progress(Option<Subscription>),
    Complete {
        sink: super::wait::ObserverSink,
        retired: Option<Subscription>,
    },
    Done,
}

impl WaitAdvance {
    /// 来源锁已释放后交接订阅引用与完成责任；返回 true 表示扫描结束。
    pub(crate) fn finish(self) -> bool {
        match self {
            Self::Progress(retired) => drop(retired),
            Self::Complete { sink, retired } => {
                drop(retired);
                super::wait::finish_offered(sink);
            }
            Self::Done => return true,
        }
        false
    }
}

/// 嵌入具体对象状态锁中的电平、发布代次与订阅槽。发布者只更新固定八个
/// signal epoch；逐订阅匹配、offer 与注销由通知债务按游标推进。
pub struct ObjectWaitState {
    signals: ObjectSignals,
    next_id: u64,
    serial: u64,
    epochs: [SignalEpoch; SIGNAL_BITS.len()],
    waiters: alloc::vec::Vec<Option<RegisteredSubscription>>,
    active_waiters: usize,
    scan_cursor: usize,
    scan_remaining: usize,
    scan_serial: u64,
    dirty: bool,
    scheduled: bool,
    notification: Option<notify_work::Reservation>,
}

fn select_snapshot(
    epochs: &[SignalEpoch; SIGNAL_BITS.len()],
    seen: &[u64; SIGNAL_BITS.len()],
    interest: ObjectSignals,
    current: ObjectSignals,
) -> Option<ObjectSignals> {
    let mut selected: Option<(u64, ObjectSignals)> = None;
    for (index, bit) in SIGNAL_BITS.iter().copied().enumerate() {
        let epoch = epochs[index];
        if epoch.generation == seen[index]
            || (!interest.intersects(bit) && bit != ObjectSignals::CLOSED)
        {
            continue;
        }
        if selected.is_none_or(|(serial, _)| epoch.serial < serial) {
            selected = Some((epoch.serial, epoch.snapshot));
        }
    }
    selected
        .map(|(_, snapshot)| snapshot)
        .or_else(|| current.intersects(ObjectSignals::CLOSED).then_some(current))
}

pub(crate) fn check_history_for_test() {
    let mut state = ObjectWaitState::new(ObjectSignals::NONE);
    let mut seen = [0; SIGNAL_BITS.len()];
    let interest = ObjectSignals::DATA | ObjectSignals::PEER_ATTACHED;
    state.update(ObjectSignals::NONE, ObjectSignals::PEER_ATTACHED);
    state.update(ObjectSignals::PEER_ATTACHED, ObjectSignals::DATA);
    assert_eq!(
        select_snapshot(&state.epochs, &seen, interest, state.signals),
        Some(ObjectSignals::PEER_ATTACHED),
        "signal bit order replaced the earlier historical serial"
    );
    let attached = SIGNAL_BITS
        .iter()
        .position(|bit| *bit == ObjectSignals::PEER_ATTACHED)
        .unwrap();
    seen[attached] = state.epochs[attached].generation;
    assert_eq!(
        select_snapshot(&state.epochs, &seen, interest, state.signals),
        Some(ObjectSignals::DATA),
        "seen history suppressed another interested signal"
    );
    state.update(ObjectSignals::DATA, ObjectSignals::CLOSED);
    assert_eq!(
        select_snapshot(&state.epochs, &seen, interest, state.signals),
        Some(ObjectSignals::DATA),
        "terminal publication replaced unseen data history"
    );
    seen = core::array::from_fn(|index| state.epochs[index].generation);
    assert_eq!(
        select_snapshot(&state.epochs, &seen, interest, state.signals),
        Some(ObjectSignals::CLOSED),
        "seen terminal serial lost its CLOSED fallback"
    );
}

impl ObjectWaitState {
    pub const fn new(initial: ObjectSignals) -> Self {
        Self {
            signals: initial,
            next_id: 1,
            serial: 0,
            epochs: [SignalEpoch::EMPTY; SIGNAL_BITS.len()],
            waiters: alloc::vec::Vec::new(),
            active_waiters: 0,
            scan_cursor: 0,
            scan_remaining: 0,
            scan_serial: 0,
            dirty: false,
            scheduled: false,
            notification: None,
        }
    }

    pub const fn signals(&self) -> ObjectSignals {
        self.signals
    }

    /// 电平更新。终态冻结；普通发布成本恒为已知 signal 位数，不随 waiter
    /// 数增长。inactive→active 的完整快照保存在对应 epoch，后续清位不修改它。
    pub fn update(&mut self, clear: ObjectSignals, set: ObjectSignals) -> ObjectSignals {
        if self.signals.contains(ObjectSignals::CLOSED) {
            return self.signals;
        }
        let previous = self.signals;
        self.signals &= !clear;
        self.signals |= set;
        let mut activated = self.signals & !previous;
        if activated == ObjectSignals::NONE {
            if self.scheduled && self.signals != previous {
                self.dirty = true;
            }
            return self.signals;
        }
        let Some(serial) = self.serial.checked_add(1) else {
            // 内部发布代次耗尽时永久关闭对象，不回绕解释旧订阅。
            self.signals = ObjectSignals::CLOSED;
            activated = ObjectSignals::CLOSED;
            self.serial = u64::MAX;
            self.record_activated(activated, u64::MAX);
            self.dirty = true;
            return self.signals;
        };
        self.serial = serial;
        self.record_activated(activated, serial);
        self.dirty = true;
        self.signals
    }

    fn record_activated(&mut self, activated: ObjectSignals, serial: u64) {
        for (index, bit) in SIGNAL_BITS.iter().copied().enumerate() {
            if !activated.intersects(bit) {
                continue;
            }
            let epoch = &mut self.epochs[index];
            let Some(generation) = epoch.generation.checked_add(1) else {
                self.signals = ObjectSignals::CLOSED;
                let closed = SIGNAL_BITS.len() - 1;
                self.epochs[closed].generation = self.epochs[closed].generation.saturating_add(1);
                self.epochs[closed].serial = serial;
                self.epochs[closed].snapshot = ObjectSignals::CLOSED;
                return;
            };
            *epoch = SignalEpoch {
                generation,
                serial,
                snapshot: self.signals,
            };
        }
    }

    pub fn subscribe(&mut self, subscription: Subscription) -> SubscribeResult {
        let immediate = subscription.outcome(self.signals);
        let persistent = subscription.sink.persistent();
        if (!persistent || self.signals.intersects(ObjectSignals::CLOSED))
            && let Some(outcome) = immediate
        {
            return SubscribeResult::Ready {
                outcome,
                retired: subscription,
            };
        }
        let sink = subscription.sink.clone();
        if self.active_waiters >= OBJECT_WAIT_LIMIT || self.next_id == 0 {
            return SubscribeResult::ReachLimit(subscription);
        }
        if self.active_waiters == 0 && !self.scheduled && self.notification.is_none() {
            self.notification = match notify_work::reserve() {
                Ok(reservation) => Some(reservation),
                Err(()) => return SubscribeResult::OutOfMemory(subscription),
            };
        }
        let slot = match self.waiters.iter().position(Option::is_none) {
            Some(slot) => slot,
            None => {
                if self.waiters.try_reserve(1).is_err() {
                    if self.active_waiters == 0 {
                        self.notification.take();
                    }
                    return SubscribeResult::OutOfMemory(subscription);
                }
                self.waiters.push(None);
                self.waiters.len() - 1
            }
        };
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.waiters[slot] = Some(RegisteredSubscription {
            id,
            subscription,
            seen: core::array::from_fn(|index| self.epochs[index].generation),
        });
        self.active_waiters += 1;
        if self.scheduled {
            self.dirty = true;
        }
        if persistent && let Some(outcome) = immediate {
            assert!(
                sink.offer(outcome) != wait_context::OfferResult::Complete,
                "persistent observer armed before source installation"
            );
        }
        SubscribeResult::Registered(id)
    }

    pub fn rearm_observer(
        &mut self,
        id: u64,
    ) -> Result<ObserverRearm, erhino_shared::call::SystemCallError> {
        let current = self.signals;
        let seen = core::array::from_fn(|index| self.epochs[index].generation);
        let registered = self
            .waiters
            .iter_mut()
            .flatten()
            .find(|waiter| waiter.id == id)
            .ok_or(erhino_shared::call::SystemCallError::ObjectNotFound)?;
        let generation = registered.subscription.sink.restart()?;
        registered.seen = seen;
        if let Some(outcome) = registered.subscription.outcome(current) {
            registered.subscription.sink.offer(outcome);
        }
        let completion = registered.subscription.sink.arm();
        Ok(ObserverRearm {
            generation,
            completion,
        })
    }

    pub fn cancel_observer(&mut self, id: u64) -> Option<CancelledObservation> {
        let slot = self
            .waiters
            .iter_mut()
            .find(|slot| slot.as_ref().is_some_and(|waiter| waiter.id == id))?;
        let waiter = slot
            .take()
            .expect("observer slot disappeared under source lock");
        self.active_waiters -= 1;
        if self.active_waiters == 0 && !self.scheduled {
            self.notification.take();
        }
        let sink = waiter.subscription.sink.clone();
        let result = sink.offer(WaitOutcome::Cancelled);
        Some(CancelledObservation {
            sink,
            result,
            retired: waiter.subscription,
        })
    }

    pub(crate) fn unsubscribe(&mut self, id: u64) -> Option<Subscription> {
        if let Some(slot) = self
            .waiters
            .iter_mut()
            .find(|slot| slot.as_ref().is_some_and(|waiter| waiter.id == id))
        {
            let retired = slot.take().expect("subscription slot disappeared");
            self.active_waiters -= 1;
            if self.active_waiters == 0 && !self.scheduled {
                self.notification.take();
            }
            return Some(retired.subscription);
        }
        None
    }

    pub(crate) fn take_notification(&mut self) -> Option<(notify_work::Reservation, ObjectRef)> {
        if self.scheduled || !self.dirty || self.active_waiters == 0 {
            return None;
        }
        let target = self
            .waiters
            .iter()
            .flatten()
            .next()
            .expect("active waiter count lost its subscription")
            .subscription
            .object
            .clone();
        let reservation = self
            .notification
            .take()
            .expect("active waiter set lost its notification slot");
        self.scheduled = true;
        self.scan_serial = self.serial;
        self.scan_remaining = self.waiters.len();
        self.dirty = false;
        Some((reservation, target))
    }

    pub(crate) fn advance_waiter(&mut self) -> WaitAdvance {
        if !self.scheduled {
            return WaitAdvance::Done;
        }
        if self.active_waiters == 0 {
            return WaitAdvance::Done;
        }
        if self.scan_remaining == 0 {
            if self.dirty || self.scan_serial != self.serial {
                self.scan_serial = self.serial;
                self.scan_remaining = self.waiters.len();
                self.dirty = false;
            } else {
                return WaitAdvance::Done;
            }
        }
        if self.waiters.is_empty() {
            return WaitAdvance::Done;
        }
        let index = self.scan_cursor % self.waiters.len();
        self.scan_cursor = (index + 1) % self.waiters.len();
        self.scan_remaining -= 1;
        let Some(waiter) = self.waiters[index].as_mut() else {
            return WaitAdvance::Progress(None);
        };

        let selected = select_snapshot(
            &self.epochs,
            &waiter.seen,
            waiter.subscription.interest(),
            self.signals,
        );
        waiter.seen = core::array::from_fn(|signal_index| self.epochs[signal_index].generation);
        let Some(snapshot) = selected else {
            return WaitAdvance::Progress(None);
        };
        let outcome = waiter
            .subscription
            .outcome(snapshot)
            .expect("selected signal epoch must match its subscription");
        match waiter.subscription.sink.offer(outcome) {
            wait_context::OfferResult::Deferred => {
                let retired = if self.signals.intersects(ObjectSignals::CLOSED) {
                    self.active_waiters -= 1;
                    Some(
                        self.waiters[index]
                            .take()
                            .expect("closed subscription disappeared")
                            .subscription,
                    )
                } else {
                    None
                };
                WaitAdvance::Progress(retired)
            }
            wait_context::OfferResult::Lost => {
                let retired = if !waiter.subscription.sink.persistent()
                    || self.signals.intersects(ObjectSignals::CLOSED)
                {
                    let retired = self.waiters[index]
                        .take()
                        .expect("lost subscription disappeared");
                    self.active_waiters -= 1;
                    Some(retired.subscription)
                } else {
                    None
                };
                WaitAdvance::Progress(retired)
            }
            wait_context::OfferResult::Complete => {
                let (sink, retired) = if waiter.subscription.sink.persistent()
                    && !self.signals.intersects(ObjectSignals::CLOSED)
                {
                    (waiter.subscription.sink.clone(), None)
                } else {
                    let retired = self.waiters[index]
                        .take()
                        .expect("completed waiter disappeared");
                    self.active_waiters -= 1;
                    (
                        retired.subscription.sink.clone(),
                        Some(retired.subscription),
                    )
                };
                WaitAdvance::Complete { sink, retired }
            }
        }
    }

    pub(crate) fn complete_notification(
        &mut self,
        reservation: notify_work::Reservation,
    ) -> notify_work::Completion {
        assert!(
            self.scheduled,
            "notification debt completed without a scheduled task"
        );
        self.scan_remaining = 0;
        if self.active_waiters == 0 {
            self.scheduled = false;
            notify_work::Completion::Release(reservation)
        } else if self.dirty {
            self.scan_serial = self.serial;
            self.scan_remaining = self.waiters.len();
            self.dirty = false;
            notify_work::Completion::Reschedule(reservation)
        } else {
            self.scheduled = false;
            assert!(self.notification.replace(reservation).is_none());
            notify_work::Completion::Held
        }
    }

    /// 终态来源不再保留观察授权；逐槽摘除，与通知扫描共享来源锁。
    pub(crate) fn retire_closed_step(&mut self) -> WaitAdvance {
        assert!(
            self.signals().intersects(ObjectSignals::CLOSED),
            "open source retired its subscriptions"
        );
        let Some(slot) = self.waiters.pop() else {
            return WaitAdvance::Done;
        };
        let Some(waiter) = slot else {
            return WaitAdvance::Progress(None);
        };
        self.active_waiters -= 1;
        if self.active_waiters == 0 && !self.scheduled {
            self.notification.take();
        }
        let snapshot = select_snapshot(
            &self.epochs,
            &waiter.seen,
            waiter.subscription.interest(),
            self.signals,
        )
        .expect("closed source failed to select a terminal snapshot");
        let outcome = waiter
            .subscription
            .outcome(snapshot)
            .expect("closed source failed to produce a terminal observation");
        let sink = waiter.subscription.sink.clone();
        if sink.offer(outcome) == wait_context::OfferResult::Complete {
            WaitAdvance::Complete {
                sink,
                retired: Some(waiter.subscription),
            }
        } else {
            WaitAdvance::Progress(Some(waiter.subscription))
        }
    }

    pub(crate) fn subscriptions_retired(&self) -> bool {
        self.waiters.is_empty()
    }

    pub(crate) fn active_waiters_for_test(&self) -> usize {
        self.active_waiters
    }
}

pub struct ObserverRearm {
    pub(crate) generation: u64,
    pub(crate) completion: Option<super::wait::ObserverSink>,
}

pub struct CancelledObservation {
    pub(crate) sink: super::wait::ObserverSink,
    pub(crate) result: wait_context::OfferResult,
    pub(crate) retired: Subscription,
}

pub enum SubscribeResult {
    Ready {
        outcome: WaitOutcome,
        retired: Subscription,
    },
    Registered(u64),
    ReachLimit(Subscription),
    OutOfMemory(Subscription),
}

/// 所有可经 Handle 引用的内核对象。
pub trait KernelObject: Any + Send + Sync {
    fn retirement(&self) -> Option<&dyn super::retirement::RetirementTarget> {
        None
    }
    fn header(&self) -> &ObjectHeader;
    fn kind(&self) -> ObjectKind;

    fn related_id(&self) -> u64 {
        0
    }
    fn badge(&self) -> u64 {
        0
    }

    /// 本次授权的实际电平来源；使用引用由等待上下文另外保留。
    fn observation_source(&self) -> Option<ObjectRef> {
        None
    }

    /// 此对象是否接受 role；接受时返回该 role 的最大 rights。
    fn allowed_rights(&self, role: HandleRole) -> Option<Rights>;

    /// role 能观察的合法电平位；None 表示对象不是可等待对象。
    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals>;

    /// 非等待对象不维护电平或订阅；WaitMany 在调用此前已由 rights 与
    /// allowed_signals 拒绝它们。
    fn signals(&self) -> ObjectSignals {
        ObjectSignals::NONE
    }

    fn subscribe(&self, _subscription: Subscription) -> SubscribeResult {
        unreachable!("non-waitable object accepted a subscription")
    }

    fn unsubscribe(&self, _id: u64) {}

    fn rearm_observer(
        &self,
        _id: u64,
    ) -> Result<ObserverRearm, erhino_shared::call::SystemCallError> {
        Err(erhino_shared::call::SystemCallError::NotSupported)
    }

    fn cancel_observer(&self, _id: u64) -> Option<CancelledObservation> {
        None
    }

    /// Handle 从表中移除且表锁已释放后的 lifecycle 回调。
    fn close_handle(&self, role: HandleRole, owner: &Process, exiting: bool);

    /// 消息中的 transit Handle 被丢弃；只有持 TRANSIT 的 entry 可进入。
    fn close_transit(&self, role: HandleRole);

    /// 从已发布的对象候选中推进至多 `budget` 个 waiter；返回
    /// `(实际步骤, 是否已完成本次通知债务)`。
    fn drain_waiters(&self, budget: usize) -> (usize, bool) {
        let _ = budget;
        (0, true)
    }

    fn complete_waiter_drain(
        &self,
        reservation: notify_work::Reservation,
    ) -> notify_work::Completion {
        notify_work::Completion::Release(reservation)
    }

    fn as_any(&self) -> &dyn Any;
}

pub type ObjectRef = Arc<dyn KernelObject>;
