//! Mailbox 控制面；正式运输通过 Packet，原始 move 仅在显式 unsafe 边界使用。

use super::capability::{Capability, HandleSet};
use crate::call::{
    sys_discard, sys_mailbox_create, sys_mailbox_make_send_once, sys_mailbox_mint_sender, sys_peek,
    sys_receive, sys_send,
};
use alloc::vec::Vec;
use erhino_shared::{
    call::SystemCallError,
    message::{HandleMove, MESSAGE_HANDLE_MAX, MailboxBadge, MessageHeader, PAYLOAD_MAX},
    object::{Handle, HandlePair, HandleRole, ObjectSignals, Rights, SenderResult},
    time::Deadline,
    wait::{WaitItem, WaitReason},
};

#[derive(Debug)]
pub struct Delivery {
    handle: Option<Handle>,
}

impl Delivery {
    pub fn into_capability(mut self) -> Capability {
        Capability::owned(self.handle.take().expect("Delivery already consumed"))
    }
}

impl Drop for Delivery {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            super::object::close_object_owner(handle)
        }
    }
}

#[derive(Debug)]
pub struct ReceivedMessage {
    pub header: MessageHeader,
    pub payload: Vec<u8>,
    pub handles: HandleSet,
    pub delivery: Delivery,
}

/// 可复用的完整接收预算，Syscall 成功后不再分配 owner。
#[derive(Debug)]
pub struct ReceiveBuffer {
    payload: Vec<u8>,
    handles: HandleSet,
    raw_handles: [Handle; MESSAGE_HANDLE_MAX],
    header: MessageHeader,
    delivery: Option<Delivery>,
}

impl ReceiveBuffer {
    pub fn new() -> Result<Self, SystemCallError> {
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(PAYLOAD_MAX)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(Self {
            payload,
            handles: HandleSet::prepared(MESSAGE_HANDLE_MAX)?,
            raw_handles: [Handle::INVALID; MESSAGE_HANDLE_MAX],
            header: MessageHeader::new(0, 0, 0, 0, 0),
            delivery: None,
        })
    }
    pub fn receive(&mut self, mailbox: Handle) -> Result<(), SystemCallError> {
        self.delivery = None;
        self.handles.reset_prepared(MESSAGE_HANDLE_MAX);
        self.raw_handles.fill(Handle::INVALID);
        self.payload.resize(PAYLOAD_MAX, 0);
        let mut received = erhino_shared::message::ReceiveResult {
            header: MessageHeader::new(0, 0, 0, 0, 0),
            delivery: Handle::INVALID,
        };
        // SAFETY: 全预算已准备，缓冲私有，失败不安装能力，成功只移交新 owner。
        unsafe {
            sys_receive(
                mailbox,
                &mut received,
                &mut self.payload,
                &mut self.raw_handles,
            )
        }?;
        assert!(
            received.header.payload_len as usize <= PAYLOAD_MAX,
            "Receive exceeded payload limit"
        );
        assert!(
            received.header.handle_count as usize <= MESSAGE_HANDLE_MAX,
            "Receive exceeded capability limit"
        );
        self.payload.truncate(received.header.payload_len as usize);
        self.handles
            .install(&self.raw_handles[..received.header.handle_count as usize]);
        self.header = received.header;
        self.delivery = Some(Delivery {
            handle: Some(received.delivery),
        });
        Ok(())
    }
    pub fn discard(&mut self) {
        self.delivery = None;
        self.handles.reset_prepared(MESSAGE_HANDLE_MAX);
        self.payload.clear();
        self.header = MessageHeader::new(0, 0, 0, 0, 0);
    }
    pub fn header(&self) -> &MessageHeader {
        &self.header
    }
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }
}

/// 接收消息的独立预付所有权存储，可在调用发布前为其回复准备。
#[derive(Debug)]
pub struct MessageStorage {
    payload: Vec<u8>,
    handles: HandleSet,
}

