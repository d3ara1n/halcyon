//! 显式 Mailbox 消息封装（契约见 notes/ideas/message.md）。
//! Send/Receive 都是非阻塞事务；阻塞只由 WaitMany 组合完成。

use alloc::vec::Vec;

use erhino_shared::{
    call::SystemCallError,
    message::{HandleMove, MailboxBadge, MessageHeader},
    object::{Handle, HandlePair, ObjectSignals, Rights},
    wait::{WaitItem, WaitResult, WAIT_DEADLINE_INFINITE},
};

use crate::call::{
    sys_discard, sys_mailbox_create, sys_mailbox_make_send_once, sys_mailbox_mint_sender, sys_peek,
    sys_receive, sys_send, sys_wait_many,
};

pub struct ReceivedMessage {
    pub header: MessageHeader,
    pub payload: Vec<u8>,
    pub handles: Vec<Handle>,
}

pub fn create(
    owner_rights: Rights,
    sender_rights: Rights,
) -> Result<HandlePair, SystemCallError> {
    let mut output = HandlePair::new(Handle::INVALID, Handle::INVALID);
    // SAFETY: output 在 ecall 期间有效且可写。
    unsafe { sys_mailbox_create(owner_rights, sender_rights, &mut output)? };
    Ok(output)
}

/// 向显式 sender Handle 投递负载和 Handle moves。永不阻塞。
pub fn send(
    mailbox: Handle,
    kind: u64,
    payload: &[u8],
    moves: &[HandleMove],
) -> Result<(), SystemCallError> {
    // SAFETY: wrapper 仅在 ecall 期间借用切片。
    unsafe { sys_send(mailbox, kind, payload, moves) }
}

/// 非阻塞观察队头。空箱返回 ObjectNotAvailable。
pub fn peek(mailbox: Handle) -> Result<MessageHeader, SystemCallError> {
    let mut header = MessageHeader::new(0, 0, 0, 0, 0);
    // SAFETY: output 在 ecall 期间有效且可写。
    unsafe { sys_peek(mailbox, &mut header)? };
    Ok(header)
}

/// 非阻塞原子接收队头及其 Handles。
pub fn receive(mailbox: Handle) -> Result<ReceivedMessage, SystemCallError> {
    let header = peek(mailbox)?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(header.payload_len as usize)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    payload.resize(header.payload_len as usize, 0);
    let mut handles = Vec::new();
    handles
        .try_reserve_exact(header.handle_count as usize)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    handles.resize(header.handle_count as usize, Handle::INVALID);
    let mut received = MessageHeader::new(0, 0, 0, 0, 0);
    // SAFETY: 三个输出缓冲在 ecall 期间保持有效且容量与切片一致。
    unsafe { sys_receive(mailbox, &mut received, &mut payload, &mut handles)? };
    Ok(ReceivedMessage {
        header: received,
        payload,
        handles,
    })
}

/// 丢弃队头；消息携带的 transit Handles 由内核关闭。
pub fn discard(mailbox: Handle) -> Result<(), SystemCallError> {
    // SAFETY: Handle 是值参数。
    unsafe { sys_discard(mailbox) }
}

/// 从具 DUPLICATE 权的 sender 派生一次性投递权：承载一条消息后由内核
/// 摘除，经消息转移后由接收方继续一次性使用（Mach send-once 对应物）。
pub fn make_send_once(
    source: Handle,
    rights: Rights,
) -> Result<Handle, SystemCallError> {
    let mut output = Handle::INVALID;
    // SAFETY: output 在 ecall 期间有效且可写。
    unsafe { sys_mailbox_make_send_once(source, rights, &mut output)? };
    Ok(output)
}

/// 由 mailbox owner 铸造带不可变 badge 的 sender capability。
pub fn mint_sender(
    owner: Handle,
    badge: MailboxBadge,
    rights: Rights,
) -> Result<Handle, SystemCallError> {
    let mut output = Handle::INVALID;
    // SAFETY: output 在 ecall 期间有效且可写。
    unsafe { sys_mailbox_mint_sender(owner, badge, rights, &mut output)? };
    Ok(output)
}

/// 阻塞投递：满箱时等待 WRITABLE 电平（或 CLOSED 出错）后重试。
/// 与 [`wait_message`] 对偶，构成发送侧流控闭环。
pub fn send_blocking(
    mailbox: Handle,
    kind: u64,
    payload: &[u8],
    moves: &[HandleMove],
) -> Result<(), SystemCallError> {
    loop {
        match send(mailbox, kind, payload, moves) {
            Ok(()) => return Ok(()),
            Err(SystemCallError::MailboxFull) => {}
            Err(error) => return Err(error),
        }
        let items = [WaitItem::new(
            mailbox,
            ObjectSignals::WRITABLE | ObjectSignals::CLOSED,
            0,
        )];
        let mut result = WaitResult::new(
            0,
            ObjectSignals::NONE,
            0,
            erhino_shared::wait::WaitReason::Signaled,
        );
        // SAFETY: 输入和输出在阻塞 syscall 完成前都位于当前栈帧。
        unsafe { sys_wait_many(&items, &mut result, WAIT_DEADLINE_INFINITE)? };
        if result.observed.intersects(ObjectSignals::CLOSED)
            && !result.observed.intersects(ObjectSignals::WRITABLE)
        {
            return Err(SystemCallError::ObjectClosed);
        }
    }
}

/// 阻塞取得下一条消息：先尝试 Receive，空箱才等待 READABLE 并回环。
pub fn wait_message(mailbox: Handle) -> Result<ReceivedMessage, SystemCallError> {
    loop {
        match receive(mailbox) {
            Ok(message) => return Ok(message),
            Err(SystemCallError::ObjectNotAvailable) => {}
            Err(error) => return Err(error),
        }
        let items = [WaitItem::new(
            mailbox,
            ObjectSignals::READABLE | ObjectSignals::CLOSED,
            0,
        )];
        let mut result = WaitResult::new(
            0,
            ObjectSignals::NONE,
            0,
            erhino_shared::wait::WaitReason::Signaled,
        );
        // SAFETY: 输入和输出在阻塞 syscall 完成前都位于当前栈帧。
        unsafe { sys_wait_many(&items, &mut result, WAIT_DEADLINE_INFINITE)? };
        if result.observed.intersects(ObjectSignals::CLOSED)
            && !result.observed.intersects(ObjectSignals::READABLE)
        {
            return Err(SystemCallError::ObjectClosed);
        }
    }
}
