//! Mailbox：角色即对象类型（capability 分案设计轮次 4）。
//!
//! - [`MailboxCore`]：FIFO、接收预留、CLOSED/READABLE/WRITABLE 电平、发送授权聚合计数；
//!   业务仲裁（入队 vs shut、Once Pending→Sent/Abandoned）只有 core 锁一个同步域。
//! - [`MailboxReceiver`]：最后 capability 消失即 shut（Terminal）。
//! - [`MailboxSender`]：可复制发送授权，绑定不可变 badge；按 core 聚合计入 no-senders。
//! - [`MailboxOnceSender`]：每次派生铸造新对象，不可复制；move 消费；armed 后最后释放
//!   仍 Pending 即在 core 锁内转 Abandoned。
//! - [`ReplyObservation`]：OnceSender 出生配对的观察壳，DONE=Sent、PEER_CLOSED=Abandoned，
//!   终态互斥不可变。OnceSender 单向强持它（只保活存储），它不持 OnceSender/core。
//!
//! 锁序：进程 CapTable → MAILBOX(core) → LIFETIME(观察源)；观察 Query 不反向取 core 锁。

use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use core::{
    any::Any,
    sync::atomic::{AtomicBool, AtomicU8, Ordering},
};

use erhino_shared::{
    call::SystemCallError,
    message::{
        HandleMove, MAILBOX_CAPACITY, MESSAGE_HANDLE_MAX, MailboxBadge, MessageHeader, PAYLOAD_MAX,
        SendHeader,
    },
    object::{Handle, HandlePair, ObjectSignals, Rights, SenderResult},
};

use crate::{sync::Spinlock, uaccess};

use super::{
    Thread,
    cap::Cap,
    handle,
    lifetime::LifetimeOwner,
    object::{KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState, SubscribeResult},
    resources::{IpcPermit, MetadataSponsor},
    wait::Subscription,
};

pub(crate) mod selftest;

#[derive(Clone, Copy)]
struct ReceiveOutput {
    header: usize,
    payload: usize,
    handles: usize,
}

/// 已被邮箱接纳的消息：随消息移动的 capability 与本次 Delivery 都是 affine owner。
pub struct Message {
    pub header: MessageHeader,
    pub payload: Vec<u8>,
    pub handles: Vec<Cap>,
    pub delivery: Cap,
}

impl Message {
    /// 有界释放：至多 MESSAGE_HANDLE_MAX + 1 份 authority（锁外调用）。
    pub fn release(self) {
        handle::release_all(self.handles);
        handle::release_all(core::iter::once(self.delivery));
    }
}

// ---------------------------------------------------------------- core

struct CoreState {
    wait: ObjectWaitState,
    queue: VecDeque<Message>,
    receiving: Option<u64>,
    closed: bool,
    /// 仍可产生新投递的发送授权：普通 Sender 对象 + Pending 的 OnceSender。
    senders: usize,
}

impl CoreState {
    /// 电平是状态的函数：所有迁移点调用同一发布函数。
    fn publish(&mut self) {
        if self.closed {
            self.wait.update(
                ObjectSignals::READABLE | ObjectSignals::WRITABLE | ObjectSignals::PEER_CLOSED,
                ObjectSignals::CLOSED,
            );
            return;
        }
        let occupied = self.queue.len() + usize::from(self.receiving.is_some());
        let mut level = ObjectSignals::NONE;
        if self.receiving.is_none() && !self.queue.is_empty() {
            level |= ObjectSignals::READABLE;
        }
        if occupied < MAILBOX_CAPACITY {
            level |= ObjectSignals::WRITABLE;
        }
        // no-senders：可清除电平，Receiver 仍可 mint 新 Sender。
        if self.senders == 0 {
            level |= ObjectSignals::PEER_CLOSED;
        }
        self.wait.update(
            ObjectSignals::READABLE | ObjectSignals::WRITABLE | ObjectSignals::PEER_CLOSED,
            level,
        );
    }

    fn admit(&self) -> Result<(), SystemCallError> {
        if self.closed {
            return Err(SystemCallError::ObjectClosed);
        }
        let occupied = self.queue.len() + usize::from(self.receiving.is_some());
        if occupied >= MAILBOX_CAPACITY {
            return Err(SystemCallError::MailboxFull);
        }
        Ok(())
    }
}

