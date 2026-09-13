//! 显式 Mailbox 对象：FIFO、transit Handle、READABLE 与接收预留。

use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use core::any::Any;

use erhino_shared::{
    call::SystemCallError,
    message::{
        HandleMove, MAILBOX_CAPACITY, MESSAGE_HANDLE_MAX, MailboxBadge, MessageHeader, PAYLOAD_MAX,
        SendHeader,
    },
    object::{Handle, ObjectSignals, Rights},
};

use crate::{sync::Spinlock, uaccess};

use super::{
    Thread,
    handle::{ProcessHandleEntry, ProcessHandleTable, close_transit},
    lifetime::LifetimeOwner,
    object::{
        HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState,
        SubscribeResult,
    },
    proc::Process,
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

pub struct Message {
    pub header: MessageHeader,
    pub payload: Vec<u8>,
    pub handles: Vec<ProcessHandleEntry>,
    pub delivery: ProcessHandleEntry,
}

impl Message {
    pub fn close_transit_handles(self) {
        for handle in self.handles {
            close_transit(handle);
        }
        close_transit(self.delivery);
    }
}

struct MailboxState {
    wait: ObjectWaitState,
    queue: VecDeque<Message>,
    receiving: Option<u64>,
    closed: bool,
}

impl MailboxState {
    /// 电平是状态的函数：READABLE ⇔ 无接收预留且队列非空，WRITABLE ⇔
    /// 占用（队列加在途接收占位）低于容量，CLOSED 终态独占。所有迁移点调用同一发布
    /// 函数，不做增量转移——新增迁移点不可能遗漏或漂移。
    fn publish(&mut self) {
        if self.closed {
            self.wait.update(
                ObjectSignals::READABLE | ObjectSignals::WRITABLE,
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
        self.wait
            .update(ObjectSignals::READABLE | ObjectSignals::WRITABLE, level);
    }
}

pub struct Mailbox {
    header: ObjectHeader,
    state: Spinlock<MailboxState>,
    _permit: IpcPermit,
}

/// 一个可复制/转交的授权实例，badge 与目标队列都不可变。
pub struct MailboxSender {
    header: ObjectHeader,
    badge: MailboxBadge,
    queue: ObjectRef,
    _lifetime: LifetimeOwner,
    _permit: IpcPermit,
}

impl MailboxSender {
    fn create(
        queue: ObjectRef,
        badge: MailboxBadge,
        sponsor: &Arc<MetadataSponsor>,
    ) -> Result<(ObjectRef, ObjectRef), SystemCallError> {
        let header = ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?;
        let (lifetime, observer) = LifetimeOwner::new(header.koid(), sponsor)?;
        let sender = Arc::try_new(Self {
            header,
            badge,
            queue,
            _lifetime: lifetime,
            _permit: MetadataSponsor::reserve_ipc(sponsor, super::resources::IpcClass::Object)?,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok((sender, observer))
    }
}

impl KernelObject for MailboxSender {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::MailboxSender
    }
    fn related_id(&self) -> u64 {
        self.queue.header().koid()
    }
    fn badge(&self) -> u64 {
        self.badge
    }
    fn observation_source(&self) -> Option<ObjectRef> {
        Some(self.queue.clone())
    }
    fn allowed_rights(&self, role: HandleRole) -> Option<Rights> {
        match role {
            HandleRole::MailboxSender => Some(
                Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::GRANT | Rights::DUPLICATE,
            ),
            HandleRole::MailboxSenderOnce => {
                Some(Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::GRANT)
            }
            _ => None,
        }
    }
    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals> {
        matches!(
            role,
            HandleRole::MailboxSender | HandleRole::MailboxSenderOnce
        )
        .then_some(ObjectSignals::WRITABLE | ObjectSignals::CLOSED)
    }
    fn close_handle(&self, _: HandleRole, _: &Process, _: bool) {}
    fn close_transit(&self, _: HandleRole) {}
    fn as_any(&self) -> &dyn Any {
        self
    }
}

impl Mailbox {
    pub fn new(sponsor: &Arc<MetadataSponsor>) -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            state: Spinlock::new(
                crate::sync::ranks::MAILBOX,
                MailboxState {
                    // 空箱对 sender 可写；WRITABLE 电平由容量变化维护。
                    wait: ObjectWaitState::new(ObjectSignals::WRITABLE),
                    queue: VecDeque::new(),
                    receiving: None,
                    closed: false,
                },
            ),
            _permit: MetadataSponsor::reserve_ipc(sponsor, super::resources::IpcClass::Object)?,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub fn object_ref(this: &Arc<Self>) -> ObjectRef {
        this.clone()
    }

    /// 调用方已持 HandleTable 锁；本方法只再取 Mailbox 锁。
    pub fn enqueue_with(
        &self,
        table: &mut super::handle::ProcessHandleTable,
        moves: &[(erhino_shared::object::Handle, Rights)],
        header: MessageHeader,
        payload: Vec<u8>,
        deadline: erhino_shared::time::Deadline,
        delivery: ProcessHandleEntry,
    ) -> Result<(), SystemCallError> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(SystemCallError::ObjectClosed);
        }
        let occupied = state.queue.len() + usize::from(state.receiving.is_some());
        if occupied >= MAILBOX_CAPACITY {
            return Err(SystemCallError::MailboxFull);
        }
        state
            .queue
            .try_reserve(1)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        let prepared = table
            .prepare_extract_moves(moves)
            .map_err(super::handle::map_error)?;
        crate::clock::check_delivery(deadline)?;
        let handles = prepared.commit();
        state.queue.push_back(Message {
            header,
            payload,
            handles,
            delivery,
        });
        state.publish();
        Ok(())
    }

    pub fn peek(&self) -> Result<MessageHeader, SystemCallError> {
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

    /// 调用方已持 HandleTable 锁；本方法再取 Mailbox 锁并原子预留 slots/队头。
    pub fn begin_receive(
        &self,
        table: &mut ProcessHandleTable,
        token: u64,
        payload_capacity: usize,
        handle_capacity: usize,
    ) -> Result<(handle_table::Reservation, Message), SystemCallError> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(SystemCallError::ObjectClosed);
        }
        if state.receiving.is_some() {
            return Err(SystemCallError::ObjectBusy);
        }
        let Some(front) = state.queue.front_mut() else {
            return Err(SystemCallError::ObjectNotAvailable);
        };
        if payload_capacity < front.payload.len() || handle_capacity < front.handles.len() {
            return Err(SystemCallError::BufferTooSmall);
        }
        front
            .handles
            .try_reserve(1)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        let reservation = table
            .reserve(front.handles.len() + 1, token)
            .map_err(super::handle::map_error)?;
        state.receiving = Some(token);
        let message = state.queue.pop_front().expect("front was checked");
        state.publish();
        Ok((reservation, message))
    }

    pub fn commit_receive(&self, token: u64) {
        let mut state = self.state.lock();
        assert!(
            state.receiving == Some(token),
            "mailbox receive token mismatch"
        );
        state.receiving = None;
        // owner 关闭后终态冻结由 update 保证，此处无需防御。
        state.publish();
    }

    /// 返回 Some 表示 owner 已关闭，消息不得重新入队，调用方须关闭 transit。
    pub fn rollback_receive(&self, token: u64, message: Message) -> Option<Message> {
        let mut state = self.state.lock();
        assert!(
            state.receiving == Some(token),
            "mailbox receive token mismatch"
        );
        state.receiving = None;
        if state.closed {
            state.publish();
            return Some(message);
        }
        state.queue.push_front(message);
        state.publish();
        None
    }

    pub fn discard(&self) -> Result<Message, SystemCallError> {
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

    fn close_owner(&self) {
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
            message.close_transit_handles();
        }
    }

    pub(crate) fn finish_waiters(&self) {
        let pending = {
            let mut state = self.state.lock();
            state.wait.take_notification()
        };
        if let Some((reservation, target)) = pending {
            reservation.publish(target);
        }
    }
}

impl KernelObject for Mailbox {
    fn complete_waiter_drain(
        &self,
        reservation: super::notify_work::Reservation,
    ) -> super::notify_work::Completion {
        self.state.lock().wait.complete_notification(reservation)
    }

    fn drain_waiters(&self, budget: usize) -> (usize, bool) {
        let mut used = 0;
        while used < budget {
            let advance = {
                let mut state = self.state.lock();
                state.wait.advance_waiter()
            };
            if advance.finish() {
                return (used, true);
            }
            used += 1;
        }
        (used, false)
    }

    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::Mailbox
    }

    fn allowed_rights(&self, role: HandleRole) -> Option<Rights> {
        match role {
            HandleRole::MailboxOwner => {
                Some(Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT)
            }
            _ => None,
        }
    }

    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals> {
        match role {
            HandleRole::MailboxOwner => Some(ObjectSignals::READABLE | ObjectSignals::CLOSED),
            _ => None,
        }
    }

    fn signals(&self) -> ObjectSignals {
        self.state.lock().wait.signals()
    }

    fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
        self.state.lock().wait.subscribe(subscription)
    }

    fn rearm_observer(&self, id: u64) -> Result<super::object::ObserverRearm, SystemCallError> {
        self.state.lock().wait.rearm_observer(id)
    }

    fn cancel_observer(&self, id: u64) -> Option<super::object::CancelledObservation> {
        self.state.lock().wait.cancel_observer(id)
    }

    fn unsubscribe(&self, id: u64) {
        let retired = self.state.lock().wait.unsubscribe(id);
        drop(retired);
    }

    fn close_handle(&self, role: HandleRole, _owner: &Process, _exiting: bool) {
        if role == HandleRole::MailboxOwner {
            self.close_owner();
        }
    }

    fn close_transit(&self, _: HandleRole) {
        unreachable!("Mailbox owner cannot enter a message")
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn create(thread: &Thread, owner_rights: Rights, output: usize) -> Result<(), SystemCallError> {
    let mailbox = Mailbox::new(thread.process.resources.metadata())?;
    let entry = super::handle::entry(
        Mailbox::object_ref(&mailbox),
        HandleRole::MailboxOwner,
        owner_rights,
    )
    .map_err(super::handle::map_error)?;
    super::handle::install_one(thread, entry, output, || ())
}

/// 由 owner 铸造同一 mailbox 的 sender capability。badge 是该 capability
/// 的不可变授权上下文，后续 duplicate、move 与 send-once 派生均保持。
pub fn mint_sender(
    thread: &Thread,
    owner: Handle,
    badge: MailboxBadge,
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let token = super::handle::transaction_token()?;
    let mut table = thread.process.handles.lock();
    let owner_entry = table
        .get(owner, Rights::MANAGE)
        .map_err(super::handle::map_error)?;
    if *owner_entry.role() != HandleRole::MailboxOwner
        || owner_entry.object().kind() != ObjectKind::Mailbox
    {
        return Err(SystemCallError::WrongObjectType);
    }
    let queue = owner_entry.object().clone();
    let (sender, lifetime) =
        MailboxSender::create(queue, badge, thread.process.resources.metadata())?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(2)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    entries.push(
        super::handle::entry(sender, HandleRole::MailboxSender, rights)
            .map_err(super::handle::map_error)?,
    );
    entries.push(
        super::handle::entry(
            lifetime,
            HandleRole::LifetimeObserver,
            Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT,
        )
        .map_err(super::handle::map_error)?,
    );
    let reservation = table.reserve(2, token).map_err(super::handle::map_error)?;
    let result = erhino_shared::object::SenderResult {
        sender: reservation.handles()[0],
        lifetime: reservation.handles()[1],
    };
    let mut space = thread.process.space.lock();
    if let Err(error) = space.check_range(output, core::mem::size_of_val(&result), true) {
        drop(space);
        table
            .rollback(reservation)
            .expect("mint-sender reservation must remain owned");
        return Err(error.into());
    }
    // SAFETY: Handle 无 padding；复检失败即杀本进程（deliver_output），
    // 未提交的预留随进程消亡。
    unsafe { uaccess::deliver_output(thread, &mut space, output, &result) }?;
    drop(space);
    table
        .commit(reservation, entries)
        .expect("mint-sender reservation must remain owned");
    Ok(())
}

/// 从具 DUPLICATE 权的 sender 派生一次性投递权：role 换为 MailboxSenderOnce，
/// 请求 rights 必须同时是源项与 role 允许集的子集，否则拒绝（与
/// HandleDuplicate 同判：不截剪、不放大）。
pub fn make_send_once(
    thread: &Thread,
    source: Handle,
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let token = super::handle::transaction_token()?;
    let mut table = thread.process.handles.lock();
    let source_entry = table
        .get(source, Rights::DUPLICATE)
        .map_err(super::handle::map_error)?;
    if *source_entry.role() != HandleRole::MailboxSender
        || source_entry.object().kind() != ObjectKind::MailboxSender
    {
        return Err(SystemCallError::WrongObjectType);
    }
    if !rights.is_subset_of(source_entry.rights()) {
        return Err(SystemCallError::RightsDenied);
    }
    let object = source_entry.object().clone();
    // 所有可失败步骤先于预留：entry 构造与分配失败时不产生任何表状态。
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(1)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    entries.push(
        super::handle::entry(object, HandleRole::MailboxSenderOnce, rights)
            .map_err(super::handle::map_error)?,
    );
    let reservation = table.reserve(1, token).map_err(super::handle::map_error)?;
    let once = reservation.handles()[0];
    let mut space = thread.process.space.lock();
    if let Err(error) = space.check_range(output, core::mem::size_of::<Handle>(), true) {
        drop(space);
        table
            .rollback(reservation)
            .expect("make-send-once reservation must remain owned");
        return Err(error.into());
    }
    // SAFETY: Handle 无 padding；复检失败即杀本进程（deliver_output），
    // 未提交的预留随进程消亡。
    unsafe { uaccess::deliver_output(thread, &mut space, output, &once) }?;
    drop(space);
    table
        .commit(reservation, entries)
        .expect("make-send-once reservation must remain owned");
    Ok(())
}

pub fn send(
    thread: &Thread,
    mailbox_handle: Handle,
    header_ptr: usize,
    payload_ptr: usize,
    moves_ptr: usize,
    move_count: usize,
    payload_len: usize,
) -> Result<(), SystemCallError> {
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

    let object = {
        let mut table = thread.process.handles.lock();
        // 解析与入队同临界区：MailboxSenderOnce 的消费与投递原子化，
        // 并发线程无法在解析后、入队前摘除一次性项。
        let entry = table
            .get(mailbox_handle, Rights::WRITE)
            .map_err(super::handle::map_error)?;
        let once = match *entry.role() {
            HandleRole::MailboxSender | HandleRole::MailboxSenderOnce => {
                *entry.role() == HandleRole::MailboxSenderOnce
            }
            _ => return Err(SystemCallError::WrongObjectType),
        };
        if once && moves.iter().any(|(handle, _)| *handle == mailbox_handle) {
            return Err(SystemCallError::IllegalArgument);
        }
        let context = entry.object().clone();
        let sender = context
            .as_any()
            .downcast_ref::<MailboxSender>()
            .ok_or(SystemCallError::WrongObjectType)?;
        let object = sender.queue.clone();
        let mut message_header = MessageHeader::new(
            thread.process.pid,
            sender.badge,
            header.kind,
            header.payload_len,
            header.handle_count,
        );
        message_header.sender_context_id = context.header().koid();
        let delivery =
            super::delivery::Delivery::create(context, thread.process.resources.metadata())?;
        let delivery = super::handle::entry(
            delivery,
            HandleRole::Delivery,
            Rights::TRANSIT | Rights::GRANT,
        )
        .map_err(super::handle::map_error)?;
        let mailbox = concrete(&object)?;
        mailbox.enqueue_with(
            &mut table,
            &moves,
            message_header,
            payload,
            header.deadline,
            delivery,
        )?;
        if once {
            // 消费式 role：成功投递后源项仍在表内，直接摘除且不执行
            // lifecycle callback。target 与 transit alias 已在入队前拒绝。
            table
                .remove(mailbox_handle)
                .expect("successful send-once target must remain installed");
        }
        object
    };
    let mailbox = concrete(&object)?;
    mailbox.finish_waiters();
    Ok(())
}

pub fn peek(thread: &Thread, mailbox_handle: Handle, output: usize) -> Result<(), SystemCallError> {
    let object = resolve(
        thread,
        mailbox_handle,
        Rights::READ,
        HandleRole::MailboxOwner,
    )?;
    let header = concrete(&object)?.peek()?;
    let mut space = thread.process.space.lock();
    // SAFETY: MessageHeader 所有字段与 reserved 均已初始化且无 padding。
    unsafe { uaccess::write_user_value(&mut space, output, &header) }?;
    Ok(())
}

pub fn receive(
    thread: &Thread,
    mailbox_handle: Handle,
    header_output: usize,
    payload_output: usize,
    payload_capacity: usize,
    handles_output: usize,
    handle_capacity: usize,
) -> Result<(), SystemCallError> {
    if payload_capacity > PAYLOAD_MAX || handle_capacity > MESSAGE_HANDLE_MAX {
        return Err(SystemCallError::IllegalArgument);
    }
    let handle_output_bytes = handle_capacity
        .checked_mul(core::mem::size_of::<Handle>())
        .ok_or(SystemCallError::IllegalArgument)?;
    {
        let mut space = thread.process.space.lock();
        space.check_range(
            header_output,
            core::mem::size_of::<erhino_shared::message::ReceiveResult>(),
            true,
        )?;
        space.check_range(payload_output, payload_capacity, true)?;
        space.check_range(handles_output, handle_output_bytes, true)?;
    }

    let object = resolve(
        thread,
        mailbox_handle,
        Rights::READ,
        HandleRole::MailboxOwner,
    )?;
    let mailbox = concrete(&object)?;
    let token = super::handle::transaction_token()?;
    let (reservation, message) = {
        let mut table = thread.process.handles.lock();
        mailbox.begin_receive(&mut table, token, payload_capacity, handle_capacity)?
    };

    finish_receive(
        thread,
        mailbox,
        token,
        reservation,
        message,
        ReceiveOutput {
            header: header_output,
            payload: payload_output,
            handles: handles_output,
        },
    )
}

/// 队头/目标表槽已预留，复制成功原子交付；失败先归还表槽与队头，再锁外通知。
fn finish_receive(
    thread: &Thread,
    mailbox: &Mailbox,
    token: u64,
    reservation: handle_table::Reservation,
    message: Message,
    output: ReceiveOutput,
) -> Result<(), SystemCallError> {
    let output_handles = &reservation.handles()[..message.handles.len()];
    let result = erhino_shared::message::ReceiveResult {
        header: message.header,
        delivery: *reservation
            .handles()
            .last()
            .expect("Receive has a Delivery slot"),
    };
    let copied = {
        let mut space = thread.process.space.lock();
        // SAFETY: MessageHeader 无 padding，Handle 是 u64 newtype。
        let header_result =
            unsafe { uaccess::write_user_value(&mut space, output.header, &result) };
        header_result
            .and_then(|_| uaccess::copy_to_user(&mut space, output.payload, &message.payload))
            .and_then(|_| {
                let bytes = unsafe {
                    core::slice::from_raw_parts(
                        output_handles.as_ptr().cast::<u8>(),
                        core::mem::size_of_val(output_handles),
                    )
                };
                uaccess::copy_to_user(&mut space, output.handles, bytes)
            })
    };

    if let Err(error) = copied {
        let rejected = {
            let mut table = thread.process.handles.lock();
            table
                .rollback(reservation)
                .expect("Receive reservation must remain owned");
            mailbox.rollback_receive(token, message)
        };
        mailbox.finish_waiters();
        if let Some(message) = rejected {
            message.close_transit_handles();
        }
        return Err(error.into());
    }

    let Message {
        mut handles,
        delivery,
        ..
    } = message;
    handles.push(delivery);
    {
        let mut table = thread.process.handles.lock();
        table
            .commit(reservation, handles)
            .expect("Receive reservation must remain owned");
        mailbox.commit_receive(token);
    }
    // 腾出容量后唤醒等待 WRITABLE 的发送者。
    mailbox.finish_waiters();
    Ok(())
}

pub fn discard(thread: &Thread, mailbox_handle: Handle) -> Result<(), SystemCallError> {
    let object = resolve(
        thread,
        mailbox_handle,
        Rights::READ,
        HandleRole::MailboxOwner,
    )?;
    let mailbox = concrete(&object)?;
    mailbox.discard()?.close_transit_handles();
    // 腾出容量后唤醒等待 WRITABLE 的发送者。
    mailbox.finish_waiters();
    Ok(())
}

fn resolve(
    thread: &Thread,
    handle: Handle,
    rights: Rights,
    role: HandleRole,
) -> Result<ObjectRef, SystemCallError> {
    let table = thread.process.handles.lock();
    let entry = table
        .get(handle, rights)
        .map_err(super::handle::map_error)?;
    if *entry.role() != role || entry.object().kind() != ObjectKind::Mailbox {
        return Err(SystemCallError::WrongObjectType);
    }
    Ok(entry.object().clone())
}

fn concrete(object: &ObjectRef) -> Result<&Mailbox, SystemCallError> {
    object
        .as_any()
        .downcast_ref::<Mailbox>()
        .ok_or(SystemCallError::WrongObjectType)
}
