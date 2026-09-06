//! 内核对象共同部分：身份、类型、Handle role 与对象锁内等待状态。

use alloc::sync::Arc;
use core::{
    any::Any,
    sync::atomic::{AtomicU64, Ordering},
};

use erhino_shared::object::{ObjectSignals, Rights};

use super::{
    notify_work,
    proc::Process,
    wait::{Subscription, WaitContext, WaitOutcome},
};

/// 仅用于诊断和内核内部关联的对象身份；不是用户凭据。
pub type Koid = u64;

static NEXT_KOID: AtomicU64 = AtomicU64::new(1);

/// 内核对象身份的唯一铸造口。KernelObject 经 [`ObjectHeader`] 取得；需要类型化
/// 身份的对象核心（Pool 的 `PoolId`、MemoryObject 的 `ObjectId`）直接从这里取，
/// 因此全系统只有一个对象身份序列。
pub fn try_mint_koid() -> Option<Koid> {
    NEXT_KOID
        .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            (current != 0).then(|| current.wrapping_add(1))
        })
        .ok()
}

/// 单对象订阅额度；使协作式信号发布路径有明确工作上界。
pub const OBJECT_WAIT_LIMIT: usize = 1024;

/// 用户 Handle 所指对象的内核类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Job,
    MemoryPool,
    MemoryObject,
    ProcessBuilder,
    ProcessControl,
    ThreadControl,
    Mailbox,
    Notification,
    TunnelEndpoint,
    TunnelInvitation,
    SystemReset,
}

/// Handle 在对象生命周期中的角色。rights 决定操作，role 决定关系。
///
/// 收束公理（close fanout 上界的结构来源）：owner 不可 TRANSIT，
/// 因此消息内不含容器角色，唯一可 TRANSIT 的角色 close 恒为 O(1)
/// 叶子操作（不同步排空另一对象容器）。新增 role 时必须维持该
/// 推导：可 TRANSIT ⟹ close 是叶子；需要级联收束的容器角色只能作
/// owner 直接 GRANT，或改走 REAPABLE + 有界 drain（见 ideas/object.md
/// 「收束分层」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleRole {
    JobControl,
    MemoryPool,
    /// MemoryObject 没有 owner role：全部 Handle 是同一 capability，只以 rights 分权。
    MemoryObject,
    ProcessBuilder,
    ProcessControl,
    ThreadControl,
    MailboxOwner,
    MailboxSender,
    /// 一次性投递权：成功 Send 后由内核摘除，失败不消费。
    MailboxSenderOnce,
    NotificationOwner,
    NotificationSignaler,
    TunnelEndpoint,
    TunnelInvitation,
    SystemResetControl,
}

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

const SIGNAL_BITS: [ObjectSignals; 8] = [
    ObjectSignals::READABLE,
    ObjectSignals::WRITABLE,
    ObjectSignals::DATA,
    ObjectSignals::REAPABLE,
    ObjectSignals::DONE,
    ObjectSignals::EXECUTABLE,
    ObjectSignals::PEER_CLOSED,
    ObjectSignals::CLOSED,
];

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
    Progress,
    Complete(Arc<WaitContext>),
    Done,
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
        if let Some(outcome) = subscription.outcome(self.signals) {
            return SubscribeResult::Ready(outcome);
        }
        if self.active_waiters >= OBJECT_WAIT_LIMIT || self.next_id == 0 {
            return SubscribeResult::ReachLimit;
        }
        if self.active_waiters == 0 && !self.scheduled && self.notification.is_none() {
            self.notification = match notify_work::reserve() {
                Ok(reservation) => Some(reservation),
                Err(()) => return SubscribeResult::OutOfMemory,
            };
        }
        let slot = match self.waiters.iter().position(Option::is_none) {
            Some(slot) => slot,
            None => {
                if self.waiters.try_reserve(1).is_err() {
                    if self.active_waiters == 0 {
                        self.notification.take();
                    }
                    return SubscribeResult::OutOfMemory;
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
        SubscribeResult::Registered(id)
    }

    pub fn unsubscribe(&mut self, id: u64) {
        if let Some(slot) = self
            .waiters
            .iter_mut()
            .find(|slot| slot.as_ref().is_some_and(|waiter| waiter.id == id))
        {
            slot.take();
            self.active_waiters -= 1;
            if self.active_waiters == 0 && !self.scheduled {
                self.notification.take();
            }
        }
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
            return WaitAdvance::Progress;
        };

        let interest = waiter.subscription.interest();
        let mut selected: Option<(u64, ObjectSignals)> = None;
        for (signal_index, bit) in SIGNAL_BITS.iter().copied().enumerate() {
            let epoch = self.epochs[signal_index];
            if epoch.generation == waiter.seen[signal_index]
                || (!interest.intersects(bit) && bit != ObjectSignals::CLOSED)
            {
                continue;
            }
            if selected.is_none_or(|(serial, _)| epoch.serial < serial) {
                selected = Some((epoch.serial, epoch.snapshot));
            }
        }
        waiter.seen = core::array::from_fn(|signal_index| self.epochs[signal_index].generation);
        let Some((_, snapshot)) = selected else {
            return WaitAdvance::Progress;
        };
        let outcome = waiter
            .subscription
            .outcome(snapshot)
            .expect("selected signal epoch must match its subscription");
        match waiter.subscription.context.offer(outcome) {
            wait_context::OfferResult::Deferred => WaitAdvance::Progress,
            wait_context::OfferResult::Lost => {
                self.waiters[index].take();
                self.active_waiters -= 1;
                WaitAdvance::Progress
            }
            wait_context::OfferResult::Complete => {
                let context = self.waiters[index]
                    .take()
                    .expect("completed waiter disappeared")
                    .subscription
                    .context;
                self.active_waiters -= 1;
                WaitAdvance::Complete(context)
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
}

pub enum SubscribeResult {
    Ready(WaitOutcome),
    Registered(u64),
    ReachLimit,
    OutOfMemory,
}

/// 所有可经 Handle 引用的内核对象。
pub trait KernelObject: Any + Send + Sync {
    #[expect(dead_code, reason = "对象诊断接口使用")]
    fn header(&self) -> &ObjectHeader;
    fn kind(&self) -> ObjectKind;

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
