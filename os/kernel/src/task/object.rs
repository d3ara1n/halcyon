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

struct RegisteredSubscription {
    id: u64,
    subscription: Subscription,
    /// 发布者在同一对象锁内冻结的命中结果；后续清位不得抹掉已登记候选。
    pending: Option<WaitOutcome>,
    /// 该订阅未来一次命中的 work-debt 槽；发布时移交给对象 owner。
    notification: Option<notify_work::Reservation>,
}

/// 嵌入具体对象状态锁中的电平与订阅队列。所有方法都由对象锁保护。
pub struct ObjectWaitState {
    signals: ObjectSignals,
    next_id: u64,
    /// 游标只用于从上次 Deferred 候选继续；插入顺序仍是 item_index 的稳定次序。
    notify_cursor: usize,
    /// 已有一个 work-debt 任务在途；防止同一对象重复发布多个排水任务。
    scheduled: bool,
    waiters: alloc::vec::Vec<RegisteredSubscription>,
}

impl ObjectWaitState {
    pub const fn new(initial: ObjectSignals) -> Self {
        Self {
            signals: initial,
            next_id: 1,
            notify_cursor: 0,
            scheduled: false,
            waiters: alloc::vec::Vec::new(),
        }
    }

    pub const fn signals(&self) -> ObjectSignals {
        self.signals
    }

    /// 电平更新。终态冻结：CLOSED 置位后任何更新不再生效——
    /// 「单向迁移、终态不可复活」由所有对象共用的这一结构保证，
    /// 跨关闭窗口的事务收尾无需逐点防御。
    pub fn update(&mut self, clear: ObjectSignals, set: ObjectSignals) -> ObjectSignals {
        if self.signals.contains(ObjectSignals::CLOSED) {
            return self.signals;
        }
        self.signals &= !clear;
        self.signals |= set;
        let current = self.signals;
        // 命中候选与电平更新在同一对象锁内冻结。publish 后即使消费者
        // 清除 DATA/READABLE，候选仍由 pending 持有，不会被下一次重读抹掉。
        for waiter in &mut self.waiters {
            if waiter.pending.is_none() && Self::matches(current, waiter.subscription.interest) {
                waiter.pending = Some(waiter.subscription.outcome(current));
            }
        }
        current
    }

    pub fn subscribe(&mut self, subscription: Subscription) -> SubscribeResult {
        if Self::matches(self.signals, subscription.interest) {
            return SubscribeResult::Ready(subscription.outcome(self.signals));
        }
        if self.waiters.len() >= OBJECT_WAIT_LIMIT || self.next_id == 0 {
            return SubscribeResult::ReachLimit;
        }
        if self.waiters.try_reserve(1).is_err() {
            return SubscribeResult::OutOfMemory;
        }
        let notification = match notify_work::reserve() {
            Ok(reservation) => reservation,
            Err(()) => return SubscribeResult::OutOfMemory,
        };
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.waiters.push(RegisteredSubscription {
            id,
            subscription,
            pending: None,
            notification: Some(notification),
        });
        SubscribeResult::Registered(id)
    }

    pub fn unsubscribe(&mut self, id: u64) {
        if let Some(index) = self.waiters.iter().position(|waiter| waiter.id == id) {
            self.waiters.remove(index);
            if self.notify_cursor > index {
                self.notify_cursor -= 1;
            } else if self.notify_cursor >= self.waiters.len() {
                self.notify_cursor = 0;
            }
        }
    }

    /// 取出一个已在 update 同锁段冻结的命中候选。Deferred 保留候选，
    /// 直到 Installing owner 完成 arm；不重新读取当前 signals。
    pub(crate) fn take_notification(&mut self) -> Option<(notify_work::Reservation, ObjectRef)> {
        if self.scheduled {
            return None;
        }
        if self.notify_cursor >= self.waiters.len() {
            self.notify_cursor = 0;
        }
        let count = self.waiters.len();
        for _ in 0..count {
            if self.notify_cursor >= self.waiters.len() {
                self.notify_cursor = 0;
            }
            let index = self.notify_cursor;
            self.notify_cursor = (index + 1) % self.waiters.len().max(1);
            if self.waiters[index].pending.is_some()
                && let Some(reservation) = self.waiters[index].notification.take()
            {
                self.scheduled = true;
                return Some((reservation, self.waiters[index].subscription.object.clone()));
            }
        }
        None
    }

    pub fn has_pending(&self) -> bool {
        self.waiters.iter().any(|waiter| waiter.pending.is_some())
    }

    pub(crate) fn complete_notification(&mut self) {
        assert!(
            self.scheduled,
            "notification debt completed without a scheduled task"
        );
        self.scheduled = false;
        // Deferred offer 在本轮债务后仍保留注册；回收刚释放的槽，
        // 使后续信号仍能再次调度该订阅。
        if let Some(waiter) = self
            .waiters
            .iter_mut()
            .find(|waiter| waiter.pending.is_some() && waiter.notification.is_none())
        {
            waiter.notification = Some(
                notify_work::reserve()
                    .expect("completed notification debt must rearm its reservation"),
            );
        }
    }

    pub fn take_completer(&mut self) -> Option<Arc<WaitContext>> {
        if self.waiters.is_empty() {
            self.notify_cursor = 0;
            return None;
        }
        let count = self.waiters.len();
        for _ in 0..count {
            if self.notify_cursor >= self.waiters.len() {
                self.notify_cursor = 0;
            }
            let index = self.notify_cursor;
            self.notify_cursor = (index + 1) % self.waiters.len();
            let Some(outcome) = self.waiters[index].pending else {
                continue;
            };
            // 同一 WaitMany 可重复观察一个对象；其输入顺序决定最小
            // item_index 获胜。跨 Context 仍由游标提供公平轮转。
            if self.waiters.iter().any(|candidate| {
                candidate.pending.is_some()
                    && candidate.subscription.item_index
                        < self.waiters[index].subscription.item_index
                    && Arc::ptr_eq(
                        &candidate.subscription.context,
                        &self.waiters[index].subscription.context,
                    )
            }) {
                continue;
            }
            match self.waiters[index].subscription.context.offer(outcome) {
                wait_context::OfferResult::Deferred => {}
                wait_context::OfferResult::Lost => {
                    self.waiters.remove(index);
                    if self.notify_cursor > index {
                        self.notify_cursor -= 1;
                    }
                    if self.notify_cursor >= self.waiters.len() {
                        self.notify_cursor = 0;
                    }
                }
                wait_context::OfferResult::Complete => {
                    let context = self.waiters.remove(index).subscription.context;
                    if self.notify_cursor >= self.waiters.len() {
                        self.notify_cursor = 0;
                    }
                    return Some(context);
                }
            }
        }
        None
    }

    fn matches(current: ObjectSignals, interest: ObjectSignals) -> bool {
        current.intersects(interest) || current.intersects(ObjectSignals::CLOSED)
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

    fn complete_waiter_drain(&self) {}

    fn as_any(&self) -> &dyn Any;
}

pub type ObjectRef = Arc<dyn KernelObject>;
