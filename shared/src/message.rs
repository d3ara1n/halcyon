//! 消息原语（契约见 notes/ideas/message.md）：控制面小消息，投递永不阻塞。

use crate::object::{Handle, ProcessId, Rights};

/// 邮箱容量（条数）。满箱时 `Send` 立即返回
/// [`crate::call::SystemCallError::MailboxFull`]，流控由上层协议承担。
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

/// 新消息 ABI 的接收侧 envelope。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct MessageHeader {
    pub sender: ProcessId,
    pub kind: u64,
    pub payload_len: u32,
    pub handle_count: u32,
    pub reserved: [u64; 5],
}

impl MessageHeader {
    pub const fn new(sender: ProcessId, kind: u64, payload_len: u32, handle_count: u32) -> Self {
        Self {
            sender,
            kind,
            payload_len,
            handle_count,
            reserved: [0; 5],
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
