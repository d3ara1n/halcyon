//! 显式 Mailbox 对象：FIFO、transit Handle、READABLE 与接收预留。

use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use core::any::Any;

use erhino_shared::{
    call::SystemCallError,
    message::{
        HandleMove, MAILBOX_CAPACITY, MESSAGE_HANDLE_MAX, MessageHeader, PAYLOAD_MAX, SendHeader,
    },
    object::{Handle, HandlePair, ObjectSignals, Rights},
};

use crate::{sync::Spinlock, uaccess};

use super::{
    handle::{ProcessHandleEntry, ProcessHandleTable, close_transit},
    object::{
        HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState,
        SubscribeResult,
    },
    proc::Process,
    Thread,
    wait::{Subscription, finish_offered},
};

pub struct Message {
    pub header: MessageHeader,
    pub payload: Vec<u8>,
    pub handles: Vec<ProcessHandleEntry>,
}

impl Message {
    pub fn close_transit_handles(self) {
        for handle in self.handles {
            close_transit(handle);
        }
    }
}

struct MailboxState {
    wait: ObjectWaitState,
    queue: VecDeque<Message>,
    receiving: Option<u64>,
    closed: bool,
}

impl MailboxState {
    /// 电平是状态的函数：READABLE ⇔ 队列非空，WRITABLE ⇔ 占用（队列加
    /// 在逯接收占位）低于容量，CLOSED 终态独占。所有迁移点调用同一发布
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
        if !self.queue.is_empty() {
            level |= ObjectSignals::READABLE;
        }
        if occupied < MAILBOX_CAPACITY {
            level |= ObjectSignals::WRITABLE;
        }
        self.wait.update(ObjectSignals::READABLE | ObjectSignals::WRITABLE, level);
    }
}

pub struct Mailbox {
    #[expect(dead_code, reason = "KernelObject 共同头供后续对象诊断使用")]
    header: ObjectHeader,
    state: Spinlock<MailboxState>,
}

impl Mailbox {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            header: ObjectHeader::new(),
            state: Spinlock::new(MailboxState {
                // 空箱对 sender 可写；WRITABLE 电平由容量变化维护。
                wait: ObjectWaitState::new(ObjectSignals::WRITABLE),
                queue: VecDeque::new(),
                receiving: None,
                closed: false,
            }),
        })
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
    ) -> Result<(), SystemCallError> {
        let mut state = self.state.lock();
        if state.closed {
            return Err(SystemCallError::ObjectClosed);
        }
        let occupied = state.queue.len() + usize::from(state.receiving.is_some());
        if occupied >= MAILBOX_CAPACITY {
            return Err(SystemCallError::MailboxFull);
        }
        state.queue.try_reserve(1).map_err(|_| SystemCallError::OutOfMemory)?;
        let handles = table.extract_moves(moves).map_err(super::handle::map_error)?;
        state.queue.push_back(Message { header, payload, handles });
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
        state.queue.front().map(|message| message.header).ok_or(SystemCallError::ObjectNotAvailable)
    }

    /// 调用方已持 HandleTable 锁；本方法再取 Mailbox 锁并原子预留 slots/队头。
    /// 已知简化：事务窗口内不重发布电平（READABLE 乐观保持，ObjectBusy
    /// 兜底）；用户态多线程落地后评估事务内降级
    /// （notes/impls/ipc.md「消息与 Notification」）。
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
        let Some(front) = state.queue.front() else {
            return Err(SystemCallError::ObjectNotAvailable);
        };
        if payload_capacity < front.payload.len() || handle_capacity < front.handles.len() {
            return Err(SystemCallError::BufferTooSmall);
        }
        let reservation = table
            .reserve(front.handles.len(), token)
            .map_err(super::handle::map_error)?;
        state.receiving = Some(token);
        Ok((reservation, state.queue.pop_front().expect("front was checked")))
    }

    pub fn commit_receive(&self, token: u64) {
        let mut state = self.state.lock();
        assert!(state.receiving == Some(token), "mailbox receive token mismatch");
        state.receiving = None;
        // owner 关闭后终态冻结由 update 保证，此处无需防御。
        state.publish();
    }

    /// 返回 Some 表示 owner 已关闭，消息不得重新入队，调用方须关闭 transit。
    pub fn rollback_receive(&self, token: u64, message: Message) -> Option<Message> {
        let mut state = self.state.lock();
        assert!(state.receiving == Some(token), "mailbox receive token mismatch");
        state.receiving = None;
        if state.closed {
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
        let message = state.queue.pop_front().ok_or(SystemCallError::ObjectNotAvailable)?;
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
        loop {
            let context = self.state.lock().wait.take_completer();
            let Some(context) = context else { break };
            finish_offered(context);
        }
    }
}

