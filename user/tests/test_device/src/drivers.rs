//! 实际Drivers只读入口；观察关联不承担Binding安全完成证明。
use super::*;
use alloc::format;
use libdriver::view::{self, BindingInfo, InstanceId, InstanceInfo};
use liboal::protocol::{self as oal, Request, Response, Status};

fn request(
    caller: &mut Caller,
    source: &MailboxSender,
    request: &Request<'_>,
) -> (Reply, Deadline) {
    let deadline = end();
    let mut wire = [0; erhino_shared::message::PAYLOAD_MAX];
    let used = oal::encode_request(request, deadline, &mut wire).expect("Drivers request invalid");
    let reply = caller
        .call(
            source,
            deadline,
            librpc::Request::new(oal::ID, &wire[..used]).expect("Drivers framing failed"),
        )
        .expect("Drivers RPC failed");
    assert_eq!(
        reply.handles.remaining(),
        0,
        "Drivers view carried capability"
    );
    (reply, deadline)
}
fn response<'a>(reply: &'a Reply, request: &Request<'_>, deadline: Deadline) -> Response<'a> {
    oal::decode_response(&reply.payload, request, deadline, 0).expect("Drivers response invalid")
}
fn path(id: InstanceId) -> alloc::string::String {
    let mut name = [0; view::NAME_BYTES];
    view::instance_name(id.serial(), &mut name).to_string()
}
fn member(
    caller: &mut Caller,
    source: &MailboxSender,
    id: InstanceId,
    reference: BindingRef,
) -> Option<BindingInfo> {
    let mut name = [0; view::NAME_BYTES];
    let path = format!(
        "{}/{}/{}",
        path(id),
        view::BINDINGS_NAME,
        view::binding_name(reference.device().serial(), &mut name)
    );
    let command = Request::Lookup { path: &path };
    let (reply, deadline) = request(caller, source, &command);
    match response(&reply, &command, deadline) {
        Response::Found(info) => {
            let info = BindingInfo::decode(info).expect("Drivers binding info invalid");
            assert_eq!(info.instance, id, "Drivers instance changed");
            assert_eq!(info.reference, reference, "Drivers binding epoch changed");
            Some(info)
        }
        Response::Error(Status::NotFound) => None,
        _ => panic!("Drivers binding lookup failed"),
    }
}
pub(super) fn authenticate(caller: &mut Caller, source: &MailboxSender) {
    let command = Request::Lookup { path: "" };
    let (reply, deadline) = request(caller, source, &command);
    let Response::Found(info) = response(&reply, &command, deadline) else {
        panic!("Drivers root missing");
    };
    let incarnation = view::decode_root(info).expect("Drivers root info invalid");
    assert_ne!(incarnation, 0);
    // 观察入口即使携带管理opcode也不能停止服务。
    let mut wire = [0; libdriver::protocol::HEADER_LEN];
    let used = libdriver::protocol::Header {
        op: 1,
        error: None,
        deadline,
    }
    .encode(&[], &mut wire)
    .expect("Drivers control probe invalid");
    let reply = caller
        .call(
            source,
            deadline,
            librpc::Request::new(libdriver::protocol::MANAGER_ID, &wire[..used])
                .expect("Drivers control framing failed"),
        )
        .expect("Drivers control probe failed");
    let (header, body) =
        libdriver::protocol::Header::decode(&reply.payload).expect("Drivers denial invalid");
    assert!(
        header.op == 1
            && header.error == Some(Error::Permission)
            && body.is_empty()
            && reply.handles.remaining() == 0,
        "Drivers Observer gained control"
    );
    let device = DeviceId::new(incarnation, 1).expect("Drivers policy probe identity invalid");
    let mut wire = [0; libdriver::protocol::HEADER_LEN + DeviceId::ENCODED_LEN];
    let deadline = end();
    let used = libdriver::protocol::encode_manager_retry(device, deadline, &mut wire)
        .expect("Drivers Retry probe invalid");
    let reply = caller
        .call(
            source,
            deadline,
            librpc::Request::new(libdriver::protocol::MANAGER_ID, &wire[..used])
                .expect("Drivers Retry framing failed"),
        )
        .expect("Drivers Retry probe failed");
    let (header, body) =
        libdriver::protocol::Header::decode(&reply.payload).expect("Drivers Retry denial invalid");
    assert!(
        header.op == libdriver::protocol::MANAGER_RETRY
            && header.error == Some(Error::Permission)
            && body.is_empty()
            && reply.handles.remaining() == 0,
        "Drivers Observer gained Retry policy"
    );
    libkernel::debug!("DEVICE Drivers Observer authenticated; control denied");
}
pub(super) fn locate(
    caller: &mut Caller,
    source: &MailboxSender,
    reference: BindingRef,
) -> InstanceId {
    let mut cursor = Vec::new();
    loop {
        let command = Request::Enumerate {
            path: "",
            cursor: &cursor,
            max_entries: 1,
        };
        let (reply, deadline) = request(caller, source, &command);
        let Response::Page(page) = response(&reply, &command, deadline) else {
            panic!("Drivers enumeration failed");
        };
        for entry in page.entries() {
            // Page已在解码时完整校验各项。
            let instance =
                InstanceInfo::decode(entry.info.expect("Drivers instance summary missing"))
                    .expect("Drivers instance invalid");
            let mut name = [0; view::NAME_BYTES];
            assert_eq!(
                entry.name,
                view::instance_name(instance.id.serial(), &mut name)
            );
            if member(caller, source, instance.id, reference).is_some() {
                libkernel::debug!(
                    "DEVICE Drivers binding observed: instance={}, pid={}, serial={}",
                    instance.id.serial(),
                    instance.pid.expect("Drivers live PID missing"),
                    reference.device().serial()
                );
                return instance.id;
            }
        }
        if page.done() {
            break;
        }
        cursor = page.next_cursor().to_vec();
    }
    panic!("Drivers live binding not projected");
}
pub(super) fn detached(
    caller: &mut Caller,
    source: &MailboxSender,
    id: InstanceId,
    reference: BindingRef,
) {
    let deadline = end();
    while member(caller, source, id, reference).is_some() {
        assert!(
            !libkernel::time::expired(deadline).expect("Drivers clock failed"),
            "Drivers detach deadline exceeded"
        );
        libkernel::time::sleep_until(
            libkernel::time::timeout_millis(1).expect("Drivers timer failed"),
        )
        .expect("Drivers wait failed");
    }
}

