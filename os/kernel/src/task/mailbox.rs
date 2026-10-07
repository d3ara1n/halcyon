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
//! 表锁只接管/返还 authority；业务在表锁外。MAILBOX 仲裁后锁外发布
//! ReplyObservation/Lifetime 终态与门铃，来源扫描不反向取业务锁。

use alloc::sync::Arc;
use core::{
    any::Any,
    pin::Pin,
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

use crate::{
    maintenance::{self as mt, Work, WorkRecord},
    sync::Spinlock,
    uaccess,
};
use maintenance::record::Outcome;

use super::{
    Thread,
    cap::Cap,
    handle,
    lifetime::LifetimeOwner,
    memory_pool::MemoryPool,
    object::{KernelObject, ObjectHeader, ObjectKind, ObjectRef},
    resources::{MetadataBuffer, MetadataFunding, map_storage_error},
    wait_source::{Hint, ObservationSource, SignalSource, Snapshot, Step, WaitNode, WaitQueue},
};

static NEXT_RECEIVE: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

#[derive(Clone, Copy)]
struct ReceiveOutput {
    header: usize,
    payload: usize,
    handles: usize,
}

/// 已被邮箱接纳的消息：随消息移动的 capability 与本次 Delivery 都是 affine owner。
pub struct Message {
    pub header: MessageHeader,
    pub payload: MetadataBuffer<u8>,
    pub handles: MetadataBuffer<Cap>,
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

struct MessageQueue {
    slots: MetadataBuffer<Option<Message>>,
    head: usize,
    len: usize,
}

impl MessageQueue {
    fn new(pool: &Arc<MemoryPool>) -> Result<Self, SystemCallError> {
        let mut slots = MetadataBuffer::try_empty(MAILBOX_CAPACITY, &MetadataFunding(pool))
            .map_err(map_storage_error)?;
        for _ in 0..MAILBOX_CAPACITY {
            slots.push(None);
        }
        Ok(Self {
            slots,
            head: 0,
            len: 0,
        })
    }
    fn len(&self) -> usize {
        self.len
    }
    fn is_empty(&self) -> bool {
        self.len == 0
    }
    fn front(&self) -> Option<&Message> {
        if self.is_empty() {
            None
        } else {
            Some(
                self.slots[self.head]
                    .as_ref()
                    .expect("mailbox ring lost front"),
            )
        }
    }
    fn push_back(&mut self, message: Message) {
        assert!(
            self.len < self.slots.len(),
            "mailbox ring capacity exceeded"
        );
        let tail = (self.head + self.len) % self.slots.len();
        assert!(self.slots[tail].is_none(), "mailbox ring tail occupied");
        self.slots[tail] = Some(message);
        self.len += 1;
    }
    fn push_front(&mut self, message: Message) {
        assert!(
            self.len < self.slots.len(),
            "mailbox rollback capacity exceeded"
        );
        self.head = (self.head + self.slots.len() - 1) % self.slots.len();
        assert!(
            self.slots[self.head].is_none(),
            "mailbox rollback slot occupied"
        );
        self.slots[self.head] = Some(message);
        self.len += 1;
    }
    fn pop_front(&mut self) -> Option<Message> {
        if self.is_empty() {
            return None;
        }
        let message = self.slots[self.head]
            .take()
            .expect("mailbox ring lost front");
        self.head = (self.head + 1) % self.slots.len();
        self.len -= 1;
        Some(message)
    }
}

struct CoreState {
    queue: MessageQueue,
    receiving: Option<u64>,
    closed: bool,
    senders: usize,
    cleanup_turn: bool,
}

impl CoreState {
    fn level(&self) -> ObjectSignals {
        if self.closed {
            return ObjectSignals::CLOSED;
        }
        let occupied = self.queue.len() + usize::from(self.receiving.is_some());
        let mut level = ObjectSignals::NONE;
        if self.receiving.is_none() && !self.queue.is_empty() {
            level |= ObjectSignals::READABLE;
        }
        if occupied < MAILBOX_CAPACITY {
            level |= ObjectSignals::WRITABLE;
        }
        if self.senders == 0 {
            level |= ObjectSignals::PEER_CLOSED;
        }
        level
    }
    fn admit(&self) -> Result<(), SystemCallError> {
        if self.closed {
            return Err(SystemCallError::ObjectClosed);
        }
        if self.queue.len() + usize::from(self.receiving.is_some()) >= MAILBOX_CAPACITY {
            return Err(SystemCallError::MailboxFull);
        }
        Ok(())
    }
}

pub struct MailboxCore {
    header: ObjectHeader,
    state: Spinlock<CoreState>,
    source: WaitQueue,
    _metadata: super::resources::MetadataCharge,
}

impl MailboxCore {
    fn new(pool: Arc<MemoryPool>) -> Result<Arc<Self>, SystemCallError> {
        let metadata = super::resources::MetadataCharge::arc::<Self>(&pool)?;
        let queue = MessageQueue::new(&pool)?;
        let core = Arc::try_new(Self {
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            state: Spinlock::new(
                crate::sync::ranks::MAILBOX,
                CoreState {
                    queue,
                    receiving: None,
                    closed: false,
                    senders: 0,
                    cleanup_turn: true,
                },
            ),
            source: WaitQueue::new(pool.maintenance_account().clone()),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        {
            let state = core.state.lock();
            core.publish(&state);
        }
        Ok(core)
    }

    /// 只在 core 锁下，公开电平与缓冲状态/来源链同域。
    fn publish(&self, state: &CoreState) {
        // SAFETY: Arc core 稳定，调用者持此来源唯一业务锁。
        unsafe {
            self.source.update(state.level(), state.closed);
        }
    }

    fn add_sender(self: &Arc<Self>) -> Result<(), SystemCallError> {
        {
            let mut state = self.state.lock();
            if state.closed {
                return Err(SystemCallError::ObjectClosed);
            }
            state.senders = state
                .senders
                .checked_add(1)
                .ok_or(SystemCallError::ReachLimit)?;
            self.publish(&state);
        }
        self.kick();
        Ok(())
    }

    fn drop_sender(self: &Arc<Self>) {
        {
            let mut state = self.state.lock();
            state.senders = state
                .senders
                .checked_sub(1)
                .expect("mailbox sender count underflow");
            self.publish(&state);
        }
        self.kick();
    }

    /// 此方法内只取 core 锁。cap claim/extract 由 caller 在表锁外准备，
    /// 已接管 authority 保证 Pending/使用域在入队线性化前不会消散。
    fn enqueue(
        self: &Arc<Self>,
        message: Message,
        rights: &[Rights],
        deadline: erhino_shared::time::Deadline,
        accepted: impl FnOnce(&mut CoreState),
    ) -> Result<(), (SystemCallError, Cap, MetadataBuffer<Cap>)> {
        let Message {
            header,
            payload,
            delivery,
            mut handles,
        } = message;
        let mut state = self.state.lock();
        if let Err(error) = state.admit() {
            return Err((error, delivery, handles));
        }
        if let Err(error) = crate::clock::check_delivery(deadline) {
            return Err((error, delivery, handles));
        }
        for (cap, rights) in handles.iter_mut().zip(rights) {
            cap.attenuate(*rights)
                .expect("validated transit rights changed under claimed owner");
        }
        state.queue.push_back(Message {
            header,
            payload,
            handles,
            delivery,
        });
        accepted(&mut state);
        self.publish(&state);
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
        let front = state
            .queue
            .front()
            .ok_or(SystemCallError::ObjectNotAvailable)?;
        if payload_capacity < front.payload.len() || handle_capacity < front.handles.len() {
            return Err(SystemCallError::BufferTooSmall);
        }
        state.receiving = Some(token);
        let message = state.queue.pop_front().expect("mailbox checked front lost");
        self.publish(&state);
        Ok(message)
    }

    fn commit_receive(&self, token: u64) {
        let mut state = self.state.lock();
        assert_eq!(
            state.receiving,
            Some(token),
            "mailbox receive token mismatch"
        );
        state.receiving = None;
        self.publish(&state);
    }

    fn rollback_receive(&self, token: u64, message: Message) -> Option<Message> {
        let mut state = self.state.lock();
        assert_eq!(
            state.receiving,
            Some(token),
            "mailbox receive token mismatch"
        );
        state.receiving = None;
        let retired = if state.closed {
            Some(message)
        } else {
            state.queue.push_front(message);
            None
        };
        self.publish(&state);
        retired
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
        self.publish(&state);
        Ok(message)
    }

    /// 最后 Receiver 本地关准入；队列/来源清理只交给出生记录。
    fn shut(self: &Arc<Self>) {
        {
            let mut state = self.state.lock();
            if state.closed {
                return;
            }
            state.closed = true;
            self.publish(&state);
        }
        self.kick();
    }

    fn kick(self: &Arc<Self>) {
        let root: Arc<dyn Work> = self.clone();
        mt::publish(root);
        mt::kick();
    }
}

impl ObservationSource for MailboxCore {
    fn snapshot(&self) -> Snapshot {
        let _state = self.state.lock();
        // SAFETY: 持来源唯一同步域。
        unsafe { self.source.snapshot() }
    }
    unsafe fn attach(&self, node: Pin<&WaitNode>, interest: ObjectSignals, hint: Hint) {
        let _state = self.state.lock();
        // SAFETY: 调用者给出稳定 node/loan 寿命，来源锁串行链操作。
        unsafe { self.source.attach(node, interest, hint) }
    }
    unsafe fn detach(&self, node: Pin<&WaitNode>) -> Hint {
        let _state = self.state.lock();
        // SAFETY: 同 attach 约束；返回无节点引用。
        unsafe { self.source.detach(node) }
    }
}

impl Work for MailboxCore {
    fn record(&self) -> &WorkRecord {
        self.source.record()
    }
    fn step(self: Arc<Self>) -> Outcome {
        let (retired, step) = {
            let mut state = self.state.lock();
            if state.closed && state.cleanup_turn && !state.queue.is_empty() {
                state.cleanup_turn = false;
                (state.queue.pop_front(), None)
            } else {
                state.cleanup_turn = true;
                // SAFETY: 持来源业务锁，只复制提示，不触结果或跨对象回调。
                let step = unsafe { self.source.step() };
                (None, Some(step))
            }
        };
        if let Some(message) = retired {
            message.release();
            return Outcome::Ready;
        }
        match step.expect("mailbox work lost action") {
            Step::Progress => Outcome::Ready,
            Step::Hint(delivery) => {
                delivery.send();
                Outcome::Ready
            }
            Step::Idle => {
                let state = self.state.lock();
                if state.closed && !state.queue.is_empty() {
                    Outcome::Ready
                } else {
                    Outcome::Idle
                }
            }
        }
    }
}

macro_rules! core_wait_delegate {
    () => {
        fn wait_source(&self) -> Option<Arc<dyn ObservationSource>> {
            Some(self.core.clone())
        }
        fn related_id(&self) -> u64 {
            self.core.header.koid()
        }
    };
}

// ------------------------------------------------------------ receiver

pub struct MailboxReceiver {
    holders: capability::Holders,
    core: Arc<MailboxCore>,
    _metadata: super::resources::MetadataCharge,
}

impl KernelObject for MailboxReceiver {
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }
    fn header(&self) -> &ObjectHeader {
        // 唯一 Receiver authority 与来源 core 共用 Mailbox 身份；
        // Sender 的 related_id 指向这个身份，不再铸造第二个无独立领域含义的 KOID。
        &self.core.header
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

/// 已准备的发送授权计入 no-senders；失败出生也有同一有界 rollback owner。
struct SenderCount {
    core: Arc<MailboxCore>,
    active: AtomicBool,
}
impl SenderCount {
    fn new(core: Arc<MailboxCore>) -> Result<Self, SystemCallError> {
        core.add_sender()?;
        Ok(Self {
            core,
            active: AtomicBool::new(true),
        })
    }
    fn release(&self) {
        if self.active.swap(false, Ordering::AcqRel) {
            self.core.drop_sender();
        }
    }
}
impl Drop for SenderCount {
    fn drop(&mut self) {
        self.release();
    }
}

pub struct MailboxSender {
    holders: capability::Holders,
    header: ObjectHeader,
    badge: MailboxBadge,
    core: Arc<MailboxCore>,
    _lifetime: LifetimeOwner,
    sender_count: SenderCount,
    _metadata: super::resources::MetadataCharge,
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
        self.sender_count.release();
        self._lifetime.release();
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
    parent_authority: LifetimeOwner,
    _metadata: super::resources::MetadataCharge,
}

impl MailboxOnceSender {
    /// 发布协议提交点：authority 已对用户可见时才承担放弃可见性，并计入 no-senders。
    fn arm(&self) {
        let mut core = self.core.state.lock();
        let previous = self.state.swap(ONCE_PENDING, Ordering::Relaxed);
        debug_assert_eq!(previous, ONCE_PREPARED);
        core.senders += 1;
        self.core.publish(&core);
    }

    /// 与 `accepted` 同一 core 临界区：Pending→Sent。
    fn mark_sent(&self, core: &mut CoreState) {
        let previous = self.state.swap(ONCE_SENT, Ordering::Relaxed);
        debug_assert_eq!(previous, ONCE_PENDING);
        core.senders -= 1;
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
                self.core.publish(&core);
                true
            }
        };
        if abandoned {
            self.observation.publish(ReplyResult::Abandoned);
            self.core.kick();
        }
        self.parent_authority.release();
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
    source: Arc<SignalSource>,
    _metadata: super::resources::MetadataCharge,
}

/// 出生后写入一次的 OnceSender 身份（构造顺序：观察壳先于 OnceSender）。
struct AtomicU64Cell(core::sync::atomic::AtomicU64);

impl ReplyObservation {
    fn new(pool: Arc<MemoryPool>) -> Result<Arc<Self>, SystemCallError> {
        let metadata = super::resources::MetadataCharge::arc::<Self>(&pool)?;
        Arc::try_new(Self {
            holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            once_id: AtomicU64Cell(core::sync::atomic::AtomicU64::new(0)),
            published: AtomicBool::new(false),
            source: SignalSource::try_new(pool)?,
            _metadata: metadata,
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
        self.source.publish(level, false);
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

    fn wait_source(&self) -> Option<Arc<dyn ObservationSource>> {
        Some(self.source.clone())
    }
    // 关闭观察壳只放弃观察：不取消发送、不触发 Abandoned、不等待 OnceSender。
    fn as_any(&self) -> &dyn Any {
        self
    }
}

// ------------------------------------------------------------ syscalls

use super::handle::{Authorization, Invocation};

fn receiver_core(
    thread: &Thread,
    handle: Handle,
    rights: Rights,
) -> Result<(Authorization, Arc<MailboxCore>), SystemCallError> {
    let invocation = Authorization::accept(thread, handle, rights)?;
    let receiver = invocation
        .object()
        .as_any()
        .downcast_ref::<MailboxReceiver>()
        .ok_or(SystemCallError::WrongObjectType)?;
    let core = receiver.core.clone();
    Ok((invocation, core))
}

pub fn create(
    thread: &Thread,
    receiver_rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let core = MailboxCore::new(thread.process.funding_pool())?;
    let metadata =
        super::resources::MetadataCharge::arc::<MailboxReceiver>(&thread.process.funding_pool())?;
    let receiver: ObjectRef = Arc::try_new(MailboxReceiver {
        holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
        core,
        _metadata: metadata,
    })
    .map_err(|_| SystemCallError::OutOfMemory)?;
    let caps = [handle::mint(receiver, receiver_rights)?];
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
    let authority = Authorization::accept(thread, receiver, Rights::MANAGE)?;
    let receiver = authority
        .object()
        .as_any()
        .downcast_ref::<MailboxReceiver>()
        .ok_or(SystemCallError::WrongObjectType)?;
    let core = receiver.core.clone();
    let header = ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?;
    let (lifetime, observer) = LifetimeOwner::new(header.koid(), thread.process.funding_pool())?;
    let metadata =
        super::resources::MetadataCharge::arc::<MailboxSender>(&thread.process.funding_pool())?;
    let sender: ObjectRef = Arc::try_new(MailboxSender {
        holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
        header,
        badge,
        core: core.clone(),
        _lifetime: lifetime,
        sender_count: SenderCount::new(core.clone())?,
        _metadata: metadata,
    })
    .map_err(|_| SystemCallError::OutOfMemory)?;
    // 计入发生在 mint（authority 已存在）时：安装失败的 release 经 on_last 对称扣减。
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
            return Err(e);
        }
    };
    let caps = [sender_cap, observer_cap];
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
    let (authority, parent_authority) = Authorization::accept_with(
        thread,
        source,
        Rights::DUPLICATE,
        |object, source_rights| {
            if !rights.is_subset_of(source_rights) {
                return Err(SystemCallError::RightsDenied);
            }
            let sender = object
                .as_any()
                .downcast_ref::<MailboxSender>()
                .ok_or(SystemCallError::WrongObjectType)?;
            sender._lifetime.derive()
        },
    )?;
    let sender = authority
        .object()
        .as_any()
        .downcast_ref::<MailboxSender>()
        .ok_or(SystemCallError::WrongObjectType)?;
    let core = sender.core.clone();
    let badge = sender.badge;
    let observation = ReplyObservation::new(thread.process.funding_pool())?;
    let metadata =
        super::resources::MetadataCharge::arc::<MailboxOnceSender>(&thread.process.funding_pool())?;
    let once = Arc::try_new(MailboxOnceSender {
        holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
        header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
        badge,
        core,
        observation: observation.clone(),
        state: AtomicU8::new(ONCE_PREPARED),
        parent_authority,
        _metadata: metadata,
    })
    .map_err(|_| SystemCallError::OutOfMemory)?;
    observation
        .once_id
        .0
        .store(once.header.koid(), Ordering::Relaxed);
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
    let caps = [once_cap, observation_cap];
    handle::install(
        thread,
        caps,
        |handles| {
            let result = HandlePair {
                owner: handles[0],
                peer: handles[1],
            };
            uaccess::write_user(thread, output, &result)
        },
        || {
            once.arm();
        },
    )
    .map(|_| ())
}

struct SendInput {
    header: SendHeader,
    payload: MetadataBuffer<u8>,
    moves: MetadataBuffer<(Handle, Rights)>,
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
    let pool = thread.process.funding_pool();
    let funding = MetadataFunding(&pool);
    let mut payload =
        MetadataBuffer::try_empty(payload_len, &funding).map_err(map_storage_error)?;
    for _ in 0..payload_len {
        payload.push(0);
    }
    let move_bytes = move_count
        .checked_mul(core::mem::size_of::<HandleMove>())
        .ok_or(SystemCallError::IllegalArgument)?;
    let mut raw_moves =
        MetadataBuffer::try_empty(move_bytes, &funding).map_err(map_storage_error)?;
    for _ in 0..move_bytes {
        raw_moves.push(0);
    }
    let header: SendHeader = {
        let body = thread
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let mut vm = body.vm.space.lock();
        let space = vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?;
        // SAFETY: SendHeader 仅含整数且无 padding，任意位型有效。
        let header = unsafe { uaccess::read_user_value(space, header_ptr) }?;
        uaccess::copy_from_user(space, &mut payload, payload_ptr)?;
        uaccess::copy_from_user(space, &mut raw_moves, moves_ptr)?;
        header
    };
    if header.payload_len as usize != payload_len
        || header.handle_count as usize != move_count
        || header.reserved != [0; 3]
    {
        return Err(SystemCallError::IllegalArgument);
    }
    let mut moves = MetadataBuffer::try_empty(move_count, &funding).map_err(map_storage_error)?;
    for bytes in raw_moves
        .as_chunks::<{ core::mem::size_of::<HandleMove>() }>()
        .0
    {
        // SAFETY: HandleMove 仅含整数 newtype；缓冲可能不对齐，故 unaligned 读。
        let item = unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<HandleMove>()) };
        if moves.iter().any(|(handle, _)| *handle == item.handle) {
            return Err(SystemCallError::IllegalArgument);
        }
        moves.push((item.handle, item.rights));
    }
    Ok(SendInput {
        header,
        payload,
        moves,
    })
}

/// Send：普通发送在表内接受独立 use，Once 与消息移交独占 claim；
/// core 锁仲裁 open/容量与入队，Once 的 Pending→Sent 与入队同一临界区。
pub fn send(
    thread: &Thread,
    sender_handle: Handle,
    header_ptr: usize,
    payload_ptr: usize,
    moves_ptr: usize,
    move_count: usize,
    payload_len: usize,
) -> Result<(), SystemCallError> {
    let input = read_send_input(
        thread,
        header_ptr,
        payload_ptr,
        moves_ptr,
        move_count,
        payload_len,
    )?;
    if input
        .moves
        .iter()
        .any(|(handle, _)| *handle == sender_handle)
    {
        return Err(SystemCallError::IllegalArgument);
    }
    let (authorization, usage) =
        Authorization::accept_with(thread, sender_handle, Rights::WRITE, |object, _| {
            if let Some(sender) = object.as_any().downcast_ref::<MailboxSender>() {
                sender._lifetime.accept().map(Some)
            } else if object.kind() == ObjectKind::MailboxOnceSender {
                // 一次性额度只由随后真正取得的消费 claim 使用。
                Ok(None)
            } else {
                Err(SystemCallError::WrongObjectType)
            }
        })?;
    let mut invocation = if usage.is_none() {
        Some(Invocation::claim(thread, sender_handle, Rights::WRITE)?)
    } else {
        None
    };
    let object = invocation
        .as_ref()
        .map_or(authorization.object(), Invocation::object)
        .clone();
    let send_once = invocation.is_some();
    let (core, badge, usage) = if let Some(usage) = usage {
        let sender = object
            .as_any()
            .downcast_ref::<MailboxSender>()
            .expect("accepted Sender changed kind");
        (sender.core.clone(), sender.badge, usage)
    } else {
        let once = object
            .as_any()
            .downcast_ref::<MailboxOnceSender>()
            .ok_or(SystemCallError::WrongObjectType)?;
        if once.state.load(Ordering::Relaxed) != ONCE_PENDING {
            return Err(SystemCallError::ObjectClosed);
        }
        (
            once.core.clone(),
            once.badge,
            once.parent_authority.accept()?,
        )
    };
    let pool = thread.process.funding_pool();
    let funding = MetadataFunding(&pool);
    let mut claims =
        MetadataBuffer::try_empty(input.moves.len(), &funding).map_err(map_storage_error)?;
    // 随消息付款并运输接收所需的 Delivery 槽，出队后不再增长该数组。
    let mut handles =
        MetadataBuffer::try_empty(input.moves.len() + 1, &funding).map_err(map_storage_error)?;
    let mut rights =
        MetadataBuffer::try_empty(input.moves.len(), &funding).map_err(map_storage_error)?;
    let delivery = super::delivery::Delivery::create(
        object.header().koid(),
        usage,
        thread.process.funding_pool(),
    )?;
    let delivery = handle::mint(delivery, Rights::TRANSIT | Rights::GRANT)?;
    for (handle, wanted) in input.moves.iter() {
        match Invocation::claim(thread, *handle, Rights::TRANSIT) {
            Ok(claim) if wanted.is_known() && wanted.is_subset_of(claim.cap().rights()) => {
                rights.push(*wanted);
                claims.push(claim);
            }
            Ok(_) => {
                super::cap::deliver(delivery.release());
                return Err(SystemCallError::RightsDenied);
            }
            Err(error) => {
                super::cap::deliver(delivery.release());
                return Err(error);
            }
        }
    }
    for claim in claims.iter_mut() {
        handles.push(claim.take_for_transfer());
    }
    let mut header = MessageHeader::new(
        thread.process.pid,
        badge,
        input.header.kind,
        input.payload.len() as u32,
        handles.len() as u32,
    );
    header.sender_context_id = object.header().koid();
    let result = core.enqueue(
        Message {
            header,
            payload: input.payload,
            delivery,
            handles,
        },
        &rights,
        input.header.deadline,
        |state| {
            if send_once {
                object
                    .as_any()
                    .downcast_ref::<MailboxOnceSender>()
                    .expect("validated OnceSender changed kind")
                    .mark_sent(state);
            }
        },
    );
    match result {
        Ok(()) => {
            // 消息已接 use/移动 authority；撤预约不受调用者 Close/Frozen 回滚。
            for claim in claims.iter_mut() {
                claim.commit_transfer();
            }
            if send_once {
                let once = object
                    .as_any()
                    .downcast_ref::<MailboxOnceSender>()
                    .expect("validated OnceSender changed kind");
                once.observation.publish(ReplyResult::Sent);
                once.parent_authority.release();
                invocation
                    .as_mut()
                    .expect("OnceSender lost claim")
                    .consume();
            }
            core.kick();
            Ok(())
        }
        Err((error, delivery, returned)) => {
            for (claim, cap) in claims.iter_mut().zip(returned) {
                claim.restore_transfer(cap);
            }
            super::cap::deliver(delivery.release());
            // Invocation Drop 同原槽返还，撤销预约的 authority 在表锁外放弃。
            Err(error)
        }
    }
}

pub fn peek(thread: &Thread, receiver: Handle, output: usize) -> Result<(), SystemCallError> {
    let (_authority, core) = receiver_core(thread, receiver, Rights::READ)?;
    let header = core.peek()?;
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
    let (authority, core) = receiver_core(thread, receiver, Rights::READ)?;
    let token = NEXT_RECEIVE.allocate().ok_or(SystemCallError::ReachLimit)?;
    let message = core.begin_receive(token, payload_capacity, handle_capacity)?;
    finish_receive(
        thread,
        authority.body(),
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
    body: &super::process::shell::BodyLease,
    core: &Arc<MailboxCore>,
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
    handles.push(delivery);
    let reserved = body.handles.lock().reserve(
        handles.len(),
        &super::resources::MetadataFunding(thread.process.pool()),
    );
    let reservation = match reserved {
        Ok(reservation) => reservation,
        Err(error) => {
            let delivery = handles.pop().expect("delivery pushed");
            return rollback(
                core,
                token,
                Message {
                    header,
                    payload,
                    handles,
                    delivery,
                },
                handle::map_error(error),
            );
        }
    };
    let slots = reservation.handles();
    let result = erhino_shared::message::ReceiveResult {
        header,
        delivery: slots[count],
    };
    let copied = {
        let mut vm = body.vm.space.lock();
        match vm.space.as_mut() {
            None => Err(SystemCallError::ObjectClosed),
            Some(space) => {
                // SAFETY: ReceiveResult/Handle 是全整数 ABI；Protected SUM 由 uaccess 拥有。
                unsafe { uaccess::deliver_output(thread, space, output.header, &result) }
                    .and_then(|_| uaccess::deliver_bytes(thread, space, output.payload, &payload))
                    .and_then(|_| {
                        // SAFETY: Handle 为透明 u64，slots 在本调用期间稳定存活。
                        let bytes = unsafe {
                            core::slice::from_raw_parts(
                                slots[..count].as_ptr().cast::<u8>(),
                                core::mem::size_of_val(&slots[..count]),
                            )
                        };
                        uaccess::deliver_bytes(thread, space, output.handles, bytes)
                    })
            }
        }
    };
    if let Err(error) = copied {
        body.handles.lock().rollback(reservation);
        let delivery = handles.pop().expect("delivery pushed");
        return rollback(
            core,
            token,
            Message {
                header,
                payload,
                handles,
                delivery,
            },
            error,
        );
    }
    let committed = body.handles.lock().commit(reservation, handles);
    // 复制完成后出队责任不可回退；shut 只撤销新槽公开，不撤销消息 Sent。
    core.commit_receive(token);
    core.kick();
    match committed {
        Ok(capability::Commit::Installed(_)) => Ok(()),
        Ok(capability::Commit::Released { events, .. }) => {
            for event in events {
                super::cap::deliver(Some(event));
            }
            Ok(())
        }
        Err((caps, error)) => {
            handle::release_all(caps);
            Err(handle::map_error(error))
        }
    }
}
fn rollback(
    core: &Arc<MailboxCore>,
    token: u64,
    message: Message,
    error: SystemCallError,
) -> Result<(), SystemCallError> {
    if let Some(message) = core.rollback_receive(token, message) {
        message.release();
    }
    core.kick();
    Err(error)
}

pub fn discard(thread: &Thread, receiver: Handle) -> Result<(), SystemCallError> {
    let (_authority, core) = receiver_core(thread, receiver, Rights::READ)?;
    core.discard()?.release();
    core.kick();
    Ok(())
}
