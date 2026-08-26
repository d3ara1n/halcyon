//! pm：sleep 异步通路 + 消息接收 + Runnel 数据面 + 发送侧流控的集成验证负载。
//!
//! 剧本：两次睡眠（timer 通路观测）→ 阻塞等 init 的 Invitation 消息 →
//! 消费 Invitation → 写入 8192 字节校验模式（跨回绕、逐批摇铃）→ EOF+摇铃 →
//! 接收流控验证请求：填满目标邮箱、确认满箱错误、在 WRITABLE 上阻塞，
//! 被 init 腾位唤醒后补发末尾消息 → 退出（触发对端 PEER_CLOSED）。

#![no_std]

use rinlib::{
    env,
    ipc::{message::{send, wait_message}, notification, wait::wait_many},
    preclude::*,
    shared::{
        call::SystemCallError,
        message::MAILBOX_CAPACITY,
        object::ObjectSignals,
        startup::TAG_MAILBOX_OWNER,
        wait::{WaitItem, WAIT_DEADLINE_INFINITE},
    },
    sys_sleep,
};
use librunnel::blocking;

/// 隧道页映射地址：与 init 约定的一致（各自进程空间内的同一常量）。
const TUNNEL_VA: usize = 0x4000_0000;
const STREAM_LEN: usize = 8192;
/// 与 init 约定的流控验证消息号（见 init 的 WRITABLE_WAKE_* 常量）。
const WRITABLE_WAKE_REQUEST: u64 = 640;
const WRITABLE_WAKE_FILL: u64 = 641;
const WRITABLE_WAKE_TAIL: u64 = 642;

fn main() {
    debug!("Hello, pm!");
    // 服务出生自带的邮箱 owner（StartupBlock 授予；见 shared::startup）。
    let mailbox = env::startup_handle(TAG_MAILBOX_OWNER).expect("pm: mailbox owner grant is missing");
    // sleep 异步通路验证：登记期限 → Waiting → timer 唤醒 → 继续。
    unsafe {
        sys_sleep(30).expect("sleep");
        sys_sleep(10).expect("sleep again");
    }
    debug!("awake after two sleeps");

    // 阻塞等 init 转移 Tunnel Invitation（消息到达 → WaitMany 唤醒）。
    let message = match wait_message(mailbox) {
        Ok(r) => r,
        Err(e) => {
            debug!("wait_message failed: {:?}", e);
            return;
        }
    };
    if message.header.payload_len != 0 || message.handles.len() != 1 {
        debug!("unexpected message kind {}", message.header.kind);
        return;
    }
    let invitation = message.handles[0];

    let mut tunnel = match blocking::attach_producer(invitation, TUNNEL_VA) {
        Ok(t) => t,
        Err(e) => {
            debug!("tunnel attach failed: {:?}", e);
            return;
        }
    };
    debug!("tunnel attached");

    // 校验模式写入：i%251+1，跨回绕分批，每批落页即摇铃。
    let mut sent = 0usize;
    let mut chunk = [0u8; 512];
    while sent < STREAM_LEN {
        let n = (STREAM_LEN - sent).min(chunk.len());
        for (i, b) in chunk.iter_mut().enumerate().take(n) {
            *b = ((sent + i) % 251 + 1) as u8;
        }
        if let Err(e) = tunnel.write_all(&chunk[..n]) {
            debug!("stream write failed at {}: {:?}", sent, e);
            return;
        }
        sent += n;
    }
    if let Err(e) = tunnel.finish() {
        debug!("finish failed: {:?}", e);
        return;
    }
    debug!("stream written {} bytes", sent);

    // 流控验证：请求携带 [目标邮箱 sender(WRITE|WAIT)、确认 signaler、
    // 虚假唤醒 signaler]。内联 send/wait 循环：醒来后再撞满箱即为虚假
    // 唤醒（唤醒必须由腾位引起），置 spin 位供 init 校验。
    let message = match wait_message(mailbox) {
        Ok(r) => r,
        Err(e) => {
            debug!("wake request wait failed: {:?}", e);
            return;
        }
    };
    if message.header.kind != WRITABLE_WAKE_REQUEST || message.handles.len() != 3 {
        debug!("unexpected wake request kind {}", message.header.kind);
        return;
    }
    let target = message.handles[0];
    let done = message.handles[1];
    let spin = message.handles[2];

    for _ in 0..MAILBOX_CAPACITY {
        send(target, WRITABLE_WAKE_FILL, &[], &[]).expect("wake fill failed");
    }
    assert!(matches!(
        send(target, WRITABLE_WAKE_FILL, &[], &[]),
        Err(SystemCallError::MailboxFull)
    ));
    // 满箱错误是可观测失败；确认后置位通知，随后阻塞在 WRITABLE 上。
    notification::signal(done, 1).expect("wake confirm signal failed");
    let items = [WaitItem::new(
        target,
        ObjectSignals::WRITABLE | ObjectSignals::CLOSED,
        0,
    )];
    let mut woke = false;
    loop {
        match send(target, WRITABLE_WAKE_TAIL, &[], &[]) {
            Ok(()) => break,
            Err(SystemCallError::MailboxFull) => {
                if woke {
                    notification::signal(spin, 1).expect("spurious wake signal failed");
                }
                let result = wait_many(&items, WAIT_DEADLINE_INFINITE).expect("writable wait failed");
                assert!(result.observed.intersects(ObjectSignals::WRITABLE));
                woke = true;
            }
            Err(e) => panic!("unexpected tail send error: {:?}", e),
        }
    }
    debug!("writable wake passed");
}
