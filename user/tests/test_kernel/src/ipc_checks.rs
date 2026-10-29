//! 消息面：badge 与 affine owner、WaitSet 退休、一次性投递、WRITABLE 流控与控制面压力。
use super::*;

/// 与 pm 约定的流控验证消息号：请求携带 [目标邮箱 sender、确认 signaler、
/// 虚假唤醒 signaler]；pm 填满目标邮箱后回样确认，末尾补发 WRITABLE_WAKE_TAIL。
pub(super) const WRITABLE_WAKE_REQUEST: u64 = 640;
pub(super) const WRITABLE_WAKE_FILL: u64 = 641;
pub(super) const WRITABLE_WAKE_TAIL: u64 = 642;

/// badged sender 的来源盖章，以及 owner 的 GRANT/TRANSIT 运输边界。
pub(super) fn test_capability_badges_and_affine_owners() {
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
    let lifetime = badged.lifetime;
    let badged = badged.sender;
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

    unsafe { send_raw(mailbox.peer, 880, &[], &[]) }
        .expect("explicit zero-badge sender send failed");
    unsafe { send_raw(badged, 881, &[], &[]) }.expect("badged sender send failed");
    unsafe { send_raw(copy, 882, &[], &[]) }.expect("badged sender copy send failed");
    for (kind, badge) in [(880, 0), (881, BADGE), (882, BADGE)] {
        let message = receive(mailbox.owner).expect("badged message receive failed");
        assert_eq!(message.header.kind, kind);
        assert_eq!(message.header.sender_pid, current_pid() as u64);
        assert_eq!(message.header.sender_badge, badge);
    }
    let once_pair = make_send_once(badged, Rights::WRITE).expect("badged send-once mint failed");
    let once = once_pair.owner;
    // SAFETY: 本夹具不观察回复终态，关闭本次派生的独立观察表项。
    unsafe { libkernel::ipc::object::close(once_pair.peer) }.expect("reply observer close failed");
    unsafe { send_raw(once, 887, &[], &[]) }.expect("badged send-once send failed");
    let message = receive(mailbox.owner).expect("badged send-once receive failed");
    assert_eq!(message.header.sender_badge, BADGE);

    let transit =
        duplicate(badged, Rights::WRITE | Rights::TRANSIT).expect("badged transit copy failed");
    let moves = [HandleMove {
        handle: transit,
        rights: Rights::WRITE,
    }];
    unsafe { send_raw(transport.peer, 888, &[], &moves) }.expect("badged sender transit failed");
    let transferred = receive(transport.owner)
        .expect("badged sender transit receive failed")
        .handles
        .take(0)
        .expect("badged sender slot missing")
        .into_raw();
    unsafe { send_raw(transferred, 889, &[], &[]) }.expect("transferred badged sender send failed");
    let message = receive(mailbox.owner).expect("transferred badged message receive failed");
    assert_eq!(message.header.sender_badge, BADGE);
    unsafe { close(transferred) }.expect("transferred badged sender close failed");
    unsafe { close(copy) }.expect("badged sender copy close failed");
    unsafe { close(badged) }.expect("badged sender close failed");
    unsafe { close(lifetime) }.expect("badged sender observer close failed");

    assert!(matches!(
        duplicate(mailbox.owner, Rights::READ),
        Err(SystemCallError::RightsDenied)
    ));
    let owner_move = [HandleMove {
        handle: mailbox.owner,
        rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
    }];
    assert!(matches!(
        unsafe { send_raw(transport.peer, 883, &[], &owner_move) },
        Err(SystemCallError::RightsDenied)
    ));
    unsafe { send_raw(mailbox.peer, 884, &[], &[]) }.expect("owner must survive rejected transit");
    assert_eq!(
        receive(mailbox.owner)
            .expect("owner receive after rejected transit failed")
            .header
            .kind,
        884
    );

    let event = { let mut pair = libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */ unsafe { libkernel::sys_notification_create(
        Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT,
        Rights::SIGNAL, &mut pair) }.map(|()| pair) }
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
        unsafe { send_raw(transport.peer, 886, &[], &owner_move) },
        Err(SystemCallError::RightsDenied)
    ));
    /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */
    unsafe { libkernel::sys_notification_signal(event.peer, 1) }
        .expect("notification owner must survive rejected transit");
    assert_eq!(
        {
            let mut bits = 0; /* SAFETY: ABI 夹具借用有效 handle，输出在同步调用期间私有。 */
            unsafe { libkernel::sys_notification_take(event.owner, 1, &mut bits) }.map(|()| bits)
        }
        .expect("notification take after rejected transit failed"),
        1
    );

    unsafe { close(event.peer) }.expect("notification signaler close failed");
    unsafe { close(event.owner) }.expect("notification owner close failed");
    unsafe { close(mailbox.peer) }.expect("default sender close failed");
    unsafe { close(mailbox.owner) }.expect("mailbox owner close failed");
    unsafe { close(transport.peer) }.expect("owner transport sender close failed");
    unsafe { close(transport.owner) }.expect("owner transport owner close failed");
    debug!("capability badge and owner transport passed");
}

