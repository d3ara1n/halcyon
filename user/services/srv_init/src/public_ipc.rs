//! 公共 IPC 的真实用户线程组合；不依赖概率要求某方必须赢自由竞争。

use alloc::{sync::Arc, vec, vec::Vec};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use libkernel::preclude::*;
use libkernel::{
    ipc::{
        message::{create, make_send_once, mint_sender, receive, send_raw, wait_message_until},
        object::{close, query},
        wait::wait_until,
    },
    shared::{
        call::SystemCallError,
        message::{HandleMove, MAILBOX_CAPACITY, PAYLOAD_MAX},
        object::{Handle, ObjectKind, ObjectSignals, Rights},
    },
    time,
};
use libruntime::thread;
use libthread::ThreadOutcome;

const MESSAGES: usize = MAILBOX_CAPACITY * 4;

/// WRITE 可反复使用且不依赖 DUPLICATE；同一表项的 Query/WAIT 不被普通 Send 排斥。
pub(super) fn shared_sender() {
    use libkernel::{ipc::wait_set::WaitSet, shared::wait_set::MemberRequest};
    const PER_WORKER: usize = MAILBOX_CAPACITY * 2;
    let inbox = create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::WRITE).unwrap();
    let sender = mint_sender(inbox.owner, 42, Rights::WRITE | Rights::WAIT).unwrap();
    let sender_id = query(sender.sender).unwrap().object_id;
    // SAFETY: 默认 peer 未交给消费者；所有线程共享的是另一份无 DUPLICATE 的表项。
    unsafe { close(inbox.peer) }.unwrap();
    let deadline = time::timeout_millis(30_000).unwrap();
    let go = Arc::new(AtomicBool::new(false));
    let mut workers = Vec::new();
    for index in 0..2 {
        let go = go.clone();
        let handle = sender.sender;
        workers.push(
            thread::Builder::new()
                .spawn(move || {
                    let set = WaitSet::create(1).unwrap();
                    while !go.load(Ordering::Acquire) {
                        thread::yield_now().unwrap();
                    }
                    for sequence in 0..PER_WORKER {
                        let description = query(handle).expect("shared Sender query failed");
                        assert_eq!(description.rights, Rights::WRITE | Rights::WAIT);
                        let token = set
                            .register(MemberRequest::new(handle, ObjectSignals::WRITABLE, 0))
                            .expect("shared Sender registration failed");
                        // 零期限探测允许此刻满箱；只要求授权入口不因别人的普通 Send 返回 Busy。
                        wait_until(
                            handle,
                            ObjectSignals::WRITABLE,
                            time::timeout_millis(0).unwrap(),
                        )
                        .expect("shared Sender wait probe failed");
                        set.remove(token).unwrap();
                        let kind = (index * PER_WORKER + sequence) as u64;
                        let payload = vec![index as u8; PAYLOAD_MAX];
                        loop {
                            // SAFETY: 普通发送无移交，父线程在全部 join 之前保留唯一表项。
                            match unsafe { send_raw(handle, kind, &payload, &[]) } {
                                Ok(()) => break,
                                Err(SystemCallError::MailboxFull) => {
                                    wait_writable(handle, deadline)
                                }
                                Err(error) => panic!("shared Sender send failed: {error:?}"),
                            }
                        }
                    }
                })
                .unwrap(),
        );
    }
    go.store(true, Ordering::Release);
    let mut seen = [false; PER_WORKER * 2];
    for _ in 0..seen.len() {
        let message = wait_message_until(inbox.owner, deadline).unwrap();
        let index = message.header.kind as usize;
        assert!(
            index < seen.len() && !seen[index],
            "shared Sender message lost or duplicated"
        );
        assert_eq!(message.header.sender_badge, 42);
        assert_eq!(message.header.sender_context_id, sender_id);
        assert_eq!(
            message
                .delivery
                .into_capability()
                .description()
                .unwrap()
                .related_object_id,
            sender_id
        );
        assert_eq!(
            message.payload,
            vec![(index / PER_WORKER) as u8; PAYLOAD_MAX]
        );
        seen[index] = true;
    }
    for worker in workers {
        assert!(
            matches!(worker.join(), ThreadOutcome::Returned { leave: Ok(()), .. }),
            "shared Sender worker did not finish"
        );
    }
    // SAFETY: 所有线程已 join，所有 Message/Delivery 已释放。
    unsafe { close(sender.sender) }.unwrap();
    let observed = wait_until(sender.lifetime, ObjectSignals::CLOSED, deadline).unwrap();
    assert!(observed.observed.intersects(ObjectSignals::CLOSED));
    unsafe { close(sender.lifetime) }.unwrap();
    unsafe { close(inbox.owner) }.unwrap();
    debug!(
        "shared Sender concurrent Send/Query/WAIT passed: no duplicate authority, complete delivery"
    );
}

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
        let moved_pair = make_send_once(sender.sender, Rights::WRITE | Rights::TRANSIT)
            .expect("threaded transit sender derivation failed");
        let moved = moved_pair.owner;
        // SAFETY: 本夹具不观察回复终态，关闭本次派生的独立观察表项。
        unsafe { libkernel::ipc::object::close(moved_pair.peer) }
            .expect("reply observer close failed");
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
                query(moved).unwrap().kind,
                ObjectKind::MailboxOnceSender as u32
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
        match worker.join() {
            ThreadOutcome::Returned {
                value,
                leave: Ok(()),
            } => delivered.extend(value),
            ThreadOutcome::Returned { leave: Err(()), .. }
            | ThreadOutcome::EnterFailed(())
            | ThreadOutcome::Incomplete => panic!("threaded Mailbox worker did not complete"),
        }
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
        let result = wait_until(lifetime, ObjectSignals::CLOSED, deadline).unwrap();
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
    debug!(
        "threaded mailbox receive contention passed: {} messages, gated FIFO, independent authority, complete delivery",
        MESSAGES
    );
}