pub struct MailboxCore {
    header: ObjectHeader,
    state: Spinlock<CoreState>,
    _permit: IpcPermit,
}

impl MailboxCore {
    fn new(sponsor: &Arc<MetadataSponsor>) -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            state: Spinlock::new(
                crate::sync::ranks::MAILBOX,
                CoreState {
                    wait: ObjectWaitState::new(ObjectSignals::WRITABLE | ObjectSignals::PEER_CLOSED),
                    queue: VecDeque::new(),
                    receiving: None,
                    closed: false,
                    senders: 0,
                },
            ),
            _permit: MetadataSponsor::reserve_ipc(sponsor, super::resources::IpcClass::Object)?,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    fn add_sender(&self) {
        let mut state = self.state.lock();
        state.senders += 1;
        state.publish();
    }

    fn drop_sender(&self) {
        {
            let mut state = self.state.lock();
            state.senders = state.senders.checked_sub(1).expect("mailbox sender count underflow");
            state.publish();
        }
        self.finish_waiters();
    }

    /// 在 core 锁下判定 open/容量并入队；`extract` 在同一临界区摘出移动项，
    /// `accepted` 在入队成功的同一临界区执行（Once Pending→Sent）。
    fn enqueue(
        &self,
        header: MessageHeader,
        payload: Vec<u8>,
        delivery: Cap,
        extract: impl FnOnce() -> Result<Vec<Cap>, SystemCallError>,
        accepted: impl FnOnce(&mut CoreState),
    ) -> Result<(), (SystemCallError, Cap)> {
        let mut state = self.state.lock();
        if let Err(error) = state.admit() {
            return Err((error, delivery));
        }
        if state.queue.try_reserve(1).is_err() {
            return Err((SystemCallError::OutOfMemory, delivery));
        }
        let handles = match extract() {
            Ok(handles) => handles,
            Err(error) => return Err((error, delivery)),
        };
        state.queue.push_back(Message {
            header,
            payload,
            handles,
            delivery,
        });
        accepted(&mut state);
        state.publish();
        Ok(())
    }

    fn peek(&self) -> Result<MessageHeader, SystemCallError> {
        let state = self.state.lock();
        if state.closed {
            return Err(SystemCallError::ObjectClosed);
        }
        if state.receiving.is_some() {
            return Err(SystemCallError::ObjectBusy);
        }
        state
            .queue
            .front()
            .map(|message| message.header)
            .ok_or(SystemCallError::ObjectNotAvailable)
    }

    /// 出队即建立在途接收事务：ReceiveTxn（调用栈上的 token + Message）是稳定 owner，
    /// Close 不取消它。
    fn begin_receive(
        &self,
        token: u64,
        payload_capacity: usize,
        handle_capacity: usize,
    ) -> Result<Message, SystemCallError> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(SystemCallError::ObjectClosed);
        }
        if state.receiving.is_some() {
            return Err(SystemCallError::ObjectBusy);
        }
        let Some(front) = state.queue.front() else {
            return Err(SystemCallError::ObjectNotAvailable);
        };
        if payload_capacity < front.payload.len() || handle_capacity < front.handles.len() {
            return Err(SystemCallError::BufferTooSmall);
        }
        state.receiving = Some(token);
        let message = state.queue.pop_front().expect("front was checked");
        state.publish();
        Ok(message)
    }

    fn commit_receive(&self, token: u64) {
        let mut state = self.state.lock();
        assert!(state.receiving == Some(token), "mailbox receive token mismatch");
        state.receiving = None;
        state.publish();
    }

    /// 返回 Some 表示已 shut：消息不重新入队，由调用方锁外有界释放。
    fn rollback_receive(&self, token: u64, message: Message) -> Option<Message> {
        let mut state = self.state.lock();
        assert!(state.receiving == Some(token), "mailbox receive token mismatch");
        state.receiving = None;
        if state.closed {
            state.publish();
            return Some(message);
        }
        state.queue.push_front(message);
        state.publish();
        None
    }

    fn discard(&self) -> Result<Message, SystemCallError> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(SystemCallError::ObjectClosed);
        }
        if state.receiving.is_some() {
            return Err(SystemCallError::ObjectBusy);
        }
        let message = state
            .queue
            .pop_front()
            .ok_or(SystemCallError::ObjectNotAvailable)?;
        state.publish();
        Ok(message)
    }

    /// 最后 Receiver 消失：锁内 shut 并发布 CLOSED，之后每次摘一条消息锁外释放。
    /// 单次摘取有界（一条消息）；整队排空改为 retire Work 由维护执行推进，
    /// 见入口 todo 债务 D-mailbox-retire。
    fn shut(&self) {
        {
            let mut state = self.state.lock();
            if state.closed {
                return;
            }
            state.closed = true;
            state.publish();
        }
        self.finish_waiters();
        loop {
            let message = self.state.lock().queue.pop_front();
            let Some(message) = message else { break };
            message.release();
        }
    }

    pub(crate) fn finish_waiters(&self) {
        let pending = self.state.lock().wait.take_notification();
        if let Some((reservation, target)) = pending {
            reservation.publish(target);
        }
    }
}

