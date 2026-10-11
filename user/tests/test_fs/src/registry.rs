//! 正式消费者的服务发布、替换与名字订阅；队列边界探针在出生前由 init 调用。
use alloc::vec::Vec;
use erhino_shared::{object::Rights, time::Deadline};
use libkernel::ipc::{capability::Capability, message::MailboxSender};
use librpc::{CallOperation, CallPhase, Caller, Request as RpcRequest};
use libservice::{
    client::{Client, Publisher, RegistrationControl, Watch},
    protocol::{
        self, Addressing, Descriptors, ProtocolDescriptor, PublishRights, Reason, RegisterMode,
        RegistrationInfo, Request, Response, State, Status,
    },
};
const DESCRIPTORS: [ProtocolDescriptor; 1] = [ProtocolDescriptor {
    id: libfs::protocol::ID,
    min_version: libfs::protocol::VERSION,
    max_version: libfs::protocol::VERSION,
    addressing: Addressing::Relative,
}];
fn limits() -> liboal::FollowLimits {
    liboal::FollowLimits::new(16, 4 * erhino_shared::message::PAYLOAD_MAX, 5_000_000_000)
        .expect("registry follow policy invalid")
}
fn deadline() -> Result<Deadline, &'static str> {
    libkernel::time::timeout_millis(5_000).map_err(|_| "registry deadline invalid")
}
fn namespace(sender: &MailboxSender) -> Result<liboal::Namespace, &'static str> {
    let transport = Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE;
    let handle = libkernel::ipc::object::duplicate(sender.as_handle(), transport)
        .map_err(|_| "registry entry duplicate failed")?;
    // SAFETY: Duplicate 交付唯一新、未映射表项，typed owner 接管失败仍保持关闭责任。
    let (sender, _) = MailboxSender::from_capability(unsafe { Capability::from_raw(handle) })
        .map_err(|_| "registry entry role invalid")?;
    let mut namespace = liboal::Namespace::new();
    namespace
        .bind(
            "/",
            alloc::sync::Arc::try_new(sender).map_err(|_| "registry entry storage failed")?,
        )
        .map_err(|_| "registry entry binding failed")?;
    Ok(namespace)
}
fn register(
    authority: &MailboxSender,
    endpoint: &MailboxSender,
    caller: &mut Caller,
    name: &str,
    mode: RegisterMode,
) -> Result<(RegistrationInfo, RegistrationControl), libservice::client::ClientError> {
    Publisher {
        sender: authority,
        caller,
    }
    .register(
        name,
        mode,
        &DESCRIPTORS,
        endpoint,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
        libkernel::time::timeout_millis(5000).map_err(libservice::client::ClientError::System)?,
    )
}

/// 分步调用仍使用正式 codec/CallOperation；仅推迟领取私有回复，不代理 provider。
fn prepare_registration(
    command: &Request<'_>,
    endpoint: &MailboxSender,
    until: Deadline,
) -> Result<RpcRequest, &'static str> {
    let len = protocol::HEADER_LEN
        .checked_add(
            command
                .body_len()
                .map_err(|_| "staged registration body invalid")?,
        )
        .ok_or("staged registration size overflow")?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len)
        .map_err(|_| "staged registration allocation failed")?;
    bytes.resize(len, 0);
    let used = protocol::encode_request(command, until, &mut bytes)
        .map_err(|_| "staged registration encoding failed")?;
    let mut request = RpcRequest::new(protocol::ID, &bytes[..used])
        .map_err(|_| "staged registration RPC allocation failed")?;
    let transport = Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE;
    let handle = libkernel::ipc::object::duplicate(endpoint.as_handle(), transport)
        .map_err(|_| "staged registration endpoint duplicate failed")?;
    // SAFETY: Duplicate 为此函数交付唯一新、未映射表项；Packet 失败仍由 owner 关闭。
    let capability = unsafe { Capability::from_raw(handle) };
    request
        .push(capability, transport)
        .map_err(|_| "staged registration endpoint slot failed")?;
    Ok(request)
}

