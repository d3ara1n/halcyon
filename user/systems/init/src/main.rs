//! init：消息 + 信号 + 隧道/Runnel 全通路的集成验证负载。
//!
//! 剧本：
//! 1. 创建显式 Mailbox 并同步自发自收一条消息；
//! 2. 创建 Runnel 隧道，把 Invitation 经启动授权的 pm sender 转移，随后阻塞读
//!    8192 字节（跨回绕、走到达移交唤醒）并校验数据；
//! 3. 等 pm 退出后的 PEER_CLOSED 终态位，关闭本端（帧归还）。

#![no_std]

use rinlib::{
    env,
    ipc::{
        message::{create, discard, receive, send},
        notification,
        object::close,
        tunnel as tunnel_sys,
        wait::wait_many,
    },
    preclude::*,
    shared::{
        call::SystemCallError,
        message::{HandleMove, MAILBOX_CAPACITY},
        object::{ObjectSignals, Rights},
        startup::GRANT_PM_MAILBOX,
        wait::WaitItem,
    },
};
use librunnel::blocking;

/// 隧道页在本进程的映射地址（VA 分配器落地前由调用方自报）。
const TUNNEL_VA: usize = 0x4000_0000;
const LIFECYCLE_VA: usize = TUNNEL_VA + 0x1000;
const FAILED_ATTACH_VA: usize = TUNNEL_VA + 0x2000;
/// 验证数据量：超过环形容量（3968），强制写端分批与回绕。
const STREAM_LEN: usize = 8192;
const CONTROL_STRESS: usize = 128;
const TUNNEL_STRESS: usize = 64;

fn main() {
    debug!("Hello, init!");
    let pair = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSFER | Rights::DUPLICATE,
    )
    .expect("mailbox create failed");

    let event = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::WAIT | Rights::TRANSFER | Rights::DUPLICATE,
    )
    .expect("notification create failed");
    let moves = [HandleMove {
        handle: event.peer,
        rights: Rights::SIGNAL,
    }];

    // —— 同步消息 + Handle move + Notification/WaitMany 快路径 ——
    match send(pair.peer, 114, &[5u8, 1u8, 4u8], &moves) {
        Ok(()) => match receive(pair.owner) {
            Ok(message) => {
                debug!(
                    "message: kind={}, payload={:?}",
                    message.header.kind,
                    message.payload
                );
                let moved = message.handles[0];
                notification::signal(moved, 0x5).expect("notification signal failed");
                let result = wait_many(&[WaitItem::new(
                    event.owner,
                    ObjectSignals::READABLE,
                    7,
                )])
                .expect("notification wait failed");
                let bits = notification::take(event.owner, u64::MAX)
                    .expect("notification take failed");
                debug!("notification: cookie={}, bits={:#x}", result.cookie, bits);
                let _ = close(moved);
            }
            Err(e) => debug!("receive failed: {:?}", e),
        },
        Err(e) => debug!("send failed: {:?}", e),
    }
    let _ = close(event.owner);
    let _ = close(pair.peer);
    let _ = close(pair.owner);

    stress_control_plane();
    test_tunnel_lifecycle();

    // —— 数据面：建隧道 → Invitation 经消息面转移 → 阻塞读流 ——
    let (mut tunnel, invitation) = match blocking::create_consumer(TUNNEL_VA) {
        Ok(t) => t,
        Err(e) => {
            debug!("tunnel create failed: {:?}", e);
            return;
        }
    };
    debug!("tunnel created");
    let Some(pm_mailbox) = env::startup_handle(GRANT_PM_MAILBOX) else {
        debug!("pm mailbox grant is missing");
        return;
    };
    let invitation_move = [HandleMove {
        handle: invitation,
        rights: Rights::MAP,
    }];
    if let Err(e) = send(pm_mailbox, 514, &[], &invitation_move) {
        debug!("send tunnel invitation failed: {:?}", e);
        return;
    }

    let mut buf = [0u8; STREAM_LEN];
    match tunnel.read_exact_or_eof(&mut buf) {
        Ok(n) => {
            let ok = buf.iter().enumerate().all(|(i, &b)| b == (i % 251 + 1) as u8);
            debug!("stream received {} bytes, pattern {}", n, if ok { "ok" } else { "MISMATCH" });
        }
        Err(e) => debug!("stream read failed: {:?}", e),
    }

    // —— 事件面：等 pm 退出后的 PEER_CLOSED 终态位 ——
    let items = [WaitItem::new(
        tunnel.handle(),
        ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED,
        0,
    )];
    match wait_many(&items) {
        Ok(result) => debug!("peer closed observed: bits={:#x}", result.observed.raw()),
        Err(e) => debug!("peer-closed wait failed: {:?}", e),
    }
    let _ = tunnel.close();
}

