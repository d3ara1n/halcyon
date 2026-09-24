//! 同步与异步调用共用的投递状态、运输 owner 和服务端回复责任。

use crate::{
    FrameError, PREFIX_LEN, ResponseRejection, RpcMessageKind, RpcPrefix, next_txid,
    validate_response,
};
use erhino_shared::{
    call::SystemCallError,
    message::{MESSAGE_HANDLE_MAX, MessageHeader, PAYLOAD_MAX},
    object::{Handle, HandleRole, Rights},
    time::Deadline,
};
use rinlib::ipc::{
    capability::{Capability, HandleSet},
    message::{Delivery, MailboxSender, ReceivedMessage, SendOnce, send_once},
    packet::{Packet, PushFailure},
};

pub type FrameRejection = ResponseRejection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallPhase {
    Unsent,
    Sent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallCause {
    Timeout,
    Cancelled,
    ServiceClosed,
    Shutdown,
    Frame(FrameRejection),
    System(SystemCallError),
}

#[derive(Debug)]
pub struct CallError {
    pub phase: CallPhase,
    pub cause: CallCause,
    pub owner: CallOwner,
}

#[derive(Debug)]
pub enum CallOwner {
    Unsent(Request),
    Reply(crate::caller::ReplyCleanup),
    None,
}

impl CallError {
    pub(crate) fn unsent(cause: CallCause, mut request: Request) -> Self {
        // 本次尝试不再投递；撤回的 send-once 回复授权由完整返还的 owner
        // 显式关闭，不依赖 tombstone 状态表达消费。
        drop(request.detach_reply());
        Self {
            phase: CallPhase::Unsent,
            cause,
            owner: CallOwner::Unsent(request),
        }
    }
    pub(crate) fn sent(cause: CallCause) -> Self {
        Self::sent_with_cleanup(cause, None)
    }

    pub(crate) fn sent_with_cleanup(
        cause: CallCause,
        cleanup: Option<crate::caller::ReplyCleanup>,
    ) -> Self {
        Self {
            phase: CallPhase::Sent,
            cause,
            owner: cleanup.map_or(CallOwner::None, CallOwner::Reply),
        }
    }

    pub fn has_unretired_owners(&self) -> bool {
        match &self.owner {
            CallOwner::Unsent(_) => true,
            CallOwner::Reply(cleanup) => cleanup.has_owners(),
            CallOwner::None => false,
        }
    }

    pub fn take_unsent_request(&mut self) -> Option<Request> {
        if !matches!(self.owner, CallOwner::Unsent(_)) {
            return None;
        }
        match core::mem::replace(&mut self.owner, CallOwner::None) {
            CallOwner::Unsent(request) => Some(request),
            _ => unreachable!("checked CallOwner variant changed"),
        }
    }

    pub fn retry_cleanup(&mut self) -> Result<(), SystemCallError> {
        match &mut self.owner {
            CallOwner::Unsent(_) => Err(SystemCallError::ObjectBusy),
            CallOwner::Reply(cleanup) => {
                cleanup.retry_close()?;
                self.owner = CallOwner::None;
                Ok(())
            }
            CallOwner::None => Ok(()),
        }
    }
}

/// 出站请求；成功投递后 packet 已移交，请求仅保留 txid/协议标识供回复匹配。
#[derive(Debug)]
pub struct Request {
    pub(crate) txid: u64,
    protocol: u64,
    packet: Option<Packet>,
    reply_attached: bool,
}

impl Request {
    pub fn new(protocol: u64, body: &[u8]) -> Result<Self, SystemCallError> {
        if body.len() > PAYLOAD_MAX - PREFIX_LEN {
            return Err(SystemCallError::IllegalArgument);
        }
        let txid = 0;
        let mut payload = [0; PAYLOAD_MAX];
        RpcPrefix::new(RpcMessageKind::Request, txid).encode(&mut payload);
        payload[PREFIX_LEN..PREFIX_LEN + body.len()].copy_from_slice(body);
        Ok(Self {
            txid,
            protocol,
            packet: Some(Packet::new(protocol, &payload[..PREFIX_LEN + body.len()])?),
            reply_attached: false,
        })
    }
    pub fn txid(&self) -> u64 {
        self.txid
    }
    pub(crate) fn new_attempt(&mut self) -> Result<(), SystemCallError> {
        if self.reply_attached {
            return Err(SystemCallError::ObjectBusy);
        }
        let packet = self.packet.as_mut().ok_or(SystemCallError::ObjectBusy)?;
        self.txid = next_txid().ok_or(SystemCallError::ReachLimit)?;
        RpcPrefix::new(RpcMessageKind::Request, self.txid).encode(packet.payload_mut());
        Ok(())
    }
    pub fn protocol(&self) -> u64 {
        self.protocol
    }
    pub fn push(&mut self, capability: Capability, rights: Rights) -> Result<(), PushFailure> {
        let packet = match self.packet.as_mut() {
            Some(packet) => packet,
            None => {
                return Err(PushFailure {
                    error: SystemCallError::ObjectBusy,
                    capability,
                });
            }
        };
        if self.reply_attached || packet.handle_count() >= MESSAGE_HANDLE_MAX - 1 {
            return Err(PushFailure {
                error: SystemCallError::IllegalArgument,
                capability,
            });
        }
        packet.push(capability, rights)
    }
    pub(crate) fn attach_reply(&mut self, source: &MailboxSender) -> Result<(), SystemCallError> {
        if self.reply_attached {
            return Err(SystemCallError::ObjectBusy);
        }
        let packet = self.packet.as_mut().ok_or(SystemCallError::ObjectBusy)?;
        let rights = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
        let reply = send_once(source, rights)?;
        packet
            .push_front(reply.into_capability(), rights)
            .map_err(|failure| failure.error)?;
        self.reply_attached = true;
        Ok(())
    }
    /// 撤回随附的 send-once 回复授权并完整返还 owner；未附或已投递时返回 None。
    pub fn detach_reply(&mut self) -> Option<Capability> {
        if !self.reply_attached {
            return None;
        }
        self.reply_attached = false;
        self.packet
            .as_mut()
            .and_then(Packet::pop_front)
            .map(|(capability, _)| capability)
    }
    /// 消费式投递：成功后 Packet 已移交；失败完整返还，可重试或拆解回收。
    pub(crate) fn try_send(
        &mut self,
        service: &MailboxSender,
        deadline: Deadline,
    ) -> Result<(), SystemCallError> {
        let packet = self.packet.take().ok_or(SystemCallError::ObjectBusy)?;
        match packet.try_send(service, deadline) {
            Ok(()) => Ok(()),
            Err(failure) => {
                self.packet = Some(failure.packet);
                Err(failure.error)
            }
        }
    }
}

#[derive(Debug)]
pub struct Reply {
    pub sender_pid: u64,
    pub sender_badge: u64,
    pub sender_context_id: u64,
    pub payload: alloc::vec::Vec<u8>,
    pub handles: HandleSet,
    pub delivery: Delivery,
}

impl Reply {
    pub(crate) fn accept(
        mut message: ReceivedMessage,
        protocol: u64,
        txid: u64,
        deadline: Deadline,
    ) -> Result<Self, CallCause> {
        validate_response(protocol, txid, message.header.kind, &message.payload)
            .map_err(CallCause::Frame)?;
        // 在已接收的私有字节中移除 prefix，不引入接受后的分配失败。
        message.payload.drain(..PREFIX_LEN);
        if rinlib::time::expired(deadline).map_err(CallCause::System)? {
            return Err(CallCause::Timeout);
        }
        Ok(Self {
            sender_pid: message.header.sender_pid,
            sender_badge: message.header.sender_badge,
            sender_context_id: message.header.sender_context_id,
            payload: message.payload,
            handles: message.handles,
            delivery: message.delivery,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestRejection {
    Protocol,
    Frame(FrameError),
    NotRequest,
    ReplyRole,
    System(SystemCallError),
}

#[derive(Debug)]
pub struct RejectedRequest {
    pub reason: RequestRejection,
    pub message: ReceivedMessage,
}

/// 服务端已接受的请求上下文：回复授权与交付责任保留到处理终结。
#[derive(Debug)]
pub struct RequestContext {
    pub envelope: MessageHeader,
    pub txid: u64,
    pub payload: alloc::vec::Vec<u8>,
    pub handles: HandleSet,
    reply: Option<SendOnce>,
    delivery: Delivery,
}

impl RequestContext {
    #[expect(
        clippy::result_large_err,
        reason = "拒绝请求原样返还消息与交付拥有者，错误路径不分配"
    )]
    pub fn decode(mut message: ReceivedMessage, protocol: u64) -> Result<Self, RejectedRequest> {
        let checked = (|| {
            if message.header.kind != protocol {
                return Err(RequestRejection::Protocol);
            }
            let prefix = RpcPrefix::decode(&message.payload).map_err(RequestRejection::Frame)?;
            if prefix.kind != RpcMessageKind::Request {
                return Err(RequestRejection::NotRequest);
            }
            let reply = message.handles.get(0).map_err(RequestRejection::System)?;
            let description = reply.description().map_err(RequestRejection::System)?;
            if description.role != HandleRole::MailboxSenderOnce as u32
                || !description.rights.contains(Rights::WRITE | Rights::WAIT)
            {
                return Err(RequestRejection::ReplyRole);
            }
            Ok(prefix)
        })();
        let prefix = match checked {
            Ok(prefix) => prefix,
            Err(reason) => return Err(RejectedRequest { reason, message }),
        };
        // 校验通过后才取出槽位；role 已验证，typed owner 不再重复 Query。
        let reply = SendOnce::from_validated(
            message
                .handles
                .take(0)
                .expect("validated reply slot disappeared"),
        );
        message.payload.drain(..PREFIX_LEN);
        Ok(Self {
            envelope: message.header,
            txid: prefix.txid,
            payload: message.payload,
            handles: message.handles,
            reply: Some(reply),
            delivery: message.delivery,
        })
    }

    /// 回复授权；投递成功后消费，失败路径会完整恢复。
    pub fn reply(&self) -> Option<&SendOnce> {
        self.reply.as_ref()
    }
    /// 交付责任保持到请求处理与回复责任终结；由调用方决定何时移交或关闭。
    pub fn delivery(&self) -> &Delivery {
        &self.delivery
    }
    fn take_reply(&mut self) -> Option<SendOnce> {
        self.reply.take()
    }
}

pub(crate) fn cause(error: SystemCallError) -> CallCause {
    match error {
        SystemCallError::DeadlineExpired => CallCause::Timeout,
        SystemCallError::ObjectClosed => CallCause::ServiceClosed,
        error => CallCause::System(error),
    }
}

#[derive(Debug)]
pub struct PreparedResponse {
    context: RequestContext,
    packet: Option<Packet>,
}

#[derive(Debug)]
pub struct ResponseFailure {
    pub error: SystemCallError,
    pub context: RequestContext,
}

impl PreparedResponse {
    /// 在业务 Commit 前取得完整输出预算；后续编码、裁剪与投递无需分配。
    #[expect(
        clippy::result_large_err,
        reason = "预备失败返还请求及交付责任，错误路径不分配"
    )]
    pub fn new(context: RequestContext, capacity: usize) -> Result<Self, ResponseFailure> {
        if capacity > PAYLOAD_MAX - PREFIX_LEN {
            return Err(ResponseFailure {
                error: SystemCallError::IllegalArgument,
                context,
            });
        }
        let mut bytes = [0; PAYLOAD_MAX];
        RpcPrefix::new(RpcMessageKind::Response, context.txid).encode(&mut bytes);
        match Packet::new(context.envelope.kind, &bytes[..PREFIX_LEN + capacity]) {
            Ok(packet) => Ok(Self {
                context,
                packet: Some(packet),
            }),
            Err(error) => Err(ResponseFailure { error, context }),
        }
    }
    fn packet(&mut self) -> Result<&mut Packet, SystemCallError> {
        self.packet.as_mut().ok_or(SystemCallError::ObjectBusy)
    }
    pub fn body_mut(&mut self) -> Result<&mut [u8], SystemCallError> {
        Ok(&mut self.packet()?.payload_mut()[PREFIX_LEN..])
    }
    pub fn parts(&mut self) -> Result<(&mut RequestContext, &mut [u8]), SystemCallError> {
        match (&mut self.context, self.packet.as_mut()) {
            (context, Some(packet)) => {
                let body = &mut packet.payload_mut()[PREFIX_LEN..];
                Ok((context, body))
            }
            (_, None) => Err(SystemCallError::ObjectBusy),
        }
    }
    pub fn finish_body(&mut self, used: usize) -> Result<(), SystemCallError> {
        let end = PREFIX_LEN
            .checked_add(used)
            .ok_or(SystemCallError::IllegalArgument)?;
        self.packet()?.truncate_payload(end)
    }
    pub fn push(&mut self, capability: Capability, rights: Rights) -> Result<(), PushFailure> {
        match self.packet.as_mut() {
            Some(packet) => packet.push(capability, rights),
            None => Err(PushFailure {
                error: SystemCallError::ObjectBusy,
                capability,
            }),
        }
    }

    /// 逐项取回尚未成功投递的业务能力，逆序访问 Packet 的尾部。
    /// 成功投递后 Packet 已被消费，不访问任何项。
    pub fn drain_capabilities(&mut self, mut visit: impl FnMut(Capability, Rights)) {
        let Some(packet) = self.packet.as_mut() else {
            return;
        };
        while let Some((capability, rights)) = packet.pop() {
            visit(capability, rights);
        }
    }
    /// 消费式回复：成功即交付 Packet 与 send-once 授权；失败二者完整返还。
    pub fn try_send(&mut self, deadline: Deadline) -> Result<(), SystemCallError> {
        let packet = self.packet.take().ok_or(SystemCallError::ObjectBusy)?;
        let reply = self
            .context
            .take_reply()
            .ok_or(SystemCallError::ObjectBusy)?;
        match packet.try_reply(reply, deadline) {
            Ok(()) => Ok(()),
            Err(failure) => {
                self.packet = Some(failure.packet);
                self.context.reply = Some(failure.reply);
                Err(failure.error)
            }
        }
    }
    pub(crate) fn reply_handle(&self) -> Result<Handle, SystemCallError> {
        self.context
            .reply()
            .map(SendOnce::as_handle)
            .ok_or(SystemCallError::ObjectBusy)
    }
    pub fn into_context(self) -> RequestContext {
        self.context
    }
}
