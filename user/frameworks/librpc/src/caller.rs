//! 同步 RPC 调用：per-thread ReplyPort + 单 outstanding 约束
//! （契约见 notes/ideas/rpc.md「并发与回复路由」）。
//!
//! mailbox 严格 FIFO 且无选择性 receive；同步调用线程在等待期间本就
//! 阻塞，私有回复端口内不可能出现他人的 response。超时即关闭整个
//! ReplyPort 废弃重建：迟到回复随 owner 关闭消亡，服务端对 send-once
//! 的投递失败即干净丢弃，无需回收协议。

use alloc::vec::Vec;

use erhino_shared::{
    call::SystemCallError,
    message::{HandleMove, MESSAGE_HANDLE_MAX, PAYLOAD_MAX},
    object::{Handle, HandlePair, ObjectSignals, Rights},
    wait::{WaitItem, WaitReason},
};
use rinlib::ipc::{
    message::{create as mailbox_create, make_send_once, receive, send_blocking},
    object::close,
    wait::wait_many,
};

use crate::{RpcMessageKind, RpcPrefix, next_txid, validate_response};

/// 同步调用错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallError {
    /// 超时，ReplyPort 已废弃重建；请求可能已被服务端处理，
    /// 重试语义（idempotency）由各协议自持。
    Timeout,
    /// 服务邮箱关闭（观察 CLOSED）。
    ServiceClosed,
    /// 应答 framing 违约。
    Frame(FrameRejection),
    System(SystemCallError),
}

/// 应答 framing 违约的具体原因。
pub type FrameRejection = crate::ResponseRejection;

impl From<SystemCallError> for CallError {
    fn from(value: SystemCallError) -> Self {
        Self::System(value)
    }
}

/// 一条已验证的应答：RpcPrefix 已剥离，payload 为协议层字节。
pub struct Reply {
    pub sender_pid: u64,
    pub sender_badge: u64,
    pub payload: Vec<u8>,
    pub handles: Vec<Handle>,
}

/// 同步调用者：懒创建并复用私有 ReplyPort。
///
/// 同一 Caller 同时只允许一个 outstanding call（同步线程本就阻塞，
/// 约束无代价；Caller 的线程私有化随用户态多线程落地）。
pub struct Caller {
    port: Option<HandlePair>,
}

impl Caller {
    pub const fn new() -> Self {
        Self { port: None }
    }

    fn ensure_port(&mut self) -> Result<HandlePair, SystemCallError> {
        if let Some(port) = self.port {
            return Ok(port);
        }
        let port = mailbox_create(
            Rights::READ | Rights::WAIT,
            Rights::WRITE | Rights::DUPLICATE | Rights::TRANSIT,
        )?;
        self.port = Some(port);
        Ok(port)
    }

    /// 废弃端口：迟到回复隔离，下次调用懒重建。
    fn discard_port(&mut self) {
        if let Some(port) = self.port.take() {
            // SAFETY: 私有 port 只由 mailbox_create 生成，本 Caller 独占；无映射 owner。
            let _ = unsafe { close(port.peer) };
            let _ = unsafe { close(port.owner) };
        }
    }

    /// 已接收但未接受的 response 统一收束：transit 不允许 Tunnel Endpoint，
    /// 因而逐项 Handle close 与 ReplyPort discard 都是固定上界叶操作。
    fn reject_reply(
        &mut self,
        message: rinlib::ipc::message::ReceivedMessage,
        rejection: FrameRejection,
    ) -> CallError {
        for handle in message.handles {
            // SAFETY: 私有拒绝路径只接收真实 receive 结果；Endpoint 不允许 TRANSIT。
            let _ = unsafe { close(handle) };
        }
        self.discard_port();
        CallError::Frame(rejection)
    }

    /// 发起一次同步调用。`body` 为协议层字节（不含 RpcPrefix）；
    /// `extra_moves` 追加在 slot 0 的 send-once 回复授权之后；
    /// `timeout_ms` 为相对毫秒超时（0 = 无限）。
    pub fn call(
        &mut self,
        service: Handle,
        protocol_id: u64,
        timeout_ms: u64,
        body: &[u8],
        extra_moves: &[HandleMove],
    ) -> Result<Reply, CallError> {
        if body.len() + crate::PREFIX_LEN > PAYLOAD_MAX
            || 1 + extra_moves.len() > MESSAGE_HANDLE_MAX
        {
            return Err(CallError::System(SystemCallError::IllegalArgument));
        }
        let port = self.ensure_port()?;
        let txid = next_txid().ok_or(CallError::System(SystemCallError::ReachLimit))?;

        let mut payload = [0u8; PAYLOAD_MAX];
        RpcPrefix::new(RpcMessageKind::Request, txid).encode(&mut payload);
        let used = crate::PREFIX_LEN + body.len();
        payload[crate::PREFIX_LEN..used].copy_from_slice(body);

        // slot 0：裁剪至 WRITE|TRANSIT 的一次性回复授权（跨协议公共约定；
        // TRANSIT 是暂存于消息并由接收方安装的内核前提）。
        let reply_once = make_send_once(port.peer, Rights::WRITE | Rights::TRANSIT)?;
        let mut moves_storage = [HandleMove {
            handle: Handle::INVALID,
            rights: Rights::NONE,
        }; 1 + MESSAGE_HANDLE_MAX];
        moves_storage[0] = HandleMove {
            handle: reply_once,
            rights: Rights::WRITE | Rights::TRANSIT,
        };
        moves_storage[1..1 + extra_moves.len()].copy_from_slice(extra_moves);
        let moves = &moves_storage[..1 + extra_moves.len()];

        send_blocking(service, protocol_id, &payload[..used], moves).inspect_err(|_| {
            // 发送失败：尚未转移的 reply_once 留在本地，关闭防止泄漏。
            let _ = unsafe { close(reply_once) };
        })?;

        let items = [
            WaitItem::new(
                port.owner,
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                0,
            ),
            WaitItem::new(service, ObjectSignals::CLOSED, 1),
        ];
        let result = wait_many(&items, timeout_ms).inspect_err(|_| {
            // 等待失败：迟到回复可能落地，废弃端口隔离。
            self.discard_port();
        })?;
        match WaitReason::from_u32(result.reason) {
            Some(WaitReason::Timeout) => {
                self.discard_port();
                Err(CallError::Timeout)
            }
            // 任一 CLOSED 都废弃本次 ReplyPort，隔离与关闭并发到达的回复。
            Some(WaitReason::Closed) if result.item_index == 1 => {
                self.discard_port();
                Err(CallError::ServiceClosed)
            }
            Some(WaitReason::Closed) => {
                self.discard_port();
                Err(CallError::System(SystemCallError::ObjectClosed))
            }
            _ => {
                let message = receive(port.owner).inspect_err(|_| {
                    // 接收失败：同上，废弃端口隔离迟到回复。
                    self.discard_port();
                })?;
                if let Err(rejection) =
                    validate_response(protocol_id, txid, message.header.kind, &message.payload)
                {
                    return Err(self.reject_reply(message, rejection));
                }
                Ok(Reply {
                    sender_pid: message.header.sender_pid,
                    sender_badge: message.header.sender_badge,
                    payload: message.payload[crate::PREFIX_LEN..].to_vec(),
                    handles: message.handles,
                })
            }
        }
    }
}

impl Default for Caller {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Caller {
    fn drop(&mut self) {
        self.discard_port();
    }
}
