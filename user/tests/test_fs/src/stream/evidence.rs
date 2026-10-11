//! 流的真实准入/寿命证据；沿现有消费者与 A/B 发行入口，不建立运行体。
use super::{attach_producer, options};
use crate::{bind, deadline, limits};
use alloc::vec::Vec;
use erhino_shared::object::{ObjectSignals, Rights};
use libfs::{
    FsRights,
    client::{Client, ClientError, Offer, stream_control},
    node::NodeKind,
    protocol::{self, Direction, Request, Status},
};
use libkernel::ipc::{
    message::{Mailbox, MailboxSender, send_once},
    packet::Packet,
};
use liboal::Namespace;
use librpc::Caller;

fn release(offer: Offer) {
    drop(offer.invitation);
    drop(offer.control);
}
pub(crate) fn check_accounts(namespace: &Namespace, caller: &mut Caller) {
    use srv_fs::deployment::{ENTRY_SESSIONS, SESSIONS};
    let path = "/quota-session";
    Client::new(namespace, caller, limits())
        .create(path, NodeKind::Stream, FsRights::ALL, &[], deadline())
        .expect("quota stream creation failed");
    // Tunnel 最大合法几何仍受同一真实 Bytes 预算约束，不能当 Resource 成功出生。
    let mut huge = options(Direction::Read);
    huge.tunnel_bytes = (erhino_shared::tunnel::TUNNEL_MAX_PAGES
        * erhino_shared::proc::PROCESS_PAGE_SIZE as u64) as u32;
    assert!(
        matches!(
            Client::new(namespace, caller, limits()).open(path, huge, deadline()),
            Err(ClientError::Status(Status::Quota))
        ),
        "stream Bytes quota missing"
    );
    let mut a = Vec::new();
    let mut b = Vec::new();
    a.try_reserve_exact(ENTRY_SESSIONS[0])
        .expect("A stream fixture allocation failed");
    b.try_reserve_exact(SESSIONS)
        .expect("B stream fixture allocation failed");
    for _ in 0..ENTRY_SESSIONS[0] {
        a.push(
            Client::new(namespace, caller, limits())
                .open(path, options(Direction::Read), deadline())
                .expect("A stream admission failed"),
        );
    }
    assert!(
        matches!(
            Client::new(namespace, caller, limits()).open(
                path,
                options(Direction::Read),
                deadline()
            ),
            Err(ClientError::Status(Status::Quota))
        ),
        "A local stream quota missing"
    );
    for _ in ENTRY_SESSIONS[0]..SESSIONS {
        b.push(
            Client::new(namespace, caller, limits())
                .open(
                    "/tenant-b/quota-session",
                    options(Direction::Read),
                    deadline(),
                )
                .expect("B rejected while shared stream capacity remained"),
        );
    }
    assert!(
        b.len() < ENTRY_SESSIONS[1],
        "B fixture exhausted local quota"
    );
    assert!(
        matches!(
            Client::new(namespace, caller, limits()).open(
                "/tenant-b/quota-session",
                options(Direction::Read),
                deadline()
            ),
            Err(ClientError::Status(Status::Quota))
        ),
        "global stream quota missing"
    );
    let unknown = "/quota-not-created";
    assert!(
        matches!(
            Client::new(namespace, caller, limits()).create_open(
                unknown,
                options(Direction::Write),
                FsRights::ALL,
                deadline()
            ),
            Err(ClientError::Status(Status::Quota))
        ),
        "CreateOpen bypassed quota"
    );
    assert!(
        matches!(
            Client::new(namespace, caller, limits()).read_at(unknown, 0, 1, deadline()),
            Err(ClientError::Status(Status::NotFound))
        ),
        "rejected CreateOpen published name"
    );
    release(a.pop().expect("A stream owner missing"));
    let until = deadline();
    let recovered = loop {
        match Client::new(namespace, caller, limits()).open(
            "/tenant-b/quota-session",
            options(Direction::Read),
            until,
        ) {
            Ok(offer) => break offer,
            Err(ClientError::Status(Status::Quota)) => assert!(
                !libkernel::time::expired(until).expect("stream refund clock failed"),
                "A stream release did not restore shared quota"
            ),
            Err(error) => panic!("B stream recovery failed: {error:?}"),
        }
    };
    b.push(recovered);
    for offer in a.into_iter().chain(b) {
        release(offer);
    }
    // 每次重试是新的有限调用，只有再次实际准入才证明物理 owner 的退款。
    let until = deadline();
    let recovered = loop {
        match Client::new(namespace, caller, limits()).open(path, options(Direction::Read), until) {
            Ok(offer) => break offer,
            Err(ClientError::Status(Status::Quota)) => assert!(
                !libkernel::time::expired(until).expect("local refund clock failed"),
                "local stream quota did not refund"
            ),
            Err(error) => panic!("A stream recovery failed: {error:?}"),
        }
    };
    release(recovered);
    Client::new(namespace, caller, limits())
        .remove(path, deadline())
        .expect("quota stream cleanup failed");
    libkernel::debug!("FS1 stream A/B Bytes, local/global quota and owner refund passed");
    libkernel::debug!("FAL2 independent Open offer discarded");
    libkernel::debug!("FAL2 resource quota saturation and recovery passed");
}