/// 三个对象类型共用的等待接口委托。
macro_rules! core_wait_delegate {
    () => {
        fn signals(&self) -> ObjectSignals {
            self.core.state.lock().wait.signals()
        }
        fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
            self.core.state.lock().wait.subscribe(subscription)
        }
        fn rearm_observer(
            &self,
            id: u64,
        ) -> Result<super::object::ObserverRearm, SystemCallError> {
            self.core.state.lock().wait.rearm_observer(id)
        }
        fn cancel_observer(&self, id: u64) -> Option<super::object::CancelledObservation> {
            self.core.state.lock().wait.cancel_observer(id)
        }
        fn unsubscribe(&self, id: u64) {
            let retired = self.core.state.lock().wait.unsubscribe(id);
            drop(retired);
        }
        fn complete_waiter_drain(
            &self,
            reservation: super::notify_work::Reservation,
        ) -> super::notify_work::Completion {
            self.core.state.lock().wait.complete_notification(reservation)
        }
        fn advance_waiter(&self) -> super::object::WaitAdvance {
            self.core.state.lock().wait.advance_waiter()
        }
        fn related_id(&self) -> u64 {
            self.core.header.koid()
        }
    };
}

// ------------------------------------------------------------ receiver

pub struct MailboxReceiver {
    holders: capability::Holders,
    header: ObjectHeader,
    core: Arc<MailboxCore>,
}

