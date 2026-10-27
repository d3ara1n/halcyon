//! init自身的注册回复队列/退款验收；全部外部等待由root推进。
use super::{RootSupervisor, deadline, limits, namespace};
use alloc::vec::Vec;
use erhino_shared::object::Rights;
use libkernel::ipc::message::MailboxSender;
use librpc::{CallOperation, CallPhase, Caller};
use libservice::{
    client::{Client, Publisher},
    protocol::{self, Descriptors, PublishRights, Reason, RegisterMode, Request, State, Status},
};
use test_fs::registry::{DESCRIPTORS, prepare_registration, register};

/// 不领取的成功回复是真实持有；超时不是注册租约，清理后空名也不恢复 A。
pub(crate) fn queued_control(
    root: &mut RootSupervisor,
    directory: &MailboxSender,
    authority: &MailboxSender,
    primary: &MailboxSender,
) -> Result<(), &'static str> {
    const NAME: &str = "init.registry-unknown";
    const QUOTA_NAME: &str = "init.registry-quota";
    let ns = namespace(directory)?;
    let scoped = Publisher {
        sender: authority,
        caller: root,
    }
    .delegate_name(
        NAME,
        PublishRights::CREATE | PublishRights::REPLACE,
        deadline()?,
    )
    .map_err(|_| "unknown registration delegation failed")?;
    let quota_scoped = Publisher {
        sender: authority,
        caller: root,
    }
    .delegate_name(
        QUOTA_NAME,
        PublishRights::CREATE | PublishRights::REPLACE,
        deadline()?,
    )
    .map_err(|_| "registration quota delegation failed")?;
    let watch = Client::new(&ns, root, limits())
        .watch_name("/", NAME, deadline()?)
        .map_err(|_| "unknown registration name watch failed")?;
    let (a_info, a) = register(&scoped, primary, root, NAME, RegisterMode::CreateOnly)
        .map_err(|_| "unknown registration initial publication failed")?;
    let mut descriptor_bytes = [0; 20];
    let used = Descriptors::encode(&DESCRIPTORS, &mut descriptor_bytes)
        .map_err(|_| "unknown registration descriptors invalid")?;
    let command = Request::Register {
        name: NAME,
        mode: RegisterMode::Replace,
        descriptors: Descriptors::decode(&descriptor_bytes[..used])
            .map_err(|_| "unknown registration descriptors decode failed")?,
        export_transport: Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    };
    // 仅用于启动前探针；仍真实等待到期，不与消费者命令窗口竞争。
    let until = libkernel::time::timeout_millis(500)
        .map_err(|_| "queued registration fixture deadline failed")?;
    let mut unknown_caller = Caller::new();
    let mut operation = CallOperation::new_composed(
        &mut unknown_caller,
        &scoped,
        until,
        prepare_registration(&command, primary, until)?,
        None,
    )
    .map_err(|_| "unknown registration preparation failed")?;
    if operation
        .advance()
        .map_err(|_| "unknown registration send failed")?
        .is_some()
        || operation.phase() != Some(CallPhase::Sent)
    {
        return Err("unknown registration request was not sent");
    }
    root.wait_rpc_ready(&operation, until)
        .map_err(|_| "unknown registration reply was not queued")?;
    libkernel::debug!("SRV2 queued probe: replacement reply ready");
    let b_entry = Client::new(&ns, root, limits())
        .get_entry(NAME, deadline()?)
        .map_err(|_| "queued registration was not visible")?;
    if b_entry.id == a_info.id {
        return Err("queued registration did not replace A");
    }
    let b_id = b_entry.id;
    drop(b_entry);
    libkernel::debug!("SRV2 queued probe: replacement visible before deadline");
    // 真实 deadline 到期，仍不 Receive；持有接收队列本身不会主动取消其中的控制。
    root.sleep_until(until)
        .map_err(|_| "queued registration deadline wait failed")?;
    if !libkernel::time::expired(until).map_err(|_| "queued registration expiry check failed")? {
        return Err("queued registration deadline wait returned early");
    }
    libkernel::debug!("SRV2 queued probe: original deadline elapsed");
    let mut unknown = operation
        .abort(librpc::CallCause::Timeout)
        .ok_or("unknown registration lost pending operation")?;
    if unknown.phase != CallPhase::Sent || !unknown.has_unretired_owners() {
        return Err("unknown registration lost reply cleanup owner");
    }
    let queued = Client::new(&ns, root, limits())
        .get_entry(NAME, deadline()?)
        .map_err(|_| "queued registration expired like a lease")?;
    if queued.id != b_id {
        return Err("queued registration changed before cleanup");
    }
    drop(queued);
    let old = a
        .query(root, deadline()?)
        .map_err(|_| "unknown registration old control query failed")?;
    if old.state != State::Withdrawn || old.reason != Reason::Replaced {
        return Err("unknown registration restored A");
    }
    libkernel::debug!("SRV2 queued probe: queued control retained registration");

    // 填满真实注册准入并保留每个控制；没有模拟预算或复制宿主容量常数。
    let mut held = Vec::new();
    let saturation_deadline = libkernel::time::timeout_millis(30_000)
        .map_err(|_| "registration saturation deadline failed")?;
    loop {
        if libkernel::time::expired(saturation_deadline)
            .map_err(|_| "registration saturation clock failed")?
        {
            return Err("registration quota did not saturate");
        }
        held.try_reserve(1)
            .map_err(|_| "registration control storage allocation failed")?;
        match register(
            &quota_scoped,
            primary,
            root,
            QUOTA_NAME,
            RegisterMode::Replace,
        ) {
            Ok((_, control)) => held.push(control),
            Err(libservice::client::ClientError::Status(Status::Quota)) => break,
            Err(_) => return Err("registration saturation failed before quota"),
        }
    }
    libkernel::debug!(
        "SRV2 queued probe: admission saturated with {} held controls",
        held.len()
    );
    // 只释放 B 的私有回复队列；其它注册控制保持存活，使恢复准入证明 B 的退款。
    unknown
        .retry_cleanup()
        .map_err(|_| "queued registration reply cleanup failed")?;
    if unknown.has_unretired_owners() {
        return Err("queued registration reply owners remained");
    }
    let removed_deadline = deadline()?;
    loop {
        let snapshot = watch
            .control
            .query(root, removed_deadline)
            .map_err(|_| "unknown registration watch query failed")?;
        match Client::new(&ns, root, limits()).get_entry(NAME, removed_deadline) {
            Err(libservice::client::ClientError::Status(Status::NotFound)) => break,
            Ok(entry) if entry.id == b_id => {}
            _ => return Err("unknown registration restored a replaced name"),
        }
        watch
            .control
            .ack(root, snapshot.sequence, removed_deadline)
            .map_err(|_| "unknown registration watch ack failed")?;
        let ready = root
            .wait_until(
                watch.control.waiter.as_handle(),
                erhino_shared::object::ObjectSignals::READABLE
                    | erhino_shared::object::ObjectSignals::CLOSED,
                removed_deadline,
            )
            .map_err(|_| "unknown registration withdrawal wait failed")?;
        if !ready
            .observed
            .intersects(erhino_shared::object::ObjectSignals::READABLE)
        {
            return Err("unknown registration withdrawal timed out");
        }
    }
    let refund_deadline = deadline()?;
    let recovered = loop {
        if libkernel::time::expired(refund_deadline)
            .map_err(|_| "registration refund clock failed")?
        {
            return Err("queued registration did not refund admission");
        }
        match register(
            &quota_scoped,
            primary,
            root,
            QUOTA_NAME,
            RegisterMode::Replace,
        ) {
            Ok((_, control)) => break control,
            // Quota 是明确未提交，可有限重试；provider 的原退休 owner 独立推进。
            Err(libservice::client::ClientError::Status(Status::Quota)) => {
                libruntime::thread::yield_now().map_err(|_| "registration refund yield failed")?
            }
            Err(_) => return Err("registration refund retry failed"),
        }
    };
    let final_info = recovered
        .withdraw(root, deadline()?)
        .map_err(|_| "registration refund cleanup failed")?;
    if final_info.state != State::Withdrawn || final_info.reason != Reason::Explicit {
        return Err("registration refund control cleanup did not withdraw");
    }
    for control in &held {
        let info = control
            .query(root, deadline()?)
            .map_err(|_| "registration held control cleanup query failed")?;
        if info.state != State::Withdrawn || info.reason != Reason::Replaced {
            return Err("registration held control remained active");
        }
    }
    drop(recovered);
    drop(held);
    let old_info = a
        .withdraw(root, deadline()?)
        .map_err(|_| "unknown registration old control cleanup failed")?;
    if old_info.state != State::Withdrawn || old_info.reason != Reason::Replaced {
        return Err("unknown registration old control cleanup changed state");
    }
    drop(a);
    let cancelled = watch
        .control
        .cancel(root, deadline()?)
        .map_err(|_| "unknown registration watch cleanup failed")?;
    if cancelled.state != protocol::WatchState::Terminal
        || cancelled.reason != protocol::WatchReason::Cancelled
    {
        return Err("unknown registration watch remained active");
    }
    drop(watch);
    for name in [NAME, QUOTA_NAME] {
        if !matches!(
            Client::new(&ns, root, limits()).get_entry(name, deadline()?),
            Err(libservice::client::ClientError::Status(Status::NotFound))
        ) {
            return Err("queued registration probe left a visible name");
        }
    }
    libkernel::debug!(
        "SRV2 unknown Replace reply passed: queued control survives deadline, empty name never restores A"
    );
    libkernel::debug!(
        "SRV2 queued registration control refund passed: saturated admission recovers after reply cleanup"
    );
    Ok(())
}
