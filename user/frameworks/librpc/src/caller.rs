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

use crate::{next_txid, RpcMessageKind, RpcPrefix};

/// 同步调用错误。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallError {
    /// 期限到达，ReplyPort 已废弃重建；请求可能已被服务端处理，
    /// 重试语义（idempotency）由各协议自持。
    Deadline,
    /// 服务邮箱关闭（观察 CLOSED）。
    ServiceClosed,
    /// 应答 framing 违约。
    Frame(FrameRejection),
    System(SystemCallError),
}

/// 应答 framing 违约的具体原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRejection {
    UnknownVersion,
    NotResponse,
    TxidMismatch,
    ProtocolMismatch,
}

impl From<SystemCallError> for CallError {
    fn from(value: SystemCallError) -> Self {
        Self::System(value)
    }
}

/// 一条已验证的应答：RpcPrefix 已剥离，payload 为协议层字节。
pub struct Reply {
    pub sender: u64,
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
            Rights::WRITE | Rights::DUPLICATE | Rights::TRANSFER,
        )?;
        self.port = Some(port);
        Ok(port)
    }

    /// 废弃端口：超时后的迟到回复隔离，下次调用懒重建。
    fn discard_port(&mut self) {
        if let Some(port) = self.port.take() {
            let _ = close(port.peer);
            let _ = close(port.owner);
        }
    }

    /// 发起一次同步调用。`body` 为协议层字节（不含 RpcPrefix）；
    /// `extra_moves` 追加在 slot 0 的 send-once 回复授权之后；
    /// `deadline_ms` 为相对毫秒期限（0 = 无限）。
    pub fn call(
        &mut self,
        service: Handle,
        protocol_id: u64,
        deadline_ms: u64,
        body: &[u8],
        extra_moves: &[HandleMove],
    ) -> Result<Reply, CallError> {
        if body.len() + crate::PREFIX_LEN > PAYLOAD_MAX
            || 1 + extra_moves.len() > MESSAGE_HANDLE_MAX
        {
            return Err(CallError::System(SystemCallError::IllegalArgument));
        }
        let port = self.ensure_port()?;
        let txid = next_txid();

        let mut payload = [0u8; PAYLOAD_MAX];
        RpcPrefix::new(RpcMessageKind::Request, txid).encode(&mut payload);
        let used = crate::PREFIX_LEN + body.len();
        payload[crate::PREFIX_LEN..used].copy_from_slice(body);

        // slot 0：裁剪至 WRITE|TRANSFER 的一次性回复授权（跨协议公共约定；
        // TRANSFER 是随消息 move 的内核前提）。
        let reply_once = make_send_once(port.peer, Rights::WRITE | Rights::TRANSFER)?;
        let mut moves_storage = [HandleMove { handle: Handle::INVALID, rights: Rights::NONE };
            1 + MESSAGE_HANDLE_MAX];
        moves_storage[0] =
            HandleMove { handle: reply_once, rights: Rights::WRITE | Rights::TRANSFER };
        moves_storage[1..1 + extra_moves.len()].copy_from_slice(extra_moves);
        let moves = &moves_storage[..1 + extra_moves.len()];

        send_blocking(service, protocol_id, &payload[..used], moves)
            .map_err(|error| {
                // 发送失败：尚未转移的 reply_once 留在本地，关闭防止泄漏。
                let _ = close(reply_once);
                error
            })?;

        let items = [
            WaitItem::new(port.owner, ObjectSignals::READABLE | ObjectSignals::CLOSED, 0),
            WaitItem::new(service, ObjectSignals::CLOSED, 1),
        ];
        let result = wait_many(&items, deadline_ms).map_err(|error| {
            // 等待失败：迟到回复可能落地，废弃端口隔离。
            self.discard_port();
            error
        })?;
        match WaitReason::from_u32(result.reason) {
            Some(WaitReason::Deadline) => {
                self.discard_port();
                Err(CallError::Deadline)
            }
            // 观察到 CLOSED 必然以 Closed 收尾（终态独占电平）。
            Some(WaitReason::Closed) if result.item_index == 1 => Err(CallError::ServiceClosed),
            Some(WaitReason::Closed) => {
                Err(CallError::System(SystemCallError::ObjectClosed))
            }
            _ => {
                let message = receive(port.owner).map_err(|error| {
                    // 接收失败：同上，废弃端口隔离迟到回复。
                    self.discard_port();
                    error
                })?;
                if message.header.kind != protocol_id {
                    return Err(CallError::Frame(FrameRejection::ProtocolMismatch));
                }
                let prefix = RpcPrefix::decode(&message.payload)
                    .map_err(|_| CallError::Frame(FrameRejection::UnknownVersion))?;
                if prefix.kind != RpcMessageKind::Response {
                    return Err(CallError::Frame(FrameRejection::NotResponse));
                }
                if prefix.txid != txid {
                    return Err(CallError::Frame(FrameRejection::TxidMismatch));
                }
                Ok(Reply {
                    sender: message.header.sender,
                    payload: message.payload[crate::PREFIX_LEN..].to_vec(),
                    handles: message.handles,
                })
            }
        }
    }
}
