//! 公共 IPC 的真实用户线程组合；不依赖概率要求某方必须赢自由竞争。

use alloc::{sync::Arc, vec, vec::Vec};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use rinlib::preclude::*;
use rinlib::{
    ipc::{
        message::{create, make_send_once, mint_sender, receive, send_raw, wait_message_until},
        object::{close, query},
        wait::wait_until,
    },
    shared::{
        call::SystemCallError,
        message::{HandleMove, MAILBOX_CAPACITY, PAYLOAD_MAX},
        object::{Handle, HandleRole, ObjectSignals, Rights},
        wait::WaitItem,
    },
    thread, time,
};

const MESSAGES: usize = MAILBOX_CAPACITY * 4;

pub(super) fn threaded_receive() {
    let inbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT,
    )
    .expect("threaded Mailbox creation failed");
    let deadline = time::timeout_millis(30_000).expect("threaded Mailbox deadline failed");
    let turn = Arc::new(AtomicUsize::new(0));
    let concurrent = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();
    for index in 0..2 {
        let turn = turn.clone();
        let concurrent = concurrent.clone();
        let owner = inbox.owner;
        workers.push(
            thread::Builder::new()
                .stack_size(64 * 1024)
                .spawn(move || {
                    while turn.load(Ordering::Acquire) != index {
                        thread::yield_now().expect("Mailbox gate yield failed");
                    }
                    let mut seen = Vec::new();
                    let first =
                        wait_message_until(owner, deadline).expect("gated Mailbox receive failed");
                    assert_eq!(
                        first.header.kind, index as u64,
                        "gated cross-thread FIFO changed"
                    );
                    inspect(first, &mut seen);
                    turn.store(index + 1, Ordering::Release);
                    while !concurrent.load(Ordering::Acquire) {
                        thread::yield_now().expect("Mailbox release yield failed");
                    }
                    loop {
                        let message = wait_message_until(owner, deadline)
                            .expect("concurrent Mailbox receive failed");
                        if message.header.kind == u64::MAX {
                            break;
                        }
                        inspect(message, &mut seen);
                    }
                    assert!(
                        seen.windows(2).all(|pair| pair[0] < pair[1]),
                        "one receiver observed non-FIFO delivery order"
                    );
                    seen
                })
                .expect("threaded Mailbox receiver spawn failed"),
        );
    }
    let mut lifetimes = Vec::new();
    let mut full_hits = 0;
    for index in 0..MESSAGES {
        if index == MAILBOX_CAPACITY {
            while turn.load(Ordering::Acquire) != 2 {
                thread::yield_now().expect("Mailbox handoff yield failed");
            }
        }
        let sender = mint_sender(
            inbox.owner,
            index as u64 + 1,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .expect("threaded sender mint failed");
        lifetimes.push(sender.lifetime);
        let moved = make_send_once(sender.sender, Rights::WRITE | Rights::TRANSIT)
            .expect("threaded transit sender derivation failed");
        let moves = [HandleMove {
            handle: moved,
            rights: Rights::WRITE,
        }];
        let mut payload = vec![0u8; PAYLOAD_MAX];
        for chunk in payload.as_chunks_mut::<8>().0 {
            chunk.copy_from_slice(&(index as u64).to_le_bytes());
        }
        if index == MAILBOX_CAPACITY + 2 {
            // 两条 gated 消费后再补两条，消费者仍停在 release，队列确定性满箱。
            assert_eq!(
                unsafe { send_raw(sender.sender, index as u64, &payload, &moves) },
                Err(SystemCallError::MailboxFull),
                "gated traffic failed to reach full backpressure"
            );
            assert_eq!(
                query(moved).unwrap().role,
                HandleRole::MailboxSenderOnce as u32
            );
            full_hits += 1;
            concurrent.store(true, Ordering::Release);
            wait_writable(sender.sender, deadline);
        }
        loop {
            // SAFETY: 本测试唯一持有 move 的发送责任，失败保留原表项，成功后不再使用。
            match unsafe { send_raw(sender.sender, index as u64, &payload, &moves) } {
                Ok(()) => break,
                Err(SystemCallError::MailboxFull) => {
                    full_hits += 1;
                    wait_writable(sender.sender, deadline);
                }
                Err(error) => panic!("threaded Mailbox send failed: {error:?}"),
            }
        }
        // SAFETY: 原 sender 不再发送，消息/接收能力独立承担在途引用。
        unsafe { close(sender.sender) }.expect("threaded sender close failed");
    }
    for _ in 0..2 {
        loop {
            // SAFETY: 空消息无 consuming moves，默认 peer 在全部 worker join 前存活。
            match unsafe { send_raw(inbox.peer, u64::MAX, &[], &[]) } {
                Ok(()) => break,
                Err(SystemCallError::MailboxFull) => {
                    wait_writable(inbox.peer, deadline);
                }
                Err(error) => panic!("threaded Mailbox terminator send failed: {error:?}"),
            }
        }
    }
    let mut delivered = Vec::new();
    for worker in workers {
        delivered.extend(worker.join());
    }
    delivered.sort_unstable();
    assert!(
        full_hits > 0,
        "threaded traffic never exercised backpressure"
    );
    assert_eq!(
        delivered,
        (0..MESSAGES).collect::<Vec<_>>(),
        "threaded Mailbox lost or duplicated a message"
    );
    assert!(
        matches!(
            receive(inbox.owner),
            Err(SystemCallError::ObjectNotAvailable)
        ),
        "threaded Mailbox retained an unconsumed queue head"
    );
    for lifetime in lifetimes {
        let result = wait_until(
            &[WaitItem::new(lifetime, ObjectSignals::CLOSED, 0)],
            deadline,
        )
        .unwrap();
        assert!(
            result.observed.intersects(ObjectSignals::CLOSED),
            "threaded Delivery retained its sender"
        );
        // SAFETY: worker 都已离场，仅本测试保留这些观察 entry。
        unsafe { close(lifetime) }.unwrap();
    }
    // SAFETY: worker 都已 join，无消费者再使用 owner/peer。
    unsafe { close(inbox.peer) }.unwrap();
    unsafe { close(inbox.owner) }.unwrap();
    assert_eq!(
        thread::stack_cleanup_snapshot().abandoned,
        0,
        "threaded Mailbox abandoned a stack mapping"
    );
    debug!(
        "threaded mailbox receive contention passed: {} messages, gated FIFO, independent authority, complete delivery",
        MESSAGES
    );
}