impl MessageStorage {
    pub fn new() -> Result<Self, SystemCallError> {
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(PAYLOAD_MAX)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(Self {
            payload,
            handles: HandleSet::prepared(MESSAGE_HANDLE_MAX)?,
        })
    }
    pub fn take(mut self, buffer: &mut ReceiveBuffer) -> Result<ReceivedMessage, SystemCallError> {
        let delivery = buffer
            .delivery
            .take()
            .ok_or(SystemCallError::ObjectNotAvailable)?;
        self.payload.extend_from_slice(&buffer.payload);
        self.handles.transfer_from(&mut buffer.handles);
        Ok(ReceivedMessage {
            header: buffer.header,
            payload: self.payload,
            handles: self.handles,
            delivery,
        })
    }
}

#[derive(Debug)]
pub struct Mailbox {
    owner: Option<Handle>,
}

pub struct MintedSender {
    pub sender: Capability,
    pub lifetime: Capability,
}

impl Mailbox {
    pub fn create(rights: Rights) -> Result<Self, SystemCallError> {
        let mut owner = Handle::INVALID;
        // SAFETY: 新创建的 receiver-owner 由此对象独占。
        unsafe { sys_mailbox_create(rights, &mut owner) }?;
        Ok(Self { owner: Some(owner) })
    }

    pub fn as_handle(&self) -> Handle {
        self.owner.expect("Mailbox owner already consumed")
    }

    pub fn mint(
        &self,
        badge: MailboxBadge,
        rights: Rights,
    ) -> Result<MintedSender, SystemCallError> {
        let minted = mint_sender(self.as_handle(), badge, rights)?;
        Ok(MintedSender {
            sender: Capability::owned(minted.sender),
            lifetime: Capability::owned(minted.lifetime),
        })
    }

    pub fn receive(&self) -> Result<ReceivedMessage, SystemCallError> {
        receive(self.as_handle())
    }

    pub fn register(
        &self,
        set: &super::wait_set::WaitSet,
        cookie: u64,
    ) -> Result<u64, SystemCallError> {
        set.register(WaitItem::new(
            self.as_handle(),
            ObjectSignals::READABLE | ObjectSignals::CLOSED,
            cookie,
        ))
    }

    pub fn into_raw(mut self) -> Handle {
        self.owner.take().expect("Mailbox owner already consumed")
    }
}

impl Drop for Mailbox {
    fn drop(&mut self) {
        if let Some(owner) = self.owner.take() {
            // SAFETY: 此 owner 唯一且不绑定映射，关闭只排空硬容量内的消息。
            unsafe { super::object::close(owner) }.unwrap_or_else(|error| {
                panic!("Mailbox owner close invariant violated: {error:?}")
            });
        }
    }
}

/// 组合式原始队列工厂；正式服务可以分别创建与铸造并保留 Lifetime。
pub fn create(owner_rights: Rights, sender_rights: Rights) -> Result<HandlePair, SystemCallError> {
    let owner = Mailbox::create(owner_rights)?;
    let minted = owner.mint(0, sender_rights)?;
    Ok(HandlePair::new(owner.into_raw(), minted.sender.into_raw()))
}

/// 不附带能力的普通发送，不接受会消费另一 owner 的 send-once。
pub fn send(mailbox: Handle, kind: u64, payload: &[u8]) -> Result<(), SystemCallError> {
    if super::object::query(mailbox)?.role != HandleRole::MailboxSender as u32 {
        return Err(SystemCallError::WrongObjectType);
    }
    // SAFETY: 没有 move，目标普通 sender 不被消费。
    unsafe { sys_send(mailbox, kind, payload, &[], Deadline::INFINITE) }
}

/// # Safety
/// 调用者独占 moves 的运输责任；若目标是 send-once，也必须独占其消费责任。
/// 任何源项均不得由存活的安全 owner 继续持有。失败不消费，成功移交全部责任。
pub unsafe fn send_raw(
    mailbox: Handle,
    kind: u64,
    payload: &[u8],
    moves: &[HandleMove],
) -> Result<(), SystemCallError> {
    // SAFETY: 本函数的调用者提供完整的原始运输所有权契约。
    unsafe { send_raw_until(mailbox, kind, payload, moves, Deadline::INFINITE) }
}

/// # Safety
/// 同 send_raw；期限到达和其他失败均不解除调用者的原始 owner 责任。
pub unsafe fn send_raw_until(
    mailbox: Handle,
    kind: u64,
    payload: &[u8],
    moves: &[HandleMove],
    deadline: Deadline,
) -> Result<(), SystemCallError> {
    // SAFETY: 输入仅在 ecall 期间借用，所有权契约由调用者保证。
    unsafe { sys_send(mailbox, kind, payload, moves, deadline) }
}