impl KernelObject for MailboxReceiver {
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::MailboxReceiver
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT)
    }
    fn allowed_signals(&self) -> Option<ObjectSignals> {
        Some(ObjectSignals::READABLE | ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED)
    }
    core_wait_delegate!();
    fn on_last_capability(&self) {
        self.core.shut();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// -------------------------------------------------------------- sender

pub struct MailboxSender {
    holders: capability::Holders,
    header: ObjectHeader,
    badge: MailboxBadge,
    core: Arc<MailboxCore>,
    _lifetime: LifetimeOwner,
    _permit: IpcPermit,
}

impl KernelObject for MailboxSender {
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::MailboxSender
    }
    fn badge(&self) -> u64 {
        self.badge
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::GRANT | Rights::DUPLICATE)
    }
    fn allowed_signals(&self) -> Option<ObjectSignals> {
        Some(ObjectSignals::WRITABLE | ObjectSignals::CLOSED)
    }
    core_wait_delegate!();
    fn on_last_capability(&self) {
        self.core.drop_sender();
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ---------------------------------------------------------- once sender

const ONCE_PREPARED: u8 = 0;
const ONCE_PENDING: u8 = 1;
const ONCE_SENT: u8 = 2;
const ONCE_ABANDONED: u8 = 3;

/// 一次性发送授权。`state` 只在持 core 锁时修改（原子仅为跨引用可读）。
pub struct MailboxOnceSender {
    holders: capability::Holders,
    header: ObjectHeader,
    badge: MailboxBadge,
    core: Arc<MailboxCore>,
    observation: Arc<ReplyObservation>,
    state: AtomicU8,
    _permit: IpcPermit,
}

impl MailboxOnceSender {
    /// 发布协议提交点：authority 已对用户可见时才承担放弃可见性，并计入 no-senders。
    fn arm(&self) {
        let mut core = self.core.state.lock();
        let previous = self.state.swap(ONCE_PENDING, Ordering::Relaxed);
        debug_assert_eq!(previous, ONCE_PREPARED);
        core.senders += 1;
        core.publish();
    }

    /// 与 `accepted` 同一 core 临界区：Pending→Sent。
    fn mark_sent(&self, core: &mut CoreState) {
        let previous = self.state.swap(ONCE_SENT, Ordering::Relaxed);
        debug_assert_eq!(previous, ONCE_PENDING);
        core.senders -= 1;
        self.observation.publish(ReplyResult::Sent);
    }
}

impl KernelObject for MailboxOnceSender {
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::MailboxOnceSender
    }
    fn badge(&self) -> u64 {
        self.badge
    }
    fn allowed_rights(&self) -> Option<Rights> {
        // 不可复制：每份一次性额度是独立对象。
        Some(Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::GRANT)
    }
    fn allowed_signals(&self) -> Option<ObjectSignals> {
        Some(ObjectSignals::WRITABLE | ObjectSignals::CLOSED)
    }
    core_wait_delegate!();
    /// 本地 Close、transit 丢弃、进程回收、restore 撤销同一路径：
    /// Pending 恰好一次转 Abandoned；未 armed（未发布准备回滚）只退款不产生事件。
    fn on_last_capability(&self) {
        let abandoned = {
            let mut core = self.core.state.lock();
            if self.state.load(Ordering::Relaxed) != ONCE_PENDING {
                false
            } else {
                self.state.store(ONCE_ABANDONED, Ordering::Relaxed);
                core.senders -= 1;
                self.observation.publish(ReplyResult::Abandoned);
                core.publish();
                true
            }
        };
        if abandoned {
            self.core.finish_waiters();
        }
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// --------------------------------------------------------- observation

#[derive(Clone, Copy)]
enum ReplyResult {
    Sent,
    Abandoned,
}

/// OnceSender 结果的观察壳：只存 OnceSender 数值身份、结果与等待源。
/// 只可 READ/WAIT（及 DUPLICATE/TRANSIT/GRANT），无设置结果或取消发送的权限。
pub struct ReplyObservation {
    holders: capability::Holders,
    header: ObjectHeader,
    once_id: AtomicU64Cell,
    published: AtomicBool,
    wait: Spinlock<ObjectWaitState>,
    _permit: IpcPermit,
}

/// 出生后写入一次的 OnceSender 身份（构造顺序：观察壳先于 OnceSender）。
struct AtomicU64Cell(core::sync::atomic::AtomicU64);

impl ReplyObservation {
    fn new(sponsor: &Arc<MetadataSponsor>) -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            once_id: AtomicU64Cell(core::sync::atomic::AtomicU64::new(0)),
            published: AtomicBool::new(false),
            wait: Spinlock::new(
                crate::sync::ranks::LIFETIME,
                ObjectWaitState::new(ObjectSignals::NONE),
            ),
            _permit: MetadataSponsor::reserve_ipc(sponsor, super::resources::IpcClass::Object)?,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    /// 由 core 锁内的唯一决定调用（锁序 core → 观察源）；终态互斥不可变。
    fn publish(&self, result: ReplyResult) {
        let already = self.published.swap(true, Ordering::AcqRel);
        assert!(!already, "reply observation result is immutable");
        let level = match result {
            ReplyResult::Sent => ObjectSignals::DONE,
            ReplyResult::Abandoned => ObjectSignals::PEER_CLOSED,
        };
        let pending = {
            let mut wait = self.wait.lock();
            wait.update(ObjectSignals::NONE, level);
            wait.take_notification()
        };
        // 通知扫描交既有 Work 推进，最后 release 不遍历观察者。
        if let Some((reservation, target)) = pending {
            reservation.publish(target);
        }
    }
}

impl KernelObject for ReplyObservation {
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::ReplyObservation
    }
    fn related_id(&self) -> u64 {
        self.once_id.0.load(Ordering::Relaxed)
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(Rights::READ | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT)
    }
    fn allowed_signals(&self) -> Option<ObjectSignals> {
        Some(ObjectSignals::DONE | ObjectSignals::PEER_CLOSED)
    }
    fn signals(&self) -> ObjectSignals {
        self.wait.lock().signals()
    }
    fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
        self.wait.lock().subscribe(subscription)
    }
    fn rearm_observer(&self, id: u64) -> Result<super::object::ObserverRearm, SystemCallError> {
        self.wait.lock().rearm_observer(id)
    }
    fn cancel_observer(&self, id: u64) -> Option<super::object::CancelledObservation> {
        self.wait.lock().cancel_observer(id)
    }
    fn unsubscribe(&self, id: u64) {
        let retired = self.wait.lock().unsubscribe(id);
        drop(retired);
    }
    fn complete_waiter_drain(
        &self,
        reservation: super::notify_work::Reservation,
    ) -> super::notify_work::Completion {
        self.wait.lock().complete_notification(reservation)
    }
    fn advance_waiter(&self) -> super::object::WaitAdvance {
        self.wait.lock().advance_waiter()
    }
    // 关闭观察壳只放弃观察：不取消发送、不触发 Abandoned、不等待 OnceSender。
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ------------------------------------------------------------ syscalls

fn receiver_core(thread: &Thread, handle: Handle, rights: Rights) -> Result<Arc<MailboxCore>, SystemCallError> {
    let object = {
        let table = thread.process.handles.lock();
        handle::resolve(&table, handle, rights, ObjectKind::MailboxReceiver)?
    };
    let receiver = object
        .as_any()
        .downcast_ref::<MailboxReceiver>()
        .ok_or(SystemCallError::WrongObjectType)?;
    Ok(receiver.core.clone())
}

fn single(cap: Cap) -> Result<Vec<Cap>, SystemCallError> {
    let mut caps = Vec::new();
    if caps.try_reserve_exact(1).is_err() {
        handle::release_all(core::iter::once(cap));
        return Err(SystemCallError::OutOfMemory);
    }
    caps.push(cap);
    Ok(caps)
}

fn pair(first: Cap, second: Cap) -> Result<Vec<Cap>, SystemCallError> {
    let mut caps = Vec::new();
    if caps.try_reserve_exact(2).is_err() {
        handle::release_all([first, second]);
        return Err(SystemCallError::OutOfMemory);
    }
    caps.push(first);
    caps.push(second);
    Ok(caps)
}

pub fn create(thread: &Thread, receiver_rights: Rights, output: usize) -> Result<(), SystemCallError> {
    let sponsor = thread.process.resources.metadata();
    let core = MailboxCore::new(sponsor)?;
    let receiver: ObjectRef = Arc::try_new(MailboxReceiver {
        holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
        header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
        core,
    })
    .map_err(|_| SystemCallError::OutOfMemory)?;
    let caps = single(handle::mint(receiver, receiver_rights)?)?;
    handle::install(
        thread,
        caps,
        |handles| uaccess::write_user(thread, output, &handles[0]),
        || {},
    )
    .map(|_| ())
}

/// 由 Receiver 铸造新的 MailboxSender（新身份、不可变 badge）与其 Lifetime 观察。
pub fn mint_sender(
    thread: &Thread,
    receiver: Handle,
    badge: MailboxBadge,
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let core = receiver_core(thread, receiver, Rights::MANAGE)?;
    let sponsor = thread.process.resources.metadata();
    let header = ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?;
    let (lifetime, observer) = LifetimeOwner::new(header.koid(), sponsor)?;
    let sender: ObjectRef = Arc::try_new(MailboxSender {
        holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
        header,
        badge,
        core: core.clone(),
        _lifetime: lifetime,
        _permit: MetadataSponsor::reserve_ipc(sponsor, super::resources::IpcClass::Object)?,
    })
    .map_err(|_| SystemCallError::OutOfMemory)?;
    // 计入发生在 mint（authority 已存在）时：安装失败的 release 经 on_last 对称扣减。
    core.add_sender();
    let sender_cap = handle::mint(sender, rights);
    let observer_cap = handle::mint(
        observer,
        Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT,
    );
    let (sender_cap, observer_cap) = match (sender_cap, observer_cap) {
        (Ok(s), Ok(o)) => (s, o),
        (Ok(s), Err(e)) | (Err(e), Ok(s)) => {
            handle::release_all(core::iter::once(s));
            return Err(e);
        }
        (Err(e), Err(_)) => {
            // sender 未被任何 authority 持有：手动对称扣减。
            core.drop_sender();
            return Err(e);
        }
    };
    let caps = pair(sender_cap, observer_cap)?;
    handle::install(
        thread,
        caps,
        |handles| {
            let result = SenderResult {
                sender: handles[0],
                lifetime: handles[1],
            };
            uaccess::write_user(thread, output, &result)
        },
        || {},
    )
    .map(|_| ())
}

/// 从具 DUPLICATE 的 MailboxSender 派生新的 MailboxOnceSender 与配对 ReplyObservation。
/// 输出 HandlePair{first: once, second: observation}。二者与 armed 按一次发布协议提交。
pub fn make_send_once(
    thread: &Thread,
    source: Handle,
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let (core, badge) = {
        let table = thread.process.handles.lock();
        let cap = table.get(source, Rights::DUPLICATE).map_err(handle::map_error)?;
        if !rights.is_subset_of(cap.rights()) {
            return Err(SystemCallError::RightsDenied);
        }
        let sender = cap
            .object()
            .0
            .as_any()
            .downcast_ref::<MailboxSender>()
            .ok_or(SystemCallError::WrongObjectType)?;
        (sender.core.clone(), sender.badge)
    };
    let sponsor = thread.process.resources.metadata();
    let observation = ReplyObservation::new(sponsor)?;
    let once = Arc::try_new(MailboxOnceSender {
        holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
        header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
        badge,
        core,
        observation: observation.clone(),
        state: AtomicU8::new(ONCE_PREPARED),
        _permit: MetadataSponsor::reserve_ipc(sponsor, super::resources::IpcClass::Object)?,
    })
    .map_err(|_| SystemCallError::OutOfMemory)?;
    observation.once_id.0.store(once.header.koid(), Ordering::Relaxed);
    let once_ref: ObjectRef = once.clone();
    let observation_ref: ObjectRef = observation;
    // 未 armed：任何半安装失败的最后释放只退款，不产生 Abandoned。
    let once_cap = handle::mint(once_ref, rights)?;
    let observation_cap = match handle::mint(
        observation_ref,
        Rights::READ | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT,
    ) {
        Ok(cap) => cap,
        Err(error) => {
            handle::release_all(core::iter::once(once_cap));
            return Err(error);
        }
    };
    let caps = pair(once_cap, observation_cap)?;
    handle::install(
        thread,
        caps,
        |handles| {
            let result = HandlePair {
                first: handles[0],
                second: handles[1],
            };
            uaccess::write_user(thread, output, &result)
        },
        || once.arm(),
    )
    .map(|_| ())
}

struct SendInput {
    header: SendHeader,
    payload: Vec<u8>,
    moves: Vec<(Handle, Rights)>,
}

fn read_send_input(
    thread: &Thread,
    header_ptr: usize,
    payload_ptr: usize,
    moves_ptr: usize,
    move_count: usize,
    payload_len: usize,
) -> Result<SendInput, SystemCallError> {
    if payload_len > PAYLOAD_MAX || move_count > MESSAGE_HANDLE_MAX {
        return Err(SystemCallError::IllegalArgument);
    }
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(payload_len)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    payload.resize(payload_len, 0);
    let move_bytes = move_count
        .checked_mul(core::mem::size_of::<HandleMove>())
        .ok_or(SystemCallError::IllegalArgument)?;
    let mut raw_moves = Vec::new();
    raw_moves
        .try_reserve_exact(move_bytes)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    raw_moves.resize(move_bytes, 0);
    let header: SendHeader = {
        let mut space = thread.process.space.lock();
        // SAFETY: SendHeader 仅含整数且无 padding，任意位型有效。
        let header = unsafe { uaccess::read_user_value(&mut space, header_ptr) }?;
        uaccess::copy_from_user(&mut space, &mut payload, payload_ptr)?;
        uaccess::copy_from_user(&mut space, &mut raw_moves, moves_ptr)?;
        header
    };
    if header.payload_len as usize != payload_len
        || header.handle_count as usize != move_count
        || header.reserved != [0; 3]
    {
        return Err(SystemCallError::IllegalArgument);
    }
    let mut moves = Vec::new();
    moves
        .try_reserve_exact(move_count)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    for bytes in raw_moves
        .as_chunks::<{ core::mem::size_of::<HandleMove>() }>()
        .0
    {
        // SAFETY: HandleMove 仅含整数 newtype；缓冲可能不对齐，故 unaligned 读。
        let item = unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<HandleMove>()) };
        moves.push((item.handle, item.rights));
    }
    Ok(SendInput {
        header: header,
        payload,
        moves,
    })
}

