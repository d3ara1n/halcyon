//! 真实目录签发及双目录 Rename：运输上限、独立寿命与直接父事件。
use super::{bind, deadline, limits};
use erhino_shared::object::Rights;
use libfs::{
    FsRights,
    client::{Client, ClientError, WatchControl},
    node::NodeKind,
    protocol::{self, Status},
    value::Value,
};
use liboal::Namespace;
use librpc::Caller;

fn query(control: &WatchControl, caller: &mut Caller) -> protocol::WatchInfo {
    control
        .query(caller, deadline())
        .expect("rename watch query failed")
}
fn clear(control: &WatchControl, caller: &mut Caller) -> protocol::WatchInfo {
    let info = query(control, caller);
    control
        .ack(caller, info.sequence, deadline())
        .expect("rename watch ack failed")
}
pub(super) fn check(namespace: &Namespace, caller: &mut Caller, prefix: &str) {
    let base = alloc::format!("{prefix}/consumer-derive");
    let a = alloc::format!("{base}/a");
    let b = alloc::format!("{base}/b");
    let restricted = alloc::format!("{base}/restricted");
    let source = alloc::format!("{a}/value");
    let destination = alloc::format!("{b}/renamed");
    let mandatory = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
    for path in [&base, &a, &b] {
        Client::new(namespace, caller, limits())
            .create(path, NodeKind::Directory, FsRights::ALL, &[], deadline())
            .expect("derive directory fixture failed");
    }
    Client::new(namespace, caller, limits())
        .create(
            &restricted,
            NodeKind::Directory,
            FsRights::TRAVERSE | FsRights::READ_PROPERTY,
            &[],
            deadline(),
        )
        .expect("restricted directory fixture failed");
    let mut bytes = [0; 20];
    let used = Value::Unsigned64(73)
        .encode(&mut bytes)
        .expect("derive property fixture invalid");
    Client::new(namespace, caller, limits())
        .create(
            &source,
            NodeKind::Property,
            FsRights::ALL,
            &bytes[..used],
            deadline(),
        )
        .expect("derive property fixture failed");
    assert!(matches!(
        Client::new(namespace, caller, limits()).derive_directory(
            &restricted,
            FsRights::READ_PROPERTY,
            mandatory,
            Rights::NONE,
            deadline()
        ),
        Err(ClientError::Status(Status::Permission))
    ));
    assert!(matches!(
        Client::new(namespace, caller, limits()).derive_directory(
            &source,
            FsRights::READ_PROPERTY,
            mandatory,
            Rights::NONE,
            deadline()
        ),
        Err(ClientError::Status(Status::NotDirectory))
    ));
    let issuer = Client::new(namespace, caller, limits())
        .derive_directory(
            &a,
            FsRights::TRAVERSE | FsRights::READ_PROPERTY | FsRights::DERIVE,
            mandatory | Rights::DUPLICATE,
            mandatory | Rights::DUPLICATE,
            deadline(),
        )
        .expect("scoped directory issuance failed");
    let mut scope = Namespace::new();
    let issuer = bind(&mut scope, "/", issuer);
    // 同一真实普通 grant 只切管理 kind，必须在管理授权边界拒绝。
    let denial_deadline = deadline();
    let mut route_bytes = [0; protocol::HEADER_LEN + 32];
    let route_used = libfs::route::encode_request(
        &libfs::route::Request::Unbind { path: "value" },
        denial_deadline,
        &mut route_bytes,
    )
    .expect("management negative request encoding failed");
    let denial = caller
        .call(
            &issuer,
            denial_deadline,
            librpc::Request::new(libfs::route::ID, &route_bytes[..route_used])
                .expect("management negative RPC preparation failed"),
        )
        .expect("management kind negative RPC failed");
    assert_eq!(
        libfs::route::decode_response(
            &denial.payload,
            2,
            denial_deadline,
            denial.handles.remaining() as u64
        )
        .expect("management negative response invalid"),
        Status::Permission
    );
    libkernel::debug!("FS1 ordinary sender management kind denied");
    for (rights, sender, output) in [
        (FsRights::WRITE_PROPERTY, mandatory, Rights::NONE),
        (
            FsRights::READ_PROPERTY,
            mandatory | Rights::GRANT,
            Rights::NONE,
        ),
        (FsRights::READ_PROPERTY, mandatory, Rights::GRANT),
    ] {
        assert!(matches!(
            Client::new(&scope, caller, limits()).derive_directory(
                "/",
                rights,
                sender,
                output,
                deadline()
            ),
            Err(ClientError::Status(Status::Permission))
        ));
    }
    let reader = Client::new(&scope, caller, limits())
        .derive_directory(
            "/",
            FsRights::TRAVERSE | FsRights::READ_PROPERTY,
            mandatory,
            Rights::NONE,
            deadline(),
        )
        .expect("independent child issuance failed");
    drop(scope);
    drop(issuer);
    // 最后母 sender 关闭不能撤销独立 child；child 无 D/G，仍是正式路径入口。
    let mut reader_scope = Namespace::new();
    bind(&mut reader_scope, "/", reader);
    assert_eq!(
        Client::new(&reader_scope, caller, limits())
            .get::<u64>("/value", deadline())
            .expect("child grant was revoked with parent"),
        73
    );
    assert!(matches!(
        Client::new(&reader_scope, caller, limits()).set(
            "/value",
            Value::Unsigned64(0),
            deadline()
        ),
        Err(ClientError::Status(Status::Permission))
    ));
    assert!(matches!(
        Client::new(&reader_scope, caller, limits()).get_value("/outside", deadline()),
        Err(ClientError::Status(Status::NotFound))
    ));
    drop(reader_scope);

    let mut watches = alloc::vec::Vec::new();
    for path in [&base, &a, &b, &source] {
        watches
            .try_reserve(1)
            .expect("rename watch fixture allocation failed");
        let watch = Client::new(namespace, caller, limits())
            .watch(path, protocol::WATCH_RENAME, deadline())
            .expect("rename watch fixture failed");
        clear(&watch.control, caller);
        watches.push(watch);
    }
    let untouched_parent = query(&watches[0].control, caller);
    let untouched_file = query(&watches[3].control, caller);
    // missing 被词法抵消，不要求远端存在；目标父授权由 Client 的同一预算派生。
    let lexical_destination = alloc::format!("{b}/./missing/../renamed");
    Client::new(namespace, caller, limits())
        .rename(&source, &lexical_destination, false, deadline())
        .expect("same backing rename failed");
    assert!(matches!(
        Client::new(namespace, caller, limits()).get_value(&source, deadline()),
        Err(ClientError::Status(Status::NotFound))
    ));
    assert_eq!(
        Client::new(namespace, caller, limits())
            .get::<u64>(&destination, deadline())
            .expect("renamed value missing"),
        73
    );
    for index in [1, 2] {
        assert_ne!(
            query(&watches[index].control, caller).pending & protocol::WATCH_RENAME,
            0
        );
        clear(&watches[index].control, caller);
    }
    assert_eq!(
        query(&watches[0].control, caller),
        untouched_parent,
        "rename propagated to ancestor"
    );
    assert_eq!(
        query(&watches[3].control, caller),
        untouched_file,
        "rename signaled moved target"
    );
    let other = if prefix.is_empty() { "/provider-b" } else { "" };
    let foreign = alloc::format!("{other}/cross-filesystem-target");
    assert!(matches!(
        Client::new(namespace, caller, limits()).rename(&destination, &foreign, false, deadline()),
        Err(ClientError::Status(Status::CrossFilesystem))
    ));
    assert_eq!(
        Client::new(namespace, caller, limits())
            .get::<u64>(&destination, deadline())
            .expect("cross filesystem rename changed source"),
        73
    );

    // 错型但合法运输能力必须收到 Permission；由原 RPC owner 清理所有输入槽。
    let pair = libkernel::ipc::notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT,
    )
    .expect("rename role fixture failed");
    let mut frame = [0; protocol::BODY_MAX + protocol::HEADER_LEN];
    let until = deadline();
    let budget = limits()
        .start(
            libkernel::time::snapshot().expect("rename fixture clock failed"),
            until,
        )
        .expect("rename fixture budget invalid");
    let walk = liboal::walk::Walk::new(namespace, &destination, "/", budget)
        .expect("rename fixture binding failed");
    let (endpoint, path) = walk.target().expect("rename fixture target missing");
    let operation = protocol::Request::Rename {
        source: path,
        destination_name: "invalid",
        replace: false,
    };
    let used = protocol::encode_request(&operation, until, &mut frame)
        .expect("rename role request invalid");
    let same_mailbox_control = Client::new(namespace, caller, limits())
        .watch(&b, 0, deadline())
        .expect("same mailbox role fixture failed");
    for (capability, rights) in [
        (
            pair.peer.into_capability(),
            Rights::SIGNAL | Rights::TRANSIT,
        ),
        (
            same_mailbox_control.control.sender.into_capability(),
            mandatory,
        ),
    ] {
        let mut request = librpc::Request::new(protocol::ID, &frame[..used])
            .expect("rename role request allocation failed");
        request
            .push(capability, rights)
            .map_err(|failed| failed.error)
            .expect("rename role input failed");
        let reply = caller
            .call(endpoint, until, request)
            .expect("wrong role did not return domain response");
        assert_eq!(reply.handles.remaining(), 0);
        assert!(matches!(
            protocol::decode_response(&reply.payload, &operation, until, 0),
            Ok(protocol::Response::Error(Status::Permission))
        ));
    }
    drop(same_mailbox_control.control.waiter);
    drop(pair.owner);

    let renamed_a = alloc::format!("{base}/renamed-a");
    let child_before = query(&watches[1].control, caller);
    Client::new(namespace, caller, limits())
        .rename(&a, &renamed_a, false, deadline())
        .expect("directory rename failed");
    assert_ne!(
        query(&watches[0].control, caller).pending & protocol::WATCH_RENAME,
        0
    );
    assert_eq!(
        query(&watches[1].control, caller),
        child_before,
        "directory rename signaled target itself"
    );
    for watch in watches {
        watch
            .control
            .cancel(caller, deadline())
            .expect("rename watch cleanup failed");
    }
    // Replace 冲突保持两边，成功替换只沿真实双目录授权提交。
    let incoming = alloc::format!("{b}/incoming");
    let replacement = alloc::format!("{b}/replacement");
    let mut incoming_bytes = [0; 20];
    let incoming_used = Value::Unsigned64(73)
        .encode(&mut incoming_bytes)
        .expect("incoming property fixture invalid");
    let mut replacement_bytes = [0; 20];
    let replacement_used = Value::Unsigned64(91)
        .encode(&mut replacement_bytes)
        .expect("replacement property fixture invalid");
    let mut client = Client::new(namespace, caller, limits());
    client
        .create(
            &incoming,
            NodeKind::Property,
            FsRights::ALL,
            &incoming_bytes[..incoming_used],
            deadline(),
        )
        .expect("incoming property create failed");
    client
        .create(
            &replacement,
            NodeKind::Property,
            FsRights::ALL,
            &replacement_bytes[..replacement_used],
            deadline(),
        )
        .expect("replacement property create failed");
    assert!(matches!(
        client.rename(&incoming, &replacement, false, deadline()),
        Err(ClientError::Status(Status::Exists))
    ));
    assert_eq!(
        client
            .get::<u64>(&incoming, deadline())
            .expect("rename conflict lost source"),
        73
    );
    assert_eq!(
        client
            .get::<u64>(&replacement, deadline())
            .expect("rename conflict changed target"),
        91
    );
    client
        .rename(&incoming, &replacement, true, deadline())
        .expect("atomic replacement rename failed");
    assert!(matches!(
        client.get_value(&incoming, deadline()),
        Err(ClientError::Status(Status::NotFound))
    ));
    assert_eq!(
        client
            .get::<u64>(&replacement, deadline())
            .expect("replacement value missing"),
        73
    );
    let loop_parent = alloc::format!("{renamed_a}/child");
    let loop_target = alloc::format!("{loop_parent}/cycle");
    client
        .create(
            &loop_parent,
            NodeKind::Directory,
            FsRights::ALL,
            &[],
            deadline(),
        )
        .expect("cycle fixture create failed");
    assert!(matches!(
        client.rename(&renamed_a, &loop_target, false, deadline()),
        Err(ClientError::Status(Status::Invalid))
    ));
    liboal::Client::new(namespace, caller, limits())
        .lookup(&renamed_a, deadline())
        .expect("cycle rejection moved original directory");
    libkernel::debug!(
        "FS1 Rename replacement and descendant-cycle rejection passed: {}",
        prefix
    );
    for path in [
        &replacement,
        &loop_parent,
        &destination,
        &renamed_a,
        &b,
        &restricted,
        &base,
    ] {
        Client::new(namespace, caller, limits())
            .remove(path, deadline())
            .expect("derive fixture cleanup failed");
    }
    libkernel::debug!(
        "FS1 independent DeriveDirectory and Rename passed: {}",
        prefix
    );
}