pub(crate) fn check_edges(namespace: &Namespace, caller: &mut Caller, prefix: &str) {
    let path = alloc::format!("{prefix}/consumer/bounded");
    let mut fs = Client::new(namespace, caller, limits());
    fs.create(&path, NodeKind::Stream, FsRights::ALL, &[], deadline())
        .expect("bounded stream fixture creation failed");
    fs.write_at(&path, 0, b"prestreamtail", deadline())
        .expect("bounded stream fixture write failed");
    let mut read = options(Direction::Read);
    read.offset = 3;
    read.length = Some(6);
    let offer = fs
        .open(&path, read, deadline())
        .expect("bounded Open failed");
    assert_eq!((offer.info.start, offer.info.read_end), (3, 9));
    let invitation = libkernel::ipc::invitation::Invitation::from_capability(offer.invitation)
        .unwrap_or_else(|_| panic!("bounded invitation role invalid"));
    let mut consumer = librunnel::blocking::Consumer::attach(
        invitation,
        offer.info.tunnel_bytes as usize,
        libkernel::mm::Placement::Anywhere,
    )
    .unwrap_or_else(|_| panic!("bounded attachment failed"));
    stream_control(caller, &offer.control, &Request::Start, deadline())
        .expect("bounded Start failed");
    let mut bytes = [0; 7];
    assert_eq!(
        consumer
            .read_exact_or_eof_until(&mut bytes, deadline())
            .expect("bounded read failed"),
        6
    );
    assert_eq!(&bytes[..6], b"stream");
    assert_eq!(
        consumer
            .read_exact_or_eof_until(&mut bytes[..1], deadline())
            .expect("bounded EOF failed"),
        0
    );
    let terminal = stream_control(caller, &offer.control, &Request::Finish, deadline())
        .expect("bounded Finish failed");
    assert_eq!(terminal.reason, protocol::StreamReason::Completed);
    assert_eq!((terminal.accepted, terminal.transported), (6, 6));
    consumer
        .close()
        .map_err(|(_, e)| e)
        .expect("bounded endpoint close failed");
    drop(offer.control);
    libkernel::debug!(
        "FS1 bounded stream offset, frozen end and EOF passed: {}",
        prefix
    );

    let mut write = options(Direction::Write);
    write.offset = 9;
    let offer = Client::new(namespace, caller, limits())
        .open(&path, write, deadline())
        .expect("pre-Start Open failed");
    let (control, mut producer) = attach_producer(offer);
    producer.finish().expect("pre-Start EOF publication failed");
    let before = stream_control(caller, &control, &Request::QueryStream, deadline())
        .expect("pre-Start Query failed");
    assert_eq!(before.state, protocol::StreamState::Offered);
    assert_eq!((before.accepted, before.transported), (0, 0));
    let current = Client::new(namespace, caller, limits())
        .read_at(&path, 0, 32, deadline())
        .expect("pre-Start content check failed");
    assert_eq!(current.data.as_slice(), b"prestreamtail");
    stream_control(caller, &control, &Request::Start, deadline())
        .expect("pre-Start activation failed");
    let terminal = stream_control(caller, &control, &Request::Finish, deadline())
        .expect("pre-Start Finish failed");
    assert_eq!(terminal.reason, protocol::StreamReason::Completed);
    assert_eq!((terminal.accepted, terminal.transported), (0, 0));
    producer
        .close()
        .map_err(|(_, e)| e)
        .expect("pre-Start endpoint close failed");
    drop(control);
    Client::new(namespace, caller, limits())
        .remove(&path, deadline())
        .expect("bounded fixture cleanup failed");
    libkernel::debug!("FAL2 independent pre-Start EOF gate passed");
}

pub(crate) fn check_permissions(namespace: &Namespace, caller: &mut Caller, prefix: &str) {
    let base = alloc::format!("{prefix}/consumer");
    let path = alloc::format!("{base}/stream-rights");
    Client::new(namespace, caller, limits())
        .create(
            &path,
            NodeKind::Stream,
            FsRights::READ_STREAM | FsRights::DESCRIBE,
            &[],
            deadline(),
        )
        .expect("restricted stream creation failed");
    assert!(
        matches!(
            Client::new(namespace, caller, limits()).open(
                &path,
                options(Direction::Write),
                deadline()
            ),
            Err(ClientError::Status(Status::Permission))
        ),
        "node ceiling allowed stream write"
    );
    let mandatory = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
    let entry = Client::new(namespace, caller, limits())
        .derive_directory(
            &base,
            FsRights::TRAVERSE | FsRights::READ_STREAM,
            mandatory,
            mandatory | Rights::READ | Rights::SIGNAL | Rights::MANAGE,
            deadline(),
        )
        .expect("read-only stream issuance failed");
    let mut restricted = Namespace::new();
    let _entry = bind(&mut restricted, "/", entry);
    assert!(
        matches!(
            Client::new(&restricted, caller, limits()).open(
                "/stream-rights",
                options(Direction::Write),
                deadline()
            ),
            Err(ClientError::Status(Status::Permission))
        ),
        "derived grant amplified stream rights"
    );
    assert!(
        matches!(
            Client::new(&restricted, caller, limits()).create_open(
                "/new",
                options(Direction::Write),
                FsRights::ALL,
                deadline()
            ),
            Err(ClientError::Status(Status::Permission))
        ),
        "read grant allowed CreateOpen"
    );
    let offer = Client::new(&restricted, caller, limits())
        .open("/stream-rights", options(Direction::Read), deadline())
        .expect("read-only Open failed");
    assert_eq!(
        offer
            .control
            .description()
            .expect("restricted control query failed")
            .rights,
        mandatory
    );
    assert!(
        matches!(
            stream_control(caller, &_entry, &Request::QueryStream, deadline()),
            Err(ClientError::Status(Status::Permission))
        ),
        "ordinary grant impersonated stream control"
    );
    release(offer);
    Client::new(namespace, caller, limits())
        .remove(&path, deadline())
        .expect("rights cleanup failed");
    libkernel::debug!("FAL2 independent Delegate and restricted Open passed");
}