/// Send：两层线性化。CapTable 仲裁本发送 handle 的 Close/Move 与接管；core 锁仲裁
/// open/容量与入队，Once 的 Pending→Sent 与入队同一临界区。
pub fn send(
    thread: &Thread,
    sender_handle: Handle,
    header_ptr: usize,
    payload_ptr: usize,
    moves_ptr: usize,
    move_count: usize,
    payload_len: usize,
) -> Result<(), SystemCallError> {
    let input = read_send_input(thread, header_ptr, payload_ptr, moves_ptr, move_count, payload_len)?;
    if input.moves.iter().any(|(handle, _)| *handle == sender_handle) {
        return Err(SystemCallError::IllegalArgument);
    }
    let sponsor = thread.process.resources.metadata();
    let mut table = thread.process.handles.lock();
    let kind = table
        .get(sender_handle, Rights::WRITE)
        .map_err(handle::map_error)?
        .object()
        .0
        .kind();
    match kind {
        ObjectKind::MailboxSender => {
            let cap = table.get(sender_handle, Rights::WRITE).map_err(handle::map_error)?;
            let context = cap.object().0.clone();
            let sender = context
                .as_any()
                .downcast_ref::<MailboxSender>()
                .expect("kind checked");
            let core = sender.core.clone();
            let header = message_header(thread, sender.badge, context.header().koid(), &input.header);
            let delivery = delivery_cap(context, sponsor)?;
            let result = core.enqueue(
                header,
                input.payload,
                delivery,
                || table.extract(&input.moves, Rights::TRANSIT).map_err(handle::map_error),
                |_| {},
            );
            drop(table);
            finish_send(&core, result)
        }
        ObjectKind::MailboxOnceSender => {
            // move 语义：claim 接管这份一次性 authority。
            let (cap, ticket) = table
                .claim(sender_handle, Rights::WRITE)
                .map_err(handle::map_error)?;
            let context = cap.object().0.clone();
            let once = context
                .as_any()
                .downcast_ref::<MailboxOnceSender>()
                .expect("kind checked");
            let core = once.core.clone();
            let header = message_header(thread, once.badge, context.header().koid(), &input.header);
            let delivery = match delivery_cap(context.clone(), sponsor) {
                Ok(delivery) => delivery,
                Err(error) => {
                    let restored = table.restore(ticket, cap);
                    drop(table);
                    deliver_restore(restored);
                    return Err(error);
                }
            };
            let result = core.enqueue(
                header,
                input.payload,
                delivery,
                || table.extract(&input.moves, Rights::TRANSIT).map_err(handle::map_error),
                |state| once.mark_sent(state),
            );
            match result {
                Ok(()) => {
                    table.commit_claim(ticket);
                    drop(table);
                    // 已 Sent：最后释放不再产生 Abandoned。
                    handle::release_all(core::iter::once(cap));
                    finish_send(&core, Ok(()))
                }
                Err((error, delivery)) => {
                    // 未入队：恢复原 handle 不发放弃；资格已被撤销则 restore 返回最后持有事件。
                    let restored = table.restore(ticket, cap);
                    drop(table);
                    deliver_restore(restored);
                    handle::release_all(core::iter::once(delivery));
                    Err(error)
                }
            }
        }
        _ => Err(SystemCallError::WrongObjectType),
    }
}

