//! 内核对象共同部分：身份、类型、Handle role 与对象锁内等待状态。

use alloc::sync::Arc;
use core::{
    any::Any,
    sync::atomic::{AtomicU64, Ordering},
};

use erhino_shared::object::{ObjectSignals, Rights};

use super::{
    proc::Process,
    wait::{Subscription, WaitContext, WaitOutcome},
};

/// 仅用于诊断和内核内部关联的对象身份；不是用户凭据。
pub type Koid = u64;

static NEXT_KOID: AtomicU64 = AtomicU64::new(1);

/// 单对象订阅额度；使协作式信号发布路径有明确工作上界。
pub const OBJECT_WAIT_LIMIT: usize = 1024;

/// 用户 Handle 所指对象的内核类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectKind {
    Job,
    ProcessBuilder,
    ProcessControl,
    Mailbox,
    Notification,
    TunnelEndpoint,
    TunnelInvitation,
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
    ProcessBuilder,
    ProcessControl,
    MailboxOwner,
    MailboxSender,
    /// 一次性投递权：成功 Send 后由内核摘除，失败不消费。
    MailboxSenderOnce,
    NotificationOwner,
    NotificationSignaler,
    TunnelEndpoint,
    TunnelInvitation,
}

/// 只含稳定身份的共同头；对象状态和订阅必须与类型数据共用一把对象锁。
pub struct ObjectHeader {
    #[expect(dead_code, reason = "对象诊断接口落地前只建立稳定身份框架")]
    koid: Koid,
}

impl ObjectHeader {
    pub fn new() -> Self {
        let koid = NEXT_KOID.fetch_add(1, Ordering::Relaxed);
        assert!(koid != 0, "kernel object identity exhausted");
        Self { koid }
    }

    #[expect(dead_code, reason = "对象诊断接口使用")]
    pub const fn koid(&self) -> Koid {
        self.koid
    }
}

struct RegisteredSubscription {
    id: u64,
    subscription: Subscription,
}

/// 嵌入具体对象状态锁中的电平与订阅队列。所有方法都由对象锁保护。
pub struct ObjectWaitState {
    signals: ObjectSignals,
    next_id: u64,
    waiters: alloc::vec::Vec<RegisteredSubscription>,
}

impl ObjectWaitState {
    pub const fn new(initial: ObjectSignals) -> Self {
        Self { signals: initial, next_id: 1, waiters: alloc::vec::Vec::new() }
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
        self.signals
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
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.waiters.push(RegisteredSubscription { id, subscription });
        SubscribeResult::Registered(id)
    }

    pub fn unsubscribe(&mut self, id: u64) {
        if let Some(index) = self.waiters.iter().position(|waiter| waiter.id == id) {
            self.waiters.remove(index);
        }
    }

    /// 在当前电平上寻找取得完成权的 Context。调用者移除源订阅后释放
    /// 对象锁，再执行跨对象清理；Deferred 保留给 Installing 安装者。
    pub fn take_completer(&mut self) -> Option<Arc<WaitContext>> {
        let mut index = 0;
        while index < self.waiters.len() {
            if !Self::matches(self.signals, self.waiters[index].subscription.interest) {
                index += 1;
                continue;
            }
            let outcome = self.waiters[index].subscription.outcome(self.signals);
            match self.waiters[index].subscription.context.offer(outcome) {
                wait_context::OfferResult::Deferred => index += 1,
                wait_context::OfferResult::Lost => {
                    self.waiters.remove(index);
                }
                wait_context::OfferResult::Complete => {
                    return Some(self.waiters.remove(index).subscription.context);
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

    /// role 能观察的合法电平位。
    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals>;

    fn signals(&self) -> ObjectSignals;
    fn subscribe(&self, subscription: Subscription) -> SubscribeResult;
    fn unsubscribe(&self, id: u64);

    /// Handle 从表中移除且表锁已释放后的 lifecycle 回调。
    fn close_handle(&self, role: HandleRole, owner: &Process, exiting: bool);

    /// 消息中的 transit Handle 被丢弃；只有持 TRANSIT 的 entry 可进入。
    fn close_transit(&self, role: HandleRole);

    fn as_any(&self) -> &dyn Any;
}

pub type ObjectRef = Arc<dyn KernelObject>;
