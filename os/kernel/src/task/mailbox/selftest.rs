//! 消息预留、真实复制失败与授权保活的 Ready 前确定性组合。

use super::*;
use crate::task::{
    handle,
    memory_pool::MemoryPool,
    proc,
    selftest::{Activated, Caller, pump},
};
use erhino_shared::{message::ReceiveResult, object::SenderResult, wait::WaitItem};

fn object_of(caller: &Caller, h: Handle) -> ObjectRef {
    caller
        .process
        .handles
        .lock()
        .get(h, Rights::NONE)
        .unwrap()
        .object()
        .clone()
}

fn setup(caller: &Caller, badge: u64) -> (Handle, SenderResult, ObjectRef) {
    let _active = Activated::new(&caller.process);
    create(
        caller.thread(),
        Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        caller.output,
    )
    .expect("Mailbox fixture creation failed");
    let owner: Handle = caller.read(caller.output);
    let result = mint(caller, owner, badge);
    (owner, result, object_of(caller, owner))
}

fn mint(caller: &Caller, owner: Handle, badge: u64) -> SenderResult {
    let _active = Activated::new(&caller.process);
    mint_sender(
        caller.thread(),
        owner,
        badge,
        Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT,
        caller.output,
    )
    .expect("Mailbox fixture sender creation failed");
    caller.read(caller.output)
}

fn once(caller: &Caller, sender: Handle) -> Handle {
    let _active = Activated::new(&caller.process);
    make_send_once(
        caller.thread(),
        sender,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::GRANT,
        caller.output,
    )
    .expect("Mailbox fixture send-once creation failed");
    caller.read(caller.output)
}

fn close(caller: &Caller, h: Handle) {
    assert!(
        matches!(
            handle::close(caller.thread(), h),
            Ok(handle::HandleCloseStart::Ready)
        ),
        "Mailbox fixture leaf close failed"
    );
}

fn send_message(
    caller: &Caller,
    sender: Handle,
    kind: u64,
    moved: Option<Handle>,
) -> Result<(), SystemCallError> {
    let header = SendHeader::new(kind, 4, u32::from(moved.is_some()));
    caller.put(caller.output, &header);
    caller.put(caller.output + 64, &[1u8, 2, 3, kind as u8]);
    if let Some(h) = moved {
        caller.put(
            caller.output + 96,
            &HandleMove {
                handle: h,
                rights: Rights::WRITE | Rights::WAIT,
            },
        );
    }
    let _active = Activated::new(&caller.process);
    send(
        caller.thread(),
        sender,
        caller.output,
        caller.output + 64,
        caller.output + 96,
        usize::from(moved.is_some()),
        4,
    )
}

fn receive_message(caller: &Caller, owner: Handle, output: usize, handles: usize) -> ReceiveResult {
    let _active = Activated::new(&caller.process);
    receive(
        caller.thread(),
        owner,
        output,
        output + 80,
        4,
        output + 96,
        handles,
    )
    .expect("Mailbox fixture receive failed");
    caller.read(output)
}