fn stress_control_plane() {
    for index in 0..CONTROL_STRESS {
        let mailbox = create(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::TRANSFER | Rights::DUPLICATE,
        )
        .expect("stress mailbox create failed");
        let event = notification::create(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::SIGNAL | Rights::TRANSFER,
        )
        .expect("stress notification create failed");
        let moves = [HandleMove { handle: event.peer, rights: Rights::SIGNAL }];
        send(mailbox.peer, index as u64, &index.to_le_bytes(), &moves)
            .expect("stress send failed");
        assert!(matches!(close(event.peer), Err(SystemCallError::StaleHandle)));
        let message = receive(mailbox.owner).expect("stress receive failed");
        assert_eq!(message.header.kind, index as u64);
        assert_eq!(message.payload, index.to_le_bytes());
        notification::signal(message.handles[0], 1).expect("stress signal failed");
        assert_eq!(notification::take(event.owner, 1).expect("stress take failed"), 1);
        close(message.handles[0]).expect("stress moved handle close failed");
        close(event.owner).expect("stress notification owner close failed");
        close(mailbox.peer).expect("stress mailbox sender close failed");
        close(mailbox.owner).expect("stress mailbox owner close failed");
    }

    let mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSFER | Rights::DUPLICATE,
    )
    .expect("full mailbox create failed");
    for _ in 0..MAILBOX_CAPACITY {
        send(mailbox.peer, 0, &[], &[]).expect("mailbox fill failed");
    }
    let event = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSFER,
    )
    .expect("full mailbox notification create failed");
    let moves = [HandleMove { handle: event.peer, rights: Rights::SIGNAL }];
    assert!(matches!(
        send(mailbox.peer, 0, &[], &moves),
        Err(SystemCallError::MailboxFull)
    ));
    notification::signal(event.peer, 1).expect("failed Send must retain moved source");
    for _ in 0..MAILBOX_CAPACITY {
        discard(mailbox.owner).expect("mailbox discard failed");
    }
    close(event.peer).expect("retained signaler close failed");
    close(event.owner).expect("retained owner close failed");
    close(mailbox.peer).expect("full mailbox sender close failed");
    close(mailbox.owner).expect("full mailbox owner close failed");
    debug!("control-plane stress passed: {} transactions", CONTROL_STRESS);
}

fn test_tunnel_lifecycle() {
    for _ in 0..TUNNEL_STRESS {
        let abandoned = tunnel_sys::create(LIFECYCLE_VA).expect("lifecycle tunnel create failed");
        close(abandoned.peer).expect("invitation close failed");
        let result = wait_many(&[WaitItem::new(
            abandoned.owner,
            ObjectSignals::PEER_CLOSED,
            0,
        )])
        .expect("abandoned invitation wait failed");
        assert!(result.observed.intersects(ObjectSignals::PEER_CLOSED));
        close(abandoned.owner).expect("lifecycle endpoint close failed");

        let creator_closed =
            tunnel_sys::create(LIFECYCLE_VA).expect("closed-creator tunnel create failed");
        close(creator_closed.owner).expect("creator endpoint close failed");
        assert!(matches!(
            tunnel_sys::attach(creator_closed.peer, FAILED_ATTACH_VA),
            Err(SystemCallError::ObjectClosed)
        ));
        close(creator_closed.peer).expect("closed invitation close failed");
    }
    debug!("tunnel lifecycle stress passed: {} rounds", TUNNEL_STRESS);
}