fn registration_reply(
    mut reply: librpc::Reply,
    command: &Request<'_>,
    until: Deadline,
) -> Result<(RegistrationInfo, RegistrationControl), &'static str> {
    // Fresh Reply 尚未摘取任何槽，HandleSet 的每个接收槽均有一个真实 owner。
    let slots = (1u64 << reply.handles.len()) - 1;
    let Response::Registration(info) =
        protocol::decode_response(&reply.payload, command, until, slots)
            .map_err(|_| "delayed registration reply invalid")?
    else {
        return Err("delayed registration did not succeed");
    };
    let (sender, description) = MailboxSender::from_capability(
        reply
            .handles
            .take(0)
            .map_err(|_| "delayed registration control missing")?,
    )
    .map_err(|_| "delayed registration control role invalid")?;
    if !description
        .rights
        .contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
    {
        return Err("delayed registration control rights invalid");
    }
    Ok((info, RegistrationControl { sender }))
}
fn watch(directory: &MailboxSender, caller: &mut Caller) -> Result<Watch, &'static str> {
    let ns = namespace(directory)?;
    Client::new(&ns, caller, limits())
        .watch_name("/", "init.registry-probe", deadline()?)
        .map_err(|_| "registry probe name watch failed")
}
pub fn exercise(
    directory: &MailboxSender,
    authority: &MailboxSender,
    primary: &MailboxSender,
    secondary: &MailboxSender,
) -> Result<(), &'static str> {
    let ns = namespace(directory)?;
    let mut caller = Caller::new();
    // 已由 init 签发 exact-name Create|Replace；本消费者没有 Delegate。
    let scoped = authority;
    let watch = watch(directory, &mut caller)?;
    let initial = watch
        .control
        .query(&mut caller, deadline()?)
        .map_err(|_| "registry initial watch query failed")?;
    watch
        .control
        .ack(&mut caller, initial.sequence, deadline()?)
        .map_err(|_| "registry initial watch ack failed")?;
    let (_, a) = register(
        scoped,
        primary,
        &mut caller,
        "init.registry-probe",
        RegisterMode::CreateOnly,
    )
    .map_err(|_| "registry first publication failed")?;
    if !matches!(
        register(
            scoped,
            secondary,
            &mut caller,
            "init.registry-probe",
            RegisterMode::CreateOnly
        ),
        Err(libservice::client::ClientError::Status(Status::Exists))
    ) {
        return Err("registry duplicate create result invalid");
    }
    let observed = watch
        .control
        .query(&mut caller, deadline()?)
        .map_err(|_| "registry publication watch query failed")?;
    let mut descriptor_bytes = [0; 20];
    let used = Descriptors::encode(&DESCRIPTORS, &mut descriptor_bytes)
        .map_err(|_| "registry staged descriptors invalid")?;
    let b_command = Request::Register {
        name: "init.registry-probe",
        mode: RegisterMode::Replace,
        descriptors: Descriptors::decode(&descriptor_bytes[..used])
            .map_err(|_| "registry staged descriptors decode failed")?,
        export_transport: Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    };
    let b_deadline = libkernel::time::timeout_millis(30_000)
        .map_err(|_| "registry delayed reply deadline failed")?;
    let mut b_caller = Caller::new();
    let mut b_operation = CallOperation::new(
        &mut b_caller,
        scoped,
        b_deadline,
        prepare_registration(&b_command, secondary, b_deadline)?,
        None,
    )
    .map_err(|_| "registry delayed replacement preparation failed")?;
    if b_operation
        .advance()
        .map_err(|_| "registry delayed replacement send failed")?
        .is_some()
        || b_operation.phase() != Some(CallPhase::Sent)
    {
        return Err("registry delayed replacement was not sent");
    }
    // wait_ready 只观察 READABLE，不 Receive；B 的真实控制仍在独立回复队列中。
    b_operation
        .wait_ready()
        .map_err(|_| "registry delayed replacement reply not queued")?;
    watch
        .control
        .ack(&mut caller, observed.sequence, deadline()?)
        .map_err(|_| "registry stale ack failed")?;
    let pending = watch
        .control
        .query(&mut caller, deadline()?)
        .map_err(|_| "registry replacement watch query failed")?;
    if pending.sequence <= observed.sequence || pending.pending & protocol::REPLACED == 0 {
        return Err("registry replacement lost after stale ack");
    }
    if a.query(&mut caller, deadline()?)
        .map_err(|_| "registry old control query failed")?
        .state
        != State::Withdrawn
    {
        return Err("registry old registration remained active");
    }
    a.withdraw(&mut caller, deadline()?)
        .map_err(|_| "registry old control withdrawal failed")?;
    let entry = Client::new(&ns, &mut caller, limits())
        .get_entry("/init.registry-probe", deadline()?)
        .map_err(|_| "registry replacement entry missing")?;
    let b_id = entry.id;
    if entry
        .endpoint
        .description()
        .map_err(|_| "registry replacement endpoint query failed")?
        .related_object_id
        != secondary
            .description()
            .map_err(|_| "registry secondary endpoint query failed")?
            .related_object_id
    {
        return Err("registry old control changed replacement");
    }
    let frozen = namespace(&entry.endpoint)?;
    let (c_info, c) = register(
        scoped,
        primary,
        &mut caller,
        "init.registry-probe",
        RegisterMode::Replace,
    )
    .map_err(|_| "registry final replacement failed")?;
    let delayed = b_operation
        .advance()
        .map_err(|_| "registry delayed replacement receive failed")?
        .ok_or("registry delayed replacement reply disappeared")?;
    let (b_info, b) = registration_reply(delayed, &b_command, b_deadline)?;
    if b_info.id != b_id || b_info.state != State::Active || b_info.reason != Reason::None {
        return Err("registry delayed reply lost its committed snapshot");
    }
    let b_now = b
        .query(&mut caller, deadline()?)
        .map_err(|_| "registry delayed control query failed")?;
    if b_now.id != b_id || b_now.state != State::Withdrawn || b_now.reason != Reason::Replaced {
        return Err("registry delayed control lost replacement state");
    }
    b.withdraw(&mut caller, deadline()?)
        .map_err(|_| "registry previous replacement withdrawal failed")?;
    let current = Client::new(&ns, &mut caller, limits())
        .get_entry("/init.registry-probe", deadline()?)
        .map_err(|_| "registry newest entry missing")?;
    if current.id != c_info.id {
        return Err("registry newest registration changed");
    }
    let mut fs = libfs::client::Client::new(&frozen, &mut caller, limits());
    let mut bytes = [0; 20];
    let used = libfs::value::Value::Unsigned64(11)
        .encode(&mut bytes)
        .map_err(|_| "registry file fixture invalid")?;
    fs.create(
        "/registry-binding",
        libfs::node::NodeKind::Property,
        libfs::FsRights::ALL,
        &bytes[..used],
        deadline()?,
    )
    .map_err(|_| "registry frozen namespace create failed")?;
    if fs
        .get::<u64>("/registry-binding", deadline()?)
        .map_err(|_| "registry frozen namespace get failed")?
        != 11
    {
        return Err("registry frozen namespace content invalid");
    }
    fs.remove("/registry-binding", deadline()?)
        .map_err(|_| "registry frozen namespace cleanup failed")?;
    c.withdraw(&mut caller, deadline()?)
        .map_err(|_| "registry current withdrawal failed")?;
    if !matches!(
        Client::new(&ns, &mut caller, limits()).get_entry("/init.registry-probe", deadline()?),
        Err(libservice::client::ClientError::Status(Status::NotFound))
    ) {
        return Err("registry withdrawn entry remained visible");
    }
    watch
        .control
        .cancel(&mut caller, deadline()?)
        .map_err(|_| "registry watch cancel failed")?;
    let terminal = watch
        .control
        .query(&mut caller, deadline()?)
        .map_err(|_| "registry terminal watch query failed")?;
    if terminal.state != libservice::protocol::WatchState::Terminal {
        return Err("registry cancelled watch did not retain terminal shell");
    }
    watch
        .control
        .ack(&mut caller, terminal.sequence, deadline()?)
        .map_err(|_| "registry terminal watch ack failed")?;
    libkernel::debug!("SRV2 init replacement control and stale Watch Ack passed");
    libkernel::debug!(
        "SRV2 delayed replacement reply passed: B snapshot and withdrawn control survive C"
    );
    Ok(())
}