/// 回复积压在init持有的真实邮箱中，跨越客户端退出和管理者Control Stop。
pub(super) fn backlog(source: &MailboxSender, reply_port: &MailboxSender) {
    use erhino_shared::object::Rights;
    use libkernel::ipc::{message::send_once, packet::Packet};
    let deadline = end();
    let command = Request::Lookup { path: "" };
    let mut wire = [0; erhino_shared::message::PAYLOAD_MAX];
    for _ in 0..erhino_shared::message::MAILBOX_CAPACITY {
        let txid = librpc::next_txid().expect("Drivers backlog txid exhausted");
        librpc::RpcPrefix::new(librpc::RpcMessageKind::Request, txid).encode(&mut wire);
        let used = oal::encode_request(&command, deadline, &mut wire[librpc::PREFIX_LEN..])
            .expect("Drivers backlog request invalid");
        let permit = send_once(reply_port, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
            .expect("Drivers backlog Once failed");
        let mut packet = Packet::new(oal::ID, &wire[..librpc::PREFIX_LEN + used])
            .expect("Drivers backlog packet failed");
        packet
            .push(permit.reply.into_capability(), Rights::WRITE | Rights::WAIT)
            .map_err(|f| f.error)
            .expect("Drivers backlog reply grant failed");
        packet
            .try_send(source, deadline)
            .map_err(|f| f.error)
            .expect("Drivers backlog Send failed");
        let result = wait::wait_until(
            permit.observation.as_handle(),
            ObjectSignals::DONE | ObjectSignals::PEER_CLOSED,
            deadline,
        )
        .expect("Drivers backlog response wait failed");
        assert!(
            result.observed.intersects(ObjectSignals::DONE),
            "Drivers backlog reply abandoned"
        );
    }
    libkernel::debug!(
        "DEVICE Drivers Observer unread reply queue filled: messages={}",
        erhino_shared::message::MAILBOX_CAPACITY
    );
}
