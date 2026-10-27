//! 正式消费者的服务发布、替换与名字订阅；共享的发布夹具供实际消费者及init装配验收复用。
use alloc::vec::Vec;
use erhino_shared::{object::Rights, time::Deadline};
use libkernel::ipc::{capability::Capability, message::MailboxSender};
use librpc::{CallOperation, CallPhase, Caller, Request as RpcRequest};
use libservice::{
    client::{Client, Publisher, RegistrationControl, Watch},
    protocol::{
        self, Addressing, Descriptors, ProtocolDescriptor, Reason, RegisterMode, RegistrationInfo,
        Request, Response, State, Status,
    },
};
pub const DESCRIPTORS: [ProtocolDescriptor; 1] = [ProtocolDescriptor {
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
pub fn register(
    authority: &MailboxSender,
    endpoint: &MailboxSender,
    caller: &mut dyn librpc::Exchange,
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
pub fn prepare_registration(
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
