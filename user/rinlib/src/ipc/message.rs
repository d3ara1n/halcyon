//! 消息封装（契约见 notes/ideas/message.md）：Send 永不阻塞，Receive
//! 阻塞等待；[`wait_message`] 组合 Peek/SignalWait/Receive 提供完整的
//! 「取下一条消息」服务循环原语。

use alloc::vec::Vec;

use erhino_shared::{
    call::SystemCallError,
    message::MessageDigest,
    proc::Pid,
    signal::{ObjectKind, SignalItem, NONEMPTY},
};

use crate::call::{sys_discard, sys_receive, sys_send, sys_signal_wait, sys_peek};

/// 投递消息到目标邮箱。**永不阻塞**：满箱返回 `MailboxFull`，流控由
/// 调用方承担（请求-应答配对协议天然限流）。
pub fn send(target: Pid, kind: usize, payload: &[u8]) -> Result<(), SystemCallError> {
    unsafe { sys_send(target, kind, payload) }
}

/// 非阻塞检查自身邮箱队头。空箱返回 `Err(ObjectNotAvailable)`。
pub fn peek() -> Result<MessageDigest, SystemCallError> {
    let mut digest = MessageDigest::new(0, 0, 0);
    unsafe { sys_peek(&mut digest)? };
    Ok(digest)
}

/// 抛弃自身邮箱队头消息。空箱返回 `Err(ObjectNotAvailable)`。
pub fn discard() -> Result<(), SystemCallError> {
    unsafe { sys_discard() }
}

/// 阻塞取自身邮箱队头消息：负载拷入 buffer（长度须与队头消息一致，
/// 先经 [`peek`] 获得），返回负载长度。空箱时线程睡眠等待到达。
pub fn receive(buffer: &mut [u8]) -> Result<usize, SystemCallError> {
    unsafe { sys_receive(buffer) }
}

/// 取下一条消息的完整原语（服务主循环用）：
/// 有消息 → 立即返回；空箱 → 等待 NONEMPTY 信号再重查。
///
/// 唤醒后回环重查是必要的：NONEMPTY 为内核托管位，唤醒与取走之间
/// 可能被同进程其他消费者排空。
pub fn wait_message() -> Result<(MessageDigest, Vec<u8>), SystemCallError> {
    loop {
        if let Ok(digest) = peek() {
            let mut buf = Vec::new();
            buf.resize_with(digest.payload_length, Default::default);
            unsafe { sys_receive(&mut buf)? };
            return Ok((digest, buf));
        }
        let items = [SignalItem {
            kind: ObjectKind::Mailbox as u64,
            id: 0,
            interest: NONEMPTY,
        }];
        unsafe { sys_signal_wait(&items)? };
    }
}