fn wait_writable(sender: Handle, deadline: time::Deadline) {
    let result = wait_until(
        &[WaitItem::new(sender, ObjectSignals::WRITABLE, 0)],
        deadline,
    )
    .expect("threaded Mailbox backpressure wait failed");
    assert!(
        result.observed.intersects(ObjectSignals::WRITABLE) && !time::expired(deadline).unwrap(),
        "threaded Mailbox retry exceeded its original deadline"
    );
}

fn inspect(mut message: rinlib::ipc::message::ReceivedMessage, seen: &mut Vec<usize>) {
    let index = message.header.kind;
    assert!(
        index < MESSAGES as u64,
        "threaded Mailbox delivered an invalid index"
    );
    assert_eq!(
        message.header.sender_badge,
        index + 1,
        "threaded Mailbox badge crossed a delivery"
    );
    assert_eq!(message.header.sender_pid, rinlib::env::pid() as u64);
    assert_eq!(message.payload.len(), PAYLOAD_MAX);
    for chunk in message.payload.as_chunks::<8>().0 {
        assert_eq!(
            chunk,
            &index.to_le_bytes(),
            "threaded Mailbox payload crossed a delivery"
        );
    }
    let capability = message
        .handles
        .take(0)
        .expect("threaded Mailbox transit capability missing");
    let description = capability.description().unwrap();
    assert_eq!(description.object_id, message.header.sender_context_id);
    assert_eq!(description.badge, index + 1);
    assert_eq!(description.role, HandleRole::MailboxSenderOnce as u32);
    assert_eq!(description.rights, Rights::WRITE);
    assert_eq!(
        message
            .delivery
            .into_capability()
            .description()
            .unwrap()
            .related_object_id,
        description.object_id
    );
    seen.push(index as usize);
}

