//! 跨Pool的Once运输付款；复用真实test_target、原loader与根监督账本。
use super::*;
use alloc::vec::Vec;
use libkernel::{
    ipc::{
        message::{Mailbox, SendOnce},
        packet::Packet,
    },
    memory_pool,
    shared::memory_pool::{METADATA_SLOT_BYTES, MemoryPoolBudget},
};

pub(super) fn run(harness: &mut Harness, job: Handle, image: &[u8]) -> Result<(), &'static str> {
    let rights = Rights::READ | Rights::CREATE | Rights::DUPLICATE;
    let pool = harness
        .derive_pool(
            memory_pool(),
            MemoryPoolBudget {
                pages: 16 * 1024 * 1024 / 4096,
                metadata: 2 * 1024 * 1024 / METADATA_SLOT_BYTES,
            },
            rights,
        )
        .map_err(|_| "Once issuer Pool admission failed")?;
    let inbox = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)
        .map_err(|_| "Once fixture inbox failed")?;
    let transport =
        Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT;
    let source = inbox
        .mint(0, transport)
        .map_err(|_| "Once fixture sender failed")?;
    let grant = harness
        .duplicate(source.sender.as_handle(), transport)
        .map_err(|_| "Once issuer grant failed")?;
    let ready = libkernel::ipc::notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::GRANT | Rights::DUPLICATE,
    )
    .map_err(|_| "Once ready source failed")?;
    let ready_grant = harness
        .duplicate(ready.peer.as_handle(), Rights::SIGNAL | Rights::GRANT)
        .map_err(|_| "Once ready grant failed")?;
    harness
        .reserve_process()
        .map_err(|_| "Once issuer supervision failed")?;
    let started = spawn(SpawnRequest {
        memory_pool: pool,
        parent: current_process(),
        job,
        image,
        payload: b"once-payer",
        grants: &[
            HandleGrant {
                handle: grant,
                rights: transport,
            },
            HandleGrant {
                handle: ready_grant,
                rights: Rights::SIGNAL,
            },
        ],
        control_rights: SUPERVISOR_RIGHTS,
    });
    if started.is_ok()
        || matches!(&started, Err(f) if f.grants == libprocess::GrantOutcome::Consumed)
    {
        harness.transferred(grant);
        harness.transferred(ready_grant);
    }
    let started = started.map_err(|_| "Once issuer spawn failed")?;
    harness.track_process(Supervised {
        pid: started.pid,
        control: started.control,
    });
    let deadline =
        libkernel::time::timeout_millis(30_000).map_err(|_| "Once test deadline failed")?;
    wait_until(ready.owner.as_handle(), ObjectSignals::READABLE, deadline)
        .map_err(|_| "Once issuer did not finish handoff")?;
    let mut message = harness
        .receive_until(inbox.as_handle(), deadline)
        .map_err(|_| "Once issuer handoff failed")?;
    assert_eq!(message.header.kind, u64::from_le_bytes(*b"ONCEPAY1"));
    assert_eq!(message.handles.remaining(), 4);
    let mut once = Vec::new();
    for index in 0..4 {
        once.push(
            SendOnce::from_capability(
                message
                    .handles
                    .take(index)
                    .map_err(|_| "Once handoff missing")?,
            )
            .map_err(|_| "Once handoff role invalid")?
            .0,
        );
    }
    drop(message);
    // A(test_kernel)创建目标Mailbox与Sender并执行Send，B(test_target)发行Once；原payer必须是B。
    let before = memory_pool::query(pool).map_err(|_| "Once initial Pool query failed")?;
    let ballast = memory_pool::derive(
        pool,
        MemoryPoolBudget {
            pages: 0,
            metadata: before
                .metadata
                .available
                .checked_sub(256)
                .ok_or("Once quota fixture too small")?,
        },
        Rights::READ,
    )
    .map_err(|_| "Once quota fixture admission failed")?;
    let constrained = memory_pool::query(pool).map_err(|_| "Once quota Pool query failed")?;
    assert!(
        constrained.metadata.available * METADATA_SLOT_BYTES
            < libkernel::shared::message::PAYLOAD_MAX as u64
    );
    let reply = once.pop().ok_or("Once quota sender missing")?;
    let payload = [0x5a; libkernel::shared::message::PAYLOAD_MAX];
    let moved = libkernel::ipc::object::duplicate(
        source.sender.as_handle(),
        Rights::WRITE | Rights::TRANSIT,
    )
    .map_err(|_| "Once quota move preparation failed")?;
    // SAFETY: duplicate刚产生唯一新表项，Packet接管其关闭/失败返还责任。
    let moved = unsafe { libkernel::ipc::capability::Capability::from_raw(moved) };
    let moved_info = moved
        .description()
        .map_err(|_| "Once quota move query failed")?;
    let mut packet = Packet::new(1, &payload).map_err(|_| "Once quota packet failed")?;
    packet
        .push(moved, Rights::WRITE)
        .map_err(|_| "Once quota move admission failed")?;
    let mut failure = packet
        .try_reply(reply, deadline)
        .expect_err("Once ignored issuer quota");
    assert_eq!(failure.error, SystemCallError::QuotaExceeded);
    let after_failure = memory_pool::query(pool).map_err(|_| "Once failed Pool query failed")?;
    assert_eq!(
        after_failure.metadata.allocated, constrained.metadata.allocated,
        "Once quota leaked transient charge"
    );
    failure
        .reply
        .description()
        .map_err(|_| "Once quota consumed original capability")?;
    once.push(failure.reply);
    let (moved, _) = failure.packet.pop().ok_or("Once quota lost move owner")?;
    let restored = moved
        .description()
        .map_err(|_| "Once quota move was consumed")?;
    assert_eq!(
        (restored.object_id, restored.rights),
        (moved_info.object_id, moved_info.rights),
        "Once quota changed move authority"
    );
    drop(moved);
    drop(failure.packet);
    drop(ballast);
    let before = memory_pool::query(pool).map_err(|_| "Once restored Pool query failed")?;
    for reply in once {
        Packet::new(2, &payload)
            .map_err(|_| "Once packet failed")?
            .try_reply(reply, deadline)
            .map_err(|_| "Once send failed after quota refund")?;
    }
    let queued = memory_pool::query(pool).map_err(|_| "Once queued Pool query failed")?;
    assert!(
        queued.metadata.allocated > before.metadata.allocated,
        "Unread Once payload did not charge issuer"
    );
    // 发行者CPU与Body先实际收集，排队责任仍须独立保留原付款。
    libkernel::process::kill(started.control).map_err(|_| "Once issuer stop failed")?;
    harness.collect_process(started.pid)?;
    let orphaned = memory_pool::query(pool).map_err(|_| "Once dead issuer Pool query failed")?;
    assert_eq!(
        orphaned.allocated, 0,
        "Once issuer Body pages remained after collection"
    );
    assert!(
        orphaned.metadata.allocated > 0,
        "Issuer death refunded unread Once messages"
    );
    debug!(
        "Once issuer collected with unread replies retained: pool={}, metadata={}",
        orphaned.identity, orphaned.metadata.allocated
    );
    let mut deliveries = Vec::new();
    for _ in 0..4 {
        let message = inbox.receive().map_err(|_| "Once queued reply missing")?;
        assert_eq!(message.payload, payload);
        assert_eq!(message.handles.remaining(), 0);
        deliveries.push(message.delivery);
    }
    let tail = memory_pool::query(pool).map_err(|_| "Once tail Pool query failed")?;
    assert!(
        tail.metadata.allocated < orphaned.metadata.allocated,
        "Once Receive did not refund body storage"
    );
    for mut delivery in deliveries {
        delivery
            .finish()
            .map_err(|_| "Once Delivery finish failed")?;
    }
    let retired = memory_pool::query(pool).map_err(|_| "Once retired Pool query failed")?;
    assert!(
        retired.metadata.allocated < tail.metadata.allocated,
        "Once Delivery tail did not retain issuer charge"
    );
    debug!(
        "Once cross-Pool transport observed: issuer={}, before={}, queued={}, delivery_tail={}, retired={}",
        before.identity,
        before.metadata.allocated,
        queued.metadata.allocated,
        tail.metadata.allocated,
        retired.metadata.allocated
    );
    // 对照普通Sender：即使原Sender由A创建并交B，当前A发送仍不借B的额度。
    source
        .sender
        .send(3, &payload)
        .map_err(|_| "Ordinary Sender control send failed")?;
    assert_eq!(
        memory_pool::query(pool)
            .map_err(|_| "Ordinary Sender Pool query failed")?
            .metadata
            .allocated,
        retired.metadata.allocated,
        "Ordinary Sender charged Once issuer"
    );
    drop(
        inbox
            .receive()
            .map_err(|_| "Ordinary Sender message missing")?,
    );
    // 剩余费用按实际消息/Delivery存储退休，不按Sent或CPU DONE推定。
    loop {
        let after = memory_pool::query(pool).map_err(|_| "Once final Pool query failed")?;
        if after.allocated == 0
            && after.metadata.allocated == 0
            && after.delegated == 0
            && after.metadata.delegated == 0
        {
            break;
        }
        if libkernel::time::expired(deadline).map_err(|_| "Once refund clock failed")? {
            return Err("Once issuer charge remained after actual retirement");
        }
        libkernel::time::sleep_until(
            libkernel::time::timeout_millis(1).map_err(|_| "Once refund timer failed")?,
        )
        .map_err(|_| "Once refund wait failed")?;
    }
    harness.close_control(pool)?;
    debug!("Once cross-Pool issuer quota, unread replies, Delivery tail and final refund passed");
    Ok(())
}
