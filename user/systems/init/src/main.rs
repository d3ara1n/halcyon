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
        message::{create, discard, make_send_once, mint_sender, receive, send, wait_message},
        notification,
        object::{close, duplicate},
        tunnel as tunnel_sys,
        wait::wait_many,
    },
    preclude::*,
    process,
    shared::{
        call::SystemCallError,
        message::{HandleMove, MAILBOX_CAPACITY},
        object::{Handle, ObjectSignals, Rights},
        proc::{HandleGrant, ProcessExitReason, ProcessState},
        wait::{WaitItem, WAIT_DEADLINE_INFINITE},
    },
};
use libprocess::{SpawnRequest, spawn};
use librunnel::blocking;

/// 受监督服务：init 保留的 control 与 pid。
struct Supervised {
    pid: u64,
    control: Handle,
}

/// init 自身 control（launch_bootstrap 安装为启动 Handle 1）。
fn self_control() -> Handle {
    env::startup_handle(1).expect("init must hold its own ProcessControl")
}

const SUPERVISOR_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);

/// 隧道页在本进程的映射地址（VA 分配器落地前由调用方自报）。
const TUNNEL_VA: usize = 0x4000_0000;
const LIFECYCLE_VA: usize = TUNNEL_VA + 0x1000;
const FAILED_ATTACH_VA: usize = TUNNEL_VA + 0x2000;
/// 验证数据量：超过环形容量（3968），强制写端分批与回绕。
const STREAM_LEN: usize = 8192;
const CONTROL_STRESS: usize = 128;
const TUNNEL_STRESS: usize = 64;

fn launch_test_services() -> Result<(Handle, alloc::vec::Vec<Supervised>), (&'static str, alloc::vec::Vec<Supervised>)> {
    let root_job = env::startup_handle(0)
        .ok_or(("missing root job", alloc::vec::Vec::new()))?;
    let pm_mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        Rights::WRITE
            | Rights::WAIT
            | Rights::TRANSIT
            | Rights::GRANT
            | Rights::DUPLICATE,
    )
    .map_err(|_| ("pm mailbox create failed", alloc::vec::Vec::new()))?;
    let control_rights = SUPERVISOR_RIGHTS;
    let mut supervised = alloc::vec::Vec::new();
    let mut pm_started = false;
    let result = tar::walk(env::startup_payload(), |entry| {
        if !entry.name.starts_with("bin/") || entry.name.ends_with('/') {
            return;
        }
        let pm_grants = [HandleGrant {
            handle: pm_mailbox.owner,
            rights: Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        }];
        let grants = if entry.name == "bin/srv_pm" {
            pm_grants.as_slice()
        } else {
            &[]
        };
        match spawn(SpawnRequest {
            job: root_job,
            image: entry.data,
            payload: &[],
            grants,
            control_rights,
        }) {
            Ok(process) => {
                debug!("started {} as pid {}", entry.name, process.pid);
                // 持久 init 保留 control：监督、等待与收束的 authority 源。
                if entry.name == "bin/srv_target" {
                    // live 外部 kill 正路径：目标处于 Waiting（Sleep 期限
                    // 等待）或 Running（4 核竞态）——分别验证 WaitContext
                    // 取消与 IPI 吸收；或已越过终止边界则幂等接受。
                    let target = alloc::vec::Vec::from([Supervised {
                        pid: process.pid,
                        control: process.control,
                    }]);
                    process::kill(target[0].control, 0x77)
                        .expect("live kill of a fresh process must be accepted");
                    kill_and_supervise(target);
                } else {
                    supervised.push(Supervised { pid: process.pid, control: process.control });
                }
                if entry.name == "bin/srv_pm" {
                    pm_started = true;
                }
            }
            Err(error) => debug!("failed to start {}: {:?}", entry.name, error),
        }
    });
    if let Err(error) = result {
        debug!("initfs parse failed: {:?}", error);
    }
    if !pm_started {
        let _ = close(pm_mailbox.owner);
        let _ = close(pm_mailbox.peer);
        // 已启动的服务仍需收束：失败路径统一携带 supervised 交回
        // kill_and_supervise，不丢弃。
        return Err(("pm service launch failed", supervised));
    }
    Ok((pm_mailbox.peer, supervised))
}

fn main() {
    debug!("Hello, init!");
    match run() {
        Ok(()) => {}
        Err((stage, supervised)) => {
            debug!("init test stage failed: {}", stage);
            kill_and_supervise(supervised);
        }
    }
    self_terminate();
}

