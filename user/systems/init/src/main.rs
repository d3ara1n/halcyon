//! init：消息 + 信号 + 隧道/Runnel 全通路的集成验证负载。
//!
//! 剧本：
//! 1. 创建显式 Mailbox 并同步自发自收一条消息；
//! 2. 创建 Runnel 隧道，把 Invitation 经启动授权的 pm sender 转移，随后阻塞读
//!    8192 字节（跨回绕、走到达移交唤醒）并校验数据；
//! 3. 与 pm 协作验证发送侧流控：pm 填满目标邮箱后在 WRITABLE 上阻塞，
//!    init 腾出容量唤醒，pm 补发尾部消息；
//! 4. 等 pm 退出后的 PEER_CLOSED 终态位，关闭本端（帧归还）。

#![no_std]

use rinlib::{
    env,
    ipc::{
        message::{create, discard, make_send_once, receive, send, wait_message},
        notification,
        object::close,
        tunnel as tunnel_sys,
        wait::wait_many,
    },
    preclude::*,
    shared::{
        call::SystemCallError,
        message::{HandleMove, MAILBOX_CAPACITY},
        object::{Handle, ObjectSignals, Rights},
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
    test_send_once();
    test_writable_level();

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

    // —— 流控唤醒面：pm 填满目标邮箱后在 WRITABLE 上阻塞，腾位唤醒 ——
    test_writable_wake(pm_mailbox);

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

/// 与 pm 约定的流控验证消息号：请求携带 [目标邮箱 sender、确认 signaler、
/// 虚假唤醒 signaler]；pm 填满目标邮箱后回样确认，末尾补发 WRITABLE_WAKE_TAIL。
const WRITABLE_WAKE_REQUEST: u64 = 640;
const WRITABLE_WAKE_FILL: u64 = 641;
const WRITABLE_WAKE_TAIL: u64 = 642;

/// 一次性投递权（send-once）：本进程内验证 mint、用后即摘、
/// 经消息转移后由接收方一次性使用，以及原 sender 不受影响。
fn test_send_once() {
    let mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSFER | Rights::DUPLICATE,
    )
    .expect("send-once mailbox create failed");
    let once = make_send_once(
        mailbox.peer,
        Rights::WRITE | Rights::WAIT | Rights::TRANSFER,
    )
    .expect("make send once failed");
    send(once, 900, &[1], &[]).expect("send once failed");
    assert!(matches!(
        send(once, 901, &[], &[]),
        Err(SystemCallError::StaleHandle)
    ));

    let once = make_send_once(
        mailbox.peer,
        Rights::WRITE | Rights::WAIT | Rights::TRANSFER,
    )
    .expect("transferred send-once mint failed");
    let moves = [HandleMove { handle: once, rights: Rights::WRITE }];
    send(mailbox.peer, 902, &[], &moves).expect("send-once transit failed");
    let first = receive(mailbox.owner).expect("send-once receive failed");
    assert_eq!(first.header.kind, 900);
    let second = receive(mailbox.owner).expect("send-once transit receive failed");
    assert_eq!(second.header.kind, 902);
    send(second.handles[0], 903, &[2], &[]).expect("transferred once send failed");
    assert!(matches!(
        send(second.handles[0], 904, &[], &[]),
        Err(SystemCallError::StaleHandle)
    ));

    // 原 sender 仍可长期使用，不受派生影响。
    send(mailbox.peer, 905, &[], &[]).expect("original sender still usable");
    for expected in [903u64, 905] {
        let message = receive(mailbox.owner).expect("tail receive failed");
        assert_eq!(message.header.kind, expected);
    }

    // 满箱失败不消费：撞 MailboxFull 后腾位，同一 once 仍可投递。
    let full = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSFER | Rights::DUPLICATE,
    )
    .expect("send-once full mailbox create failed");
    let once = make_send_once(full.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSFER)
        .expect("send-once full mint failed");
    for _ in 0..MAILBOX_CAPACITY {
        send(full.peer, 0, &[], &[]).expect("send-once full fill failed");
    }
    assert!(matches!(
        send(once, 910, &[], &[]),
        Err(SystemCallError::MailboxFull)
    ));
    discard(full.owner).expect("send-once full make-room failed");
    send(once, 911, &[], &[]).expect("failed send must not consume once");
    assert!(matches!(
        send(once, 912, &[], &[]),
        Err(SystemCallError::StaleHandle)
    ));
    for _ in 0..MAILBOX_CAPACITY {
        discard(full.owner).expect("send-once full drain failed");
    }
    close(full.peer).expect("send-once full sender close failed");
    close(full.owner).expect("send-once full owner close failed");

    // once 同时作为发送目标与 transit move：作为 move 先被摘除，目标消费
    // 遇 StaleHandle 无害；接收方取得该 once 并一次性使用。
    let both = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSFER | Rights::DUPLICATE,
    )
    .expect("send-once both mailbox create failed");
    let once = make_send_once(both.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSFER)
        .expect("send-once both mint failed");
    assert!(matches!(
        make_send_once(once, Rights::WRITE),
        Err(SystemCallError::RightsDenied)
    ));
    let moves = [HandleMove { handle: once, rights: Rights::WRITE }];
    send(once, 920, &[], &moves).expect("send-once as target and move failed");
    assert!(matches!(
        send(once, 921, &[], &[]),
        Err(SystemCallError::StaleHandle)
    ));
    let message = receive(both.owner).expect("send-once both receive failed");
    assert_eq!(message.header.kind, 920);
    send(message.handles[0], 922, &[], &[]).expect("transferred once send failed");
    assert!(matches!(
        send(message.handles[0], 923, &[], &[]),
        Err(SystemCallError::StaleHandle)
    ));
    discard(both.owner).expect("send-once both drain failed");
    close(both.peer).expect("send-once both sender close failed");
    close(both.owner).expect("send-once both owner close failed");
    debug!("send-once passed");
}