pub fn peek(mailbox: Handle) -> Result<MessageHeader, SystemCallError> {
    let mut header = MessageHeader::new(0, 0, 0, 0, 0);
    // SAFETY: output 在调用期间有效且可写。
    unsafe { sys_peek(mailbox, &mut header) }?;
    Ok(header)
}

pub fn receive(mailbox: Handle) -> Result<ReceivedMessage, SystemCallError> {
    let header = peek(mailbox)?;
    if header.payload_len as usize > PAYLOAD_MAX
        || header.handle_count as usize > MESSAGE_HANDLE_MAX
    {
        return Err(SystemCallError::InternalError);
    }
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(header.payload_len as usize)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    payload.resize(header.payload_len as usize, 0);
    let mut raw_handles = Vec::new();
    raw_handles
        .try_reserve_exact(header.handle_count as usize)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    raw_handles.resize(header.handle_count as usize, Handle::INVALID);
    let mut handles = HandleSet::prepared(raw_handles.len())?;
    let mut received = erhino_shared::message::ReceiveResult {
        header: MessageHeader::new(0, 0, 0, 0, 0),
        delivery: Handle::INVALID,
    };
    // SAFETY: 全部缓冲在调用期间私有，能力 owner 存储在 Receive 前已准备。
    unsafe { sys_receive(mailbox, &mut received, &mut payload, &mut raw_handles) }?;
    assert!(
        received.header.payload_len as usize <= payload.len(),
        "Receive exceeded payload capacity"
    );
    assert!(
        received.header.handle_count as usize <= raw_handles.len(),
        "Receive exceeded capability capacity"
    );
    payload.truncate(received.header.payload_len as usize);
    raw_handles.truncate(received.header.handle_count as usize);
    handles.install(&raw_handles);
    Ok(ReceivedMessage {
        header: received.header,
        payload,
        handles,
        delivery: Delivery {
            handle: Some(received.delivery),
        },
    })
}

pub fn discard(mailbox: Handle) -> Result<(), SystemCallError> {
    // SAFETY: Discard 只清理由队列持有的尚未接收项。
    unsafe { sys_discard(mailbox) }
}

pub fn make_send_once(source: Handle, rights: Rights) -> Result<Handle, SystemCallError> {
    let mut output = Handle::INVALID;
    // SAFETY: 新派生项的原始消费责任完整交付调用者。
    unsafe { sys_mailbox_make_send_once(source, rights, &mut output) }?;
    Ok(output)
}

pub fn send_once(source: &Capability, rights: Rights) -> Result<Capability, SystemCallError> {
    let handle = make_send_once(source.as_handle(), rights)?;
    Ok(Capability::owned(handle))
}

pub fn mint_sender(
    owner: Handle,
    badge: MailboxBadge,
    rights: Rights,
) -> Result<SenderResult, SystemCallError> {
    let mut output = SenderResult {
        sender: Handle::INVALID,
        lifetime: Handle::INVALID,
    };
    // SAFETY: 此原始工厂交付两项新能力的关闭责任，不构造重复 owner。
    unsafe { sys_mailbox_mint_sender(owner, badge, rights, &mut output) }?;
    Ok(output)
}

pub fn wait_message_until(
    mailbox: Handle,
    deadline: Deadline,
) -> Result<ReceivedMessage, SystemCallError> {
    loop {
        match receive(mailbox) {
            Ok(message) => return Ok(message),
            Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => (),
            Err(error) => return Err(error),
        }
        if crate::time::expired(deadline)? {
            return Err(SystemCallError::DeadlineExpired);
        }
        let result = super::wait::wait_until(
            &[WaitItem::new(
                mailbox,
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                0,
            )],
            deadline,
        )?;
        if WaitReason::from_u32(result.reason) == Some(WaitReason::Timeout) {
            return Err(SystemCallError::DeadlineExpired);
        }
        if result.observed.intersects(ObjectSignals::CLOSED) {
            return Err(SystemCallError::ObjectClosed);
        }
    }
}

pub fn wait_message(mailbox: Handle) -> Result<ReceivedMessage, SystemCallError> {
    wait_message_until(mailbox, Deadline::INFINITE)
}