/// 持久观察的轮次、立即失效与内核独立退休。
pub(super) fn test_wait_set_retirement() {
    use libkernel::{ipc::wait_set::WaitSet, time::Deadline};

    let event = { let mut pair = libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */ unsafe { libkernel::sys_notification_create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL, &mut pair) }.map(|()| pair) }
        .expect("WaitSet retirement notification create failed");
    let set = WaitSet::create(128).expect("WaitSet retirement create failed");
    let mut tokens = alloc::vec::Vec::new();
    for cookie in 0..64 {
        tokens.push(
            set.register(MemberRequest::new(
                event.owner,
                ObjectSignals::READABLE,
                cookie,
            ))
            .expect("WaitSet retirement registration failed"),
        );
    }
    set.remove(tokens[0]).expect("WaitSet removal failed");
    assert_eq!(set.remove(tokens[0]), Err(SystemCallError::ObjectNotFound));
    /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */
    unsafe { libkernel::sys_notification_signal(event.peer, 1) }
        .expect("WaitSet notification signal failed");
    set.wait(Deadline::INFINITE)
        .expect("WaitSet readiness wait failed");
    let records = set.receive(64).expect("WaitSet ready receive failed");
    assert!(records.iter().all(|record| record.token != tokens[0]));
    set.close()
        .unwrap_or_else(|(_, error)| panic!("nonempty WaitSet close failed: {error:?}"));
    unsafe { close(event.owner) }.expect("WaitSet source owner close failed");
    unsafe { close(event.peer) }.expect("WaitSet source signaler close failed");

    let event = { let mut pair = libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */ unsafe { libkernel::sys_notification_create(Rights::READ | Rights::WAIT | Rights::MANAGE, Rights::SIGNAL, &mut pair) }.map(|()| pair) }
        .expect("persistent WaitSet source create failed");
    let set = WaitSet::create(1).expect("persistent WaitSet create failed");
    let token = set
        .register(MemberRequest::new(event.owner, ObjectSignals::READABLE, 4))
        .unwrap();
    for _ in 0..16 {
        /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */
        unsafe { libkernel::sys_notification_signal(event.peer, 1) }.unwrap();
        set.wait(Deadline::INFINITE).unwrap();
        let records = set.receive(1).unwrap();
        assert_eq!((records[0].token, records[0].cookie), (token, 4));
        // 未消费来源时持续返回同一电平；消费后旧提示必须重查并淘汰。
        assert_eq!(set.receive(1).unwrap()[0].token, token);
        {
            let mut bits = 0; /* SAFETY: ABI 夹具借用有效 handle，输出在同步调用期间私有。 */
            unsafe { libkernel::sys_notification_take(event.owner, 1, &mut bits) }.map(|()| bits)
        }
        .unwrap();
        assert!(set.receive(1).unwrap().is_empty());
    }
    set.remove(token).unwrap();
    let replacement = set
        .register(MemberRequest::new(event.owner, ObjectSignals::READABLE, 5))
        .unwrap();
    assert_ne!(replacement, token);
    /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */
    unsafe { libkernel::sys_notification_signal(event.peer, 1) }.unwrap();
    set.wait(Deadline::INFINITE).unwrap();
    assert_eq!(set.receive(1).unwrap()[0].token, replacement);
    drop(set);
    // SAFETY: 本地集合 owner 已提交退休，两个剩余来源表项均属于本函数。
    unsafe { close(event.owner) }.unwrap();
    unsafe { close(event.peer) }.unwrap();

    let first = WaitSet::create(4).unwrap();
    let second = WaitSet::create(4).unwrap();
    assert_eq!(
        first.register(MemberRequest::new(
            first.handle(),
            ObjectSignals::READABLE,
            1
        )),
        Err(SystemCallError::WrongObjectType)
    );
    assert_eq!(
        first.register(MemberRequest::new(
            second.handle(),
            ObjectSignals::READABLE,
            2
        )),
        Err(SystemCallError::WrongObjectType)
    );
    assert_eq!(
        second.register(MemberRequest::new(
            first.handle(),
            ObjectSignals::READABLE,
            3
        )),
        Err(SystemCallError::WrongObjectType)
    );
    first
        .close()
        .unwrap_or_else(|(_, error)| panic!("first flat WaitSet close failed: {error:?}"));
    second
        .close()
        .unwrap_or_else(|(_, error)| panic!("second flat WaitSet close failed: {error:?}"));
    debug!(
        "WaitSet persistent levels, stale removal, nonempty retirement and nested rejection passed"
    );
}

