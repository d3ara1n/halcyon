//! 同步与异步调用共用的投递状态、运输 owner 和服务端回复责任。

use erhino_shared::{
    call::SystemCallError,
    message::{MessageHeader, MESSAGE_HANDLE_MAX, PAYLOAD_MAX},
    object::{HandleRole, Rights},
    time::Deadline,
};
use rinlib::ipc::{
    capability::{Capability, HandleSet},
    message::{Delivery, ReceivedMessage, send_once},
    packet::{Packet, PushFailure},
};
use crate::{FrameError, PREFIX_LEN, ResponseRejection, RpcMessageKind, RpcPrefix, next_txid, validate_response};

pub type FrameRejection = ResponseRejection;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallPhase { Unsent, Sent }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallCause {
    Timeout,
    ServiceClosed,
    Shutdown,
    Frame(FrameRejection),
    System(SystemCallError),
}

#[derive(Debug)]
pub struct CallError {
    pub phase: CallPhase,
    pub cause: CallCause,
    pub request: Option<Request>,
}

impl CallError {
    pub(crate) fn unsent(cause: CallCause, mut request: Request) -> Self {
        request.detach_reply();
        Self { phase: CallPhase::Unsent, cause, request: Some(request) }
    }
    pub(crate) fn sent(cause: CallCause) -> Self {
        Self { phase: CallPhase::Sent, cause, request: None }
    }
}

#[derive(Debug)]
pub struct Request {
    pub(crate) txid: u64,
    pub(crate) packet: Packet,
    reply_attached: bool,
}

