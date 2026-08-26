//! 消息原语（契约见 notes/ideas/message.md）：控制面小消息，投递永不阻塞。

use crate::object::{Handle, ProcessId, Rights};

/// 邮箱容量（条数）。满箱时 `Send` 立即返回
/// [`crate::call::SystemCallError::MailboxFull`]；发送侧可观察 `WRITABLE`
/// 电平并经 WaitMany 等待腾位（rinlib `send_blocking` 封装该闭环）。
pub const MAILBOX_CAPACITY: usize = 16;

/// 单条消息负载上限（字节）。超限在发送侧拒绝（IllegalArgument），
/// 消息层不做分片重组——大块数据走隧道。
pub const PAYLOAD_MAX: usize = 4096;

/// 单条消息可原子转入的 Handle 上限。
pub const MESSAGE_HANDLE_MAX: usize = 8;

/// 新消息 ABI 的发送侧 header；sender 永远由内核生成。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct SendHeader {
    pub kind: u64,
    pub payload_len: u32,
    pub handle_count: u32,
    pub reserved: [u64; 5],
}

impl SendHeader {
    pub const fn new(kind: u64, payload_len: u32, handle_count: u32) -> Self {
        Self {
            kind,
            payload_len,
            handle_count,
            reserved: [0; 5],
        }
    }
}

/// Mailbox sender capability 携带的不可变授权上下文。
pub type MailboxBadge = u64;

/// 新消息 ABI 的接收侧 envelope。两项来源信息均由内核生成：PID 只表示
/// 发送进程 provenance，badge 表示目标 sender capability 的授权上下文。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct MessageHeader {
    pub sender_pid: ProcessId,
    pub sender_badge: MailboxBadge,
    pub kind: u64,
    pub payload_len: u32,
    pub handle_count: u32,
    pub reserved: [u64; 4],
}

impl MessageHeader {
    pub const fn new(
        sender_pid: ProcessId,
        sender_badge: MailboxBadge,
        kind: u64,
        payload_len: u32,
        handle_count: u32,
    ) -> Self {
        Self {
            sender_pid,
            sender_badge,
            kind,
            payload_len,
            handle_count,
            reserved: [0; 4],
        }
    }
}

/// Send 时请求移动的一项 Handle 及其目标 rights。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct HandleMove {
    pub handle: Handle,
    pub rights: Rights,
}

const _: () = {
    assert!(core::mem::size_of::<SendHeader>() == 56);
    assert!(core::mem::align_of::<SendHeader>() == 8);
    assert!(core::mem::size_of::<MessageHeader>() == 64);
    assert!(core::mem::align_of::<MessageHeader>() == 8);
    assert!(core::mem::size_of::<HandleMove>() == 16);
    assert!(core::mem::align_of::<HandleMove>() == 8);
};