fn lifetime_delivery(root: &Arc<MemoryPool>) {
    let caller = Caller::new(root);
    let (owner, a, queue) = setup(&caller, 0xa1);
    let b = mint(&caller, owner, 0xb2);
    let watch_a = observe(&caller, a.lifetime, ObjectSignals::CLOSED);
    let watch_b = observe(&caller, b.lifetime, ObjectSignals::CLOSED);
    let observer_a = object_of(&caller, a.lifetime);
    let observer_b = object_of(&caller, b.lifetime);
    let sender_a = object_of(&caller, a.sender);
    let sender_b = object_of(&caller, b.sender);
    let context_a = sender_a.header().koid();
    let context_b = sender_b.header().koid();
    assert_ne!(
        context_a, context_b,
        "independent senders shared an authorization identity"
    );
    assert_eq!(
        (sender_a.related_id(), sender_b.related_id()),
        (queue.header().koid(), queue.header().koid())
    );
    assert_eq!((sender_a.badge(), sender_b.badge()), (0xa1, 0xb2));
    assert_eq!(
        (observer_a.related_id(), observer_b.related_id()),
        (context_a, context_b)
    );
    let weak_a = Arc::downgrade(&sender_a);
    let weak_b = Arc::downgrade(&sender_b);
    drop((sender_a, sender_b));
    let alias = {
        let _active = Activated::new(&caller.process);
        handle::duplicate(
            caller.thread(),
            a.sender,
            Rights::WRITE | Rights::WAIT,
            caller.output,
        )
        .expect("sender duplicate failed");
        caller.read::<Handle>(caller.output)
    };
    assert_eq!(
        (
            object_of(&caller, alias).header().koid(),
            object_of(&caller, alias).badge()
        ),
        (context_a, 0xa1),
        "duplicate changed sender identity or badge"
    );
    let moved = once(&caller, b.sender);
    send_message(&caller, a.sender, 11, Some(moved)).unwrap();
    send_message(&caller, alias, 22, None).unwrap();
    close(&caller, a.sender);
    close(&caller, alias);
    close(&caller, b.sender);
    assert!(
        !observer_a.signals().intersects(ObjectSignals::CLOSED),
        "queued Delivery failed to preserve sender"
    );
    assert!(
        !observer_b.signals().intersects(ObjectSignals::CLOSED),
        "transit sender failed to preserve its authority"
    );
    let first = receive_message(&caller, owner, caller.output, 1);
    assert_eq!(
        (
            first.header.kind,
            first.header.sender_context_id,
            first.header.sender_badge
        ),
        (11, context_a, 0xa1)
    );
    let moved: Handle = caller.read(caller.output + 96);
    let received_sender = object_of(&caller, moved);
    assert_eq!(
        (received_sender.header().koid(), received_sender.badge()),
        (context_b, 0xb2)
    );
    drop(received_sender);
    {
        let table = caller.process.handles.lock();
        let entry = table.get(moved, Rights::WRITE).unwrap();
        assert_eq!(
            *entry.role(),
            HandleRole::MailboxSenderOnce,
            "transit changed send-once role"
        );
        assert_eq!(
            entry.rights(),
            Rights::WRITE | Rights::WAIT,
            "transit failed to attenuate sender rights"
        );
    }
    send_message(&caller, moved, 33, None).expect("received send-once failed its first use");
    assert!(
        caller
            .process
            .handles
            .lock()
            .get(moved, Rights::NONE)
            .is_err(),
        "successful Send failed to consume received send-once"
    );
    assert!(
        send_message(&caller, moved, 34, None).is_err(),
        "received send-once succeeded twice"
    );
    assert!(
        !observer_b.signals().intersects(ObjectSignals::CLOSED),
        "new Delivery failed to preserve consumed send-once"
    );
    assert_eq!(object_of(&caller, first.delivery).related_id(), context_a);
    {
        let _active = Activated::new(&caller.process);
        assert_eq!(
            handle::duplicate(
                caller.thread(),
                first.delivery,
                Rights::TRANSIT,
                caller.output
            ),
            Err(SystemCallError::RightsDenied),
            "Delivery acquired duplicable authority"
        );
    }
    let second = receive_message(&caller, owner, caller.output, 0);
    assert_eq!(second.header.kind, 22, "Mailbox receive reordered its FIFO");
    let third = receive_message(&caller, owner, caller.output, 0);
    assert_eq!(
        (
            third.header.kind,
            third.header.sender_context_id,
            third.header.sender_badge
        ),
        (33, context_b, 0xb2)
    );
    assert!(
        !queue.signals().intersects(ObjectSignals::READABLE),
        "empty Mailbox remained readable"
    );
    close(&caller, first.delivery);
    assert!(
        !observer_a.signals().intersects(ObjectSignals::CLOSED),
        "one Delivery closed another live obligation"
    );
    close(&caller, second.delivery);
    assert!(
        observer_a.signals().intersects(ObjectSignals::CLOSED) && weak_a.upgrade().is_none(),
        "received Delivery failed to release the last sender obligation"
    );
    close(&caller, third.delivery);
    assert!(
        observer_b.signals().intersects(ObjectSignals::CLOSED) && weak_b.upgrade().is_none(),
        "Lifetime observer retained authority after its last Delivery closed"
    );
    pump();
    assert!(
        watch_a.signals().intersects(ObjectSignals::READABLE)
            && watch_b.signals().intersects(ObjectSignals::READABLE),
        "last Delivery close failed to notify an installed Lifetime observer"
    );
    caller.cleanup();
}