impl KernelObject for Mailbox {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::Mailbox
    }

    fn allowed_rights(&self, role: HandleRole) -> Option<Rights> {
        match role {
            HandleRole::MailboxOwner => Some(Rights::READ | Rights::WAIT | Rights::MANAGE),
            HandleRole::MailboxSender => {
                Some(Rights::WRITE | Rights::WAIT | Rights::TRANSFER | Rights::DUPLICATE)
            }
            HandleRole::MailboxSenderOnce => Some(Rights::WRITE | Rights::WAIT | Rights::TRANSFER),
            _ => None,
        }
    }

    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals> {
        match role {
            HandleRole::MailboxOwner => Some(ObjectSignals::READABLE | ObjectSignals::CLOSED),
            HandleRole::MailboxSender | HandleRole::MailboxSenderOnce => {
                Some(ObjectSignals::WRITABLE | ObjectSignals::CLOSED)
            }
            _ => None,
        }
    }

    fn signals(&self) -> ObjectSignals {
        self.state.lock().wait.signals()
    }

    fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
        self.state.lock().wait.subscribe(subscription)
    }

    fn unsubscribe(&self, id: u64) {
        self.state.lock().wait.unsubscribe(id);
    }

    fn close_handle(&self, role: HandleRole, _owner: &Process, _exiting: bool) {
        if role == HandleRole::MailboxOwner {
            self.close_owner();
        }
    }

    fn close_transit(&self, role: HandleRole) {
        debug_assert!(matches!(
            role,
            HandleRole::MailboxSender | HandleRole::MailboxSenderOnce
        ));
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn create(
    thread: &Thread,
    owner_rights: Rights,
    sender_rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let mailbox = Mailbox::new();
    let object = Mailbox::object_ref(&mailbox);
    let mut entries = Vec::new();
    entries.try_reserve(2).map_err(|_| SystemCallError::OutOfMemory)?;
    entries.push(
        super::handle::entry(object.clone(), HandleRole::MailboxOwner, owner_rights)
            .map_err(super::handle::map_error)?,
    );
    entries.push(
        super::handle::entry(object, HandleRole::MailboxSender, sender_rights)
            .map_err(super::handle::map_error)?,
    );

    let token = super::handle::transaction_token();
    let mut table = thread.process.handles.lock();
    let reservation = table.reserve(2, token).map_err(super::handle::map_error)?;
    let pair = HandlePair::new(reservation.handles()[0], reservation.handles()[1]);
    let mut space = thread.process.space.lock();
    if let Err(error) = space.check_range(output, core::mem::size_of::<HandlePair>(), true) {
        drop(space);
        table.rollback(reservation).expect("MailboxCreate reservation must remain owned");
        return Err(error.into());
    }
    // SAFETY: HandlePair 无 padding，输出已在同一 space 锁下校验。
    unsafe { uaccess::write_user_value(&mut space, output, &pair) }
        .expect("validated MailboxCreate output must remain writable");
    drop(space);
    table.commit(reservation, entries).expect("MailboxCreate reservation must remain owned");
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
    let token = super::handle::transaction_token();
    let mut table = thread.process.handles.lock();
    let source_entry = table.get(source, Rights::DUPLICATE).map_err(super::handle::map_error)?;
    if *source_entry.role() != HandleRole::MailboxSender
        || source_entry.object().kind() != ObjectKind::Mailbox
    {
        return Err(SystemCallError::WrongObjectType);
    }
    if !rights.is_subset_of(source_entry.rights()) {
        return Err(SystemCallError::RightsDenied);
    }
    // 所有可失败步骤先于预留：entry 构造与分配失败时不产生任何表状态。
    let mut entries = Vec::new();
    entries.try_reserve_exact(1).map_err(|_| SystemCallError::OutOfMemory)?;
    entries.push(super::handle::entry(
        source_entry.object().clone(),
        HandleRole::MailboxSenderOnce,
        rights,
    )
    .map_err(super::handle::map_error)?);
    let reservation = table.reserve(1, token).map_err(super::handle::map_error)?;
    let once = reservation.handles()[0];
    let mut space = thread.process.space.lock();
    if let Err(error) = space.check_range(output, core::mem::size_of::<Handle>(), true) {
        drop(space);
        table.rollback(reservation).expect("make-send-once reservation must remain owned");
        return Err(error.into());
    }
    // SAFETY: Handle 无 padding，输出已在同一 space 锁下校验。
    unsafe { uaccess::write_user_value(&mut space, output, &once) }
        .expect("validated make-send-once output must remain writable");
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
    payload.try_reserve_exact(payload_len).map_err(|_| SystemCallError::OutOfMemory)?;
    payload.resize(payload_len, 0);
    let move_bytes = move_count
        .checked_mul(core::mem::size_of::<HandleMove>())
        .ok_or(SystemCallError::IllegalArgument)?;
    let mut raw_moves = Vec::new();
    raw_moves.try_reserve_exact(move_bytes).map_err(|_| SystemCallError::OutOfMemory)?;
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
        || header.reserved != [0; 5]
    {
        return Err(SystemCallError::IllegalArgument);
    }

    let mut moves = Vec::new();
    moves.try_reserve_exact(move_count).map_err(|_| SystemCallError::OutOfMemory)?;
    for bytes in raw_moves.chunks_exact(core::mem::size_of::<HandleMove>()) {
        // SAFETY: HandleMove 仅含整数 newtype；缓冲可能不对齐，故 unaligned 读。
        let item = unsafe { core::ptr::read_unaligned(bytes.as_ptr().cast::<HandleMove>()) };
        moves.push((item.handle, item.rights));
    }

    let message_header = MessageHeader::new(
        thread.process.pid as u64,
        header.kind,
        header.payload_len,
        header.handle_count,
    );
    let object = {
        let mut table = thread.process.handles.lock();
        // 解析与入队同临界区：MailboxSenderOnce 的消费与投递原子化，
        // 并发线程无法在解析后、入队前摘除一次性项。
        let entry = table.get(mailbox_handle, Rights::WRITE).map_err(super::handle::map_error)?;
        let once = match *entry.role() {
            HandleRole::MailboxSender | HandleRole::MailboxSenderOnce => {
                *entry.role() == HandleRole::MailboxSenderOnce
            }
            _ => return Err(SystemCallError::WrongObjectType),
        };
        let object = entry.object().clone();
        if object.kind() != ObjectKind::Mailbox {
            return Err(SystemCallError::WrongObjectType);
        }
        let mailbox = concrete(&object)?;
        mailbox.enqueue_with(&mut table, &moves, message_header, payload)?;
        if once {
            // 消费式 role：成功投递即摘除源项（消费而非关闭，不执行
            // lifecycle callback）。若该项同时作为 transit move 进入了
            // 本条消息，extract_moves 已先行摘除，remove 的两种结果
            // （Ok(entry) / Err(StaleHandle)）都已消费。
            drop(table.remove(mailbox_handle));
        }
        object
    };
    let mailbox = concrete(&object)?;
    mailbox.finish_waiters();
    Ok(())
}

pub fn peek(
    thread: &Thread,
    mailbox_handle: Handle,
    output: usize,
) -> Result<(), SystemCallError> {
    let object = resolve(thread, mailbox_handle, Rights::READ, HandleRole::MailboxOwner)?;
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
        space.check_range(header_output, core::mem::size_of::<MessageHeader>(), true)?;
        space.check_range(payload_output, payload_capacity, true)?;
        space.check_range(handles_output, handle_output_bytes, true)?;
    }

    let object = resolve(thread, mailbox_handle, Rights::READ, HandleRole::MailboxOwner)?;
    let mailbox = concrete(&object)?;
    let token = super::handle::transaction_token();
    let (reservation, message) = {
        let mut table = thread.process.handles.lock();
        mailbox.begin_receive(&mut table, token, payload_capacity, handle_capacity)?
    };

    let output_handles = reservation.handles();
    let copied = {
        let mut space = thread.process.space.lock();
        // SAFETY: MessageHeader 无 padding，Handle 是 u64 newtype。
        let header_result = unsafe {
            uaccess::write_user_value(&mut space, header_output, &message.header)
        };
        header_result
            .and_then(|_| uaccess::copy_to_user(&mut space, payload_output, &message.payload))
            .and_then(|_| {
                let bytes = unsafe {
                    core::slice::from_raw_parts(
                        output_handles.as_ptr().cast::<u8>(),
                        core::mem::size_of_val(output_handles),
                    )
                };
                uaccess::copy_to_user(&mut space, handles_output, bytes)
            })
    };

    if let Err(error) = copied {
        let rejected = {
            let mut table = thread.process.handles.lock();
            table.rollback(reservation).expect("Receive reservation must remain owned");
            mailbox.rollback_receive(token, message)
        };
        if let Some(message) = rejected {
            message.close_transit_handles();
        }
        return Err(error.into());
    }

    let Message { handles, .. } = message;
    {
        let mut table = thread.process.handles.lock();
        table.commit(reservation, handles).expect("Receive reservation must remain owned");
        mailbox.commit_receive(token);
    }
    // 腾出容量后唤醒等待 WRITABLE 的发送者。
    mailbox.finish_waiters();
    Ok(())
}

pub fn discard(thread: &Thread, mailbox_handle: Handle) -> Result<(), SystemCallError> {
    let object = resolve(thread, mailbox_handle, Rights::READ, HandleRole::MailboxOwner)?;
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
    let entry = table.get(handle, rights).map_err(super::handle::map_error)?;
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
