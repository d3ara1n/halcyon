//! RPC 调用生命周期：可取消调用、帧拒绝清理与 Outbox 退休。
use super::*;

pub(super) fn exercise_cancellable_rpc(harness: &mut Harness) -> Result<(), &'static str> {
    let deadline = libkernel::time::timeout_millis(5_000)
        .map_err(|_| "FS1 cancellable RPC deadline failed")?;
    let event = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT,
    )
    .map_err(|_| "FS1 cancellable RPC Notification failed")?;
    let cancel = event.owner;
    event
        .peer
        .signal(1)
        .map_err(|_| "FS1 cancellable RPC signal failed")?;
    let blocked = harness
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .map_err(|_| "FS1 cancellable RPC blocked mailbox failed")?;
    for index in 0..MAILBOX_CAPACITY {
        harness
            .sender(blocked.peer)
            .send(0x4341_4e43_454c_0000 + index as u64, &[])
            .map_err(|_| "FS1 cancellable RPC mailbox fill failed")?;
    }
    let mut caller = librpc::Caller::new();
    let request = librpc::Request::new(u64::from_le_bytes(*b"CANCEL01"), b"cancel")
        .map_err(|_| "FS1 cancellable RPC first request failed")?;
    let first = caller.call_cancelable(
        harness.sender(blocked.peer),
        deadline,
        request,
        Some(cancel.as_handle()),
    );
    let Err(mut first) = first else {
        return Err("FS1 cancellable RPC did not return unsent owner");
    };
    if first.phase != librpc::CallPhase::Unsent
        || first.cause != librpc::CallCause::Cancelled
        || !first.has_unretired_owners()
    {
        return Err("FS1 cancellable RPC unsent phase invalid");
    }
    drop(
        first
            .take_unsent_request()
            .ok_or("FS1 cancellable RPC request missing")?,
    );
    if first.has_unretired_owners() {
        return Err("FS1 cancellable RPC request owner not released");
    }
    harness.close_control(blocked.owner)?;
    harness.close_control(blocked.peer)?;

    let sent = harness
        .create_mailbox(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .map_err(|_| "FS1 cancellable RPC sent mailbox failed")?;
    let request = librpc::Request::new(u64::from_le_bytes(*b"CANCEL01"), b"cancel")
        .map_err(|_| "FS1 cancellable RPC second request failed")?;
    cancel
        .take(u64::MAX)
        .map_err(|_| "FS1 cancellation reset failed")?;
    let mut operation = librpc::CallOperation::new(
        &mut caller,
        harness.sender(sent.peer),
        deadline,
        request,
        Some(cancel.as_handle()),
    )
    .map_err(|_| "FS1 cancellable RPC preparation failed")?;
    if operation
        .advance()
        .map_err(|_| "FS1 cancellable RPC send failed")?
        .is_some()
        || operation.phase() != Some(librpc::CallPhase::Sent)
    {
        return Err("FS1 cancellable RPC did not enter Sent");
    }
    // 取消在真实运输提交后发布；不再以预先有信号的空 mailbox 偶然进入 Sent。
    event
        .peer
        .signal(1)
        .map_err(|_| "FS1 sent cancellation signal failed")?;
    let cause = operation
        .poll_ready()
        .expect_err("sent cancellation was ignored");
    let mut second = operation.abort(cause).expect("Sent operation lost owner");
    drop(operation);
    if second.phase != librpc::CallPhase::Sent
        || second.cause != librpc::CallCause::Cancelled
        || !second.has_unretired_owners()
    {
        return Err("FS1 cancellable RPC sent phase invalid");
    }
    second
        .retry_cleanup()
        .map_err(|_| "FS1 cancellable RPC reply owner close failed")?;
    if second.has_unretired_owners() {
        return Err("FS1 cancellable RPC reply owner not released");
    }
    harness.close_control(sent.owner)?;
    harness.close_control(sent.peer)?;
    cancel
        .close()
        .map_err(|_| "FS1 cancellable RPC owner close failed")?;
    // SAFETY: 本函数仍独占 Notification 的信号端点。
    event
        .peer
        .close()
        .map_err(|(_, error)| error)
        .map_err(|_| "FS1 cancellable RPC peer close failed")?;
    debug!("FS1 cancellable RPC phases passed");
    Ok(())
}

