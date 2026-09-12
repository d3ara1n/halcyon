//! pm：受托进程管理服务 + IPC 集成验证负载。
//!
//! 剧本：两次睡眠（timer 通路观测）→ 阻塞等 init 的 Invitation 消息 →
//! 消费 Invitation → 写入 65536 字节校验模式（跨回绕、逐批摇铃）→ EOF+摇铃 →
//! 接收流控验证请求：填满目标邮箱、确认满箱错误、在 WRITABLE 上阻塞，
//! 被 init 腾位唤醒后补发末尾消息 → 收束显式委托的 pm_domain 子域
//! （枚举 → 派生 kill → drain → 封口）→ 退出（触发对端 PEER_CLOSED）。
//!
//! pm 只持委托域的 JobControl（MANAGE|READ|WAIT，经 StartupBlock grants
//! 交付），域外无任何进程管理 authority；递归收束是本服务组合内核原语
//! 的用户态政策，init 保留直接收束权作兜底。

#![no_std]

use libprocess::{
    DEFAULT_SUPERVISION_POLICY, DERIVED_CONTROL_RIGHTS, SupervisionTarget, collect_process,
    enumerate_members, job_kill,
};
use librunnel::blocking;
use rinlib::{
    env,
    ipc::{
        message::{send, wait_message},
        notification,
        object::close,
        wait::wait_many,
    },
    preclude::*,
    process,
    shared::{
        call::SystemCallError,
        message::MAILBOX_CAPACITY,
        object::{Handle, ObjectSignals},
        proc::{JobMemberKind, ProcessExitReason},
        wait::{WAIT_TIMEOUT_INFINITE, WaitItem},
    },
    sys_sleep,
};

const STREAM_LEN: usize = 65536;
/// 与 init 约定的流控验证消息号（见 init 的 WRITABLE_WAKE_* 常量）。
const WRITABLE_WAKE_REQUEST: u64 = 640;
const WRITABLE_WAKE_FILL: u64 = 641;
const WRITABLE_WAKE_TAIL: u64 = 642;

fn main() {
    debug!("Hello, pm!");
    // 服务出生自带的邮箱 owner（StartupBlock Handle[0]）。
    let mailbox = env::startup_handle(0).expect("pm: mailbox owner grant is missing");
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

    let mut tunnel = match blocking::attach_producer(invitation, rinlib::mm::Placement::Anywhere) {
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
                let result =
                    wait_many(&items, WAIT_TIMEOUT_INFINITE).expect("writable wait failed");
                assert!(result.observed.intersects(ObjectSignals::WRITABLE));
                woke = true;
            }
            Err(e) => panic!("unexpected tail send error: {:?}", e),
        }
    }
    debug!("writable wake passed");

    // —— 委托域管理：pm 作为受托管理者收束显式委托的子域 ——
    // StartupBlock Handle[1] = init 授出的 pm_domain JobControl。
    let domain = env::startup_handle(1).expect("pm: delegated domain control is missing");
    manage_delegated_domain(domain);
    debug!("pm: delegated domain managed");
}

/// 委托域管理：成员使用有限 wait/drain/query policy；任何局部失败保留派生
/// control 并升级到域级 JobKill。域级仍失败时不关闭 authority，明确交回持有独立
/// control 的 init 接管。
fn manage_delegated_domain(domain: Handle) {
    let members = match enumerate_members(domain, JobMemberKind::MemberProcesses) {
        Ok(members) => members,
        Err(error) => {
            debug!("pm: domain enumerate failed, escalating: {:?}", error);
            return escalate_domain(domain, None);
        }
    };
    for pid in members {
        let control = match process::derive_job(
            domain,
            JobMemberKind::MemberProcesses,
            pid,
            DERIVED_CONTROL_RIGHTS,
        ) {
            Ok(control) => control,
            Err(SystemCallError::ObjectNotFound) => continue,
            Err(error) => {
                debug!(
                    "pm: domain derive pid {} failed, escalating: {:?}",
                    pid, error
                );
                return escalate_domain(domain, None);
            }
        };
        if let Err(error) = process::kill(control, 0x66) {
            debug!(
                "pm: domain kill pid {} failed, escalating: {:?}",
                pid, error
            );
            return escalate_domain(domain, Some(control));
        }
        match collect_process(
            SupervisionTarget::new(pid, control),
            DEFAULT_SUPERVISION_POLICY,
        ) {
            Ok(collected)
                if collected.snapshot.reason == ProcessExitReason::Killed as u32
                    && collected.snapshot.code == 0x66 =>
            {
                debug!(
                    "pm: delegated member pid {} collected: Dead/Killed/0x66",
                    pid
                );
            }
            Ok(collected) => {
                debug!(
                    "pm: delegated member pid {} inconsistent terminal reason={}, code={}",
                    pid, collected.snapshot.reason, collected.snapshot.code
                );
                return escalate_domain(domain, None);
            }
            Err(failure) => {
                debug!(
                    "pm: delegated member pid {} supervision failed at {:?}: {:?}, progress {:?}",
                    pid, failure.stage, failure.cause, failure.progress
                );
                return escalate_domain(domain, Some(failure.target.into_control()));
            }
        }
    }
    escalate_domain(domain, None);
}

fn escalate_domain(domain: Handle, retained_process: Option<Handle>) {
    match job_kill(domain, 0x66) {
        Ok(()) => {
            if let Some(control) = retained_process {
                let _ = unsafe { close(control) };
            }
            let snapshot = process::query_job(domain);
            debug!(
                "pm: delegated domain seal passed (state {:?})",
                snapshot.as_ref().map(|state| state.state)
            );
            let _ = unsafe { close(domain) };
        }
        Err(failure) => {
            debug!(
                "pm: delegated domain unmanaged; init handoff required: stage={:?}, cause={:?}, progress={:?}, retained_process={}",
                failure.stage,
                failure.cause,
                failure.progress,
                retained_process.is_some() || failure.process.is_some()
            );
            // retained_process、failure.process 与 domain entry 都留在本进程表中；
            // pm 退出后 ProcessDrain 关闭它们，init 的独立 domain control 不受影响。
        }
    }
}
