//! 通过原 init 交付的管理角色与两个真实 provider 验证 Bind/Unbind。
use super::{deadline, limits};
use alloc::sync::Arc;
use erhino_shared::object::Rights;
use libfs::{
    FsRights,
    client::{Client, ClientError},
    node::NodeKind,
    protocol::Status,
    route,
    value::Value,
};
use libkernel::ipc::{capability::Capability, message::MailboxSender};
use liboal::Namespace;
use librpc::Caller;

fn manage(
    caller: &mut Caller,
    manager: &MailboxSender,
    request: route::Request<'_>,
    issuer: Option<&MailboxSender>,
) -> Status {
    manage_until(caller, manager, request, issuer, deadline())
}
fn manage_until(
    caller: &mut Caller,
    manager: &MailboxSender,
    request: route::Request<'_>,
    issuer: Option<&MailboxSender>,
    until: erhino_shared::time::Deadline,
) -> Status {
    let rpc = management_request(&request, issuer, until);
    let reply = caller
        .call(manager, until, rpc)
        .expect("management call failed");
    route::decode_response(
        &reply.payload,
        request.op(),
        until,
        (1u64 << reply.handles.len()) - 1,
    )
    .expect("management response invalid")
}
fn management_request(
    request: &route::Request<'_>,
    issuer: Option<&MailboxSender>,
    until: erhino_shared::time::Deadline,
) -> librpc::Request {
    let mut bytes = [0; libfs::protocol::HEADER_LEN + libfs::protocol::BODY_MAX];
    let used = route::encode_request(request, until, &mut bytes)
        .expect("management request encoding failed");
    let mut rpc =
        librpc::Request::new(route::ID, &bytes[..used]).expect("management RPC allocation failed");
    if let Some(issuer) = issuer {
        let rights = Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE;
        let handle = libkernel::ipc::object::duplicate(issuer.as_handle(), rights)
            .expect("binding issuer duplicate failed");
        // SAFETY: duplicate 返回本函数独占的新、未映射 handle，由 RPC Packet 接管。
        rpc.push(unsafe { Capability::from_raw(handle) }, rights)
            .expect("binding issuer input failed");
    }
    rpc
}
fn bind(
    caller: &mut Caller,
    manager: &MailboxSender,
    path: &str,
    issuer: &MailboxSender,
    rights: FsRights,
    output: Rights,
    replace: bool,
) -> Status {
    manage(
        caller,
        manager,
        route::Request::Bind {
            path,
            max_rights: rights,
            output_transport: output,
            replace,
        },
        Some(issuer),
    )
}
fn unbind(caller: &mut Caller, manager: &MailboxSender, path: &str) -> Status {
    manage(caller, manager, route::Request::Unbind { path }, None)
}
pub(super) fn check_capacity(
    namespace: &Namespace,
    caller: &mut Caller,
    management: &MailboxSender,
    management_b: &MailboxSender,
) {
    use srv_fs::deployment::BINDINGS;
    let stored = Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE;
    let output = srv_fs::deployment::root_output_transport();
    let mut fs = Client::new(namespace, caller, limits());
    for path in [
        "/quota-bindings",
        "/quota-bind-a",
        "/provider-b/quota-bind-b",
    ] {
        fs.create(path, NodeKind::Directory, FsRights::ALL, &[], deadline())
            .expect("binding quota directory create failed");
    }
    let issuer_a = fs
        .derive_directory("/quota-bind-a", FsRights::ALL, stored, output, deadline())
        .expect("binding quota first issuer failed");
    let issuer_b = fs
        .derive_directory(
            "/provider-b/quota-bind-b",
            FsRights::ALL,
            stored,
            output,
            deadline(),
        )
        .expect("binding quota second issuer failed");
    let issuers = [&issuer_a, &issuer_b];
    let requested = FsRights::TRAVERSE | FsRights::READ_PROPERTY | FsRights::ENUMERATE;
    let mut held = alloc::vec::Vec::new();
    held.try_reserve_exact(BINDINGS)
        .expect("binding quota storage failed");
    let mut quota = false;
    // 每项 Bind 会签发独立 shared grant；分摊到两个真实 provider，避免先耗尽目标 Grant。
    for index in 0..=BINDINGS {
        let path = alloc::format!("quota-bindings/n-{index}");
        match bind(
            caller,
            management,
            &path,
            issuers[index % issuers.len()],
            requested,
            stored,
            false,
        ) {
            Status::Ok => held.push(path),
            Status::Quota => {
                quota = true;
                break;
            }
            status => panic!("binding quota admission failed: {status:?}"),
        }
    }
    assert!(
        quota && held.len() == BINDINGS,
        "shared Binding limit was not the saturated resource"
    );
    let replacement = "quota-bindings/recovered";
    assert_eq!(
        bind(
            caller,
            management_b,
            replacement,
            &issuer_a,
            requested,
            stored,
            false
        ),
        Status::Quota,
        "other entry bypassed shared binding quota"
    );
    let released = held.pop().expect("binding quota owner missing");
    assert_eq!(unbind(caller, management, &released), Status::Ok);
    let until = deadline();
    loop {
        let status = manage_until(
            caller,
            management_b,
            route::Request::Bind {
                path: replacement,
                max_rights: requested,
                output_transport: stored,
                replace: false,
            },
            Some(&issuer_a),
            until,
        );
        match status {
            Status::Ok => break,
            Status::Quota => assert!(
                !libkernel::time::expired(until).expect("binding recovery clock failed"),
                "unbound binding did not retire and refund"
            ),
            status => panic!("binding quota recovery failed: {status:?}"),
        }
    }
    // Unbind 的名字撤销不代表记录/issuer/shared 已释放；成功 Bind 才核实际退款。
    assert_eq!(
        bind(
            caller, management, &released, &issuer_a, requested, stored, false
        ),
        Status::Quota,
        "recovered binding failed to consume shared capacity"
    );
    assert_eq!(unbind(caller, management_b, replacement), Status::Ok);
    for path in held {
        assert_eq!(unbind(caller, management, &path), Status::Ok);
    }
    drop(issuer_a);
    drop(issuer_b);
    let mut fs = Client::new(namespace, caller, limits());
    for path in [
        "/quota-bindings",
        "/quota-bind-a",
        "/provider-b/quota-bind-b",
    ] {
        fs.remove(path, deadline())
            .expect("binding quota directory cleanup failed");
    }
    libkernel::debug!("FS1 shared Binding quota, other entry rejection and owner refund passed");
}
fn boundary_transport(caller: &mut Caller, sender: &MailboxSender, path: &str, expected: Rights) {
    let request = libfs::protocol::Request::Get { path };
    let until = deadline();
    let mut bytes = [0; libfs::protocol::HEADER_LEN + libfs::protocol::BODY_MAX];
    let used = libfs::protocol::encode_request(&request, until, &mut bytes)
        .expect("boundary probe encoding failed");
    let reply = caller
        .call(
            sender,
            until,
            librpc::Request::new(libfs::protocol::ID, &bytes[..used])
                .expect("boundary probe allocation failed"),
        )
        .expect("boundary probe call failed");
    let response = libfs::protocol::decode_response(
        &reply.payload,
        &request,
        until,
        (1u64 << reply.handles.len()) - 1,
    )
    .expect("boundary probe response invalid");
    let libfs::protocol::Response::Boundary(liboal::Boundary::Provider(boundary)) = response else {
        panic!("boundary probe did not return provider");
    };
    assert_eq!(
        boundary.remaining, "value",
        "binding consumed business suffix"
    );
    assert_eq!(
        reply
            .handles
            .get(0)
            .expect("boundary output missing")
            .description()
            .expect("boundary output query failed")
            .rights,
        expected
    );
}
fn get_request(path: &str, until: erhino_shared::time::Deadline) -> librpc::Request {
    let mut bytes = [0; libfs::protocol::HEADER_LEN + libfs::protocol::BODY_MAX];
    let used =
        libfs::protocol::encode_request(&libfs::protocol::Request::Get { path }, until, &mut bytes)
            .expect("queued boundary request encoding failed");
    librpc::Request::new(libfs::protocol::ID, &bytes[..used])
        .expect("queued boundary request allocation failed")
}
fn abandon_queued(
    sender: &MailboxSender,
    request: librpc::Request,
    until: erhino_shared::time::Deadline,
) {
    // 复用正式 CallOperation 私有回复队列；不 Receive，关闭真实 queued reply owner。
    let mut caller = Caller::new();
    let mut operation =
        librpc::caller::CallOperation::new(&mut caller, sender, until, request, None)
            .expect("unknown binding operation preparation failed");
    assert!(
        operation
            .advance()
            .expect("unknown binding send failed")
            .is_none()
            && operation.phase() == Some(librpc::CallPhase::Sent),
        "unknown binding was not Sent"
    );
    operation
        .wait_ready()
        .expect("binding reply was not queued");
    let mut unknown = operation
        .abort(librpc::CallCause::Cancelled)
        .expect("unknown binding operation lost owner");
    assert_eq!(unknown.phase, librpc::CallPhase::Sent);
    assert!(
        unknown.has_unretired_owners(),
        "unknown binding lost reply queue owner"
    );
    unknown
        .retry_cleanup()
        .expect("unknown binding reply cleanup failed");
    assert!(
        !unknown.has_unretired_owners(),
        "unknown binding reply cleanup retained owner"
    );
}
fn unknown_operation(caller: &mut Caller, manager: &MailboxSender) {
    let until = deadline();
    let mut bytes = [0; libfs::protocol::HEADER_LEN];
    // 管理请求/回复共用 Header；status=0、未知 op 与空正文是合法未知操作夹具。
    let used = route::encode_response(99, Status::Ok, until, &mut bytes)
        .expect("unknown management header encoding failed");
    let reply = caller
        .call(
            manager,
            until,
            librpc::Request::new(route::ID, &bytes[..used])
                .expect("unknown management request allocation failed"),
        )
        .expect("unknown management operation call failed");
    assert_eq!(
        route::decode_response(&reply.payload, 99, until, (1u64 << reply.handles.len()) - 1)
            .expect("unknown management response invalid"),
        Status::Unsupported
    );
}
pub(super) fn check(
    namespace: &Namespace,
    caller: &mut Caller,
    management: &MailboxSender,
    management_b: &MailboxSender,
    secondary: &MailboxSender,
) {
    let output = srv_fs::deployment::root_output_transport();
    unknown_operation(caller, management);
    unknown_operation(caller, management_b);
    let stored = Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE;
    let mut fs = Client::new(namespace, caller, limits());
    for path in [
        "/bind-host",
        "/provider-b/bind-target",
        "/provider-b/bind-other",
    ] {
        fs.create(path, NodeKind::Directory, FsRights::ALL, &[], deadline())
            .expect("binding directory create failed");
    }
    for (path, value) in [
        ("/provider-b/bind-target/value", 7u64),
        ("/provider-b/bind-other/value", 43),
        ("/bind-host/local", 11),
    ] {
        let mut bytes = [0; 20];
        let used = Value::Unsigned64(value)
            .encode(&mut bytes)
            .expect("binding value encoding failed");
        fs.create(
            path,
            NodeKind::Property,
            FsRights::ALL,
            &bytes[..used],
            deadline(),
        )
        .expect("binding value create failed");
    }
    fs.create(
        "/bind-host/link",
        NodeKind::SymbolicLink,
        FsRights::ALL,
        b"local",
        deadline(),
    )
    .expect("binding link create failed");
    let issuer = fs
        .derive_directory(
            "/provider-b/bind-target",
            FsRights::ALL,
            stored,
            output,
            deadline(),
        )
        .expect("binding target issuer failed");
    let other = fs
        .derive_directory(
            "/provider-b/bind-other",
            FsRights::ALL,
            stored,
            output,
            deadline(),
        )
        .expect("binding replacement issuer failed");
    let local = fs
        .derive_directory("/bind-host", FsRights::ALL, stored, output, deadline())
        .expect("binding reverse issuer failed");
    assert_eq!(
        bind(
            caller,
            management,
            "bind-host/forbidden",
            &issuer,
            FsRights::ALL,
            Rights::EXECUTE,
            false
        ),
        Status::Permission
    );
    assert_eq!(
        unbind(caller, management, "bind-host/missing"),
        Status::NotFound
    );
    assert_eq!(
        unbind(caller, management, "bind-host/local"),
        Status::TypeMismatch
    );
    assert_eq!(
        bind(
            caller,
            management,
            "bind-host/local",
            &issuer,
            FsRights::ALL,
            output,
            true
        ),
        Status::TypeMismatch
    );
    assert_eq!(
        bind(
            caller,
            management,
            "bind-host/link/remote",
            &issuer,
            FsRights::ALL,
            output,
            false
        ),
        Status::TypeMismatch
    );
    assert_eq!(
        bind(
            caller,
            management,
            "bind-host/remote",
            &issuer,
            FsRights::ALL,
            output,
            false
        ),
        Status::Ok
    );
    assert_eq!(
        bind(
            caller,
            management_b,
            "bind-host/remote",
            &other,
            FsRights::ALL,
            output,
            false
        ),
        Status::Exists
    );
    assert_eq!(
        unbind(caller, management, "bind-host/remote/value"),
        Status::CrossFilesystem
    );
    assert_eq!(
        bind(
            caller,
            management,
            "bind-host/remote/nested",
            &other,
            FsRights::ALL,
            output,
            false
        ),
        Status::CrossFilesystem
    );
    assert_eq!(
        Client::new(namespace, caller, limits())
            .get::<u64>("/bind-host/remote/value", deadline())
            .expect("shared binding read failed"),
        7
    );
    let exported = Client::new(namespace, caller, limits())
        .derive_directory(
            "/bind-host/remote",
            FsRights::ALL,
            stored,
            output,
            deadline(),
        )
        .expect("binding exported grant failed");
    boundary_transport(caller, &local, "remote/value", stored);
    // 源授权比 Binding 更窄，必须通过原 issuer 真实派生，不能只衰减运输 cap。
    let rights = FsRights::TRAVERSE | FsRights::DESCRIBE | FsRights::READ_PROPERTY;
    let narrow_output = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
    let narrow = Client::new(namespace, caller, limits())
        .derive_directory("/bind-host", rights, stored, narrow_output, deadline())
        .expect("narrow binding source failed");
    assert_eq!(
        bind(
            caller,
            management,
            "bind-host/no-derive",
            &narrow,
            rights,
            narrow_output,
            false
        ),
        Status::Permission
    );
    assert_eq!(
        unbind(caller, management, "bind-host/no-derive"),
        Status::NotFound
    );
    let narrow = Arc::try_new(narrow).expect("narrow binding entry allocation failed");
    let mut restricted = Namespace::new();
    restricted
        .bind("/", narrow.clone())
        .map_err(|failed| failed.cause)
        .expect("narrow namespace binding failed");
    assert_eq!(
        Client::new(&restricted, caller, limits())
            .get::<u64>("/remote/value", deadline())
            .expect("attenuated binding read failed"),
        7
    );
    assert!(matches!(
        Client::new(&restricted, caller, limits()).set(
            "/remote/value",
            Value::Unsigned64(99),
            deadline()
        ),
        Err(ClientError::Status(Status::Permission))
    ));
    boundary_transport(caller, &narrow, "remote/value", narrow_output);
    let until = deadline();
    abandon_queued(&narrow, get_request("remote/value", until), until);
    drop(restricted);
    drop(narrow);
    let until = deadline();
    let request = route::Request::Bind {
        path: "bind-host/unknown",
        max_rights: FsRights::ALL,
        output_transport: output,
        replace: false,
    };
    abandon_queued(
        management,
        management_request(&request, Some(&issuer), until),
        until,
    );
    // 回复未领取，已提交事实仍存在；调用者不重放原 Bind，只用管理权显式收束。
    assert_eq!(
        Client::new(namespace, caller, limits())
            .get::<u64>("/bind-host/unknown/value", deadline())
            .expect("unknown binding was rolled back"),
        7
    );
    assert_eq!(unbind(caller, management, "bind-host/unknown"), Status::Ok);
    assert_eq!(
        bind(
            caller,
            management_b,
            "bind-host/remote",
            &other,
            FsRights::ALL,
            output,
            true
        ),
        Status::Ok
    );
    assert_eq!(
        Client::new(namespace, caller, limits())
            .get::<u64>("/bind-host/remote/value", deadline())
            .expect("replacement binding read failed"),
        43
    );
    assert_eq!(unbind(caller, management, "bind-host/remote"), Status::Ok);
    assert!(matches!(
        Client::new(namespace, caller, limits()).get::<u64>("/bind-host/remote/value", deadline()),
        Err(ClientError::Status(Status::NotFound))
    ));
    let mut detached = Namespace::new();
    detached
        .bind(
            "/",
            Arc::try_new(exported).expect("exported entry allocation failed"),
        )
        .map_err(|failed| failed.cause)
        .expect("exported namespace binding failed");
    assert_eq!(
        Client::new(&detached, caller, limits())
            .get::<u64>("/value", deadline())
            .expect("exported grant was recalled by Unbind"),
        7
    );
    drop(detached);
    assert_eq!(
        bind(
            caller,
            secondary,
            "bind-target/back",
            &local,
            FsRights::ALL,
            output,
            false
        ),
        Status::Ok
    );
    assert_eq!(
        Client::new(namespace, caller, limits())
            .get::<u64>("/provider-b/bind-target/back/local", deadline())
            .expect("secondary management binding read failed"),
        11
    );
    assert_eq!(
        bind(
            caller,
            management,
            "bind-host/remote",
            &issuer,
            FsRights::ALL,
            output,
            false
        ),
        Status::Ok
    );
    let cycle_limit =
        liboal::FollowLimits::new(3, 4 * erhino_shared::message::PAYLOAD_MAX, 5_000_000_000)
            .expect("binding cycle budget invalid");
    assert!(matches!(
        Client::new(namespace, caller, cycle_limit).get::<u64>(
            "/bind-host/remote/back/remote/back/remote/value",
            deadline()
        ),
        Err(ClientError::Local(liboal::LocalError::BudgetExceeded))
    ));
    assert_eq!(unbind(caller, secondary, "bind-target/back"), Status::Ok);
    assert_eq!(unbind(caller, management, "bind-host/remote"), Status::Ok);
    drop(local);
    drop(other);
    drop(issuer);
    let mut fs = Client::new(namespace, caller, limits());
    for path in [
        "/bind-host/link",
        "/bind-host/local",
        "/provider-b/bind-target/value",
        "/provider-b/bind-other/value",
    ] {
        fs.remove(path, deadline())
            .expect("binding member cleanup failed");
    }
    for path in [
        "/bind-host",
        "/provider-b/bind-target",
        "/provider-b/bind-other",
    ] {
        fs.remove(path, deadline())
            .expect("binding directory cleanup failed");
    }
    libkernel::debug!(
        "FS1 Bind Unbind shared attenuated replacement and management isolation passed"
    );
}