pub(super) fn test_rpc_reject_cleanup() {
    const PROTOCOL: u64 = 0x7270_6301;

    let service = create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
    )
    .expect("RPC test service mailbox create failed");
    let rejected = { let mut pair = libkernel::shared::object::HandlePair::new(Handle::INVALID, Handle::INVALID); /* SAFETY: ABI 夹具独占有效输出及新表项关闭责任。 */ unsafe { libkernel::sys_notification_create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT, &mut pair) }.map(|()| pair) }
    .expect("RPC rejected Handle notification create failed");
    let rejected_alias = rejected.peer;
    let worker = libruntime::thread::Builder::new()
        .spawn(move || {
            for attempt in 0..2 {
                let mut request =
                    wait_message(service.owner).expect("RPC test request receive failed");
                if attempt == 0 {
                    let prefix = RpcPrefix::decode(&request.payload)
                        .expect("RPC test request prefix invalid");
                    assert_eq!(prefix.kind, RpcMessageKind::Request);
                    let (reply_once, _) = SendOnce::from_capability(
                        request
                            .handles
                            .take(0)
                            .expect("RPC test reply slot missing"),
                    )
                    .map_err(|failure| failure.error)
                    .expect("RPC test reply slot is not a send-once");
                    let mut response = [0u8; librpc::PREFIX_LEN + 1];
                    RpcPrefix::new(RpcMessageKind::Response, prefix.txid).encode(&mut response);
                    response[librpc::PREFIX_LEN] = attempt;
                    let mut packet = Packet::new(PROTOCOL + 1, &response)
                        .expect("RPC response packet allocation failed");
                    // SAFETY: 新创建的原始 signaler 尚未被其他 owner 接管，alias 仅用于 stale 检查。
                    let capability = unsafe { Capability::from_raw(rejected.peer) };
                    packet
                        .push(capability, Rights::SIGNAL)
                        .expect("RPC response capability preparation failed");
                    packet
                        .try_reply(reply_once, libkernel::time::Deadline::INFINITE)
                        .expect("RPC rejection response publication failed");
                    continue;
                }

                let context = RequestContext::decode(request, PROTOCOL)
                    .map_err(|rejected| rejected.reason)
                    .expect("RPC test request context invalid");
                let mut outbox = Outbox::prepare(
                    context,
                    1,
                    libkernel::time::Deadline::INFINITE,
                    1,
                    &libexecution::SourceFunding::Task,
                )
                .map_err(|failure| failure.error)
                .expect("RPC test Outbox preparation failed");
                outbox
                    .response_mut()
                    .expect("RPC test response remains owned")
                    .body_mut()
                    .expect("RPC test response body unavailable")[0] = attempt;
                outbox
                    .response_mut()
                    .expect("RPC test response remains owned")
                    .finish_body(1)
                    .expect("RPC test response body finalize failed");
                assert_eq!(
                    run_rpc_outbox(outbox),
                    OutboxResult::Sent,
                    "RPC Outbox did not deliver the accepted response"
                );
            }
            unsafe { close(service.owner) }.expect("RPC test service owner close failed");
        })
        .expect("RPC test worker spawn failed");

    // SAFETY: 此 sender 由原始队列工厂创建，本测试在此唯一接管关闭责任。
    let (service_sender, _) =
        MailboxSender::from_capability(unsafe { Capability::from_raw(service.peer) })
            .map_err(|failure| failure.error)
            .expect("RPC test service sender adoption failed");
    let mut caller = Caller::new();
    assert!(matches!(
        caller.call(
            &service_sender,
            libkernel::time::Deadline::INFINITE,
            Request::new(PROTOCOL, b"reject").expect("RPC request preparation failed")
        ),
        Err(CallError {
            phase: CallPhase::Sent,
            cause: CallCause::Frame(FrameRejection::ProtocolMismatch),
            ..
        })
    ));
    assert!(matches!(
        /* SAFETY: 原始 ABI 夹具的值参数，不改变表项关闭责任。 */
        unsafe { libkernel::sys_notification_signal(rejected_alias, 1) },
        Err(SystemCallError::StaleHandle)
    ));
    let reply = caller
        .call(
            &service_sender,
            libkernel::time::Deadline::INFINITE,
            Request::new(PROTOCOL, b"accept").expect("RPC request preparation failed"),
        )
        .expect("RPC call after rejected reply failed");
    assert_eq!(reply.payload, [1]);
    assert!(reply.handles.is_empty());

    take_thread_result(worker.join());
    unsafe { close(rejected.owner) }.expect("RPC rejected Handle owner close failed");
    drop(service_sender);
    debug!("RPC rejected reply cleanup passed");
}