/// WRITABLE 电平快路径：空箱即时就绪，填满后腾位重新就绪。
/// “满箱清零”的阻塞侧由 [`test_writable_wake`] 跨进程验证。
fn test_writable_level() {
    let mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT,
    )
    .expect("writable mailbox create failed");
    let result = wait_many(&[WaitItem::new(
        mailbox.peer,
        ObjectSignals::WRITABLE,
        1,
    )])
    .expect("empty mailbox must be writable");
    assert!(result.observed.intersects(ObjectSignals::WRITABLE));

    for _ in 0..MAILBOX_CAPACITY {
        send(mailbox.peer, 0, &[], &[]).expect("writable fill failed");
    }
    discard(mailbox.owner).expect("writable make-room failed");
    let result = wait_many(&[WaitItem::new(
        mailbox.peer,
        ObjectSignals::WRITABLE,
        2,
    )])
    .expect("mailbox below capacity must be writable");
    assert!(result.observed.intersects(ObjectSignals::WRITABLE));

    for _ in 0..MAILBOX_CAPACITY - 1 {
        discard(mailbox.owner).expect("writable drain failed");
    }
    close(mailbox.peer).expect("writable sender close failed");
    close(mailbox.owner).expect("writable owner close failed");
    debug!("writable level passed");
}

/// 跨进程流控唤醒：pm 填满目标邮箱后在 WRITABLE 上阻塞，本进程腾出一个
/// 位置唤醒它。pm 侧内联检测虚假唤醒（醒来后再撞满箱即置 spin 位），
/// 末尾校验 spin 为空——唤醒只能由腾位引起，证明等待路径真实走过。
fn test_writable_wake(pm_mailbox: Handle) {
    let target = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSFER,
    )
    .expect("wake target mailbox create failed");
    let done = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSFER,
    )
    .expect("wake done notification create failed");
    let spin = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSFER,
    )
    .expect("wake spin notification create failed");
    let moves = [
        HandleMove { handle: target.peer, rights: Rights::WRITE | Rights::WAIT },
        HandleMove { handle: done.peer, rights: Rights::SIGNAL },
        HandleMove { handle: spin.peer, rights: Rights::SIGNAL },
    ];
    send(pm_mailbox, WRITABLE_WAKE_REQUEST, &[], &moves)
        .expect("wake request send failed");

    // pm 确认已满后置位通知，此时它正阻塞在 WRITABLE 上。
    wait_many(&[WaitItem::new(done.owner, ObjectSignals::READABLE, 0)])
        .expect("wake notification wait failed");
    notification::take(done.owner, u64::MAX).expect("wake notification take failed");
    discard(target.owner).expect("wake make-room failed");

    // 队列：15 × FILL + TAIL。逐条校验，末尾必然是被唤醒后补发的 TAIL。
    for index in 0..MAILBOX_CAPACITY {
        let message = wait_message(target.owner).expect("wake drain failed");
        let expected = if index + 1 == MAILBOX_CAPACITY {
            WRITABLE_WAKE_TAIL
        } else {
            WRITABLE_WAKE_FILL
        };
        assert_eq!(message.header.kind, expected);
    }
    // TAIL 已入队即 pm 已退出循环；此前的任何虚假唤醒都会留下 spin 位。
    assert!(matches!(
        notification::take(spin.owner, u64::MAX),
        Err(SystemCallError::ObjectNotAvailable)
    ));
    close(done.owner).expect("wake done owner close failed");
    close(spin.owner).expect("wake spin owner close failed");
    close(target.owner).expect("wake target owner close failed");
    debug!("writable wake passed");
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
