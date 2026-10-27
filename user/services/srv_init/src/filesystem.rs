//! init 的正式文件/服务消费者与启动交付核对。
pub(super) mod copy;
pub(super) mod registry;
use super::{AcceptedFsProvider, RootSupervisor};
use alloc::sync::Arc;
use erhino_shared::{
    object::{Handle, ObjectSignals, Rights},
    time::Deadline,
};
use libfs::{FsRights, client::Client, node::NodeKind, value::Value};
use libkernel::ipc::{capability::Capability, message::MailboxSender, object::duplicate};
use liboal::{FollowLimits, Namespace};
use librpc::Exchange;
use libservice::{
    client::{Client as Services, Watch},
    protocol as service,
};
pub(super) fn limits() -> FollowLimits {
    limits_with_timeout(5_000_000_000)
}
fn limits_with_timeout(timeout_ns: u64) -> FollowLimits {
    FollowLimits::new(16, 4 * erhino_shared::message::PAYLOAD_MAX, timeout_ns)
        .expect("init follow policy invalid")
}
pub(super) fn deadline() -> Result<Deadline, &'static str> {
    libkernel::time::timeout_millis(5_000).map_err(|_| "filesystem deadline invalid")
}
pub(super) fn namespace(sender: &MailboxSender) -> Result<Namespace, &'static str> {
    let handle = duplicate(
        sender.as_handle(),
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
    )
    .map_err(|_| "entry duplicate failed")?; // SAFETY: duplicate 交付唯一新、未映射 handle，由下面的 typed owner 接管。
    let cap = unsafe { Capability::from_raw(handle) };
    let (sender, _) = MailboxSender::from_capability(cap).map_err(|_| "entry role invalid")?;
    let mut namespace = Namespace::new();
    namespace
        .bind(
            "/",
            Arc::try_new(sender).map_err(|_| "namespace entry storage failed")?,
        )
        .map_err(|_| "namespace binding failed")?;
    Ok(namespace)
}
pub(super) fn name_watch(
    root: &mut RootSupervisor,
    directory: &MailboxSender,
) -> Result<Watch, &'static str> {
    let namespace = namespace(directory)?;
    let watch = Services::new(&namespace, root, limits())
        .watch_name("/", "fs.secondary", deadline()?)
        .map_err(|error| {
            libkernel::debug!("service name subscription failed: {:?}", error);
            "service name subscription failed"
        })?;
    let initial = watch
        .control
        .query(root, deadline()?)
        .map_err(|_| "service initial query failed")?;
    watch
        .control
        .ack(root, initial.sequence, deadline()?)
        .map_err(|_| "service initial ack failed")?;
    Ok(watch)
}
pub(super) fn accept_fs_provider(
    root: &mut RootSupervisor,
    bootstrap: Handle,
    release: Handle,
    primary: bool,
) -> Result<AcceptedFsProvider, &'static str> {
    let mut result = AcceptedFsProvider {
        pid: 0,
        grant: None,
        sender_identity: None,
        mailbox_identity: None,
        directory: None,
        registration_root: None,
        grant_b: None,
        directory_b: None,
        registration_root_b: None,
        management: None,
        management_b: None,
        bootstrap,
        release,
    };
    loop {
        let mut message = root
            .receive_until(bootstrap, Deadline::INFINITE)
            .map_err(|_| "provider bootstrap receive failed")?;
        if result.pid == 0 {
            result.pid = message.header.sender_pid;
        }
        if message.header.sender_pid != result.pid {
            return Err("provider bootstrap sender changed");
        }
        if message.header.kind == libfs::protocol::PROVIDER_READY_KIND {
            if primary
                || !message.handles.is_empty()
                || !message.payload.is_empty()
                || result.management.is_none()
            {
                return Err("unexpected provider ready layout");
            }
            break;
        }
        if (!primary && message.header.kind != libfs::route::MANAGEMENT_GRANT_KIND)
            || message.handles.remaining()
                != if primary {
                    srv_fs::deployment::ENTRIES
                } else {
                    1
                }
            || !message.payload.is_empty()
        {
            return Err("provider root layout invalid");
        }
        for entry in 0..if primary {
            srv_fs::deployment::ENTRIES
        } else {
            1
        } {
            let capability = message
                .handles
                .take(entry)
                .map_err(|_| "provider root missing")?;
            let (sender, description) = MailboxSender::from_capability(capability)
                .map_err(|_| "provider root role invalid")?;
            if !description.rights.contains(
                Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE | Rights::GRANT,
            ) {
                return Err("provider startup root rights invalid");
            }
            match message.header.kind {
                libfs::route::MANAGEMENT_GRANT_KIND => {
                    let management = if entry == 0 {
                        &mut result.management
                    } else {
                        &mut result.management_b
                    };
                    if management.is_some() {
                        return Err("duplicate management root");
                    }
                    *management = Some(sender);
                }
                libfs::protocol::ROOT_GRANT_KIND => {
                    let grant = if entry == 0 {
                        &mut result.grant
                    } else {
                        &mut result.grant_b
                    };
                    if grant.is_some() {
                        return Err("duplicate file root");
                    }
                    if entry == 0 {
                        result.sender_identity = Some(description.object_id);
                        result.mailbox_identity = Some(description.related_object_id);
                    }
                    *grant = Some(sender);
                }
                service::DIRECTORY_GRANT_KIND => {
                    let directory = if entry == 0 {
                        &mut result.directory
                    } else {
                        &mut result.directory_b
                    };
                    if directory.is_some() {
                        return Err("duplicate service directory");
                    }
                    *directory = Some(sender);
                }
                service::AUTHORITY_GRANT_KIND => {
                    let authority = if entry == 0 {
                        &mut result.registration_root
                    } else {
                        &mut result.registration_root_b
                    };
                    if authority.is_some() {
                        return Err("duplicate publication root");
                    }
                    *authority = Some(sender);
                }
                _ => return Err("unknown provider bootstrap kind"),
            }
        }
        if result.grant.is_some()
            && result.directory.is_some()
            && result.registration_root.is_some()
            && result.grant_b.is_some()
            && result.directory_b.is_some()
            && result.registration_root_b.is_some()
            && result.management.is_some()
            && result.management_b.is_some()
        {
            break;
        }
    }
    Ok(result)
}
pub(super) fn confirm_primary_fs_ready(
    root: &mut RootSupervisor,
    provider: &AcceptedFsProvider,
) -> Result<(), &'static str> {
    let message = root
        .receive_until(provider.bootstrap, Deadline::INFINITE)
        .map_err(|_| "provider ready receive failed")?;
    if message.header.sender_pid != provider.pid
        || message.header.kind != libfs::protocol::PROVIDER_READY_KIND
        || !message.handles.is_empty()
        || !message.payload.is_empty()
    {
        return Err("primary provider ready invalid");
    }
    Ok(())
}
pub(super) fn discover_registered_fs(
    root: &mut RootSupervisor,
    directory: &MailboxSender,
    primary_mailbox: u64,
    watch: &Watch,
) -> Result<(MailboxSender, u64, u64), &'static str> {
    let namespace = namespace(directory)?;
    loop {
        let observed = watch
            .control
            .query(root, deadline()?)
            .map_err(|_| "service watch query failed")?;
        match Services::new(&namespace, root, limits()).get_entry("/fs.secondary", deadline()?) {
            Ok(entry) => {
                if !entry.descriptors.iter().any(|d| {
                    d.id == libfs::protocol::ID
                        && d.min_version <= libfs::protocol::VERSION
                        && d.max_version >= libfs::protocol::VERSION
                        && d.addressing == service::Addressing::Relative
                }) {
                    return Err("service file protocol descriptor invalid");
                }
                let description = entry
                    .endpoint
                    .description()
                    .map_err(|_| "service endpoint query failed")?;
                if description.related_object_id == primary_mailbox
                    || description.rights.contains(Rights::GRANT)
                {
                    return Err("service shared endpoint scope invalid");
                }
                watch
                    .control
                    .ack(root, observed.sequence, deadline()?)
                    .map_err(|_| "service discovery ack failed")?;
                libkernel::debug!("SRV2 init WatchName Query GetEntry passed");
                return Ok((
                    entry.endpoint,
                    description.object_id,
                    description.related_object_id,
                ));
            }
            Err(libservice::client::ClientError::Status(service::Status::NotFound)) => {
                watch
                    .control
                    .ack(root, observed.sequence, deadline()?)
                    .map_err(|_| "service absent ack failed")?;
                root.wait_until(
                    watch.control.waiter.as_handle(),
                    ObjectSignals::READABLE,
                    deadline()?,
                )
                .map_err(|_| "service discovery wait failed")?;
            }
            Err(_) => return Err("service entry retrieval failed"),
        }
    }
}
fn prepare_probe(namespace: &Namespace, caller: &mut dyn Exchange) -> Result<(), &'static str> {
    let mut bytes = [0; 20];
    let used = Value::Unsigned64(73)
        .encode(&mut bytes)
        .map_err(|_| "property fixture invalid")?;
    let mut fs = Client::new(namespace, caller, limits());
    fs.create(
        "/init-probe",
        NodeKind::Property,
        FsRights::ALL,
        &bytes[..used],
        deadline()?,
    )
    .map_err(|_| "init property create failed")?;
    if fs
        .get::<u64>("/init-probe", deadline()?)
        .map_err(|_| "init property get failed")?
        != 73
    {
        return Err("init property value wrong");
    }
    fs.set("/init-probe", Value::Unsigned64(91), deadline()?)
        .map_err(|_| "init property set failed")?;
    if fs
        .get::<u64>("/init-probe", deadline()?)
        .map_err(|_| "init changed property get failed")?
        != 91
    {
        return Err("init changed property wrong");
    }
    Ok(())
}
pub(super) fn exercise_fs_basic(
    root: &mut RootSupervisor,
    sender: &MailboxSender,
) -> Result<(), &'static str> {
    let namespace = namespace(sender)?;
    prepare_probe(&namespace, root)?;
    Client::new(&namespace, root, limits())
        .remove("/init-probe", deadline()?)
        .map_err(|_| "init property removal failed")?;
    libkernel::debug!("FS1 init direct file paths passed");
    Ok(())
}
pub(super) fn exercise_fs_provider(
    root: &mut RootSupervisor,
    sender: &MailboxSender,
) -> Result<(), &'static str> {
    let namespace = namespace(sender)?;
    prepare_probe(&namespace, root)?;
    let watch = Client::new(&namespace, root, limits())
        .watch(
            "/init-probe",
            libfs::protocol::WATCH_MODIFY | libfs::protocol::WATCH_UNBOUND,
            deadline()?,
        )
        .map_err(|_| "init file watch failed")?;
    root.start_fs_watch(watch, deadline()?)?;
    root.await_client_snapshot()?;
    let initial = root
        .read_world()
        .fs_snapshot
        .expect("file snapshot remains owned");
    if Client::new(&namespace, root, limits())
        .get::<u64>("/init-probe", deadline()?)
        .map_err(|_| "initial watch recheck failed")?
        != 91
    {
        return Err("initial watch recheck wrong");
    }
    root.acknowledge_client_snapshot()?;
    root.await_client_cycle(1)?;
    Client::new(&namespace, root, limits())
        .set("/init-probe", Value::Unsigned64(92), deadline()?)
        .map_err(|_| "async watched property set failed")?;
    root.await_client_snapshot()?;
    let observed = root
        .read_world()
        .fs_snapshot
        .expect("file snapshot remains owned");
    if observed.sequence <= initial.sequence
        || observed.pending & libfs::protocol::WATCH_MODIFY == 0
    {
        return Err("async file change snapshot invalid");
    }
    // 重查在途期间再提交变化；确认 captured old sequence 后必须再次重查。
    Client::new(&namespace, root, limits())
        .set("/init-probe", Value::Unsigned64(93), deadline()?)
        .map_err(|_| "in-flight watched property set failed")?;
    if Client::new(&namespace, root, limits())
        .get::<u64>("/init-probe", deadline()?)
        .map_err(|_| "async property recheck failed")?
        != 93
    {
        return Err("async property recheck wrong");
    }
    root.acknowledge_client_snapshot()?;
    root.await_client_snapshot()?;
    let newer = root
        .read_world()
        .fs_snapshot
        .expect("newer file snapshot remains owned");
    if newer.sequence <= observed.sequence || newer.pending & libfs::protocol::WATCH_MODIFY == 0 {
        return Err("async old ack lost newer change");
    }
    Client::new(&namespace, root, limits())
        .get::<u64>("/init-probe", deadline()?)
        .map_err(|_| "newer property recheck failed")?;
    root.acknowledge_client_snapshot()?;
    root.await_client_cycle(3)?;
    Client::new(&namespace, root, limits())
        .remove("/init-probe", deadline()?)
        .map_err(|_| "init property removal failed")?;
    root.await_client_snapshot()?;
    loop {
        let snapshot = root
            .read_world()
            .fs_snapshot
            .expect("retirement snapshot remains owned");
        if !matches!(
            Client::new(&namespace, root, limits()).get_value("/init-probe", deadline()?),
            Err(libfs::client::ClientError::Status(
                libfs::protocol::Status::NotFound
            ))
        ) {
            return Err("retired file remained bound");
        }
        root.acknowledge_client_snapshot()?;
        if snapshot.state == libfs::protocol::WatchState::Terminal {
            break;
        }
        root.await_client_snapshot()?;
    }
    root.await_read()?;
    root.close_read()?;
    libkernel::debug!("FS1 async Watch recheck and old Ack passed");
    libkernel::debug!("FS1 init direct file paths passed");
    Ok(())
}
pub(super) fn exercise_async_name_watch(
    root: &mut RootSupervisor,
    directory: &MailboxSender,
    authority: &MailboxSender,
    endpoint: &MailboxSender,
) -> Result<(), &'static str> {
    const NAME: &str = "init.async-watch";
    const PATH: &str = "/init.async-watch";
    let namespace = namespace(directory)?;
    let scoped = libservice::client::Publisher {
        sender: authority,
        caller: root,
    }
    .delegate_name(
        NAME,
        service::PublishRights::CREATE | service::PublishRights::REPLACE,
        deadline()?,
    )
    .map_err(|_| "async name delegation failed")?;
    let watch = Services::new(&namespace, root, limits())
        .watch_name("/", NAME, deadline()?)
        .map_err(|_| "async name subscription failed")?;
    root.start_service_watch(watch, deadline()?)?;
    root.await_client_snapshot()?;
    if !matches!(
        Services::new(&namespace, root, limits()).get_entry(PATH, deadline()?),
        Err(libservice::client::ClientError::Status(
            service::Status::NotFound
        ))
    ) {
        return Err("async initial name was not absent");
    }
    root.acknowledge_client_snapshot()?;
    root.await_client_cycle(1)?;
    let descriptors = [service::ProtocolDescriptor {
        id: libfs::protocol::ID,
        min_version: libfs::protocol::VERSION,
        max_version: libfs::protocol::VERSION,
        addressing: service::Addressing::Relative,
    }];
    let (_, control) = libservice::client::Publisher {
        sender: &scoped,
        caller: root,
    }
    .register(
        NAME,
        service::RegisterMode::CreateOnly,
        &descriptors,
        endpoint,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
        deadline()?,
    )
    .map_err(|_| "async name registration failed")?;
    root.await_client_snapshot()?;
    if root
        .read_world()
        .service_snapshot
        .expect("registered snapshot remains owned")
        .pending
        & service::REGISTERED
        == 0
    {
        return Err("async registration hint missing");
    }
    Services::new(&namespace, root, limits())
        .get_entry(PATH, deadline()?)
        .map_err(|_| "async registered entry recheck failed")?;
    root.acknowledge_client_snapshot()?;
    root.await_client_cycle(2)?;
    let withdrawn = control
        .withdraw(root, deadline()?)
        .map_err(|_| "async name withdrawal failed")?;
    if withdrawn.state != service::State::Withdrawn || withdrawn.reason != service::Reason::Explicit
    {
        return Err("async name control did not withdraw");
    }
    root.await_client_snapshot()?;
    if root
        .read_world()
        .service_snapshot
        .expect("withdrawn snapshot remains owned")
        .pending
        & service::WITHDRAWN
        == 0
    {
        return Err("async withdrawal hint missing");
    }
    if !matches!(
        Services::new(&namespace, root, limits()).get_entry(PATH, deadline()?),
        Err(libservice::client::ClientError::Status(
            service::Status::NotFound
        ))
    ) {
        return Err("async withdrawn entry remained visible");
    }
    root.acknowledge_client_snapshot()?;
    root.await_client_cycle(3)?;
    root.stop_client()?;
    drop(control);
    drop(scoped);
    libkernel::debug!("SRV2 async WatchName recheck and stop passed");
    Ok(())
}
pub(super) fn observe_withdrawal(
    root: &mut RootSupervisor,
    directory: &MailboxSender,
    watch: &Watch,
) -> Result<(), &'static str> {
    let namespace = namespace(directory)?;
    loop {
        let observed = watch
            .control
            .query(root, deadline()?)
            .map_err(|_| "withdrawal watch query failed")?;
        match Services::new(&namespace, root, limits()).get_entry("/fs.secondary", deadline()?) {
            Err(libservice::client::ClientError::Status(service::Status::NotFound)) => {
                if observed.pending & service::WITHDRAWN == 0 {
                    return Err("withdrawal event missing");
                }
                watch
                    .control
                    .ack(root, observed.sequence, deadline()?)
                    .map_err(|_| "withdrawal ack failed")?;
                break;
            }
            Ok(_) => {
                watch
                    .control
                    .ack(root, observed.sequence, deadline()?)
                    .map_err(|_| "old entry ack failed")?;
                root.wait_until(
                    watch.control.waiter.as_handle(),
                    ObjectSignals::READABLE,
                    deadline()?,
                )
                .map_err(|_| "withdrawal wait failed")?;
            }
            Err(_) => return Err("withdrawal retrieval failed"),
        }
    }
    watch
        .control
        .cancel(root, deadline()?)
        .map_err(|_| "withdrawal subscription cancel failed")?;
    libkernel::debug!("SRV2 init endpoint withdrawal passed");
    Ok(())
}
pub(super) fn collect_fs_provider(
    root: &mut RootSupervisor,
    provider: &AcceptedFsProvider,
) -> Result<(), &'static str> {
    let message = root
        .receive_until(provider.bootstrap, Deadline::INFINITE)
        .map_err(|_| "provider stopped receive failed")?;
    if message.header.sender_pid != provider.pid
        || message.header.kind != libfs::protocol::PROVIDER_STOPPED_KIND
        || !message.handles.is_empty()
        || !message.payload.is_empty()
    {
        return Err("provider stopped layout invalid");
    }
    root.close_control(provider.release)?;
    root.close_control(provider.bootstrap)?;
    Ok(())
}
pub(super) fn release_fs_provider(
    root: &mut RootSupervisor,
    provider: &AcceptedFsProvider,
) -> Result<(), &'static str> {
    root.notification_signaler(provider.release)
        .signal(1)
        .map_err(|_| "provider release signal failed")?;
    collect_fs_provider(root, provider)
}
