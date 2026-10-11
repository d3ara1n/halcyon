//! 正式消费者的流合同：真实控制/Runnel、EOF、冻结末端、取消与无人继续调用的期限。
use super::{deadline, limits};
use libfs::{
    FsRights,
    client::{Client, ClientError, stream_control},
    protocol::{Direction, OpenOptions, Request, Status, StreamReason, StreamState},
};
use libkernel::{ipc::invitation::Invitation, mm::Placement};
use liboal::Namespace;
use librpc::Caller;
mod evidence;
pub(super) use evidence::{check_abandoned, check_accounts, check_edges, check_permissions};
fn options(direction: Direction) -> OpenOptions {
    OpenOptions {
        direction,
        offset: 0,
        length: None,
        session_deadline: deadline(),
        tunnel_bytes: 0,
    }
}
fn attach_producer(
    offer: libfs::client::Offer,
) -> (
    libkernel::ipc::message::MailboxSender,
    librunnel::blocking::Producer,
) {
    let invitation = Invitation::from_capability(offer.invitation)
        .unwrap_or_else(|_| panic!("write invitation role invalid"));
    let producer = librunnel::blocking::Producer::attach(
        invitation,
        offer.info.tunnel_bytes as usize,
        Placement::Anywhere,
    )
    .unwrap_or_else(|_| panic!("write Runnel attachment failed"));
    (offer.control, producer)
}
pub(super) fn check(namespace: &Namespace, caller: &mut Caller, prefix: &str) {
    let file = alloc::format!("{prefix}/consumer/session");
    let bytes: alloc::vec::Vec<u8> = (0..16384).map(|i| (i % 251) as u8).collect();
    let offer = Client::new(namespace, caller, limits())
        .create_open(&file, options(Direction::Write), FsRights::ALL, deadline())
        .expect("CreateOpen failed");
    assert_eq!(offer.info.start, 0);
    assert!(matches!(
        stream_control(caller, &offer.control, &Request::Start, deadline()),
        Err(ClientError::Status(Status::Busy))
    ));
    assert!(matches!(
        stream_control(caller, &offer.control, &Request::Finish, deadline()),
        Err(ClientError::Status(Status::Conflict))
    ));
    assert_eq!(
        stream_control(caller, &offer.control, &Request::QueryStream, deadline())
            .expect("offered query failed")
            .state,
        StreamState::Offered
    );
    let (control, mut producer) = attach_producer(offer);
    assert_eq!(
        stream_control(caller, &control, &Request::Start, deadline())
            .expect("write Start failed")
            .state,
        StreamState::Active
    );
    assert_eq!(
        stream_control(caller, &control, &Request::Start, deadline())
            .expect("repeated Start failed")
            .state,
        StreamState::Active
    );
    producer
        .write_all_until(&bytes, deadline())
        .expect("Runnel file write failed");
    producer.finish().expect("Runnel file EOF failed");
    let info = stream_control(caller, &control, &Request::Finish, deadline())
        .expect("write Finish failed");
    assert_eq!(info.state, StreamState::Terminal);
    assert_eq!(info.reason, StreamReason::Completed);
    assert_eq!(
        (info.accepted, info.transported),
        (bytes.len() as u64, bytes.len() as u64)
    );
    assert_eq!(
        stream_control(caller, &control, &Request::Finish, deadline())
            .expect("repeated Finish failed"),
        info
    );
    assert!(matches!(
        stream_control(caller, &control, &Request::Start, deadline()),
        Err(ClientError::Status(Status::Conflict))
    ));
    assert_eq!(
        stream_control(caller, &control, &Request::Cancel, deadline())
            .expect("terminal Cancel failed"),
        info
    );
    producer
        .close()
        .map_err(|(_, e)| e)
        .expect("write endpoint release failed");
    drop(control);
    libkernel::debug!("FS1 Open control and terminal cases passed after 16384 bytes");

    let offer = Client::new(namespace, caller, limits())
        .open(&file, options(Direction::Read), deadline())
        .expect("read Open failed");
    assert_eq!(offer.info.read_end, bytes.len() as u64);
    // Offered 不提前搬内容；后续修改可见，增长不扩大已冻结末端。
    let mut expected = bytes;
    expected[..6].copy_from_slice(b"change");
    {
        let mut fs = Client::new(namespace, caller, limits());
        fs.write_at(&file, 0, b"change", deadline())
            .expect("live content change failed");
        fs.write_at(
            &file,
            expected.len() as u64,
            b"outside frozen end",
            deadline(),
        )
        .expect("live content growth failed");
    }
    let invitation = Invitation::from_capability(offer.invitation)
        .unwrap_or_else(|_| panic!("read invitation role invalid"));
    let mut consumer = librunnel::blocking::Consumer::attach(
        invitation,
        offer.info.tunnel_bytes as usize,
        Placement::Anywhere,
    )
    .unwrap_or_else(|_| panic!("read Runnel attachment failed"));
    stream_control(caller, &offer.control, &Request::Start, deadline()).expect("read Start failed");
    let mut observed = alloc::vec![0; expected.len() + 32];
    let count = consumer
        .read_exact_or_eof_until(&mut observed, deadline())
        .expect("Runnel file read failed");
    assert_eq!(count, expected.len());
    assert_eq!(&observed[..count], expected.as_slice());
    let info = stream_control(caller, &offer.control, &Request::Finish, deadline())
        .expect("read Finish failed");
    assert_eq!(info.reason, StreamReason::Completed);
    assert_eq!(
        (info.accepted, info.transported),
        (count as u64, count as u64)
    );
    consumer
        .close()
        .map_err(|(_, e)| e)
        .expect("read endpoint release failed");
    drop(offer.control);
    libkernel::debug!("FS1 independent stream content and frozen read passed");

    let offer = Client::new(namespace, caller, limits())
        .open(&file, options(Direction::Write), deadline())
        .expect("cancel Open failed");
    let (control, producer) = attach_producer(offer);
    stream_control(caller, &control, &Request::Start, deadline())
        .expect("cancel stream Start failed");
    let until = deadline();
    let mut encoded = [0; libfs::protocol::HEADER_LEN];
    let used = libfs::protocol::encode_request(&Request::Finish, until, &mut encoded)
        .expect("Finish encoding failed");
    let request = librpc::Request::new(libfs::protocol::ID, &encoded[..used])
        .expect("Finish request allocation failed");
    let mut finish_caller = Caller::new();
    let mut operation =
        librpc::caller::CallOperation::new(&mut finish_caller, &control, until, request, None)
            .expect("Finish operation preparation failed");
    assert!(operation.advance().expect("Finish send failed").is_none());
    let current = stream_control(caller, &control, &Request::Cancel, deadline())
        .expect("stream Cancel failed");
    assert!(matches!(
        current.state,
        StreamState::Active | StreamState::Terminal
    ));
    operation
        .wait_ready()
        .expect("cancelled Finish reply did not become ready");
    let reply = operation
        .advance()
        .expect("cancelled Finish receive failed")
        .expect("ready Finish reply missing");
    let libfs::protocol::Response::Stream(info) =
        libfs::protocol::decode_response(&reply.payload, &Request::Finish, until, 0)
            .expect("cancelled Finish reply invalid")
    else {
        panic!("cancelled Finish response kind invalid");
    };
    assert_eq!(info.reason, StreamReason::Cancelled);
    assert_eq!((info.accepted, info.transported), (0, 0));
    drop(operation);
    producer
        .close()
        .map_err(|(_, e)| e)
        .expect("cancelled endpoint release failed");
    drop(control);

    let mut infinite = options(Direction::Write);
    infinite.session_deadline = erhino_shared::time::Deadline::INFINITE;
    let offer = Client::new(namespace, caller, limits())
        .open(&file, infinite, deadline())
        .expect("last-control Open failed");
    let (control, producer) = attach_producer(offer);
    stream_control(caller, &control, &Request::Start, deadline())
        .expect("last-control Start failed");
    // 原 RPC 公共 Prefix/Packet/SendOnce；发送后不借控制，保留真实回复队列。
    let reply_owner = libkernel::ipc::message::Mailbox::create(
        erhino_shared::object::Rights::READ
            | erhino_shared::object::Rights::WAIT
            | erhino_shared::object::Rights::MANAGE,
    )
    .expect("Finish reply mailbox creation failed");
    let reply_minted = reply_owner
        .mint(
            0,
            erhino_shared::object::Rights::WRITE
                | erhino_shared::object::Rights::WAIT
                | erhino_shared::object::Rights::DUPLICATE
                | erhino_shared::object::Rights::TRANSIT,
        )
        .expect("Finish reply sender creation failed");
    let reply_permit = libkernel::ipc::message::send_once(
        &reply_minted.sender,
        erhino_shared::object::Rights::WRITE
            | erhino_shared::object::Rights::WAIT
            | erhino_shared::object::Rights::TRANSIT,
    )
    .expect("Finish reply permit failed");
    let mut payload = [0; librpc::PREFIX_LEN + libfs::protocol::HEADER_LEN];
    librpc::RpcPrefix::new(librpc::RpcMessageKind::Request, 42).encode(&mut payload);
    libfs::protocol::encode_request(
        &Request::Finish,
        erhino_shared::time::Deadline::INFINITE,
        &mut payload[librpc::PREFIX_LEN..],
    )
    .expect("infinite Finish encoding failed");
    let mut packet = libkernel::ipc::packet::Packet::new(libfs::protocol::ID, &payload)
        .expect("infinite Finish packet failed");
    packet
        .push(
            reply_permit.reply.into_capability(),
            erhino_shared::object::Rights::WRITE
                | erhino_shared::object::Rights::WAIT
                | erhino_shared::object::Rights::TRANSIT,
        )
        .map_err(|failed| failed.error)
        .expect("infinite Finish reply slot failed");
    packet
        .try_send(&control, erhino_shared::time::Deadline::INFINITE)
        .map_err(|failed| failed.error)
        .expect("infinite Finish send failed");
    drop(control);
    producer
        .wait_closed_until(deadline())
        .expect("last control did not stop infinite stream independently");
    let reply = libkernel::ipc::message::wait_message_until(reply_owner.as_handle(), deadline())
        .expect("last-control Finish did not reply");
    let info = match libfs::protocol::decode_response(
        &reply.payload[librpc::PREFIX_LEN..],
        &Request::Finish,
        erhino_shared::time::Deadline::INFINITE,
        0,
    )
    .expect("last-control Finish invalid")
    {
        libfs::protocol::Response::Stream(info) => info,
        _ => panic!("last-control Finish response kind invalid"),
    };
    assert_eq!(info.reason, StreamReason::PeerClosed);
    assert_eq!((info.accepted, info.transported), (0, 0));
    drop(reply);
    drop(reply_permit.observation);
    drop(reply_minted);
    drop(reply_owner);
    producer
        .close()
        .map_err(|(_, e)| e)
        .expect("last-control endpoint release failed");

    let mut options = options(Direction::Read);
    options.session_deadline =
        libkernel::time::timeout_millis(1000).expect("offer expiry deadline failed");
    let offer = Client::new(namespace, caller, limits())
        .open(&file, options, deadline())
        .expect("unattached Open failed");
    // 消费者不推进控制/Runnel；provider 必须独立靠已安装期限终结。
    libkernel::time::sleep_until(offer.info.offer_deadline).expect("offer expiry wait failed");
    // 在第一次控制 Query 前，只通过 Kernel 数据端观察实际退休。
    let invitation = Invitation::from_capability(offer.invitation)
        .unwrap_or_else(|_| panic!("expiry invitation role invalid"));
    match invitation.attach() {
        Err(failed) => {
            assert_eq!(
                failed.error,
                erhino_shared::call::SystemCallError::ObjectClosed
            );
            failed
                .invitation
                .close()
                .map_err(|(_, e)| e)
                .expect("expired invitation release failed");
        }
        Ok(endpoint) => {
            let observed = endpoint
                .events()
                .wait_until(
                    erhino_shared::object::ObjectSignals::PEER_CLOSED,
                    deadline(),
                )
                .expect("expired endpoint did not retire independently");
            assert!(
                observed
                    .observed
                    .intersects(erhino_shared::object::ObjectSignals::PEER_CLOSED),
                "expired endpoint retirement timed out"
            );
            endpoint
                .close()
                .map_err(|(_, e)| e)
                .expect("expiry endpoint release failed");
        }
    }
    let info = stream_control(caller, &offer.control, &Request::QueryStream, deadline())
        .expect("expired terminal query failed");
    assert_eq!(info.state, StreamState::Terminal);
    assert_eq!(info.reason, StreamReason::Expired);
    drop(offer.control);
    libkernel::debug!("FS1 independent unattached Open expired and retired");
    let mut fs = Client::new(namespace, caller, limits());
    fs.set_length(&file, 8, deadline())
        .expect("SetLength shrink failed");
    let short = fs
        .read_at(&file, 0, 64, deadline())
        .expect("truncated read failed");
    assert_eq!(short.data.as_slice(), &expected[..8]);
    let until = deadline();
    loop {
        match fs.set_length(&file, 32, until) {
            Ok(()) => break,
            Err(ClientError::Status(Status::Busy)) => {
                assert!(
                    !libkernel::time::expired(until).expect("trim deadline check failed"),
                    "Trimming did not complete"
                );
            }
            Err(error) => panic!("SetLength growth failed: {:?}", error),
        }
    }
    let grown = fs
        .read_at(&file, 0, 64, deadline())
        .expect("sparse growth read failed");
    assert_eq!(grown.data.len(), 32);
    assert_eq!(&grown.data[..8], &expected[..8]);
    assert!(
        grown.data[8..].iter().all(|byte| *byte == 0),
        "growth revived discarded bytes"
    );
    libkernel::debug!("FS1 SetLength visible shrink, maintenance and sparse growth passed");
    Client::new(namespace, caller, limits())
        .remove(&file, deadline())
        .expect("session file removal failed");
}