/// 失败收尾：对保留 control 的服务 kill → 等待收束 → Drain → close，
/// 不打印成功后依赖 Drain 回调。
fn kill_and_supervise(supervised: alloc::vec::Vec<Supervised>) {
    for target in &supervised {
        let _ = process::kill(target.control, 0x1F);
    }
    if let Err(error) = supervise_services(supervised) {
        debug!("failure-path supervision degraded: {:?}", error);
    }
}

/// 终局：自终止（Running 目标的 live kill；不返回用户态）。
fn self_terminate() {
    if let Err(error) = process::kill(self_control(), 0x5AA) {
        debug!("BUG: self kill returned: {:?}", error);
    }
    debug!("BUG: self kill must not return to user");
}

/// 全部测试剧本。失败只短路后续阶段，不绕过收尾监督。
fn run() -> Result<(), (&'static str, alloc::vec::Vec<Supervised>)> {
    let (pm_mailbox, supervised) = match launch_test_services() {
        Ok(launched) => launched,
        Err(failure) => return Err(failure),
    };
    let pair = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("mailbox create failed");

    let event = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
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
                let result = wait_many(
                    &[
                        WaitItem::new(
                            event.owner,
                            ObjectSignals::READABLE,
                            7,
                        ),
                    ],
                    WAIT_DEADLINE_INFINITE,
                )
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

    test_capability_badges_and_affine_owners();
    stress_control_plane();
    test_tunnel_lifecycle();
    test_send_once();
    test_writable_level();

    // —— 数据面：建隧道 → Invitation 经消息面转移 → 阻塞读流 ——
    let (mut tunnel, invitation) = match blocking::create_consumer(TUNNEL_VA) {
        Ok(t) => t,
        Err(e) => {
            debug!("tunnel create failed: {:?}", e);
            return Err(("tunnel create failed", supervised));
        }
    };
    debug!("tunnel created");
    let invitation_move = [HandleMove {
        handle: invitation,
        rights: Rights::MAP,
    }];
    if let Err(e) = send(pm_mailbox, 514, &[], &invitation_move) {
        debug!("send tunnel invitation failed: {:?}", e);
        return Err(("send tunnel invitation failed", supervised));
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

    // —— live kill 正路径：Building 目标的确定性 kill/drain/终态验证 ——
    test_building_kill(env::startup_handle(0).expect("init holds root job"));

    // —— 监督闭环：等待全部服务 REAPABLE/CLOSED，Drain 至 Complete，
    // 查询稳定终态后释放 control。对象 close 回调（含 pm 隧道端点的
    // PEER_CLOSED 发布）发生在 Drain 期间——监督先于对端终态等待。 ——
    if let Err(error) = supervise_services(supervised) {
        debug!("supervision degraded: {:?}", error);
    }
    debug!("all services supervised to completion");

    // —— 事件面：对端终态位（Drain 已置位，电平等待立即返回）——
    let items = [WaitItem::new(
        tunnel.handle(),
        ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED,
        0,
    )];
    match wait_many(&items, WAIT_DEADLINE_INFINITE) {
        Ok(result) => debug!("peer closed observed: bits={:#x}", result.observed.raw()),
        Err(e) => debug!("peer-closed wait failed: {:?}", e),
    }
    let _ = tunnel.close();
    Ok(())
}

/// 监督循环：对保留的每个 control 等待 REAPABLE|CLOSED，Drain 至
/// Complete，再以固定宽快照确认终态。逐项推进，全部完成后返回。
fn supervise_services(
    mut supervised: alloc::vec::Vec<Supervised>,
) -> Result<(), SystemCallError> {
    while !supervised.is_empty() {
        let items: alloc::vec::Vec<WaitItem> = supervised
            .iter()
            .enumerate()
            .map(|(index, s)| {
                WaitItem::new(s.control, ObjectSignals::REAPABLE | ObjectSignals::CLOSED, index as u64)
            })
            .collect();
        let result = wait_many(&items, WAIT_DEADLINE_INFINITE)?;
        let index = result.cookie as usize;
        let Some(target) = supervised.get(index) else { break };
        let pid = target.pid;
        let control = target.control;
        let drained = process::drain_to_completion(control);
        let snapshot = process::query(control);
        match (drained, snapshot) {
            (Ok(work), Ok(snapshot)) => {
                debug!(
                    "pid {} supervised: work={}, state={}, reason={}, code={}",
                    pid,
                    work,
                    snapshot.state,
                    snapshot.reason,
                    snapshot.code
                );
            }
            (work, query) => {
                debug!("pid {} supervision degraded: drain={:?} query={:?}", pid, work, query);
            }
        }
        let _ = close(control);
        supervised.swap_remove(index);
    }
    Ok(())
}

/// Building 目标的确定性 kill：Create 后未 Start，kill 冻结终因
/// (Killed, code)，builder 关闭的 abandonment 竞争不覆盖；Drain 至
/// Complete 后 shell 快照应稳定报 Dead/Killed/code。
fn test_building_kill(root_job: Handle) {
    let created = match process::create(root_job, SUPERVISOR_RIGHTS) {
        Ok(created) => created,
        Err(error) => {
            debug!("building kill: create failed: {:?}", error);
            return;
        }
    };
    let before = process::query(created.control);
    if let Ok(snapshot) = before {
        debug!("building kill: initial state={}", snapshot.state);
    }
    if let Err(error) = process::kill(created.control, 0x123) {
        debug!("building kill: kill failed: {:?}", error);
        let _ = close(created.builder);
        let _ = close(created.control);
        return;
    }
    let _ = close(created.builder);
    let drained = process::drain_to_completion(created.control);
    let snapshot = process::query(created.control);
    match (drained, snapshot) {
        (Ok(_), Ok(snapshot))
            if snapshot.state == ProcessState::Dead as u32
                && snapshot.reason == ProcessExitReason::Killed as u32
                && snapshot.code == 0x123 =>
        {
            debug!("building kill passed: pid {} Dead/Killed/{:#x}", created.pid, snapshot.code);
        }
        (work, snapshot) => {
            debug!(
                "building kill FAILED: drain={:?} snapshot={:?}",
                work, snapshot
            );
        }
    }
    let _ = close(created.control);
}

/// 与 pm 约定的流控验证消息号：请求携带 [目标邮箱 sender、确认 signaler、
/// 虚假唤醒 signaler]；pm 填满目标邮箱后回样确认，末尾补发 WRITABLE_WAKE_TAIL。
const WRITABLE_WAKE_REQUEST: u64 = 640;
const WRITABLE_WAKE_FILL: u64 = 641;
const WRITABLE_WAKE_TAIL: u64 = 642;

/// badged sender 的来源盖章，以及 owner 的 GRANT/TRANSIT 运输边界。
fn test_capability_badges_and_affine_owners() {
    const BADGE: u64 = 0x51a7_0bad_f00d;
    assert!(matches!(
        create(
            Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::TRANSIT,
            Rights::WRITE,
        ),
        Err(SystemCallError::RightsDenied)
    ));
    let mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("badged mailbox create failed");
    let badged = mint_sender(
        mailbox.owner,
        BADGE,
        Rights::WRITE | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("badged sender mint failed");
    let copy = duplicate(badged, Rights::WRITE).expect("badged sender duplicate failed");
    assert!(matches!(
        mint_sender(mailbox.owner, BADGE + 1, Rights::SIGNAL),
        Err(SystemCallError::RightsDenied)
    ));
    let transport = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::TRANSIT,
    )
    .expect("capability transport mailbox create failed");

    send(mailbox.peer, 880, &[], &[]).expect("default sender send failed");
    send(badged, 881, &[], &[]).expect("badged sender send failed");
    send(copy, 882, &[], &[]).expect("badged sender copy send failed");
    for (kind, badge) in [(880, 0), (881, BADGE), (882, BADGE)] {
        let message = receive(mailbox.owner).expect("badged message receive failed");
        assert_eq!(message.header.kind, kind);
        assert_eq!(message.header.sender_pid, env::pid() as u64);
        assert_eq!(message.header.sender_badge, badge);
    }
    let once = make_send_once(badged, Rights::WRITE)
        .expect("badged send-once mint failed");
    send(once, 887, &[], &[]).expect("badged send-once send failed");
    let message = receive(mailbox.owner).expect("badged send-once receive failed");
    assert_eq!(message.header.sender_badge, BADGE);

    let transit = duplicate(badged, Rights::WRITE | Rights::TRANSIT)
        .expect("badged transit copy failed");
    let moves = [HandleMove {
        handle: transit,
        rights: Rights::WRITE,
    }];
    send(transport.peer, 888, &[], &moves).expect("badged sender transit failed");
    let transferred = receive(transport.owner)
        .expect("badged sender transit receive failed")
        .handles[0];
    send(transferred, 889, &[], &[]).expect("transferred badged sender send failed");
    let message = receive(mailbox.owner).expect("transferred badged message receive failed");
    assert_eq!(message.header.sender_badge, BADGE);
    close(transferred).expect("transferred badged sender close failed");
    close(copy).expect("badged sender copy close failed");
    close(badged).expect("badged sender close failed");

    assert!(matches!(
        duplicate(mailbox.owner, Rights::READ),
        Err(SystemCallError::RightsDenied)
    ));
    let owner_move = [HandleMove {
        handle: mailbox.owner,
        rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
    }];
    assert!(matches!(
        send(transport.peer, 883, &[], &owner_move),
        Err(SystemCallError::RightsDenied)
    ));
    send(mailbox.peer, 884, &[], &[]).expect("owner must survive rejected transit");
    assert_eq!(
        receive(mailbox.owner)
            .expect("owner receive after rejected transit failed")
            .header
            .kind,
        884
    );

    let event = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        Rights::SIGNAL,
    )
    .expect("affine notification create failed");
    assert!(matches!(
        mint_sender(event.owner, BADGE, Rights::WRITE),
        Err(SystemCallError::WrongObjectType)
    ));
    let owner_move = [HandleMove {
        handle: event.owner,
        rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
    }];
    assert!(matches!(
        send(transport.peer, 886, &[], &owner_move),
        Err(SystemCallError::RightsDenied)
    ));
    notification::signal(event.peer, 1).expect("notification owner must survive rejected transit");
    assert_eq!(
        notification::take(event.owner, 1)
            .expect("notification take after rejected transit failed"),
        1
    );

    close(event.peer).expect("notification signaler close failed");
    close(event.owner).expect("notification owner close failed");
    close(mailbox.peer).expect("default sender close failed");
    close(mailbox.owner).expect("mailbox owner close failed");
    close(transport.peer).expect("owner transport sender close failed");
    close(transport.owner).expect("owner transport owner close failed");
    debug!("capability badge and owner transport passed");
}