/// 一次性投递权：本地、转移、失败保留与原 sender 独立性。
pub(super) fn test_send_once() {
    let mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("send-once mailbox create failed");
    let once_pair = make_send_once(mailbox.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        .expect("make send once failed");
    let once = once_pair.owner;
    // SAFETY: 本夹具不观察回复终态，关闭本次派生的独立观察表项。
    unsafe { libkernel::ipc::object::close(once_pair.peer) }.expect("reply observer close failed");
    unsafe { send_raw(once, 900, &[1], &[]) }.expect("send once failed");
    assert!(matches!(
        unsafe { send_raw(once, 901, &[], &[]) },
        Err(SystemCallError::StaleHandle)
    ));

    let once_pair = make_send_once(mailbox.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        .expect("transferred send-once mint failed");

    let once = once_pair.owner;

    // SAFETY: 本夹具不观察回复终态，关闭本次派生的独立观察表项。

    unsafe { libkernel::ipc::object::close(once_pair.peer) }.expect("reply observer close failed");
    let moves = [HandleMove {
        handle: once,
        rights: Rights::WRITE,
    }];
    unsafe { send_raw(mailbox.peer, 902, &[], &moves) }.expect("send-once transit failed");
    let first = receive(mailbox.owner).expect("send-once receive failed");
    assert_eq!(first.header.kind, 900);
    let mut second = receive(mailbox.owner).expect("send-once transit receive failed");
    assert_eq!(second.header.kind, 902);
    let transferred_once = second
        .handles
        .take(0)
        .expect("send-once slot missing")
        .into_raw();
    unsafe { send_raw(transferred_once, 903, &[2], &[]) }.expect("transferred once send failed");
    assert!(matches!(
        unsafe { send_raw(transferred_once, 904, &[], &[]) },
        Err(SystemCallError::StaleHandle)
    ));

    // 原 sender 仍可长期使用，不受派生影响。
    unsafe { send_raw(mailbox.peer, 905, &[], &[]) }.expect("original sender still usable");
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
    let once_pair = make_send_once(full.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        .expect("send-once full mint failed");
    let once = once_pair.owner;
    // SAFETY: 本夹具不观察回复终态，关闭本次派生的独立观察表项。
    unsafe { libkernel::ipc::object::close(once_pair.peer) }.expect("reply observer close failed");
    for _ in 0..MAILBOX_CAPACITY {
        unsafe { send_raw(full.peer, 0, &[], &[]) }.expect("send-once full fill failed");
    }
    assert!(matches!(
        unsafe { send_raw(once, 910, &[], &[]) },
        Err(SystemCallError::MailboxFull)
    ));
    discard(full.owner).expect("send-once full make-room failed");
    unsafe { send_raw(once, 911, &[], &[]) }.expect("failed send must not consume once");
    assert!(matches!(
        unsafe { send_raw(once, 912, &[], &[]) },
        Err(SystemCallError::StaleHandle)
    ));
    for _ in 0..MAILBOX_CAPACITY {
        discard(full.owner).expect("send-once full drain failed");
    }
    unsafe { close(full.peer) }.expect("send-once full sender close failed");
    unsafe { close(full.owner) }.expect("send-once full owner close failed");

    // once 同时作为发送目标与 transit move 会突破一次投递保证，必须在
    // 任何入队或摘除前整体拒绝；失败不消费 once。
    let both = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("send-once both mailbox create failed");
    let once_pair = make_send_once(both.peer, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        .expect("send-once both mint failed");
    let once = once_pair.owner;
    // SAFETY: 本夹具不观察回复终态，关闭本次派生的独立观察表项。
    unsafe { libkernel::ipc::object::close(once_pair.peer) }.expect("reply observer close failed");
    assert!(matches!(
        make_send_once(once, Rights::WRITE),
        Err(SystemCallError::RightsDenied)
    ));
    let moves = [HandleMove {
        handle: once,
        rights: Rights::WRITE,
    }];
    assert!(matches!(
        unsafe { send_raw(once, 920, &[], &moves) },
        Err(SystemCallError::IllegalArgument)
    ));
    unsafe { send_raw(once, 921, &[], &[]) }.expect("rejected alias must not consume send-once");
    assert!(matches!(
        unsafe { send_raw(once, 922, &[], &[]) },
        Err(SystemCallError::StaleHandle)
    ));
    let message = receive(both.owner).expect("send-once alias recovery receive failed");
    assert_eq!(message.header.kind, 921);
    assert!(message.handles.is_empty());
    unsafe { close(both.peer) }.expect("send-once both sender close failed");
    unsafe { close(both.owner) }.expect("send-once both owner close failed");
    debug!("send-once passed");
}

/// WRITABLE 电平快路径：空箱即时就绪，填满后腾位重新就绪。
/// “满箱清零”的阻塞侧由 [`test_writable_wake`] 跨进程验证。
pub(super) fn test_writable_level() {
    let mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT,
    )
    .expect("writable mailbox create failed");
    let result = wait_until(
        mailbox.peer,
        ObjectSignals::WRITABLE,
        libkernel::time::timeout_millis(0).unwrap(),
    )
    .expect("empty mailbox must be writable");
    assert!(result.observed.intersects(ObjectSignals::WRITABLE));

    for _ in 0..MAILBOX_CAPACITY {
        unsafe { send_raw(mailbox.peer, 0, &[], &[]) }.expect("writable fill failed");
    }
    discard(mailbox.owner).expect("writable make-room failed");
    let result = wait_until(
        mailbox.peer,
        ObjectSignals::WRITABLE,
        libkernel::time::timeout_millis(0).unwrap(),
    )
    .expect("mailbox below capacity must be writable");
    assert!(result.observed.intersects(ObjectSignals::WRITABLE));

    for _ in 0..MAILBOX_CAPACITY - 1 {
        discard(mailbox.owner).expect("writable drain failed");
    }
    unsafe { close(mailbox.peer) }.expect("writable sender close failed");
    unsafe { close(mailbox.owner) }.expect("writable owner close failed");
    debug!("writable level passed");
}

/// 跨进程流控唤醒：pm 填满目标邮箱后在 WRITABLE 上阻塞，本进程腾出一个
/// 位置唤醒它。pm 侧内联检测虚假唤醒（醒来后再撞满箱即置 spin 位），
/// 末尾校验 spin 为空——唤醒只能由腾位引起，证明等待路径真实走过。
pub(super) fn test_writable_wake(
    harness: &mut Harness,
    pm_mailbox: Handle,
) -> Result<(), &'static str> {
    let target = harness
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        )
        .map_err(|_| "wake target mailbox create failed")?;
    let done = harness
        .create_notification(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::SIGNAL | Rights::TRANSIT,
        )
        .map_err(|_| "wake done notification create failed")?;
    let spin = harness
        .create_notification(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::SIGNAL | Rights::TRANSIT,
        )
        .map_err(|_| "wake spin notification create failed")?;
    let moves = [
        HandleMove {
            handle: target.peer,
            rights: Rights::WRITE | Rights::WAIT,
        },
        HandleMove {
            handle: done.peer,
            rights: Rights::SIGNAL,
        },
        HandleMove {
            handle: spin.peer,
            rights: Rights::SIGNAL,
        },
    ];
    unsafe { send_raw(pm_mailbox, WRITABLE_WAKE_REQUEST, &[], &moves) }
        .map_err(|_| "wake request send failed")?;
    harness.transferred(target.peer);
    harness.transferred(done.peer);
    harness.transferred(spin.peer);

    // pm 确认已满后置位通知，此时它正阻塞在 WRITABLE 上。
    let deadline = libkernel::time::timeout_millis(5_000).map_err(|_| "wake deadline failed")?;
    let done_wait = libkernel::ipc::wait::wait_until(done.owner, ObjectSignals::READABLE, deadline)
        .map_err(|_| "wake notification wait failed")?;
    if WaitReason::from_u32(done_wait.reason) == Some(WaitReason::Timeout) {
        return Err("pm writable wake confirmation timed out");
    }
    harness
        .notification_owner(done.owner)
        .take(u64::MAX)
        .map_err(|_| "wake notification take failed")?;
    discard(target.owner).map_err(|_| "wake make-room failed")?;

    // 队列：15 × FILL + TAIL。逐条校验，末尾必然是被唤醒后补发的 TAIL。
    for index in 0..MAILBOX_CAPACITY {
        let ready = libkernel::ipc::wait::wait_until(
            target.owner,
            ObjectSignals::READABLE | ObjectSignals::CLOSED,
            deadline,
        )
        .map_err(|_| "writable tail wait failed")?;
        if WaitReason::from_u32(ready.reason) == Some(WaitReason::Timeout) {
            return Err("pm writable tail timed out");
        }
        let message = receive(target.owner).map_err(|_| "writable tail receive failed")?;
        let expected = if index + 1 == MAILBOX_CAPACITY {
            WRITABLE_WAKE_TAIL
        } else {
            WRITABLE_WAKE_FILL
        };
        if message.header.kind != expected {
            return Err("unexpected writable wake message");
        }
    }
    // TAIL 已入队即 pm 已退出循环；此前的任何虚假唤醒都会留下 spin 位。
    if !matches!(
        harness.notification_owner(spin.owner).take(u64::MAX),
        Err(SystemCallError::ObjectNotAvailable)
    ) {
        return Err("writable wake reported a spurious completion");
    }
    harness.close_control(done.owner)?;
    harness.close_control(spin.owner)?;
    harness.close_control(target.owner)?;
    debug!("writable wake passed");
    Ok(())
}

pub(super) fn stress_control_plane() {
    for index in 0..CONTROL_STRESS {
        let mailbox = create(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
        )
        .expect("stress mailbox create failed");
        let event = { let mut pair = libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */ unsafe { libkernel::sys_notification_create(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::SIGNAL | Rights::TRANSIT, &mut pair) }.map(|()| pair) }
        .expect("stress notification create failed");
        let moves = [HandleMove {
            handle: event.peer,
            rights: Rights::SIGNAL,
        }];
        unsafe { send_raw(mailbox.peer, index as u64, &index.to_le_bytes(), &moves) }
            .expect("stress send failed");
        assert!(matches!(
            unsafe { close(event.peer) },
            Err(SystemCallError::StaleHandle)
        ));
        let message = receive(mailbox.owner).expect("stress receive failed");
        assert_eq!(message.header.kind, index as u64);
        assert_eq!(message.payload, index.to_le_bytes());
        /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */
        unsafe {
            libkernel::sys_notification_signal(
                message
                    .handles
                    .get(0)
                    .expect("stress signaler slot missing")
                    .as_handle(),
                1,
            )
        }
        .expect("stress signal failed");
        assert_eq!(
            {
                let mut bits = 0; /* SAFETY: ABI 夹具借用有效 handle，输出在同步调用期间私有。 */
                unsafe { libkernel::sys_notification_take(event.owner, 1, &mut bits) }
                    .map(|()| bits)
            }
            .expect("stress take failed"),
            1
        );
        drop(message);
        unsafe { close(event.owner) }.expect("stress notification owner close failed");
        unsafe { close(mailbox.peer) }.expect("stress mailbox sender close failed");
        unsafe { close(mailbox.owner) }.expect("stress mailbox owner close failed");
    }

    let mailbox = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .expect("full mailbox create failed");
    for _ in 0..MAILBOX_CAPACITY {
        unsafe { send_raw(mailbox.peer, 0, &[], &[]) }.expect("mailbox fill failed");
    }
    let event = { let mut pair = libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */ unsafe { libkernel::sys_notification_create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT, &mut pair) }.map(|()| pair) }
    .expect("full mailbox notification create failed");
    let moves = [HandleMove {
        handle: event.peer,
        rights: Rights::SIGNAL,
    }];
    assert!(matches!(
        unsafe { send_raw(mailbox.peer, 0, &[], &moves) },
        Err(SystemCallError::MailboxFull)
    ));
    /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */
    unsafe { libkernel::sys_notification_signal(event.peer, 1) }
        .expect("failed Send must retain moved source");
    for _ in 0..MAILBOX_CAPACITY {
        discard(mailbox.owner).expect("mailbox discard failed");
    }
    unsafe { close(event.peer) }.expect("retained signaler close failed");
    unsafe { close(event.owner) }.expect("retained owner close failed");
    unsafe { close(mailbox.peer) }.expect("full mailbox sender close failed");
    unsafe { close(mailbox.owner) }.expect("full mailbox owner close failed");
    debug!(
        "control-plane stress passed: {} transactions",
        CONTROL_STRESS
    );
}