pub(super) fn committed_kill(job: rinlib::shared::object::Handle, image: &[u8]) {
    use rinlib::{
        ipc::{notification, object::duplicate, wait_set::WaitSet},
        process,
        shared::proc::{HandleGrant, ProcessExitReason, ProcessState},
    };
    let mut sources = Vec::new();
    let monitor = WaitSet::create(2).unwrap();
    let mut owners = Vec::new();
    for cookie in 0..2 {
        let source =
            notification::create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL)
                .unwrap();
        let set = WaitSet::create_with_rights(
            1024,
            Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        )
        .unwrap();
        for entry in 0..1024 {
            set.register(WaitItem::new(source.owner, ObjectSignals::READABLE, entry))
                .unwrap();
        }
        monitor
            .register(WaitItem::new(set.handle(), ObjectSignals::CLOSED, cookie))
            .unwrap();
        owners.push(set.into_capability().into_raw());
        sources.push(source);
    }
    let dropped = WaitSet::create(1).unwrap();
    let drop_source =
        notification::create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL).unwrap();
    dropped
        .register(WaitItem::new(drop_source.owner, ObjectSignals::READABLE, 7))
        .unwrap();
    let stale = dropped.handle();
    drop(dropped.into_capability());
    assert_eq!(
        query(stale),
        Err(SystemCallError::StaleHandle),
        "converted WaitSet capability Drop retained its owner"
    );
    // SAFETY: 转换后的非空集合已由 Drop 等到内核退休，不再观察此来源。
    unsafe { close(drop_source.peer) }.unwrap();
    unsafe { close(drop_source.owner) }.unwrap();
    let baseline = crate::root_pool_allocated().unwrap();
    let nested = process::create(job, crate::SUPERVISOR_RIGHTS).unwrap();
    let pool = duplicate(crate::root_memory_pool(), Rights::GRANT).unwrap();
    process::bind_memory(nested.builder, pool).unwrap();
    process::grant(
        nested.builder,
        &[HandleGrant {
            handle: owners.pop().unwrap(),
            rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
        }],
        &mut [Handle::INVALID],
    )
    .unwrap();
    process::kill(nested.control, 0x130).unwrap();
    // SAFETY: Building builder 不再组装，Kill 已冻结终因，关闭仅退还该 affine owner。
    unsafe { close(nested.builder) }.unwrap();
    let ready =
        notification::create(Rights::READ | Rights::WAIT, Rights::SIGNAL | Rights::GRANT).unwrap();
    let drain_done =
        notification::create(Rights::READ | Rights::WAIT, Rights::SIGNAL | Rights::GRANT).unwrap();
    let control = duplicate(nested.control, Rights::MANAGE | Rights::GRANT).unwrap();
    let started = crate::spawn(crate::SpawnRequest {
        memory_pool: crate::root_memory_pool(),
        job,
        image,
        payload: b"ipc-kill",
        grants: &[
            HandleGrant {
                handle: owners.pop().unwrap(),
                rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
            },
            HandleGrant {
                handle: control,
                rights: Rights::MANAGE,
            },
            HandleGrant {
                handle: ready.peer,
                rights: Rights::SIGNAL,
            },
            HandleGrant {
                handle: drain_done.peer,
                rights: Rights::SIGNAL,
            },
        ],
        control_rights: crate::SUPERVISOR_RIGHTS,
    })
    .expect("committed IPC kill target spawn failed");
    let deadline = time::timeout_millis(30_000).unwrap();
    wait_until(
        &[WaitItem::new(ready.owner, ObjectSignals::READABLE, 0)],
        deadline,
    )
    .unwrap();
    // 同 authority 的探针观察 ObjectBusy，且目标线程尚未发布 syscall 返回，
    // 才把 caller 视为处于同一在途 Drain 周期。精确的 active/parked 取消窗口
    // 由内核确定性夹具覆盖；这里证明真实用户线程退出与管理者接管的组合。
    loop {
        match process::drain(nested.control, 1) {
            Err(SystemCallError::ObjectBusy) => {
                let done = wait_until(
                    &[WaitItem::new(drain_done.owner, ObjectSignals::READABLE, 0)],
                    time::Deadline::at(0),
                )
                .unwrap();
                assert!(
                    !done.observed.intersects(ObjectSignals::READABLE),
                    "ownership probe observed a Drain that had already returned"
                );
                break;
            }
            Ok(result) => {
                assert_ne!(
                    result.status,
                    rinlib::shared::proc::ProcessDrainStatus::Complete as u32,
                    "probe completed the target before the captured Drain acquired ownership"
                );
            }
            Err(error) => panic!("committed IPC ownership probe failed: {error:?}"),
        }
        thread::yield_now().expect("IPC ownership probe yield failed");
        assert!(
            !time::expired(deadline).unwrap(),
            "committed IPC Drain never exposed its active owner"
        );
    }
    process::kill(started.control, 0x131).unwrap();
    wait_until(
        &[WaitItem::new(
            started.control,
            ObjectSignals::REAPABLE | ObjectSignals::CLOSED,
            0,
        )],
        deadline,
    )
    .unwrap();
    process::drain_to_completion(started.control).unwrap();
    let snapshot = process::query(started.control).unwrap();
    assert_eq!(snapshot.state, ProcessState::Dead as u32);
    assert_eq!(snapshot.reason, ProcessExitReason::Killed as u32);
    assert_eq!(snapshot.code, 0x131);
    // 捕获请求被 kill 时目标可能已自然结束；否则由原 control 接同一游标收口。
    loop {
        match process::drain(nested.control, 128) {
            Ok(result)
                if result.status == rinlib::shared::proc::ProcessDrainStatus::Complete as u32 =>
            {
                break;
            }
            Ok(_) => (),
            Err(SystemCallError::ObjectBusy) => {
                thread::yield_now().expect("IPC retirement yield failed");
            }
            Err(error) => panic!("committed IPC target retirement failed: {error:?}"),
        }
        assert!(
            !time::expired(deadline).unwrap(),
            "captured Native cancellation retained its batch"
        );
    }
    let snapshot = process::query(nested.control).unwrap();
    assert_eq!(
        (snapshot.state, snapshot.reason, snapshot.code),
        (
            ProcessState::Dead as u32,
            ProcessExitReason::Killed as u32,
            0x130
        ),
        "builder close changed the nested target's frozen terminal result"
    );
    let mut closed = 0;
    while closed != 3 {
        monitor.wait(deadline).unwrap();
        for record in monitor.receive(2).unwrap() {
            assert!(
                record.observed.intersects(ObjectSignals::CLOSED),
                "IPC exit monitor lost a close commit"
            );
            closed |= 1 << record.cookie;
        }
    }
    monitor.close().map_err(|(_, error)| error).unwrap();
    // SAFETY: 两个进程已完整退休，monitor已关闭，不再存在这些 source/control 的用户借用。
    for h in [
        started.control,
        nested.control,
        ready.owner,
        drain_done.owner,
    ] {
        unsafe { close(h) }.unwrap();
    }
    for source in sources {
        unsafe { close(source.peer) }.unwrap();
        unsafe { close(source.owner) }.unwrap();
    }
    assert_eq!(rinlib::ipc::wait_set::abandoned_count(), 0);
    assert_eq!(
        crate::root_pool_allocated().unwrap(),
        baseline,
        "committed IPC target processes failed to refund their Pool charge"
    );
    debug!(
        "committed IPC caller kill passed: captured in-flight Drain cycle, caller exit, manager takeover, Close, and target retirement"
    );
}