/// 一次性投递权（send-once）：本进程内验证 mint、用后即摘、
/// 经消息转移后由接收方一次性使用，以及原 sender 不受影响。
fn test_send_once() {
    let mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("send-once mailbox create failed");
    let once = make_send_once(
        mailbox.peer,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
    )
    .expect("make send once failed");
    send(once, 900, &[1], &[]).expect("send once failed");
    assert!(matches!(
        send(once, 901, &[], &[]),
        Err(SystemCallError::StaleHandle)
    ));

    let once = make_send_once(
        mailbox.peer,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
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
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("send-once full mailbox create failed");
    let once = make_send_once(full.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
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

    // once 同时作为发送目标与 transit move 会突破一次投递保证，必须在
    // 任何入队或摘除前整体拒绝；失败不消费 once。
    let both = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("send-once both mailbox create failed");
    let once = make_send_once(both.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        .expect("send-once both mint failed");
    assert!(matches!(
        make_send_once(once, Rights::WRITE),
        Err(SystemCallError::RightsDenied)
    ));
    let moves = [HandleMove { handle: once, rights: Rights::WRITE }];
    assert!(matches!(
        send(once, 920, &[], &moves),
        Err(SystemCallError::IllegalArgument)
    ));
    send(once, 921, &[], &[]).expect("rejected alias must not consume send-once");
    assert!(matches!(
        send(once, 922, &[], &[]),
        Err(SystemCallError::StaleHandle)
    ));
    let message = receive(both.owner).expect("send-once alias recovery receive failed");
    assert_eq!(message.header.kind, 921);
    assert!(message.handles.is_empty());
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
    let result = wait_many(
        &[
            WaitItem::new(
                mailbox.peer,
                ObjectSignals::WRITABLE,
                1,
            ),
        ],
        WAIT_DEADLINE_INFINITE,
    )
    .expect("empty mailbox must be writable");
    assert!(result.observed.intersects(ObjectSignals::WRITABLE));

    for _ in 0..MAILBOX_CAPACITY {
        send(mailbox.peer, 0, &[], &[]).expect("writable fill failed");
    }
    discard(mailbox.owner).expect("writable make-room failed");
    let result = wait_many(
        &[
            WaitItem::new(
                mailbox.peer,
                ObjectSignals::WRITABLE,
                2,
            ),
        ],
        WAIT_DEADLINE_INFINITE,
    )
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
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
    )
    .expect("wake target mailbox create failed");
    let done = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT,
    )
    .expect("wake done notification create failed");
    let spin = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT,
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
    wait_many(
        &[
            WaitItem::new(done.owner, ObjectSignals::READABLE, 0),
        ],
        WAIT_DEADLINE_INFINITE,
    )
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
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
        )
        .expect("stress mailbox create failed");
        let event = notification::create(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::SIGNAL | Rights::TRANSIT,
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
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("full mailbox create failed");
    for _ in 0..MAILBOX_CAPACITY {
        send(mailbox.peer, 0, &[], &[]).expect("mailbox fill failed");
    }
    let event = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT,
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
        let result = wait_many(
            &[
                WaitItem::new(
                    abandoned.owner,
                    ObjectSignals::PEER_CLOSED,
                    0,
                ),
            ],
            WAIT_DEADLINE_INFINITE,
        )
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