/// 不领取的成功回复是真实持有；超时不是注册租约，清理后空名也不恢复 A。
pub fn queued_control(
    directory: &MailboxSender,
    authority: &MailboxSender,
    primary: &MailboxSender,
) -> Result<(), &'static str> {
    const NAME: &str = "init.registry-unknown";
    const QUOTA_NAME: &str = "init.registry-quota";
    let ns = namespace(directory)?;
    let mut caller = Caller::new();
    let scoped = Publisher {
        sender: authority,
        caller: &mut caller,
    }
    .delegate_name(
        NAME,
        PublishRights::CREATE | PublishRights::REPLACE,
        deadline()?,
    )
    .map_err(|_| "unknown registration delegation failed")?;
    let quota_scoped = Publisher {
        sender: authority,
        caller: &mut caller,
    }
    .delegate_name(
        QUOTA_NAME,
        PublishRights::CREATE | PublishRights::REPLACE,
        deadline()?,
    )
    .map_err(|_| "registration quota delegation failed")?;
    let watch = Client::new(&ns, &mut caller, limits())
        .watch_name("/", NAME, deadline()?)
        .map_err(|_| "unknown registration name watch failed")?;
    let (a_info, a) = register(
        &scoped,
        primary,
        &mut caller,
        NAME,
        RegisterMode::CreateOnly,
    )
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
    let mut operation = CallOperation::new(
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
    operation
        .wait_ready()
        .map_err(|_| "unknown registration reply was not queued")?;
    libkernel::debug!("SRV2 queued probe: replacement reply ready");
    let b_entry = Client::new(&ns, &mut caller, limits())
        .get_entry(NAME, deadline()?)
        .map_err(|_| "queued registration was not visible")?;
    if b_entry.id == a_info.id {
        return Err("queued registration did not replace A");
    }
    let b_id = b_entry.id;
    drop(b_entry);
    libkernel::debug!("SRV2 queued probe: replacement visible before deadline");
    // 真实 deadline 到期，仍不 Receive；持有接收队列本身不会主动取消其中的控制。
    libkernel::time::sleep_until(until).map_err(|_| "queued registration deadline wait failed")?;
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
    let queued = Client::new(&ns, &mut caller, limits())
        .get_entry(NAME, deadline()?)
        .map_err(|_| "queued registration expired like a lease")?;
    if queued.id != b_id {
        return Err("queued registration changed before cleanup");
    }
    drop(queued);
    let old = a
        .query(&mut caller, deadline()?)
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
            &mut caller,
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
            .query(&mut caller, removed_deadline)
            .map_err(|_| "unknown registration watch query failed")?;
        match Client::new(&ns, &mut caller, limits()).get_entry(NAME, removed_deadline) {
            Err(libservice::client::ClientError::Status(Status::NotFound)) => break,
            Ok(entry) if entry.id == b_id => {}
            _ => return Err("unknown registration restored a replaced name"),
        }
        watch
            .control
            .ack(&mut caller, snapshot.sequence, removed_deadline)
            .map_err(|_| "unknown registration watch ack failed")?;
        let ready = libkernel::ipc::wait::wait_until(
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
            &mut caller,
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
        .withdraw(&mut caller, deadline()?)
        .map_err(|_| "registration refund cleanup failed")?;
    if final_info.state != State::Withdrawn || final_info.reason != Reason::Explicit {
        return Err("registration refund control cleanup did not withdraw");
    }
    for control in &held {
        let info = control
            .query(&mut caller, deadline()?)
            .map_err(|_| "registration held control cleanup query failed")?;
        if info.state != State::Withdrawn || info.reason != Reason::Replaced {
            return Err("registration held control remained active");
        }
    }
    drop(recovered);
    drop(held);
    let old_info = a
        .withdraw(&mut caller, deadline()?)
        .map_err(|_| "unknown registration old control cleanup failed")?;
    if old_info.state != State::Withdrawn || old_info.reason != Reason::Replaced {
        return Err("unknown registration old control cleanup changed state");
    }
    drop(a);
    let cancelled = watch
        .control
        .cancel(&mut caller, deadline()?)
        .map_err(|_| "unknown registration watch cleanup failed")?;
    if cancelled.state != protocol::WatchState::Terminal
        || cancelled.reason != protocol::WatchReason::Cancelled
    {
        return Err("unknown registration watch remained active");
    }
    drop(watch);
    for name in [NAME, QUOTA_NAME] {
        if !matches!(
            Client::new(&ns, &mut caller, limits()).get_entry(name, deadline()?),
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
