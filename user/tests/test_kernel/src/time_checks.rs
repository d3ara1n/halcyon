//! 公共时间的真实用户调用与所有权检查；RPC 业务接受期限由执行专题拥有。

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};
use libkernel::{
    ipc::{
        message, notification,
        object::{close, query},
        wait::wait_until,
    },
    preclude::*,
    shared::{
        call::SystemCallError,
        message::{HandleMove, MAILBOX_CAPACITY},
        object::{ObjectSignals, Rights},
        wait::WaitReason,
    },
    time::{self, Deadline, Duration},
};
use libruntime::thread;
use libthread::ThreadOutcome;

pub(super) fn run() {
    let start = time::snapshot().unwrap();
    assert_eq!(start.reserved, 0);
    assert!(start.now_ns <= start.max_deadline_ns && start.resolution_ns != 0);
    assert_eq!(
        Duration::from_millis(u64::MAX),
        Err(SystemCallError::ClockRange)
    );
    assert_eq!(
        time::after(Duration::from_nanos(u64::MAX)),
        Err(SystemCallError::ClockRange)
    );
    let event =
        notification::create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL).unwrap();
    let empty = wait_until(
        event.owner.as_handle(),
        ObjectSignals::READABLE,
        Deadline::at(0),
    )
    .unwrap();
    assert_eq!(empty.reason, WaitReason::Timeout as u32);
    event.peer.signal(1).unwrap();
    let ready = wait_until(
        event.owner.as_handle(),
        ObjectSignals::READABLE,
        Deadline::at(0),
    )
    .unwrap();
    assert_eq!(ready.reason, WaitReason::Signaled as u32);
    assert!(ready.observed.intersects(ObjectSignals::READABLE));
    event.owner.take(u64::MAX).unwrap();
    assert_eq!(
        wait_until(
            event.owner.as_handle(),
            ObjectSignals::READABLE,
            Deadline {
                kind: 1,
                reserved: 1,
                at_ns: 0
            }
        ),
        Err(SystemCallError::IllegalArgument)
    );
    if let Some(outside) = start.max_deadline_ns.checked_add(1) {
        assert_eq!(
            wait_until(
                event.owner.as_handle(),
                ObjectSignals::READABLE,
                Deadline::at(outside)
            ),
            Err(SystemCallError::ClockRange)
        );
    }
    time::sleep_until(Deadline::at(0)).unwrap();
    let deadline = time::after(Duration::from_millis(2).unwrap()).unwrap();
    time::sleep_until(deadline).unwrap();
    assert!(
        time::expired(deadline).unwrap(),
        "absolute Sleep returned before its deadline"
    );
    assert_eq!(
        wait_until(event.owner.as_handle(), ObjectSignals::READABLE, deadline)
            .unwrap()
            .reason,
        WaitReason::Timeout as u32,
        "delayed call restarted an expired absolute wait"
    );
    event.peer.close().unwrap();
    event.owner.close().unwrap();
    delivery_deadline();
    causal_reads();
    assert!(
        time::snapshot().unwrap().now_ns >= start.now_ns,
        "invalid requests poisoned the public clock"
    );
    debug!(
        "public time deadline checks passed: At0 priority, absolute Sleep, full timeout, unconsumed authority, committed delivery, causal thread reads"
    );
}

