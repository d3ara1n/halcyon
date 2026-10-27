//! 独立消费者：service 名字订阅/取得、OAL 发现与直接 fs 路径操作。
#![no_std]
#![feature(allocator_ext)]
extern crate alloc;
extern crate libruntime;
use alloc::{sync::Arc, vec::Vec};
use erhino_shared::{
    object::{ObjectSignals, Rights},
    time::Deadline,
};
use libfs::{
    FsRights,
    client::{Client, ClientError},
    node::NodeKind,
    protocol::{IoEnd, Status},
    value::Value,
};
use libkernel::ipc::{
    message::{Mailbox, MailboxSender, wait_message_until},
    notification,
    wait::wait_until,
};
use liboal::{FollowLimits, Namespace};
use librpc::Caller;
use libservice::{client::Client as Services, protocol as service};
mod binding;
mod derive;
mod quota;
mod stream;
mod watch;
fn management_sender(capability: libkernel::ipc::capability::Capability) -> MailboxSender {
    let (sender, description) = MailboxSender::from_capability(capability)
        .map_err(|failed| failed.error)
        .expect("consumer management role invalid");
    assert!(
        description
            .rights
            .contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE)
            && !description.rights.contains(Rights::GRANT),
        "consumer management transport invalid"
    );
    sender
}
fn limits() -> FollowLimits {
    FollowLimits::new(16, 4 * erhino_shared::message::PAYLOAD_MAX, 5_000_000_000)
        .expect("consumer follow policy invalid")
}
fn deadline() -> Deadline {
    libkernel::time::timeout_millis(5_000).expect("consumer deadline invalid")
}
fn wait_command(owner: &notification::NotificationOwner, bit: u64) {
    // 根阶段不同于单次文件 RPC，仍以共同协议预算保留独立故障退出界。
    let until = libkernel::time::timeout_millis(test_fs::command::PHASE_TIMEOUT_MILLIS)
        .expect("consumer command deadline invalid");
    loop {
        let bits = match owner.take(bit) {
            Ok(bits) => bits,
            Err(erhino_shared::call::SystemCallError::ObjectNotAvailable) => 0,
            Err(error) => panic!("consumer command take failed: {:?}", error),
        };
        if bits & bit != 0 {
            return;
        }
        let observed = wait_until(
            owner.as_handle(),
            ObjectSignals::READABLE | ObjectSignals::CLOSED,
            until,
        )
        .expect("consumer command wait failed");
        assert!(
            !observed.observed.intersects(ObjectSignals::CLOSED),
            "consumer command notification closed"
        );
        assert!(
            observed.observed.intersects(ObjectSignals::READABLE),
            "consumer command wait expired"
        );
    }
}
fn bind(namespace: &mut Namespace, path: &str, sender: MailboxSender) -> Arc<MailboxSender> {
    let sender = Arc::try_new(sender).expect("consumer entry allocation failed");
    namespace
        .bind(path, sender.clone())
        .map_err(|f| f.cause)
        .expect("consumer namespace binding failed");
    sender
}
fn check_basic(namespace: &Namespace, caller: &mut Caller, prefix: &str, entry: &MailboxSender) {
    let path = |name: &str| alloc::format!("{prefix}/{name}");
    let directory = path("consumer");
    let property = path("consumer/value");
    let stream = path("consumer/bytes");
    let link = path("consumer/link");
    let mut initial = [0; 20];
    let used = Value::Unsigned64(41)
        .encode(&mut initial)
        .expect("property fixture invalid");
    let mut fs = Client::new(namespace, caller, limits());
    fs.create(
        &directory,
        NodeKind::Directory,
        FsRights::ALL,
        &[],
        deadline(),
    )
    .expect("directory create failed");
    fs.create(
        &property,
        NodeKind::Property,
        FsRights::ALL,
        &initial[..used],
        deadline(),
    )
    .expect("property create failed");
    assert_eq!(
        fs.get::<u64>(&property, deadline())
            .expect("property get failed"),
        41
    );
    assert!(matches!(
        fs.get::<bool>(&property, deadline()),
        Err(ClientError::Status(Status::TypeMismatch))
    ));
    // 标量类型及有符号转换的范围边界来自纯值合同，不以编码实现作为预期。
    fs.set(&property, Value::Bool(true), deadline())
        .expect("boolean property set failed");
    assert!(
        fs.get::<bool>(&property, deadline())
            .expect("boolean property get failed")
    );
    fs.set(&property, Value::Signed64(i64::MIN), deadline())
        .expect("signed property set failed");
    assert_eq!(
        fs.get::<i64>(&property, deadline())
            .expect("signed property get failed"),
        i64::MIN
    );
    assert!(matches!(
        fs.get::<u64>(&property, deadline()),
        Err(ClientError::Status(Status::Range))
    ));
    fs.set(&property, Value::Unsigned64(u64::MAX), deadline())
        .expect("unsigned property set failed");
    assert_eq!(
        fs.get::<u64>(&property, deadline())
            .expect("unsigned property get failed"),
        u64::MAX
    );
    assert!(matches!(
        fs.get::<i64>(&property, deadline()),
        Err(ClientError::Status(Status::Range))
    ));
    let float_bits = 0x3ff4_0000_0000_0000;
    fs.set(
        &property,
        Value::Float64(f64::from_bits(float_bits)),
        deadline(),
    )
    .expect("floating property set failed");
    assert_eq!(
        fs.get::<f64>(&property, deadline())
            .expect("floating property get failed")
            .to_bits(),
        float_bits
    );
    fs.set(&property, Value::Blob(&[0, 255, 7, 0]), deadline())
        .expect("blob property set failed");
    let blob = fs
        .get_value(&property, deadline())
        .expect("blob property get failed");
    assert!(matches!(
        libfs::value::Property::decode(&blob)
            .expect("blob property invalid")
            .items()
            .next(),
        Some(libfs::value::Item::Blob([0, 255, 7, 0]))
    ));
    libkernel::debug!(
        "FS1 pure Property scalar types and range rejection passed: {}",
        prefix
    );
    fs.set(&property, Value::String("live value"), deadline())
        .expect("property set failed");
    assert_eq!(
        fs.get::<alloc::string::String>(&property, deadline())
            .expect("changed property get failed"),
        "live value"
    );
    fs.set(&property, Value::Unsigned64Array(&[2, 5, 9]), deadline())
        .expect("array property set failed");
    let encoded = fs
        .get_value(&property, deadline())
        .expect("array property get failed");
    assert_eq!(
        libfs::value::Property::decode(&encoded)
            .expect("array property invalid")
            .count(),
        3
    );
    fs.create(&stream, NodeKind::Stream, FsRights::ALL, &[], deadline())
        .expect("byte stream create failed");
    let written = fs
        .write_at(&stream, 3, b"XYZ", deadline())
        .expect("sparse write failed");
    assert_eq!(written.count, 3);
    let read = fs
        .read_at(&stream, 0, 32, deadline())
        .expect("sparse read failed");
    assert_eq!(read.data, [0, 0, 0, b'X', b'Y', b'Z']);
    assert_eq!(read.end, IoEnd::Eof);
    assert!(matches!(
        fs.sync(&stream, deadline()),
        Err(ClientError::Status(Status::Unsupported))
    ));
    stream::check(namespace, caller, prefix);
    stream::check_edges(namespace, caller, prefix);
    stream::check_permissions(namespace, caller, prefix);
    stream::check_abandoned(namespace, caller, prefix, entry);
    let mut fs = Client::new(namespace, caller, limits());
    fs.create(
        &link,
        NodeKind::SymbolicLink,
        FsRights::ALL,
        b"value",
        deadline(),
    )
    .expect("link create failed");
    assert_eq!(
        fs.read_link(&link, deadline()).expect("read link failed"),
        "value"
    );
    let target = fs
        .get_value(&link, deadline())
        .expect("symbolic link continuation failed");
    let target = libfs::value::Property::decode(&target).expect("link property value invalid");
    let values: Vec<u64> = target
        .items()
        .map(|item| match item {
            libfs::value::Item::Unsigned64(value) => value,
            _ => panic!("link property item type invalid"),
        })
        .collect();
    assert_eq!(values, [2, 5, 9]);
    assert!(matches!(
        fs.remove(&directory, deadline()),
        Err(ClientError::Status(Status::NotEmpty))
    ));
    let mut oal = liboal::Client::new(namespace, caller, limits());
    let object = oal
        .lookup(&directory, deadline())
        .expect("directory discovery failed");
    assert!(
        object
            .info()
            .traits()
            .any(|marker| marker == liboal::DIRECTORY)
    );
    let page = oal
        .enumerate(&directory, 1, deadline())
        .expect("directory enumeration failed");
    assert_eq!(page.page().count(), 1);
    let cursor = page
        .into_cursor()
        .expect("directory enumeration omitted continuation");
    let page = cursor
        .next(&mut oal, 1, deadline())
        .expect("directory continuation failed");
    assert_eq!(page.page().count(), 1);
    let mut fs = Client::new(namespace, caller, limits());
    for entry in [&link, &property, &stream] {
        fs.remove(entry, deadline())
            .expect("consumer member removal failed");
    }
    fs.remove(&directory, deadline())
        .expect("consumer directory removal failed");
    libkernel::debug!("FS1 independent direct paths passed: {}", prefix);
    watch::check(namespace, caller, prefix);
}
fn run(grants: &mut libruntime::startup::InitialGrants, parent_pid: u64) {
    let (inbox, _) = Mailbox::from_capability(
        grants
            .take(test_fs::startup::AUTHORITY_INBOX)
            .expect("consumer authority inbox missing"),
    )
    .map_err(|failure| failure.error)
    .expect("consumer authority inbox role invalid");
    let until =
        libkernel::time::timeout_millis(10_000).expect("consumer authority deadline invalid");
    let mut received =
        wait_message_until(inbox.as_handle(), until).expect("consumer authority receive failed");
    assert!(
        received.header.sender_pid == parent_pid
            && received.header.kind == service::AUTHORITY_GRANT_KIND
            && received.payload.is_empty()
            && received.handles.remaining() == test_fs::handoff::PRIMARY_SLOTS,
        "consumer authority envelope invalid"
    );
    let (authority, description) = MailboxSender::from_capability(
        received
            .handles
            .take(0)
            .expect("consumer authority slot missing"),
    )
    .map_err(|failure| failure.error)
    .expect("consumer authority role invalid");
    assert!(
        description
            .rights
            .contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
            && !description.rights.contains(Rights::GRANT),
        "consumer authority transport invalid"
    );
    let (authority_b, description_b) = MailboxSender::from_capability(
        received
            .handles
            .take(1)
            .expect("consumer B authority slot missing"),
    )
    .map_err(|failure| failure.error)
    .expect("consumer B authority role invalid");
    assert!(
        description_b
            .rights
            .contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
            && !description_b.rights.contains(Rights::GRANT),
        "consumer B authority transport invalid"
    );
    let management = management_sender(
        received
            .handles
            .take(test_fs::handoff::MANAGEMENT_A)
            .expect("consumer management A missing"),
    );
    let management_b = management_sender(
        received
            .handles
            .take(test_fs::handoff::MANAGEMENT_B)
            .expect("consumer management B missing"),
    );
    drop(received);
    let primary = MailboxSender::from_capability(
        grants
            .take(test_fs::startup::PRIMARY_ROOT)
            .expect("consumer primary missing"),
    )
    .map_err(|f| f.error)
    .expect("consumer primary role invalid")
    .0;
    let directory = MailboxSender::from_capability(
        grants
            .take(test_fs::startup::SERVICE_DIRECTORY)
            .expect("consumer service directory missing"),
    )
    .map_err(|f| f.error)
    .expect("consumer service directory role invalid")
    .0;
    let primary_b = MailboxSender::from_capability(
        grants
            .take(test_fs::startup::PRIMARY_ROOT_B)
            .expect("consumer B primary missing"),
    )
    .map_err(|failure| failure.error)
    .expect("consumer B primary role invalid")
    .0;
    let directory_b = MailboxSender::from_capability(
        grants
            .take(test_fs::startup::SERVICE_DIRECTORY_B)
            .expect("consumer B directory missing"),
    )
    .map_err(|failure| failure.error)
    .expect("consumer B directory role invalid")
    .0;
    assert!(
        !primary_b
            .description()
            .expect("consumer B primary rights query failed")
            .rights
            .contains(Rights::GRANT),
        "consumer B primary retained grant right"
    );
    assert!(
        !directory_b
            .description()
            .expect("consumer B directory rights query failed")
            .rights
            .contains(Rights::GRANT),
        "consumer B directory retained grant right"
    );
    assert!(
        !primary
            .description()
            .expect("primary rights query failed")
            .rights
            .contains(Rights::GRANT)
    );
    assert!(
        !directory
            .description()
            .expect("directory rights query failed")
            .rights
            .contains(Rights::GRANT)
    );
    let (command, _) = notification::NotificationOwner::from_capability(
        grants
            .take(test_fs::startup::COMMAND_OWNER)
            .expect("consumer command missing"),
    )
    .map_err(|f| f.error)
    .expect("consumer command role invalid");
    let (report, description) = notification::NotificationSignaler::from_capability(
        grants
            .take(test_fs::startup::REPORT_SIGNALER)
            .expect("consumer report missing"),
    )
    .map_err(|f| f.error)
    .expect("consumer report role invalid");
    assert!(
        description.rights.contains(Rights::SIGNAL),
        "consumer report signal right missing"
    );
    let mut namespace = Namespace::new();
    let primary = bind(&mut namespace, "/", primary);
    let directory = bind(&mut namespace, "/System/Services", directory);
    let primary_b = bind(&mut namespace, "/tenant-b", primary_b);
    let mut caller = Caller::new();
    let watch = Services::new(&namespace, &mut caller, limits())
        .watch_name("/System/Services", "fs.secondary", deadline())
        .expect("name subscription failed");
    let initial = watch
        .control
        .query(&mut caller, deadline())
        .expect("initial name query failed");
    watch
        .control
        .ack(&mut caller, initial.sequence, deadline())
        .expect("initial name ack failed");
    report
        .signal(test_fs::report::DISCOVERY_ARMED)
        .expect("discovery report failed");
    let entry = loop {
        let snapshot = watch
            .control
            .query(&mut caller, deadline())
            .expect("name watch query failed");
        match Services::new(&namespace, &mut caller, limits())
            .get_entry("/System/Services/fs.secondary", deadline())
        {
            Ok(entry) => {
                watch
                    .control
                    .ack(&mut caller, snapshot.sequence, deadline())
                    .expect("name watch ack failed");
                break entry;
            }
            Err(libservice::client::ClientError::Status(service::Status::NotFound)) => {
                watch
                    .control
                    .ack(&mut caller, snapshot.sequence, deadline())
                    .expect("missing name ack failed");
                wait_until(
                    watch.control.waiter.as_handle(),
                    ObjectSignals::READABLE,
                    deadline(),
                )
                .expect("name watch wait failed");
            }
            Err(error) => panic!("service discovery failed: {error:?}"),
        }
    };
    assert!(entry.descriptors.iter().any(|d| d.id == libfs::protocol::ID
        && d.min_version <= libfs::protocol::VERSION
        && d.max_version >= libfs::protocol::VERSION
        && d.addressing == service::Addressing::Relative));
    assert!(
        !entry
            .endpoint
            .description()
            .expect("service entry rights query failed")
            .rights
            .contains(Rights::GRANT)
    );
    let shutdown = Arc::try_new(entry.endpoint).expect("secondary owner allocation failed");
    namespace
        .bind("/provider-b", shutdown.clone())
        .map_err(|f| f.cause)
        .expect("secondary namespace binding failed");
    report
        .signal(test_fs::report::SECONDARY_DISCOVERED)
        .expect("secondary report failed");
    wait_command(&command, test_fs::command::CONTINUE);
    let mut received = wait_message_until(inbox.as_handle(), deadline())
        .expect("consumer secondary management receive failed");
    assert!(
        received.header.sender_pid == parent_pid
            && received.header.kind == libfs::route::MANAGEMENT_GRANT_KIND
            && received.payload.is_empty()
            && received.handles.remaining() == 1,
        "consumer secondary management envelope invalid"
    );
    let secondary_management = management_sender(
        received
            .handles
            .take(0)
            .expect("consumer secondary management missing"),
    );
    drop(received);
    inbox
        .close()
        .map_err(|(_, error)| error)
        .expect("consumer role inbox close failed");
    test_fs::registry::exercise(&directory, &authority, &primary, &shutdown)
        .expect("independent registry scenarios failed");
    test_fs::registry::exercise(&directory_b, &authority_b, &primary_b, &shutdown)
        .expect("independent B registry scenarios failed");
    drop(authority_b);
    drop(directory_b);
    drop(authority);
    check_basic(&namespace, &mut caller, "", &primary);
    check_basic(&namespace, &mut caller, "/provider-b", &shutdown);
    libkernel::debug!("FS1 independent primary and secondary bounded Open read passed");
    stream::check_accounts(&namespace, &mut caller);
    watch::check(&namespace, &mut caller, "/tenant-b");
    watch::check_accounts(&namespace, &mut caller);
    derive::check(&namespace, &mut caller, "");
    derive::check(&namespace, &mut caller, "/provider-b");
    quota::check_nodes(&namespace, &mut caller);
    quota::check_grants(&namespace, &mut caller);
    binding::check_capacity(&namespace, &mut caller, &management, &management_b);
    libkernel::debug!(
        "FS1 binding behavior scenarios started: now_ns={}",
        libkernel::time::snapshot()
            .expect("FS progress clock failed")
            .now_ns
    );
    binding::check(
        &namespace,
        &mut caller,
        &management,
        &management_b,
        &secondary_management,
    );
    libkernel::debug!(
        "FS1 binding behavior scenarios completed: now_ns={}",
        libkernel::time::snapshot()
            .expect("FS progress clock failed")
            .now_ns
    );
    drop(secondary_management);
    drop(management_b);
    drop(management);
    libkernel::debug!("SRV2 independent WatchName Query GetEntry binding passed");
    report
        .signal(test_fs::report::COMPLETE)
        .expect("consumer completion report failed");
    wait_command(&command, test_fs::command::OBSERVE_SHUTDOWN);
    wait_until(shutdown.as_handle(), ObjectSignals::CLOSED, deadline())
        .expect("secondary close observation failed");
    let response = Client::new(&namespace, &mut caller, limits())
        .get::<u64>("/provider-b/missing", deadline());
    assert!(response.is_err(), "closed provider accepted file request");
    loop {
        let snapshot = watch
            .control
            .query(&mut caller, deadline())
            .expect("withdrawal query failed");
        match Services::new(&namespace, &mut caller, limits())
            .get_entry("/System/Services/fs.secondary", deadline())
        {
            Err(libservice::client::ClientError::Status(service::Status::NotFound)) => {
                assert_ne!(snapshot.pending & service::WITHDRAWN, 0);
                watch
                    .control
                    .ack(&mut caller, snapshot.sequence, deadline())
                    .expect("withdrawal ack failed");
                break;
            }
            Ok(_) => {
                watch
                    .control
                    .ack(&mut caller, snapshot.sequence, deadline())
                    .expect("old entry ack failed");
                wait_until(
                    watch.control.waiter.as_handle(),
                    ObjectSignals::READABLE,
                    deadline(),
                )
                .expect("withdrawal wait failed");
            }
            Err(e) => panic!("withdrawal discovery failed: {e:?}"),
        }
    }
    watch
        .control
        .cancel(&mut caller, deadline())
        .expect("name subscription cancel failed");
    libkernel::debug!("SRV2 independent endpoint withdrawal passed");
    libkernel::debug!("FS1 independent consumer shutdown passed");
    report
        .signal(test_fs::report::PROVIDER_CLOSED)
        .expect("closed provider report failed");
}
fn main() {
    let startup = libruntime::startup::take().expect("consumer startup missing");
    let (process, _, mut grants) = startup.into_parts();
    run(&mut grants, process.parent_pid());
}