impl Request {
    pub fn new(protocol: u64, body: &[u8]) -> Result<Self, SystemCallError> {
        if body.len() > PAYLOAD_MAX - PREFIX_LEN { return Err(SystemCallError::IllegalArgument) }
        let txid = 0;
        let mut payload = [0; PAYLOAD_MAX];
        RpcPrefix::new(RpcMessageKind::Request, txid).encode(&mut payload);
        payload[PREFIX_LEN..PREFIX_LEN + body.len()].copy_from_slice(body);
        Ok(Self { txid, packet: Packet::new(protocol, &payload[..PREFIX_LEN + body.len()])?, reply_attached: false })
    }
    pub fn txid(&self) -> u64 { self.txid }
    pub(crate) fn new_attempt(&mut self) -> Result<(), SystemCallError> {
        if self.packet.delivered() || self.reply_attached { return Err(SystemCallError::ObjectBusy) }
        self.txid = next_txid().ok_or(SystemCallError::ReachLimit)?;
        RpcPrefix::new(RpcMessageKind::Request, self.txid).encode(self.packet.payload_mut());
        Ok(())
    }
    pub fn protocol(&self) -> u64 { self.packet.kind() }
    pub fn push(&mut self, capability: Capability, rights: Rights) -> Result<(), PushFailure> {
        if self.reply_attached || self.packet.handle_count() >= MESSAGE_HANDLE_MAX - 1 {
            return Err(PushFailure { error: SystemCallError::IllegalArgument, capability });
        }
        self.packet.push(capability, rights)
    }
    pub(crate) fn attach_reply(&mut self, source: &Capability) -> Result<(), SystemCallError> {
        if self.reply_attached { return Err(SystemCallError::ObjectBusy) }
        let rights = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
        let reply = send_once(source, rights)?;
        self.packet.push_front(reply, rights).map_err(|failure| failure.error)?;
        self.reply_attached = true;
        Ok(())
    }
    pub(crate) fn detach_reply(&mut self) {
        if self.reply_attached && !self.packet.delivered() {
            drop(self.packet.pop_front());
        }
        self.reply_attached = false;
    }
    pub(crate) fn try_send(&mut self, service: &Capability, deadline: Deadline) -> Result<(), SystemCallError> {
        self.packet.try_send(service, deadline)
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
    pub(crate) fn accept(mut message: ReceivedMessage, protocol: u64, txid: u64, deadline: Deadline) -> Result<Self, CallCause> {
        validate_response(protocol, txid, message.header.kind, &message.payload).map_err(CallCause::Frame)?;
        // 在已接收的私有字节中移除 prefix，不引入接受后的分配失败。
        message.payload.drain(..PREFIX_LEN);
        if rinlib::time::expired(deadline).map_err(CallCause::System)? { return Err(CallCause::Timeout) }
        Ok(Self {
            sender_pid: message.header.sender_pid, sender_badge: message.header.sender_badge,
            sender_context_id: message.header.sender_context_id, payload: message.payload,
            handles: message.handles, delivery: message.delivery,
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

#[derive(Debug)]
pub struct RequestContext {
    pub envelope: MessageHeader,
    pub txid: u64,
    pub payload: alloc::vec::Vec<u8>,
    pub handles: HandleSet,
    reply: Capability,
    delivery: Delivery,
}

impl RequestContext {
    #[expect(clippy::result_large_err, reason = "拒绝请求原样返还消息与交付拥有者，错误路径不分配")]
    pub fn decode(mut message: ReceivedMessage, protocol: u64) -> Result<Self, RejectedRequest> {
        let checked = (|| {
            if message.header.kind != protocol { return Err(RequestRejection::Protocol) }
            let prefix = RpcPrefix::decode(&message.payload).map_err(RequestRejection::Frame)?;
            if prefix.kind != RpcMessageKind::Request { return Err(RequestRejection::NotRequest) }
            let reply = message.handles.get(0).map_err(RequestRejection::System)?;
            let description = reply.description().map_err(RequestRejection::System)?;
            if description.role != HandleRole::MailboxSenderOnce as u32 || !description.rights.contains(Rights::WRITE | Rights::WAIT) {
                return Err(RequestRejection::ReplyRole);
            }
            Ok(prefix)
        })();
        let prefix = match checked {
            Ok(prefix) => prefix,
            Err(reason) => return Err(RejectedRequest { reason, message }),
        };
        let reply = message.handles.take(0).expect("validated reply slot disappeared");
        message.payload.drain(..PREFIX_LEN);
        Ok(Self { envelope: message.header, txid: prefix.txid, payload: message.payload,
            handles: message.handles, reply, delivery: message.delivery })
    }
}

#[derive(Debug)]
pub struct PreparedResponse {
    context: RequestContext,
    packet: Packet,
}

#[derive(Debug)]
pub struct ResponseFailure {
    pub error: SystemCallError,
    pub context: RequestContext,
}

impl PreparedResponse {
    /// 在业务 Commit 前取得完整输出预算；后续编码、裁剪与投递无需分配。
    #[expect(clippy::result_large_err, reason = "预备失败返还请求及交付责任，错误路径不分配")]
    pub fn new(context: RequestContext, capacity: usize) -> Result<Self, ResponseFailure> {
        if capacity > PAYLOAD_MAX - PREFIX_LEN {
            return Err(ResponseFailure { error: SystemCallError::IllegalArgument, context });
        }
        let mut bytes = [0; PAYLOAD_MAX];
        RpcPrefix::new(RpcMessageKind::Response, context.txid).encode(&mut bytes);
        match Packet::new(context.envelope.kind, &bytes[..PREFIX_LEN + capacity]) {
            Ok(packet) => Ok(Self { context, packet }),
            Err(error) => Err(ResponseFailure { error, context }),
        }
    }
    pub fn body_mut(&mut self) -> &mut [u8] { &mut self.packet.payload_mut()[PREFIX_LEN..] }
    pub fn parts(&mut self) -> (&mut RequestContext, &mut [u8]) {
        (&mut self.context, &mut self.packet.payload_mut()[PREFIX_LEN..])
    }
    pub fn finish_body(&mut self, used: usize) -> Result<(), SystemCallError> {
        self.packet.truncate_payload(PREFIX_LEN.checked_add(used).ok_or(SystemCallError::IllegalArgument)?)
    }
    pub fn push(&mut self, capability: Capability, rights: Rights) -> Result<(), PushFailure> {
        self.packet.push(capability, rights)
    }
    pub fn try_send(&mut self, deadline: Deadline) -> Result<(), SystemCallError> {
        self.packet.try_reply(&mut self.context.reply, deadline)
    }
    pub fn reply_handle(&self) -> erhino_shared::object::Handle { self.context.reply.as_handle() }
    pub fn delivery(&self) -> &Delivery { &self.context.delivery }
}

pub(crate) fn cause(error: SystemCallError) -> CallCause {
    match error {
        SystemCallError::DeadlineExpired => CallCause::Timeout,
        SystemCallError::ObjectClosed => CallCause::ServiceClosed,
        error => CallCause::System(error),
    }
}