fn observe(caller: &Caller, source: Handle, signals: ObjectSignals) -> ObjectRef {
    let _active = Activated::new(&caller.process);
    crate::task::wait_set::create(
        caller.thread(),
        1,
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        caller.output,
    )
    .unwrap();
    let set: Handle = caller.read(caller.output);
    caller.put(caller.output + 32, &WaitItem::new(source, signals, 73));
    crate::task::wait_set::register(caller.thread(), set, caller.output + 32, caller.output + 64)
        .unwrap();
    let object = object_of(caller, set);
    assert!(
        !object.signals().intersects(ObjectSignals::READABLE),
        "observer completed before its source signal"
    );
    object
}

fn writeback_rollback(root: &Arc<MemoryPool>) {
    let caller = Caller::new(root);
    let (owner, sender, object) = setup(&caller, 0x55);
    let moved = once(&caller, sender.sender);
    send_message(&caller, sender.sender, 1, Some(moved)).unwrap();
    send_message(&caller, sender.sender, 2, None).unwrap();
    let mailbox = concrete(&object).unwrap();
    let output = ReceiveOutput {
        header: caller.output - proc::PAGE_SIZE,
        payload: caller.output + 80,
        handles: caller.output - proc::PAGE_SIZE + 96,
    };
    {
        let mut space = caller.process.space.lock();
        space
            .check_range(output.header, core::mem::size_of::<ReceiveResult>(), true)
            .unwrap();
        space.check_range(output.payload, 4, true).unwrap();
        space
            .check_range(output.handles, core::mem::size_of::<Handle>(), true)
            .unwrap();
    }
    let token = handle::transaction_token().unwrap();
    let (reservation, message) = mailbox
        .begin_receive(&mut caller.process.handles.lock(), token, 4, 1)
        .unwrap();
    let reserved = reservation.handles().to_vec();
    let delivery_id = message.delivery.object().header().koid();
    let expected_header = message.header;
    let moved_id = message.handles[0].object().header().koid();
    assert!(
        !mailbox.signals().intersects(ObjectSignals::READABLE),
        "receive reservation left queued followers readable"
    );
    assert!(matches!(mailbox.peek(), Err(SystemCallError::ObjectBusy)));
    assert!(matches!(
        mailbox.discard(),
        Err(SystemCallError::ObjectBusy)
    ));
    assert!(matches!(
        mailbox.begin_receive(
            &mut caller.process.handles.lock(),
            handle::transaction_token().unwrap(),
            4,
            1
        ),
        Err(SystemCallError::ObjectBusy)
    ));
    let watch: Handle = {
        let _active = Activated::new(&caller.process);
        crate::task::wait_set::create(
            caller.thread(),
            1,
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            caller.output,
        )
        .unwrap();
        caller.read(caller.output)
    };
    caller.put(
        caller.output + 32,
        &WaitItem::new(owner, ObjectSignals::READABLE, 73),
    );
    {
        let _active = Activated::new(&caller.process);
        crate::task::wait_set::register(
            caller.thread(),
            watch,
            caller.output + 32,
            caller.output + 64,
        )
        .unwrap();
    }
    let watch_object = object_of(&caller, watch);
    assert!(
        !watch_object.signals().intersects(ObjectSignals::READABLE),
        "reserved Mailbox completed a readiness observer"
    );
    // 真实 Unmap 发生于范围初检和复制之间；不伪造 uaccess 返回值。
    assert!(
        caller
            .process
            .lifecycle
            .clear_active_if(crate::hart::current().slot(), || true)
    );
    let unmap = proc::memory_unmap(
        caller.thread(),
        (caller.output & !(proc::PAGE_SIZE - 1)) as u64,
        proc::PAGE_SIZE as u64,
    )
    .expect("Mailbox output revocation failed");
    pump();
    drop(unmap);
    assert_eq!(caller.process.space.lock().page_pa(caller.output), None);
    {
        let _active = Activated::new(&caller.process);
        assert_eq!(
            finish_receive(
                caller.thread(),
                mailbox,
                token,
                reservation,
                message,
                output
            ),
            Err(SystemCallError::MemoryNotAccessible)
        );
    }
    assert!(
        !caller.process.lifecycle.is_terminating(),
        "rolled-back Receive incorrectly faulted its caller"
    );
    let partial: ReceiveResult = caller.read(output.header);
    assert_eq!(
        partial.header.kind, 1,
        "Receive did not reach its partial-header write boundary"
    );
    assert!(
        reserved.last() == Some(&partial.delivery),
        "partial header lost its reserved Delivery identity"
    );
    for &h in &reserved {
        assert!(
            caller.process.handles.lock().get(h, Rights::NONE).is_err(),
            "failed Receive published a reserved handle"
        );
    }
    assert_eq!(
        mailbox.peek().unwrap().kind,
        1,
        "writeback rollback reordered the reserved queue head"
    );
    assert!(
        mailbox.signals().intersects(ObjectSignals::READABLE),
        "rollback failed to restore receive readiness"
    );
    pump();
    assert!(
        watch_object.signals().intersects(ObjectSignals::READABLE),
        "rollback failed to wake its installed observer"
    );
    let retry = caller.output - proc::PAGE_SIZE;
    let first = receive_message(&caller, owner, retry, 1);
    assert_eq!(
        first.header, expected_header,
        "rollback changed the reserved envelope"
    );
    assert_eq!(
        caller.read::<[u8; 4]>(retry + 80),
        [1, 2, 3, 1],
        "rollback changed the queued payload"
    );
    assert_eq!(
        object_of(&caller, caller.read(retry + 96)).header().koid(),
        moved_id,
        "rollback changed the transit capability identity"
    );
    for &h in &reserved {
        assert!(
            matches!(
                caller.process.handles.lock().get(h, Rights::NONE),
                Err(handle_table::TableError::StaleHandle)
            ),
            "old partial output regained authority after slot reuse"
        );
    }
    assert_eq!(
        object_of(&caller, first.delivery).header().koid(),
        delivery_id,
        "retry replaced the affine Delivery"
    );
    close(&caller, caller.read(retry + 96));
    close(&caller, first.delivery);
    let second = receive_message(&caller, owner, retry, 0);
    assert_eq!(second.header.kind, 2);
    close(&caller, second.delivery);
    caller.cleanup();
}

