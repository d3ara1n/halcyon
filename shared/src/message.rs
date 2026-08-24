//! 消息原语（契约见 notes/ideas/message.md）：控制面小消息，投递永不阻塞。

use crate::proc::Pid;

/// 邮箱容量（条数）。满箱时 `Send` 立即返回
/// [`crate::call::SystemCallError::MailboxFull`]，流控由上层协议承担。
pub const MAILBOX_CAPACITY: usize = 16;

/// 单条消息负载上限（字节）。超限在发送侧拒绝（IllegalArgument），
/// 消息层不做分片重组——大块数据走隧道。
pub const PAYLOAD_MAX: usize = 4096;

/// 消息摘要：Peek 的产出、Receive 的定长依据。
///
/// sender 由内核填写，接收方不可伪造；kind 由双方上层协议约定，
/// 内核不解释。
#[repr(C)]
pub struct MessageDigest {
    pub sender: Pid,
    pub kind: usize,
    pub payload_length: usize,
}

impl MessageDigest {
    pub const fn new(sender: Pid, kind: usize, length: usize) -> Self {
        Self { sender, kind, payload_length: length }
    }
}