fn wait_writable(sender: Handle, deadline: time::Deadline) {
    let result = wait_until(sender, ObjectSignals::WRITABLE, deadline)
        .expect("threaded Mailbox backpressure wait failed");
    assert!(
        result.observed.intersects(ObjectSignals::WRITABLE) && !time::expired(deadline).unwrap(),
        "threaded Mailbox retry exceeded its original deadline"
    );
}

fn inspect(mut message: libkernel::ipc::message::ReceivedMessage, seen: &mut Vec<usize>) {
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
    assert_eq!(message.header.sender_pid, super::init_pid() as u64);
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
    // 本消息由普通 Sender 发出，附带的是另一次派生的 OnceSender，身份各自独立。
    assert_ne!(description.object_id, message.header.sender_context_id);
    assert_eq!(description.badge, index + 1);
    assert_eq!(description.kind, ObjectKind::MailboxOnceSender as u32);
    assert_eq!(description.rights, Rights::WRITE);
    assert_eq!(
        message
            .delivery
            .into_capability()
            .description()
            .unwrap()
            .related_object_id,
        message.header.sender_context_id
    );
    seen.push(index as usize);
}

pub(super) fn committed_kill(job: Handle, image: &[u8]) {
    use libkernel::{
        ipc::{notification, object::duplicate, wait_set::WaitSet},
        process,
        shared::{
            proc::{HandleGrant, ProcessState, TerminationCause},
            wait_set::MemberRequest,
        },
    };
    let baseline = crate::root_pool_allocated().unwrap();
    let mut sources = Vec::new();
    let mut owners = Vec::new();
    for _ in 0..2 {
        let source =
            notification::create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL)
                .unwrap();
        let set = WaitSet::create_with_rights(
            1024,
            Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        )
        .unwrap();
        for entry in 0..1024 {
            set.register(MemberRequest::new(
                source.owner,
                ObjectSignals::READABLE,
                entry,
            ))
            .unwrap();
        }
        owners.push(set.into_capability().into_raw());
        sources.push(source);
    }
    let nested = process::create(job, crate::root_memory_pool(), crate::SUPERVISOR_RIGHTS).unwrap();
    process::grant(
        nested.builder,
        &[HandleGrant {
            handle: owners.pop().unwrap(),
            rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
        }],
        &mut [Handle::INVALID],
    )
    .unwrap();
    process::kill(nested.control).unwrap();
    // SAFETY: Kill 后停止组装，退还唯一 Builder；不改变已冻结终因。
    unsafe { close(nested.builder) }.unwrap();
    let ready =
        notification::create(Rights::READ | Rights::WAIT, Rights::SIGNAL | Rights::GRANT).unwrap();
    let done =
        notification::create(Rights::READ | Rights::WAIT, Rights::SIGNAL | Rights::GRANT).unwrap();
    let control = duplicate(nested.control, Rights::READ | Rights::WAIT | Rights::GRANT).unwrap();
    let started = crate::spawn(crate::SpawnRequest {
        memory_pool: crate::root_memory_pool(),
        parent: crate::init_process_info(),
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
                rights: Rights::READ | Rights::WAIT,
            },
            HandleGrant {
                handle: ready.peer,
                rights: Rights::SIGNAL,
            },
            HandleGrant {
                handle: done.peer,
                rights: Rights::SIGNAL,
            },
        ],
        control_rights: crate::SUPERVISOR_RIGHTS,
    })
    .expect("committed IPC kill target spawn failed");
    let deadline = time::timeout_millis(30_000).unwrap();
    wait_until(ready.owner, ObjectSignals::READABLE, deadline).unwrap();
    // 子线程将关闭非空集合；Kill 可落在关闭提交前后，两者都必须由内核稳定持有退休责任。
    process::kill(started.control).unwrap();
    for control in [started.control, nested.control] {
        let observed = wait_until(control, ObjectSignals::DONE, deadline).unwrap();
        assert!(
            observed.observed.intersects(ObjectSignals::DONE),
            "IPC process retirement timed out"
        );
        let snapshot = process::query(control).unwrap();
        assert_eq!(snapshot.state, ProcessState::Dead as u32);
        assert_eq!(snapshot.cause, TerminationCause::Killed as u32);
        assert_eq!(
            snapshot.detail,
            libkernel::shared::proc::TerminationDetail::PROCESS_CONTROL_KILL
        );
    }
    // SAFETY: 进程已完成独立退休，剩余父方观察/来源表项各关闭一次。
    for h in [started.control, nested.control, ready.owner, done.owner] {
        unsafe { close(h) }.unwrap();
    }
    for source in sources {
        unsafe { close(source.peer) }.unwrap();
        unsafe { close(source.owner) }.unwrap();
    }
    assert_eq!(libkernel::ipc::wait_set::abandoned_count(), 0);
    assert_eq!(
        crate::root_pool_allocated().unwrap(),
        baseline,
        "IPC process retirement failed to refund Pool charge"
    );
    debug!(
        "committed IPC caller kill passed: nonempty Close, caller exit, automatic target retirement and refund"
    );
}