fn capacity_closed(root: &Arc<MemoryPool>) {
    let caller = Caller::new(root);
    let (owner, sender, object) = setup(&caller, 4);
    let lifetime = object_of(&caller, sender.lifetime);
    for kind in 0..MAILBOX_CAPACITY {
        send_message(&caller, sender.sender, kind as u64, None).unwrap();
    }
    let mailbox = concrete(&object).unwrap();
    let token = handle::transaction_token().unwrap();
    let output = ReceiveOutput {
        header: caller.output - proc::PAGE_SIZE,
        payload: caller.output + 80,
        handles: caller.output - proc::PAGE_SIZE + 96,
    };
    {
        let mut space = caller.process.space.lock();
        space
            .check_range(output.header, core::mem::size_of::<ReceiveResult>(), true)
            .unwrap();
        space.check_range(output.payload, 4, true).unwrap();
        space.check_range(output.handles, 0, true).unwrap();
    }
    let (reservation, message) = mailbox
        .begin_receive(&mut caller.process.handles.lock(), token, 4, 0)
        .unwrap();
    assert!(
        !mailbox.signals().intersects(ObjectSignals::WRITABLE),
        "reserved queue head incorrectly freed full capacity"
    );
    let send_once = once(&caller, sender.sender);
    let moved = once(&caller, sender.sender);
    let usage = crate::task::resources::admission_usage();
    assert_eq!(
        send_message(&caller, send_once, 99, Some(moved)),
        Err(SystemCallError::MailboxFull)
    );
    assert_eq!(
        crate::task::resources::admission_usage(),
        usage,
        "failed Send leaked Delivery admission"
    );
    assert!(
        caller
            .process
            .handles
            .lock()
            .get(send_once, Rights::WRITE)
            .is_ok(),
        "failed Send consumed send-once"
    );
    assert!(
        caller
            .process
            .handles
            .lock()
            .get(moved, Rights::WRITE)
            .is_ok(),
        "failed Send consumed a source capability move"
    );
    close(&caller, sender.sender);
    close(&caller, send_once);
    close(&caller, moved);
    close(&caller, owner);
    assert!(
        mailbox.signals().intersects(ObjectSignals::CLOSED),
        "owner close failed to freeze terminal readiness"
    );
    assert!(
        !lifetime.signals().intersects(ObjectSignals::CLOSED),
        "owner close discarded the reserved in-flight obligation"
    );
    assert!(
        caller
            .process
            .lifecycle
            .clear_active_if(crate::hart::current().slot(), || true)
    );
    let unmap = proc::memory_unmap(
        caller.thread(),
        (caller.output & !(proc::PAGE_SIZE - 1)) as u64,
        proc::PAGE_SIZE as u64,
    )
    .expect("closed Mailbox output revocation failed");
    pump();
    drop(unmap);
    {
        let _active = Activated::new(&caller.process);
        assert_eq!(
            finish_receive(
                caller.thread(),
                mailbox,
                token,
                reservation,
                message,
                output
            ),
            Err(SystemCallError::MemoryNotAccessible),
            "closed Receive failed to reject its in-flight rollback"
        );
    }
    assert!(
        lifetime.signals().intersects(ObjectSignals::CLOSED),
        "closed rollback retained its last sender obligation"
    );
    assert!(
        mailbox.state.lock().queue.is_empty(),
        "closed rollback resurrected the queue"
    );
    caller.cleanup();
}

pub(crate) fn run(root: &Arc<MemoryPool>) {
    pump();
    let pool = root.snapshot();
    let metadata = crate::task::resources::admission_usage();
    let control = crate::task::notify_work::inventory_for_test();
    let deferred = crate::deferred_work::selftest::inventory();
    lifetime_delivery(root);
    writeback_rollback(root);
    capacity_closed(root);
    pump();
    assert_eq!(
        root.snapshot(),
        pool,
        "Mailbox fixtures failed to refund their funded Pool"
    );
    assert_eq!(
        crate::task::resources::admission_usage(),
        metadata,
        "Mailbox fixtures retained metadata owners"
    );
    assert_eq!(
        crate::task::notify_work::inventory_for_test(),
        control,
        "Mailbox fixtures retained control slots"
    );
    assert_eq!(
        crate::deferred_work::selftest::inventory(),
        deferred,
        "Mailbox fixtures retained deferred slots"
    );
    info!(
        Task,
        "Mailbox ownership checks passed: independent senders, transit authority, affine Delivery, receive contention, revoked-output rollback wake, closed rollback, full capacity, complete refund"
    );
}