fn delivery_deadline() {
    let inbox = message::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
    )
    .unwrap();
    for index in 0..MAILBOX_CAPACITY {
        message::send(inbox.peer, index as u64, &[]).unwrap();
    }
    let once_pair = message::make_send_once(inbox.peer, Rights::WRITE | Rights::WAIT).unwrap();
    let once = once_pair.owner;
    // SAFETY: 本夹具不观察回复终态，关闭本次派生的独立观察表项。
    unsafe { libkernel::ipc::object::close(once_pair.peer) }.expect("reply observer close failed");
    let moved_pair = message::make_send_once(inbox.peer, Rights::WRITE | Rights::TRANSIT).unwrap();
    let moved = moved_pair.owner;
    // SAFETY: 本夹具不观察回复终态，关闭本次派生的独立观察表项。
    unsafe { libkernel::ipc::object::close(moved_pair.peer) }.expect("reply observer close failed");
    let moves = [HandleMove {
        handle: moved,
        rights: Rights::WRITE,
    }];
    let deadline = time::after(Duration::from_millis(2).unwrap()).unwrap();
    assert_eq!(
        unsafe { message::send_raw_until(once, 91, &[9], &moves, deadline) },
        Err(SystemCallError::MailboxFull)
    );
    let wait = wait_until(once, ObjectSignals::WRITABLE, deadline).unwrap();
    assert_eq!(
        wait.reason,
        WaitReason::Timeout as u32,
        "full queue unexpectedly gained write capacity"
    );
    assert!(time::expired(deadline).unwrap());
    assert_eq!(message::receive(inbox.owner).unwrap().header.kind, 0);
    assert_eq!(
        unsafe { message::send_raw_until(once, 91, &[9], &moves, deadline) },
        Err(SystemCallError::DeadlineExpired),
        "expired retry consumed a fresh relative deadline"
    );
    assert_eq!(query(once).unwrap().rights, Rights::WRITE | Rights::WAIT);
    assert_eq!(
        query(moved).unwrap().rights,
        Rights::WRITE | Rights::TRANSIT
    );
    for index in 1..MAILBOX_CAPACITY {
        assert_eq!(
            message::receive(inbox.owner).unwrap().header.kind,
            index as u64
        );
    }
    assert!(matches!(
        message::receive(inbox.owner),
        Err(SystemCallError::ObjectNotAvailable)
    ));
    // 成功提交探针不依赖亚调度周期窗口。QEMU 节流周期为 0.5s，
    // 两个周期给正常调度留下执行机会；提交前过期仍是合法的未消费结果。
    // 每次重试是明确的新 Send，且逐次核验 owner；不重跑整个验收掩盖失败。
    let mut committed_deadline = None;
    for _ in 0..3 {
        let candidate = time::after(Duration::from_millis(1_000).unwrap()).unwrap();
        match unsafe { message::send_raw_until(once, 92, &[9], &moves, candidate) } {
            Ok(()) => {
                committed_deadline = Some(candidate);
                break;
            }
            Err(SystemCallError::DeadlineExpired) => {
                assert_eq!(query(once).unwrap().rights, Rights::WRITE | Rights::WAIT);
                assert_eq!(
                    query(moved).unwrap().rights,
                    Rights::WRITE | Rights::TRANSIT
                );
                debug!("delivery deadline probe expired before commit; authority retained");
            }
            Err(error) => panic!("delivery deadline probe failed: {:?}", error),
        }
    }
    let deadline = committed_deadline.expect("delivery probe received no finite execution window");
    assert_eq!(query(once), Err(SystemCallError::StaleHandle));
    assert_eq!(query(moved), Err(SystemCallError::StaleHandle));
    time::sleep_until(deadline).unwrap();
    let mut delivered = message::receive(inbox.owner).unwrap();
    assert_eq!(
        delivered.header.kind, 92,
        "deadline revoked an already committed delivery"
    );
    assert_eq!(delivered.payload, [9]);
    let capability = delivered.handles.take(0).unwrap();
    assert_eq!(capability.description().unwrap().rights, Rights::WRITE);
    drop(capability);
    drop(delivered);
    unsafe { close(inbox.peer) }.unwrap();
    unsafe { close(inbox.owner) }.unwrap();
}

fn causal_reads() {
    let high = Arc::new(AtomicU64::new(time::snapshot().unwrap().now_ns));
    let mut workers = alloc::vec::Vec::new();
    for _ in 0..3 {
        let high = high.clone();
        workers.push(
            thread::Builder::new()
                .spawn(move || {
                    for _ in 0..128 {
                        let prior = high.load(Ordering::Acquire);
                        let now = time::snapshot().unwrap().now_ns;
                        assert!(now >= prior, "causal cross-thread clock read regressed");
                        high.fetch_max(now, Ordering::AcqRel);
                        thread::yield_now().unwrap();
                    }
                })
                .unwrap(),
        );
    }
    for worker in workers {
        assert!(matches!(
            worker.join(),
            ThreadOutcome::Returned { leave: Ok(()), .. }
        ));
    }
    assert!(time::snapshot().unwrap().now_ns >= high.load(Ordering::Acquire));
}