pub(crate) fn check_abandoned(
    namespace: &Namespace,
    caller: &mut Caller,
    prefix: &str,
    entry: &MailboxSender,
) {
    // 超过 A 的真实局部会话容量；泄漏一个控制壳不能靠空余容量掩盖。
    for attempt in 0..=srv_fs::deployment::ENTRY_SESSIONS[0] {
        abandon_one(namespace, caller, prefix, entry, attempt);
    }
    libkernel::debug!("FS1 CreateOpen committed name survives queued reply abandonment");
    libkernel::debug!("FAL2 independent Open reply authority retired after abandonment");
}
fn abandon_one(
    namespace: &Namespace,
    caller: &mut Caller,
    prefix: &str,
    entry: &MailboxSender,
    attempt: usize,
) {
    let local = alloc::format!("consumer/create-unknown-{attempt}");
    let path = alloc::format!("{prefix}/{local}");
    assert!(
        matches!(
            Client::new(namespace, caller, limits()).read_at(&path, 0, 1, deadline()),
            Err(ClientError::Status(Status::NotFound))
        ),
        "abandoned CreateOpen fixture already exists"
    );
    let until = deadline();
    let owner = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)
        .expect("abandoned reply receiver creation failed");
    let minted = owner
        .mint(
            0,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE,
        )
        .expect("abandoned reply sender creation failed");
    let permit = send_once(
        &minted.sender,
        Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
    )
    .expect("abandoned reply permit failed");
    let request = Request::CreateOpen {
        path: &local,
        options: options(Direction::Write),
        rights: FsRights::ALL,
    };
    let mut bytes = [0; erhino_shared::message::PAYLOAD_MAX];
    librpc::RpcPrefix::new(librpc::RpcMessageKind::Request, 43).encode(&mut bytes);
    let used = protocol::encode_request(&request, until, &mut bytes[librpc::PREFIX_LEN..])
        .expect("abandoned CreateOpen encoding failed");
    let mut packet = Packet::new(protocol::ID, &bytes[..librpc::PREFIX_LEN + used])
        .expect("abandoned CreateOpen packet failed");
    packet
        .push(
            permit.reply.into_capability(),
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        )
        .map_err(|failed| failed.error)
        .expect("abandoned reply slot failed");
    packet
        .try_send(entry, until)
        .map_err(|failed| failed.error)
        .expect("abandoned CreateOpen send failed");
    let observed =
        libkernel::ipc::wait::wait_until(owner.as_handle(), ObjectSignals::READABLE, until)
            .expect("abandoned CreateOpen response never queued");
    assert!(
        observed.observed.intersects(ObjectSignals::READABLE),
        "CreateOpen reply wait expired"
    );
    // READABLE 证明真实回复已入队；拒收整条消息，所有业务槽沿 Kernel 队列退休。
    owner
        .close()
        .map_err(|(_, e)| e)
        .expect("abandoned reply receiver close failed");
    drop(minted);
    drop(permit.observation);
    let value = Client::new(namespace, caller, limits())
        .read_at(&path, 0, 1, deadline())
        .expect("committed CreateOpen name disappeared after lost reply");
    assert!(value.data.is_empty());
    let until = deadline();
    let offer = loop {
        match Client::new(namespace, caller, limits()).open(&path, options(Direction::Write), until)
        {
            Ok(offer) => break offer,
            Err(ClientError::Status(Status::Quota)) => assert!(
                !libkernel::time::expired(until).expect("abandoned recovery clock failed"),
                "abandoned CreateOpen did not retire"
            ),
            Err(error) => panic!("abandoned stream recovery failed: {error:?}"),
        }
    };
    let (control, producer) = attach_producer(offer);
    stream_control(caller, &control, &Request::Start, deadline()).expect("reopened Start failed");
    producer
        .close()
        .map_err(|(_, e)| e)
        .expect("reopened endpoint close failed");
    drop(control);
    Client::new(namespace, caller, limits())
        .remove(&path, deadline())
        .expect("unknown target removal failed");
}