pub(super) struct RpcOutboxTask {
    outbox: Outbox,
}

#[derive(Default)]
pub(super) struct RpcOutboxWorld {
    result: Option<OutboxResult>,
}

impl Task<RpcOutboxWorld> for RpcOutboxTask {
    type Family = Self;

    fn advance(
        &mut self,
        _id: u64,
        _world: &mut RpcOutboxWorld,
        requests: &mut Requests<'_, Self>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        let advance = self.outbox.advance(requests, input, budget)?;
        Ok(advance)
    }

    fn refused(&mut self, world: &mut RpcOutboxWorld, failure: RequestFailure<Self>) {
        self.outbox.refused(world, failure);
        world.result = self.outbox.result();
    }

    fn registered(&mut self, world: &mut RpcOutboxWorld, kind: SourceKind, source: SourceId) {
        self.outbox.registered(world, kind, source);
    }

    fn unregistered(&mut self, world: &mut RpcOutboxWorld, kind: SourceKind, source: SourceId) {
        self.outbox.unregistered(world, kind, source);
        world.result = self.outbox.result();
    }

    fn stop(&mut self, world: &mut RpcOutboxWorld) {
        self.outbox.stop(world);
        world.result = self.outbox.result();
    }

    fn deadline(&self) -> libkernel::time::Deadline {
        self.outbox.deadline()
    }
}

pub(super) fn run_rpc_outbox(outbox: Outbox) -> OutboxResult {
    let budget = Budget::new(&[2, 64 * 1024], 1).expect("RPC Outbox budget creation failed");
    let account = budget
        .account(&[2, 64 * 1024])
        .expect("RPC Outbox account creation failed");
    let account = account
        .view::<ExecutionResource>(&[budget.slot(0).unwrap(), budget.slot(1).unwrap()])
        .expect("RPC Outbox account binding failed");
    let set = WaitSet::create(4).expect("RPC Outbox WaitSet creation failed");
    let mut assembly = libexecution::assembly::Assembly::new(set, &account)
        .map_err(|(_, error)| error)
        .expect("RPC Outbox Assembly creation failed");
    let mut runtime =
        Runtime::<RpcOutboxTask, WaitSet>::new(assembly.bind().unwrap(), 1, 1, &account)
            .expect("RPC Outbox Runtime creation failed");
    runtime
        .spawn(libexecution::TaskSubmission::new(
            RpcOutboxTask { outbox },
            1,
            account.clone(),
        ))
        .map_err(|failure| failure.error)
        .expect("RPC Outbox task admission failed");
    let mut world = RpcOutboxWorld::default();
    assembly
        .run(&mut runtime, &mut world, 1)
        .expect("RPC Outbox Runtime failed");
    let result = world.result.expect("RPC Outbox lost terminal result");
    runtime
        .close()
        .map_err(|(_, error)| error)
        .expect("RPC Outbox Runtime close failed");
    assembly
        .close()
        .map_err(|(_, error)| error)
        .expect("RPC Outbox Assembly close failed");
    assert_eq!(
        account.usage(ExecutionResource::Task).0,
        0,
        "RPC Outbox task charge did not refund"
    );
    assert_eq!(
        account.usage(ExecutionResource::InputBytes).0,
        0,
        "RPC Outbox input charge did not refund"
    );
    result
}