fn message_header(thread: &Thread, badge: MailboxBadge, context_id: u64, header: &SendHeader) -> MessageHeader {
    let mut message = MessageHeader::new(
        thread.process.pid,
        badge,
        header.kind,
        header.payload_len,
        header.handle_count,
    );
    // 每份发送授权（含每个 OnceSender）的稳定身份；RPC 以此关联回复额度。
    message.sender_context_id = context_id;
    message
}

fn delivery_cap(context: ObjectRef, sponsor: &Arc<MetadataSponsor>) -> Result<Cap, SystemCallError> {
    let delivery = super::delivery::Delivery::create(context, sponsor)?;
    handle::mint(delivery, Rights::TRANSIT | Rights::GRANT)
}

fn deliver_restore(restored: capability::Restore<super::cap::ObjectAuthority>) {
    if let capability::Restore::Released(event) = restored {
        super::cap::deliver(event);
    }
}

fn finish_send(core: &MailboxCore, result: Result<(), (SystemCallError, Cap)>) -> Result<(), SystemCallError> {
    match result {
        Ok(()) => {
            core.finish_waiters();
            Ok(())
        }
        Err((error, delivery)) => {
            handle::release_all(core::iter::once(delivery));
            Err(error)
        }
    }
}

pub fn peek(thread: &Thread, receiver: Handle, output: usize) -> Result<(), SystemCallError> {
    let header = receiver_core(thread, receiver, Rights::READ)?.peek()?;
    uaccess::write_user(thread, output, &header)
}

pub fn receive(
    thread: &Thread,
    receiver: Handle,
    header_output: usize,
    payload_output: usize,
    payload_capacity: usize,
    handles_output: usize,
    handle_capacity: usize,
) -> Result<(), SystemCallError> {
    if payload_capacity > PAYLOAD_MAX || handle_capacity > MESSAGE_HANDLE_MAX {
        return Err(SystemCallError::IllegalArgument);
    }
    let core = receiver_core(thread, receiver, Rights::READ)?;
    let token = handle::transaction_token()?;
    let message = core.begin_receive(token, payload_capacity, handle_capacity)?;
    finish_receive(
        thread,
        &core,
        token,
        message,
        ReceiveOutput {
            header: header_output,
            payload: payload_output,
            handles: handles_output,
        },
    )
}

/// 出队的消息由本事务持有；安装 caps（预留→写回→提交）成功即 commit，
/// 失败则 rollback：未 shut 重新放回队头，已 shut 锁外有界释放。
fn finish_receive(
    thread: &Thread,
    core: &MailboxCore,
    token: u64,
    message: Message,
    output: ReceiveOutput,
) -> Result<(), SystemCallError> {
    let Message {
        header,
        payload,
        mut handles,
        delivery,
    } = message;
    let count = handles.len();
    if handles.try_reserve_exact(1).is_err() {
        let message = Message { header, payload, handles, delivery };
        return rollback(core, token, message, SystemCallError::OutOfMemory);
    }
    handles.push(delivery);
    let mut table = thread.process.handles.lock();
    let reservation = match table.reserve(handles.len()) {
        Ok(reservation) => reservation,
        Err(error) => {
            drop(table);
            let delivery = handles.pop().expect("delivery pushed");
            let message = Message { header, payload, handles, delivery };
            return rollback(core, token, message, handle::map_error(error));
        }
    };
    let slots = reservation.handles();
    let result = erhino_shared::message::ReceiveResult {
        header,
        delivery: slots[count],
    };
    let copied = {
        let mut space = thread.process.space.lock();
        // SAFETY: ReceiveResult 无 padding，Handle 是 u64 newtype。
        unsafe { uaccess::deliver_output(thread, &mut space, output.header, &result) }
            .and_then(|_| uaccess::deliver_bytes(thread, &mut space, output.payload, &payload))
            .and_then(|_| {
                // SAFETY: Handle 是 repr(transparent) u64，切片按字节读取。
                let bytes = unsafe {
                    core::slice::from_raw_parts(
                        slots[..count].as_ptr().cast::<u8>(),
                        core::mem::size_of_val(&slots[..count]),
                    )
                };
                uaccess::deliver_bytes(thread, &mut space, output.handles, bytes)
            })
    };
    if let Err(error) = copied {
        table.rollback(reservation);
        drop(table);
        let delivery = handles.pop().expect("delivery pushed");
        let message = Message { header, payload, handles, delivery };
        return rollback(core, token, message, error);
    }
    if let Err((caps, error)) = table.commit(reservation, handles) {
        drop(table);
        // 表已 shut（进程终止）：消息责任已出队，有界释放。
        core.commit_receive(token);
        handle::release_all(caps);
        core.finish_waiters();
        return Err(handle::map_error(error));
    }
    drop(table);
    core.commit_receive(token);
    core.finish_waiters();
    Ok(())
}

fn rollback(core: &MailboxCore, token: u64, message: Message, error: SystemCallError) -> Result<(), SystemCallError> {
    if let Some(message) = core.rollback_receive(token, message) {
        message.release();
    }
    core.finish_waiters();
    Err(error)
}

pub fn discard(thread: &Thread, receiver: Handle) -> Result<(), SystemCallError> {
    let core = receiver_core(thread, receiver, Rights::READ)?;
    core.discard()?.release();
    core.finish_waiters();
    Ok(())
}
