//! 独立 FAL 验收消费者。启动与 provider 退出由 init 持有。
#![no_std]

extern crate alloc;

use alloc::{sync::Arc, vec::Vec};
use libfal::{
    authority::FalRights,
    client::{Client, ClientCallFailure, ClientProgress, SubscriptionEvent},
    node::NodeKind,
    protocol::{self, Request, Response},
    value::Value,
};
use libfs::{
    client::{Grant, Transport},
    prefix::{DirectoryGrant, PrefixTable},
    resolve,
};
use librpc::{CallCause, CallPhase, RpcMessageKind, RpcPrefix};
use librunnel::blocking;
use libservice::record::ServiceRecord;
use rinlib::{
    env,
    ipc::{
        capability::Capability,
        message::{self, Mailbox, MailboxSender},
        notification,
        packet::Packet,
        wait,
    },
    mm::Placement,
    preclude::*,
    shared::call::SystemCallError,
    shared::{
        message::MAILBOX_CAPACITY,
        object::{Handle, ObjectSignals, Rights},
        wait::{WaitItem, WaitReason},
    },
    time,
};
use test_fal::{command, report, startup};

fn startup_capability(index: usize) -> Result<Capability, &'static str> {
    let handle = env::startup_handle(index).ok_or("FAL consumer startup grant missing")?;
    // SAFETY: StartupBlock 将各唯一的非映射 grant 移交给当前进程。
    Ok(unsafe { Capability::from_raw(handle) })
}

fn send_unawaited_request(
    grant: &MailboxSender,
    reply_sender: &MailboxSender,
    request: &Request<'_>,
    txid: u64,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let body_len = protocol::HEADER_LEN
        .checked_add(
            request
                .encoded_len()
                .ok_or("FAL consumer unawaited request length invalid")?,
        )
        .ok_or("FAL consumer unawaited request length overflow")?;
    let total = librpc::PREFIX_LEN
        .checked_add(body_len)
        .ok_or("FAL consumer unawaited request frame overflow")?;
    let mut payload = Vec::new();
    payload
        .try_reserve_exact(total)
        .map_err(|_| "FAL consumer unawaited request buffer unavailable")?;
    payload.resize(total, 0);
    RpcPrefix::new(RpcMessageKind::Request, txid).encode(&mut payload);
    let used = protocol::encode_request(request, deadline, &mut payload[librpc::PREFIX_LEN..])
        .ok_or("FAL consumer unawaited request encoding failed")?;
    payload.truncate(librpc::PREFIX_LEN + used);
    let mut packet = Packet::new(protocol::ID, &payload)
        .map_err(|_| "FAL consumer unawaited request packet failed")?;
    let reply_rights = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
    let reply = message::send_once(reply_sender, reply_rights)
        .map_err(|_| "FAL consumer unawaited reply authority failed")?;
    packet
        .push_front(reply.into_capability(), reply_rights)
        .map_err(|_| "FAL consumer unawaited reply attachment failed")?;
    packet
        .try_send(grant, deadline)
        .map_err(|_| "FAL consumer unawaited request send failed")
}

fn send_abandoned_take(
    grant: &MailboxSender,
    reply_sender: &MailboxSender,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    send_unawaited_request(
        grant,
        reply_sender,
        &Request::Take {
            path: "probe-affine-handle",
        },
        0x5441_4b45_524f_4c4c,
        deadline,
    )
}

fn exercise_affine_take(
    client: &mut Client,
    grant: &MailboxSender,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let reply_owner = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)
        .map_err(|_| "FAL consumer abandoned Take mailbox creation failed")?;
    let reply = reply_owner
        .mint(
            0,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .map_err(|_| "FAL consumer abandoned Take sender mint failed")?;
    send_abandoned_take(grant, &reply.sender, deadline)?;
    reply_owner
        .close()
        .map_err(|_| "FAL consumer abandoned Take mailbox close failed")?;
    let mut taken = loop {
        match client.take(grant, "probe-affine-handle", deadline) {
            Ok(reply) => break reply,
            Err(libfal::client::ClientError::Status(protocol::Status::Busy)) => {}
            Err(_) => return Err("FAL consumer affine Take recovery failed"),
        }
    };
    if taken.handles.remaining() != 1 {
        return Err("FAL consumer affine Take recovery layout invalid");
    }
    let capability = taken
        .handles
        .take(0)
        .map_err(|_| "FAL consumer affine Take capability missing")?;
    let (sender, _) = MailboxSender::from_capability(capability)
        .map_err(|_| "FAL consumer affine Take role invalid")?;
    sender
        .close()
        .map_err(|_| "FAL consumer affine Take sender close failed")?;
    reply
        .sender
        .close()
        .map_err(|_| "FAL consumer abandoned Take reply sender close failed")?;
    reply
        .lifetime
        .close()
        .map_err(|_| "FAL consumer abandoned Take reply lifetime close failed")?;
    let empty = client
        .call(
            grant,
            &Request::Read {
                path: "probe-affine-handle",
            },
            deadline,
        )
        .map_err(|_| "FAL consumer emptied affine Handle read failed")?;
    let (_, Response::Value(empty_value)) = protocol::decode_response(&empty.payload)
        .map_err(|_| "FAL consumer emptied affine Handle reply invalid")?
    else {
        return Err("FAL consumer emptied affine Handle reply shape invalid");
    };
    let empty_blob = Value::Blob(b"");
    let mut encoded = alloc::vec![
        0;
        empty_blob
            .encoded_len()
            .ok_or("FAL consumer empty Handle value length invalid")?
    ];
    let used = empty_blob
        .encode(&mut encoded)
        .map_err(|_| "FAL consumer empty Handle value encoding failed")?;
    if !empty.handles.is_empty() || empty_value != &encoded[..used] {
        return Err("FAL consumer affine Handle was not emptied");
    }
    Ok(())
}

fn exercise_property_watch(
    client: &mut Client,
    grant: &MailboxSender,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let root_watch = client
        .subscribe(
            grant,
            "",
            protocol::WatchMask::CREATE | protocol::WatchMask::DELETE,
            deadline,
        )
        .map_err(|_| "FAL consumer root Watch subscription failed")?;
    let root_generation = root_watch.info().generation;
    let initial = Value::Blob(b"watch-initial");
    let mut initial_bytes = alloc::vec![
        0;
        initial
            .encoded_len()
            .ok_or("FAL consumer Watch initial length invalid")?
    ];
    let initial_len = initial
        .encode(&mut initial_bytes)
        .map_err(|_| "FAL consumer Watch initial encoding failed")?;
    let created = client
        .call(
            grant,
            &Request::Create {
                name: "consumer-watch-property",
                kind: NodeKind::Property,
                rights: FalRights::READ_PROPERTY | FalRights::WRITE_PROPERTY | FalRights::WATCH,
                value: &initial_bytes[..initial_len],
            },
            deadline,
        )
        .map_err(|_| "FAL consumer watched property Create failed")?;
    let (_, Response::Node(_)) = protocol::decode_response(&created.payload)
        .map_err(|_| "FAL consumer watched Create reply invalid")?
    else {
        return Err("FAL consumer watched Create reply shape invalid");
    };
    match root_watch
        .wait(deadline)
        .map_err(|_| "FAL consumer root Watch wait failed")?
    {
        SubscriptionEvent::Events(mask) if mask.contains(protocol::WatchMask::CREATE) => {}
        _ => return Err("FAL consumer root Watch omitted Create"),
    }
    let info = client
        .query_subscription(&root_watch, deadline)
        .map_err(|_| "FAL consumer root Watch query failed")?;
    if info.generation <= root_generation || info.reason != protocol::WatchReason::Active {
        return Err("FAL consumer root Watch generation did not advance");
    }
    client
        .unsubscribe(&root_watch, deadline)
        .map_err(|_| "FAL consumer root Watch cancellation failed")?;
    let property_watch = client
        .subscribe(
            grant,
            "consumer-watch-property",
            protocol::WatchMask::MODIFY,
            deadline,
        )
        .map_err(|_| "FAL consumer property Watch subscription failed")?;
    let property_generation = property_watch.info().generation;
    let updated = Value::Blob(b"watch-updated");
    let mut updated_bytes = alloc::vec![
        0;
        updated
            .encoded_len()
            .ok_or("FAL consumer Watch update length invalid")?
    ];
    let updated_len = updated
        .encode(&mut updated_bytes)
        .map_err(|_| "FAL consumer Watch update encoding failed")?;
    client
        .call(
            grant,
            &Request::Write {
                path: "consumer-watch-property",
                value: &updated_bytes[..updated_len],
            },
            deadline,
        )
        .map_err(|_| "FAL consumer watched property Write failed")?;
    match property_watch
        .wait(deadline)
        .map_err(|_| "FAL consumer property Watch wait failed")?
    {
        SubscriptionEvent::Events(mask) if mask.contains(protocol::WatchMask::MODIFY) => {}
        _ => return Err("FAL consumer property Watch omitted Modify"),
    }
    let info = client
        .query_subscription(&property_watch, deadline)
        .map_err(|_| "FAL consumer property Watch query failed")?;
    if info.generation <= property_generation || info.reason != protocol::WatchReason::Active {
        return Err("FAL consumer property Watch generation did not advance");
    }
    client
        .unsubscribe(&property_watch, deadline)
        .map_err(|_| "FAL consumer property Watch cancellation failed")?;
    drop(property_watch);
    let delete_watch = client
        .subscribe(
            grant,
            "consumer-watch-property",
            protocol::WatchMask::DELETE,
            deadline,
        )
        .map_err(|_| "FAL consumer delete Watch subscription failed")?;
    let lookup = client
        .call(
            grant,
            &Request::Lookup {
                path: "consumer-watch-property",
            },
            deadline,
        )
        .map_err(|_| "FAL consumer watched property Lookup failed")?;
    let (_, Response::Node(node)) = protocol::decode_response(&lookup.payload)
        .map_err(|_| "FAL consumer watched property Lookup reply invalid")?
    else {
        return Err("FAL consumer watched property Lookup shape invalid");
    };
    client
        .call(
            grant,
            &Request::Delete {
                name: "consumer-watch-property",
                expected: protocol::Expected {
                    identity: node.identity,
                    version: node.version,
                },
            },
            deadline,
        )
        .map_err(|_| "FAL consumer watched property Delete failed")?;
    match delete_watch
        .wait(deadline)
        .map_err(|_| "FAL consumer delete Watch wait failed")?
    {
        SubscriptionEvent::Events(mask)
            if mask.contains(protocol::WatchMask::DELETE)
                && mask.contains(protocol::WatchMask::TERMINATED) => {}
        _ => return Err("FAL consumer delete Watch omitted terminal event"),
    }
    let info = client
        .query_subscription(&delete_watch, deadline)
        .map_err(|_| "FAL consumer delete Watch query failed")?;
    if info.reason != protocol::WatchReason::NodeDeleted || info.generation <= node.version {
        return Err("FAL consumer delete Watch terminal state invalid");
    }
    client
        .unsubscribe(&delete_watch, deadline)
        .map_err(|_| "FAL consumer delete Watch cancellation failed")?;
    drop(delete_watch);
    if !matches!(
        client.call(
            grant,
            &Request::Lookup {
                path: "consumer-watch-property"
            },
            deadline,
        ),
        Err(libfal::client::ClientError::Status(
            protocol::Status::NotFound
        ))
    ) {
        return Err("FAL consumer deleted property remained visible");
    }
    if !matches!(
        root_watch.wait(time::Deadline::at(0)),
        Err(rinlib::shared::call::SystemCallError::DeadlineExpired)
    ) {
        return Err("FAL consumer cancelled root Watch received Delete");
    }
    drop(root_watch);
    Ok(())
}

fn exercise_resource_quota_recovery(
    client: &mut Client,
    transport: &mut Transport,
    stream: &resolve::Position<Grant>,
    grant: &MailboxSender,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    const STREAM_LIMIT: usize = 15;
    const WATCH_LIMIT: usize = 24;
    let mut streams = Vec::new();
    for _ in 0..STREAM_LIMIT {
        let stream = transport
            .open_stream(
                stream,
                protocol::StreamDirection::Read,
                0,
                Some(1),
                deadline,
            )
            .map_err(|_| "FAL consumer stream quota setup failed")?;
        streams.push(stream);
    }
    match transport.open_stream(
        stream,
        protocol::StreamDirection::Read,
        0,
        Some(1),
        deadline,
    ) {
        Err(libfs::client::StreamOpenFailure::Call(
            libfal::client::ClientError::Status(protocol::Status::Quota),
        )) => {}
        Ok(stream) => {
            stream
                .close()
                .map_err(|_| "FAL consumer over-quota stream close failed")?;
            return Err("FAL consumer stream quota was not enforced");
        }
        Err(_) => return Err("FAL consumer stream quota returned the wrong error"),
    }
    for stream in streams {
        stream
            .close()
            .map_err(|_| "FAL consumer stream quota recovery close failed")?;
    }
    let recovered = transport
        .open_stream(
            stream,
            protocol::StreamDirection::Read,
            0,
            Some(1),
            deadline,
        )
        .map_err(|_| "FAL consumer stream quota did not recover")?;
    recovered
        .close()
        .map_err(|_| "FAL consumer recovered stream close failed")?;

    let mut watches = Vec::new();
    loop {
        match client.subscribe(grant, "", protocol::WatchMask::CREATE, deadline) {
            Ok(watch) if watches.len() < WATCH_LIMIT => watches.push(watch),
            Ok(watch) => {
                client
                    .unsubscribe(&watch, deadline)
                    .map_err(|_| "FAL consumer over-quota Watch cleanup failed")?;
                return Err("FAL consumer Watch quota was not enforced");
            }
            Err(libfal::client::ClientError::Status(protocol::Status::Quota)) => break,
            Err(_) => return Err("FAL consumer Watch quota returned the wrong error"),
        }
    }
    if watches.is_empty() {
        return Err("FAL consumer Watch quota setup failed");
    }
    debug!("FAL2 Watch quota saturated at {} subscriptions", watches.len());
    for watch in watches {
        client
            .unsubscribe(&watch, deadline)
            .map_err(|_| "FAL consumer Watch quota recovery cleanup failed")?;
    }
    let recovered = loop {
        match client.subscribe(grant, "", protocol::WatchMask::CREATE, deadline) {
            Ok(watch) => break watch,
            Err(libfal::client::ClientError::Status(protocol::Status::Quota)) => {
                if time::expired(deadline)
                    .map_err(|_| "FAL consumer Watch quota recovery clock failed")?
                {
                    return Err("FAL consumer Watch quota did not recover");
                }
                time::sleep_until(
                    time::timeout_millis(1)
                        .map_err(|_| "FAL consumer Watch quota recovery deadline failed")?,
                )
                .map_err(|_| "FAL consumer Watch quota recovery sleep failed")?;
            }
            Err(_) => return Err("FAL consumer Watch quota recovery returned the wrong error"),
        }
    };
    client
        .unsubscribe(&recovered, deadline)
        .map_err(|_| "FAL consumer recovered Watch cleanup failed")?;
    Ok(())
}

fn exercise_directory_move_and_enumerate(
    client: &mut Client,
    grant: &MailboxSender,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let enumeration = client
        .call(
            grant,
            &Request::Enumerate {
                path: "",
                cursor: 0,
                limit: 1024,
            },
            deadline,
        )
        .map_err(|_| "FAL consumer enumeration failed")?;
    let (_, Response::Entries(entries)) = protocol::decode_response(&enumeration.payload)
        .map_err(|_| "FAL consumer enumeration reply invalid")?
    else {
        return Err("FAL consumer enumeration reply shape invalid");
    };
    let mut saw_directory = false;
    let mut saw_link = false;
    for entry in entries.iter() {
        let entry = entry.map_err(|_| "FAL consumer enumeration entry invalid")?;
        saw_directory |= entry.name == "f2-dir" && entry.info.kind == NodeKind::Directory;
        saw_link |= entry.name == "f2-link" && entry.info.kind == NodeKind::SymbolicLink;
    }
    if !saw_directory || !saw_link {
        return Err("FAL consumer enumeration omitted created entries");
    }
    let move_source = client
        .call(
            grant,
            &Request::Lookup {
                path: "move-source",
            },
            deadline,
        )
        .map_err(|_| "FAL consumer move source lookup failed")?;
    let (_, Response::Node(move_source)) = protocol::decode_response(&move_source.payload)
        .map_err(|_| "FAL consumer move source reply invalid")?
    else {
        return Err("FAL consumer move source reply shape invalid");
    };
    let move_target = client
        .derive(
            grant,
            "move-target",
            FalRights::TRAVERSE | FalRights::CREATE | FalRights::ENUMERATE,
            deadline,
        )
        .map_err(|_| "FAL consumer move target derive failed")?;
    client
        .move_entry(
            grant,
            &move_target,
            libfal::client::MoveEntry {
                source_parent: "",
                source_name: "move-source",
                destination_name: "moved",
                expected: protocol::Expected {
                    identity: move_source.identity,
                    version: move_source.version,
                },
            },
            deadline,
        )
        .map_err(|_| "FAL consumer same-provider Move failed")?;
    if !matches!(
        client.call(
            grant,
            &Request::Lookup {
                path: "move-source",
            },
            deadline,
        ),
        Err(libfal::client::ClientError::Status(
            protocol::Status::NotFound
        ))
    ) {
        return Err("FAL consumer moved source remained visible");
    }
    let moved = client
        .call(&move_target, &Request::Lookup { path: "moved" }, deadline)
        .map_err(|_| "FAL consumer moved target lookup failed")?;
    if !matches!(
        protocol::decode_response(&moved.payload)
            .map_err(|_| "FAL consumer moved target reply invalid")?
            .1,
        Response::Node(_)
    ) {
        return Err("FAL consumer moved target reply shape invalid");
    }
    Ok(())
}

fn exercise_existing_values(
    client: &mut Client,
    grant: &MailboxSender,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let initial = client
        .call(
            grant,
            &Request::Read {
                path: "probe-property",
            },
            deadline,
        )
        .map_err(|_| "FAL consumer property Read failed")?;
    let (_, Response::Value(initial)) = protocol::decode_response(&initial.payload)
        .map_err(|_| "FAL consumer property Read reply invalid")?
    else {
        return Err("FAL consumer property Read reply shape invalid");
    };
    let expected_initial = Value::Blob(b"initial");
    let mut initial_bytes = alloc::vec![
        0;
        expected_initial
            .encoded_len()
            .ok_or("FAL consumer initial property length invalid")?
    ];
    let initial_len = expected_initial
        .encode(&mut initial_bytes)
        .map_err(|_| "FAL consumer initial property encoding failed")?;
    if initial != &initial_bytes[..initial_len] {
        return Err("FAL consumer initial property value mismatch");
    }
    let updated = Value::Blob(b"updated");
    let mut updated_bytes =
        alloc::vec![0; updated.encoded_len().ok_or("FAL consumer update length invalid")?];
    let updated_len = updated
        .encode(&mut updated_bytes)
        .map_err(|_| "FAL consumer update value encoding failed")?;
    client
        .call(
            grant,
            &Request::Write {
                path: "probe-property",
                value: &updated_bytes[..updated_len],
            },
            deadline,
        )
        .map_err(|_| "FAL consumer property Write failed")?;
    client
        .copy_property(
            grant,
            "probe-property",
            grant,
            "probe-property-copy",
            FalRights::READ_PROPERTY,
            deadline,
        )
        .map_err(|_| "FAL consumer same-provider property Copy failed")?;
    let copied = client
        .call(
            grant,
            &Request::Read {
                path: "probe-property-copy",
            },
            deadline,
        )
        .map_err(|_| "FAL consumer copied property Read failed")?;
    let (_, Response::Value(copied)) = protocol::decode_response(&copied.payload)
        .map_err(|_| "FAL consumer copied property reply invalid")?
    else {
        return Err("FAL consumer copied property reply shape invalid");
    };
    let expected = Value::Blob(b"updated");
    let mut encoded =
        alloc::vec![0; expected.encoded_len().ok_or("FAL consumer copy length invalid")?];
    let used = expected
        .encode(&mut encoded)
        .map_err(|_| "FAL consumer copy value encoding failed")?;
    if copied != &encoded[..used] {
        return Err("FAL consumer copied property value mismatch");
    }
    let read = client
        .call(
            grant,
            &Request::ReadAt {
                path: "probe-stream",
                offset: 3,
                count: 6,
            },
            deadline,
        )
        .map_err(|_| "FAL consumer stream ReadAt failed")?;
    let (_, Response::Value(value)) = protocol::decode_response(&read.payload)
        .map_err(|_| "FAL consumer stream ReadAt reply invalid")?
    else {
        return Err("FAL consumer stream ReadAt reply shape invalid");
    };
    if value != b"stream" {
        return Err("FAL consumer stream ReadAt value mismatch");
    }
    Ok(())
}

fn exercise_unattached_offer(
    grant: &MailboxSender,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let mut client = Client::new();
    let session = time::timeout_millis(15_000)
        .map_err(|_| "FAL consumer unattached Open session deadline failed")?;
    let mut reply = client
        .call(
            grant,
            &Request::Open {
                path: "probe-stream",
                expected_identity: None,
                direction: protocol::StreamDirection::Read,
                offset: 3,
                length: Some(6),
                session_deadline: session,
                stream_protocol: protocol::RNL2_PROTOCOL,
                tunnel_bytes: 0,
            },
            deadline,
        )
        .map_err(|_| "FAL consumer unattached Open failed")?;
    let (_, Response::StreamOffer(offer)) = protocol::decode_response(&reply.payload)
        .map_err(|_| "FAL consumer unattached Offer invalid")?
    else {
        return Err("FAL consumer unattached Offer shape invalid");
    };
    let control_owner = reply
        .handles
        .take(0)
        .map_err(|_| "FAL consumer unattached control missing")?;
    let invitation = reply
        .handles
        .take(1)
        .map_err(|_| "FAL consumer unattached Invitation missing")?;
    let (control, description) = MailboxSender::from_capability(control_owner)
        .map_err(|_| "FAL consumer unattached control role invalid")?;
    if description.object_id != offer.identity {
        return Err("FAL consumer unattached control identity mismatch");
    }
    time::sleep_until(offer.offer_deadline)
        .map_err(|_| "FAL consumer unattached Offer deadline sleep failed")?;
    loop {
        let reply = client
            .call(&control, &Request::QueryStream, deadline)
            .map_err(|_| "FAL consumer unattached Query failed")?;
        let (_, Response::StreamInfo(info)) = protocol::decode_response(&reply.payload)
            .map_err(|_| "FAL consumer unattached Query shape invalid")?
        else {
            return Err("FAL consumer unattached Query result invalid");
        };
        if info.state == protocol::StreamState::Terminal {
            if info.outcome != protocol::Status::Cancelled
                || info.reason != protocol::StreamReason::Expired
                || info.accepted != 0
                || info.transported != 0
            {
                return Err("FAL consumer unattached expiry result mismatch");
            }
            break;
        }
        if time::expired(deadline).map_err(|_| "FAL consumer unattached expiry clock failed")? {
            return Err("FAL consumer unattached Offer did not expire");
        }
        time::sleep_until(
            time::timeout_millis(10)
                .map_err(|_| "FAL consumer unattached retry deadline failed")?,
        )
        .map_err(|_| "FAL consumer unattached retry sleep failed")?;
    }
    control
        .close()
        .map_err(|_| "FAL consumer unattached control close failed")?;
    invitation
        .close()
        .map_err(|_| "FAL consumer unattached Invitation close failed")?;
    Ok(())
}

fn exercise_discarded_offer(
    grant: &MailboxSender,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let session = time::timeout_millis(10_000)
        .map_err(|_| "FAL consumer discarded Offer session deadline failed")?;
    let reply = Client::new()
        .call(
            grant,
            &Request::Open {
                path: "probe-stream",
                expected_identity: None,
                direction: protocol::StreamDirection::Read,
                offset: 3,
                length: Some(6),
                session_deadline: session,
                stream_protocol: protocol::RNL2_PROTOCOL,
                tunnel_bytes: 0,
            },
            deadline,
        )
        .map_err(|_| "FAL consumer discarded Open failed")?;
    if !matches!(
        protocol::decode_response(&reply.payload),
        Ok((_, Response::StreamOffer(_)))
    ) {
        return Err("FAL consumer discarded Offer invalid");
    }
    drop(reply);
    Ok(())
}

fn exercise_prestart_eof(
    grant: &MailboxSender,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let mut client = Client::new();
    let session = time::timeout_millis(10_000)
        .map_err(|_| "FAL consumer pre-Start session deadline failed")?;
    let mut reply = client
        .call(
            grant,
            &Request::Open {
                path: "probe-stream",
                expected_identity: None,
                direction: protocol::StreamDirection::Write,
                offset: 9,
                length: Some(0),
                session_deadline: session,
                stream_protocol: protocol::RNL2_PROTOCOL,
                tunnel_bytes: 0,
            },
            deadline,
        )
        .map_err(|_| "FAL consumer pre-Start Open failed")?;
    if !matches!(
        protocol::decode_response(&reply.payload),
        Ok((_, Response::StreamOffer(_)))
    ) {
        return Err("FAL consumer pre-Start Offer invalid");
    }
    let control_owner = reply
        .handles
        .take(0)
        .map_err(|_| "FAL consumer pre-Start control missing")?;
    let invitation_owner = reply
        .handles
        .take(1)
        .map_err(|_| "FAL consumer pre-Start Invitation missing")?;
    let (control, _) = MailboxSender::from_capability(control_owner)
        .map_err(|_| "FAL consumer pre-Start control role invalid")?;
    let invitation = rinlib::ipc::invitation::Invitation::from_capability(invitation_owner)
        .map_err(|_| "FAL consumer pre-Start Invitation role invalid")?;
    let mut producer = blocking::Producer::attach(invitation, Placement::Anywhere)
        .map_err(|_| "FAL consumer pre-Start Attach failed")?;
    producer
        .finish()
        .map_err(|_| "FAL consumer pre-Start EOF failed")?;
    let reply = client
        .call(&control, &Request::QueryStream, deadline)
        .map_err(|_| "FAL consumer pre-Start Query failed")?;
    if !matches!(
        protocol::decode_response(&reply.payload),
        Ok((
            _,
            Response::StreamInfo(protocol::StreamInfo {
                state: protocol::StreamState::Offered,
                ..
            })
        ))
    ) {
        return Err("FAL consumer pre-Start EOF completed without Start");
    }
    loop {
        match client.call(&control, &Request::Start, deadline) {
            Ok(reply) => {
                if !matches!(
                    protocol::decode_response(&reply.payload),
                    Ok((
                        _,
                        Response::StreamInfo(protocol::StreamInfo {
                            state: protocol::StreamState::Active,
                            ..
                        })
                    ))
                ) {
                    return Err("FAL consumer pre-Start Start did not activate");
                }
                break;
            }
            Err(libfal::client::ClientError::Status(protocol::Status::Busy)) => {
                if time::expired(deadline).map_err(|_| "FAL consumer pre-Start clock failed")? {
                    return Err("FAL consumer pre-Start attachment was not observed");
                }
                time::sleep_until(
                    time::timeout_millis(10)
                        .map_err(|_| "FAL consumer pre-Start retry deadline failed")?,
                )
                .map_err(|_| "FAL consumer pre-Start retry sleep failed")?;
            }
            Err(_) => return Err("FAL consumer pre-Start Start failed"),
        }
    }
    let reply = client
        .call(&control, &Request::FinishStream, deadline)
        .map_err(|_| "FAL consumer pre-Start Finish failed")?;
    if !matches!(
        protocol::decode_response(&reply.payload),
        Ok((
            _,
            Response::StreamInfo(protocol::StreamInfo {
                state: protocol::StreamState::Terminal,
                outcome: protocol::Status::Ok,
                reason: protocol::StreamReason::Completed,
                accepted: 0,
                transported: 0,
                ..
            })
        ))
    ) {
        return Err("FAL consumer pre-Start EOF completion invalid");
    }
    producer
        .close()
        .map_err(|_| "FAL consumer pre-Start data close failed")?;
    control
        .close()
        .map_err(|_| "FAL consumer pre-Start control close failed")?;
    Ok(())
}

fn exercise_bounded_stream_read(
    transport: &mut Transport,
    position: &resolve::Position<Grant>,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let mut read = transport
        .open_stream(
            position,
            protocol::StreamDirection::Read,
            3,
            Some(6),
            deadline,
        )
        .map_err(|_| "FAL consumer bounded Open read failed")?;
    let mut bytes = [0; 6];
    if transport
        .read_stream_until(&mut read, &mut bytes, deadline)
        .map_err(|_| "FAL consumer bounded Read failed")?
        != 6
        || &bytes != b"stream"
    {
        return Err("FAL consumer bounded Read content mismatch");
    }
    let mut tail = [0; 1];
    if transport
        .read_stream_until(&mut read, &mut tail, deadline)
        .map_err(|_| "FAL consumer bounded Read EOF failed")?
        != 0
    {
        return Err("FAL consumer bounded Read exceeded frozen end");
    }
    let info = transport
        .finish_stream(&read, deadline)
        .map_err(|_| "FAL consumer bounded Read Finish failed")?;
    if info.outcome != protocol::Status::Ok
        || info.reason != protocol::StreamReason::Completed
        || info.accepted != 6
        || info.transported != 6
    {
        return Err("FAL consumer bounded Read completion mismatch");
    }
    read.close()
        .map_err(|_| "FAL consumer bounded Read close failed")?;
    Ok(())
}

fn create_open_probe(
    client: &mut Client,
    parent: &MailboxSender,
    name: &str,
    deadline: time::Deadline,
) -> Result<protocol::NodeInfo, &'static str> {
    let reply = client
        .call(
            parent,
            &Request::Create {
                name,
                kind: NodeKind::Stream,
                rights: FalRights::READ_STREAM | FalRights::WRITE_STREAM,
                value: &[],
            },
            deadline,
        )
        .map_err(|_| "FAL consumer conditional Open Create failed")?;
    let (_, Response::Node(info)) = protocol::decode_response(&reply.payload)
        .map_err(|_| "FAL consumer conditional Open Create reply invalid")?
    else {
        return Err("FAL consumer conditional Open Create shape invalid");
    };
    Ok(info)
}

fn exercise_create_uncertainty(
    parent: &MailboxSender,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let mut client = Client::new();
    let too_long = [b'x'; 4096];
    let name = core::str::from_utf8(&too_long).map_err(|_| "FAL consumer name invalid")?;
    if !matches!(
        client.call_classified(
            parent,
            &Request::Create {
                name,
                kind: NodeKind::Stream,
                rights: FalRights::READ_STREAM | FalRights::WRITE_STREAM,
                value: &[],
            },
            deadline,
            None,
        ),
        Err(ClientCallFailure::NoRequest(_))
    ) {
        return Err("FAL consumer local Create encoding was classified as unknown");
    }
    for explicit_abort in [
        true,
        false,
    ] {
        let request = Request::Create {
            name: "probe-stream",
            kind: NodeKind::Stream,
            rights: FalRights::READ_STREAM | FalRights::WRITE_STREAM,
            value: &[],
        };
        let mut operation = client
            .begin_call(parent, &request, deadline, None)
            .map_err(|_| "FAL consumer Create operation preparation failed")?;
        loop {
            if operation.phase() == Some(CallPhase::Sent) {
                break;
            }
            if !matches!(
                operation.advance(),
                Ok(ClientProgress::Pending)
            ) {
                return Err("FAL consumer Create did not enter sent phase");
            }
            if operation.phase() == Some(CallPhase::Unsent) {
                operation
                    .wait_ready()
                    .map_err(|_| "FAL consumer Create send wait failed")?;
            }
        }
        if explicit_abort {
            let mut failure = operation
                .abort(CallCause::Cancelled)
                .ok_or("FAL consumer Create abort lost pending operation")?;
            if failure.phase != CallPhase::Sent || !failure.has_unretired_owners() {
                return Err("FAL consumer Create abort lost sent reply owner");
            }
            failure
                .retry_cleanup()
                .map_err(|_| "FAL consumer Create reply port cleanup failed")?;
        }
        drop(operation);
        // 已存在节点只会返回 Conflict；后续 Lookup 必须使用新的回复端口。
        let lookup = client
            .call(parent, &Request::Lookup { path: "probe-stream" }, deadline)
            .map_err(|_| "FAL consumer late Create reply polluted the next call")?;
        let (_, Response::Node(_)) = protocol::decode_response(&lookup.payload)
            .map_err(|_| "FAL consumer uncertain Create lookup invalid")?
        else {
            return Err("FAL consumer sent Create did not commit");
        };
        let mut completed = client
            .begin_call(
                parent,
                &Request::Lookup {
                    path: "probe-stream",
                },
                deadline,
                None,
            )
            .map_err(|_| "FAL consumer completed operation setup failed")?;
        loop {
            match completed
                .advance()
                .map_err(|_| "FAL consumer completed operation advance failed")?
            {
                ClientProgress::Pending => completed
                    .wait_ready()
                    .map_err(|_| "FAL consumer completed operation wait failed")?,
                ClientProgress::Reply(_) => break,
                ClientProgress::Rejected(_) => {
                    return Err("FAL consumer completed operation rejected");
                }
            }
        }
        if completed.phase().is_some() || completed.advance().is_ok() {
            return Err("FAL consumer completed operation remained pending");
        }
        drop(completed);
    }
    Ok(())
}

fn exercise_stream_contents(
    transport: &mut Transport,
    position: &resolve::Position<Grant>,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(16_384)
        .map_err(|_| "FAL consumer stream buffer unavailable")?;
    for index in 0..16_384 {
        bytes.push((index % 251) as u8);
    }
    let mut client = Client::new();
    let watch = client
        .subscribe(
            position.anchor.endpoint(),
            &position.rel,
            protocol::WatchMask::MODIFY,
            deadline,
        )
        .map_err(|_| "FAL consumer stream Watch failed")?;
    let mut write = transport
        .open_stream(
            position,
            protocol::StreamDirection::Write,
            9,
            Some(bytes.len() as u64),
            deadline,
        )
        .map_err(|_| "FAL consumer stream Write Open failed")?;
    write
        .write_until(&bytes, deadline)
        .map_err(|_| "FAL consumer stream Write failed")?;
    write.end_write().map_err(|_| "FAL consumer stream EOF failed")?;
    let info = transport
        .finish_stream(&write, deadline)
        .map_err(|_| "FAL consumer stream Write Finish failed")?;
    if info.outcome != protocol::Status::Ok
        || info.reason != protocol::StreamReason::Completed
        || info.accepted != bytes.len() as u64
        || info.transported != bytes.len() as u64
    {
        return Err("FAL consumer stream Write result invalid");
    }
    if !matches!(
        watch.wait(deadline).map_err(|_| "FAL consumer stream Watch wait failed")?,
        SubscriptionEvent::Events(events) if events.contains(protocol::WatchMask::MODIFY)
    ) {
        return Err("FAL consumer stream Write did not publish MODIFY");
    }
    client
        .unsubscribe(&watch, deadline)
        .map_err(|_| "FAL consumer stream Watch release failed")?;
    write.close().map_err(|_| "FAL consumer stream Write close failed")?;

    let mut verify = transport
        .open_stream(position, protocol::StreamDirection::Read, 9, None, deadline)
        .map_err(|_| "FAL consumer stream verify Open failed")?;
    let patch = b"live";
    for (offset, value) in [
        (9 + bytes.len() as u64 - patch.len() as u64, patch.as_slice()),
        (9 + bytes.len() as u64 + 256, b"grow".as_slice()),
    ] {
        let reply = client
            .call(
                position.anchor.endpoint(),
                &Request::WriteAt {
                    path: &position.rel,
                    offset,
                    value,
                },
                deadline,
            )
            .map_err(|_| "FAL consumer concurrent stream mutation failed")?;
        if !matches!(
            protocol::decode_response(&reply.payload),
            Ok((_, Response::Written(4)))
        ) {
            return Err("FAL consumer concurrent stream mutation count invalid");
        }
    }
    for (chunk_index, chunk) in bytes.chunks(1024).enumerate() {
        let mut observed = [0; 1024];
        let count = transport
            .read_stream_until(&mut verify, &mut observed[..chunk.len()], deadline)
            .map_err(|_| "FAL consumer stream verify Read failed")?;
        if count != chunk.len()
            || observed[..count].iter().enumerate().any(|(index, value)| {
                let position = chunk_index * 1024 + index;
                let expected = if position >= bytes.len() - patch.len() {
                    patch[position - (bytes.len() - patch.len())]
                } else {
                    bytes[position]
                };
                *value != expected
            })
        {
            return Err("FAL consumer stream verify content mismatch");
        }
    }
    let mut tail = [0; 1];
    if transport
        .read_stream_until(&mut verify, &mut tail, deadline)
        .map_err(|_| "FAL consumer frozen stream EOF failed")?
        != 0
    {
        return Err("FAL consumer frozen stream grew beyond Open end");
    }
    let info = transport
        .finish_stream(&verify, deadline)
        .map_err(|_| "FAL consumer stream verify Finish failed")?;
    if info.outcome != protocol::Status::Ok
        || info.accepted != bytes.len() as u64
        || info.transported != bytes.len() as u64
    {
        return Err("FAL consumer stream verify result invalid");
    }
    verify.close().map_err(|_| "FAL consumer stream verify close failed")?;
    client
        .call(
            position.anchor.endpoint(),
            &Request::WriteAt {
                path: &position.rel,
                offset: 9 + bytes.len() as u64 - patch.len() as u64,
                value: &bytes[bytes.len() - patch.len()..],
            },
            deadline,
        )
        .map_err(|_| "FAL consumer stream patch restore failed")?;
    let mut empty = transport
        .open_stream(position, protocol::StreamDirection::Read, 9, Some(0), deadline)
        .map_err(|_| "FAL consumer empty stream Open failed")?;
    if transport
        .read_stream_until(&mut empty, &mut tail, deadline)
        .map_err(|_| "FAL consumer empty stream EOF failed")?
        != 0
    {
        return Err("FAL consumer empty stream returned bytes");
    }
    let info = transport
        .finish_stream(&empty, deadline)
        .map_err(|_| "FAL consumer empty stream Finish failed")?;
    if info.outcome != protocol::Status::Ok || info.accepted != 0 || info.transported != 0 {
        return Err("FAL consumer empty stream result invalid");
    }
    empty.close().map_err(|_| "FAL consumer empty stream close failed")?;
    Ok(())
}

fn exercise_conditional_open(
    transport: &mut Transport,
    namespace: &PrefixTable<Arc<MailboxSender>>,
    parent: &MailboxSender,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let mut client = Client::new();
    let original = create_open_probe(&mut client, parent, "copy-stale", deadline)?;
    let observed = resolve::resolve(
        transport,
        namespace,
        "/copy-stale",
        protocol::ResolvePolicy::FollowAll,
    )
    .map_err(|_| "FAL consumer conditional Open resolve failed")?;
    if observed.info.identity != original.identity {
        return Err("FAL consumer conditional Open source identity changed");
    }
    client
        .call(
            parent,
            &Request::Delete {
                name: "copy-stale",
                expected: protocol::Expected {
                    identity: original.identity,
                    version: original.version,
                },
            },
            deadline,
        )
        .map_err(|_| "FAL consumer conditional Open old node delete failed")?;
    let replacement = create_open_probe(&mut client, parent, "copy-stale", deadline)?;
    if replacement.identity == original.identity {
        return Err("FAL consumer conditional Open replacement reused identity");
    }
    match transport.open_stream(
        &observed,
        protocol::StreamDirection::Write,
        0,
        Some(3),
        deadline,
    ) {
        Err(libfs::client::StreamOpenFailure::Call(libfal::client::ClientError::Status(
            protocol::Status::Conflict,
        ))) => {}
        Err(mut failure) => {
            failure
                .retry_cleanup()
                .map_err(|_| "FAL consumer unexpected conditional Open cleanup failed")?;
            return Err("FAL consumer conditional Open returned unexpected failure");
        }
        Ok(stream) => {
            stream
                .close()
                .map_err(|_| "FAL consumer unexpected conditional Stream close failed")?;
            return Err("FAL consumer conditional Open wrote replacement");
        }
    }
    let unchanged = client
        .call(
            parent,
            &Request::ReadAt {
                path: "copy-stale",
                offset: 0,
                count: 3,
            },
            deadline,
        )
        .map_err(|_| "FAL consumer conditional Open replacement read failed")?;
    if !matches!(
        protocol::decode_response(&unchanged.payload),
        Ok((_, Response::Value([])))
    ) {
        return Err("FAL consumer conditional Open replacement changed");
    }
    client
        .call(
            parent,
            &Request::Delete {
                name: "copy-stale",
                expected: protocol::Expected {
                    identity: replacement.identity,
                    version: replacement.version,
                },
            },
            deadline,
        )
        .map_err(|_| "FAL consumer conditional Open replacement cleanup failed")?;
    let pinned = create_open_probe(&mut client, parent, "copy-pinned", deadline)?;
    let position = resolve::resolve(
        transport,
        namespace,
        "/copy-pinned",
        protocol::ResolvePolicy::FollowAll,
    )
    .map_err(|_| "FAL consumer conditional Open pin resolve failed")?;
    let mut stream = transport
        .open_stream(
            &position,
            protocol::StreamDirection::Write,
            0,
            Some(3),
            deadline,
        )
        .map_err(|_| "FAL consumer conditional Open pin failed")?;
    client
        .call(
            parent,
            &Request::Delete {
                name: "copy-pinned",
                expected: protocol::Expected {
                    identity: pinned.identity,
                    version: pinned.version,
                },
            },
            deadline,
        )
        .map_err(|_| "FAL consumer conditional Open pin unlink failed")?;
    stream
        .write_until(b"pin", deadline)
        .map_err(|_| "FAL consumer conditional Open pin write failed")?;
    stream
        .end_write()
        .map_err(|_| "FAL consumer conditional Open pin EOF failed")?;
    let info = transport
        .finish_stream(&stream, deadline)
        .map_err(|_| "FAL consumer conditional Open pin Finish failed")?;
    if info.outcome != protocol::Status::Ok || info.accepted != 3 {
        return Err("FAL consumer conditional Open pin result invalid");
    }
    stream
        .close()
        .map_err(|_| "FAL consumer conditional Open pin close failed")?;
    Ok(())
}

fn exercise_abandoned_open_reply(
    grant: &MailboxSender,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let reply_owner = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)
        .map_err(|_| "FAL consumer Open reply mailbox creation failed")?;
    let filler = reply_owner
        .mint(0, Rights::WRITE | Rights::WAIT)
        .map_err(|_| "FAL consumer Open reply filler mint failed")?;
    let reply = reply_owner
        .mint(
            0,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )
        .map_err(|_| "FAL consumer Open reply sender mint failed")?;
    for index in 0..MAILBOX_CAPACITY {
        filler
            .sender
            .send(0x4f50_454e_4649_4c4c + index as u64, &[])
            .map_err(|_| "FAL consumer Open reply mailbox fill failed")?;
    }
    send_unawaited_request(
        grant,
        &reply.sender,
        &Request::Open {
            path: "probe-stream",
            expected_identity: None,
            direction: protocol::StreamDirection::Read,
            offset: 3,
            length: Some(6),
            session_deadline: deadline,
            stream_protocol: protocol::RNL2_PROTOCOL,
            tunnel_bytes: 0,
        },
        0x4f50_454e_4142_414e,
        deadline,
    )?;
    reply
        .sender
        .close()
        .map_err(|_| "FAL consumer Open reply sender close failed")?;
    reply_owner
        .close()
        .map_err(|_| "FAL consumer Open reply owner close failed")?;
    let observed = wait::wait_until(
        &[WaitItem::new(
            reply.lifetime.as_handle(),
            ObjectSignals::CLOSED,
            1,
        )],
        deadline,
    )
    .map_err(|_| "FAL consumer Open reply lifetime wait failed")?;
    if observed.item_index != 0
        || !observed.observed.contains(ObjectSignals::CLOSED)
        || !matches!(
            WaitReason::from_u32(observed.reason),
            Some(WaitReason::Closed | WaitReason::Signaled)
        )
    {
        return Err("FAL consumer Open reply authority not retired");
    }
    reply
        .lifetime
        .close()
        .map_err(|_| "FAL consumer Open reply lifetime close failed")?;
    filler
        .sender
        .close()
        .map_err(|_| "FAL consumer Open reply filler close failed")?;
    filler
        .lifetime
        .close()
        .map_err(|_| "FAL consumer Open reply filler lifetime close failed")?;
    let lookup = Client::new()
        .call(
            grant,
            &Request::Lookup {
                path: "probe-stream",
            },
            deadline,
        )
        .map_err(|_| "FAL consumer Open abandonment service probe failed")?;
    if !matches!(
        protocol::decode_response(&lookup.payload),
        Ok((
            _,
            Response::Node(protocol::NodeInfo {
                kind: NodeKind::Stream,
                ..
            })
        ))
    ) {
        return Err("FAL consumer Open abandonment service probe invalid");
    }
    Ok(())
}

fn exercise_cross_provider_copy(
    transport: &mut Transport,
    source: &resolve::Position<Grant>,
    destination: &Grant,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let copied = transport
        .copy_stream(libfs::client::CopyRequest {
            source,
            destination_parent: destination,
            destination_name: "consumer-copy-across",
            destination_rights: FalRights::READ_STREAM | FalRights::WRITE_STREAM,
            deadline,
            cancel: None,
        })
        .map_err(|mut failure| {
            let _ = failure.retry_cleanup(deadline);
            "FAL consumer cross-provider Copy failed"
        })?;
    if copied.bytes == 0
        || copied.source_result.accepted != copied.bytes
        || copied.target_result.accepted != copied.bytes
    {
        let _ = copied.target.delete_if_current(transport, deadline);
        return Err("FAL consumer cross-provider Copy count mismatch");
    }
    let mut client = Client::new();
    for (offset, count) in [(0, 9), (copied.bytes.saturating_sub(4), 4)] {
        let left = client
            .call(
                source.anchor.endpoint(),
                &Request::ReadAt {
                    path: &source.rel,
                    offset,
                    count,
                },
                deadline,
            )
            .map_err(|_| "FAL consumer cross-provider source ReadAt failed")?;
        let right = client
            .call(
                destination.endpoint(),
                &Request::ReadAt {
                    path: "consumer-copy-across",
                    offset,
                    count,
                },
                deadline,
            )
            .map_err(|_| "FAL consumer cross-provider target ReadAt failed")?;
        let (_, Response::Value(left)) = protocol::decode_response(&left.payload)
            .map_err(|_| "FAL consumer cross-provider source reply invalid")?
        else {
            return Err("FAL consumer cross-provider source reply shape invalid");
        };
        let (_, Response::Value(right)) = protocol::decode_response(&right.payload)
            .map_err(|_| "FAL consumer cross-provider target reply invalid")?
        else {
            return Err("FAL consumer cross-provider target reply shape invalid");
        };
        if left != right || left.len() != count as usize {
            return Err("FAL consumer cross-provider copied bytes mismatch");
        }
    }
    copied
        .target
        .delete_if_current(transport, deadline)
        .map_err(|_| "FAL consumer cross-provider Copy cleanup failed")?;
    Ok(())
}

fn exercise_created_target(
    transport: &mut Transport,
    parent: &Grant,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let rights = FalRights::READ_STREAM | FalRights::WRITE_STREAM;
    let target = transport
        .create_stream_target(parent, "consumer-copy-target", rights, deadline)
        .map_err(|_| "FAL consumer target exclusive Create failed")?;
    if !matches!(
        transport.create_stream_target(parent, "consumer-copy-target", rights, deadline),
        Err(libfs::client::TargetCreationFailure::Rejected {
            status: protocol::Status::Exists,
            ..
        })
    ) {
        return Err("FAL consumer target Create replaced an existing node");
    }
    let value = b"copy endpoint";
    let mut write = target
        .open_write(transport, Some(value.len() as u64), deadline)
        .map_err(|_| "FAL consumer target conditional Open failed")?;
    write
        .write_until(value, deadline)
        .map_err(|_| "FAL consumer target write failed")?;
    write
        .end_write()
        .map_err(|_| "FAL consumer target EOF failed")?;
    let info = transport
        .finish_stream(&write, deadline)
        .map_err(|_| "FAL consumer target Finish failed")?;
    if info.outcome != protocol::Status::Ok
        || info.accepted != value.len() as u64
        || info.transported != value.len() as u64
    {
        return Err("FAL consumer target accepted count invalid");
    }
    write
        .close()
        .map_err(|_| "FAL consumer target close failed")?;
    let read = Client::new()
        .call(
            parent.endpoint(),
            &Request::ReadAt {
                path: "consumer-copy-target",
                offset: 0,
                count: value.len() as u32,
            },
            deadline,
        )
        .map_err(|_| "FAL consumer target readback failed")?;
    if !matches!(
        protocol::decode_response(&read.payload),
        Ok((_, Response::Value(found))) if found == value
    ) {
        return Err("FAL consumer target readback mismatch");
    }
    target
        .delete_if_current(transport, deadline)
        .map_err(|_| "FAL consumer target conditional cleanup failed")?;
    let stale = transport
        .create_stream_target(parent, "consumer-copy-race", rights, deadline)
        .map_err(|_| "FAL consumer target race Create failed")?;
    let mut client = Client::new();
    client
        .call(
            parent.endpoint(),
            &Request::Delete {
                name: "consumer-copy-race",
                expected: protocol::Expected {
                    identity: stale.info().identity,
                    version: stale.info().version,
                },
            },
            deadline,
        )
        .map_err(|_| "FAL consumer target race original delete failed")?;
    let replacement = transport
        .create_stream_target(parent, "consumer-copy-race", rights, deadline)
        .map_err(|_| "FAL consumer target race replacement Create failed")?;
    match stale.open_write(transport, Some(1), deadline) {
        Err(libfs::client::StreamOpenFailure::Call(libfal::client::ClientError::Status(
            protocol::Status::Conflict,
        ))) => {}
        Err(mut failure) => {
            failure
                .retry_cleanup()
                .map_err(|_| "FAL consumer unexpected stale Open cleanup failed")?;
            return Err("FAL consumer stale Position returned unexpected failure");
        }
        Ok(stream) => {
            stream
                .close()
                .map_err(|_| "FAL consumer stale Open close failed")?;
            return Err("FAL consumer stale Position opened replacement");
        }
    }
    let Err((stale, libfal::client::ClientError::Status(protocol::Status::Conflict))) =
        stale.delete_if_current(transport, deadline)
    else {
        return Err("FAL consumer stale cleanup deleted replacement");
    };
    drop(stale);
    replacement
        .delete_if_current(transport, deadline)
        .map_err(|_| "FAL consumer replacement cleanup failed")?;
    Ok(())
}

fn exercise_copy_variants(
    transport: &mut Transport,
    namespace: &PrefixTable<Arc<MailboxSender>>,
    source: &resolve::Position<Grant>,
    first_parent: &Grant,
    second_parent: &Grant,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let rights = FalRights::READ_STREAM | FalRights::WRITE_STREAM;
    let existing = transport
        .create_stream_target(second_parent, "consumer-copy-exists", rights, deadline)
        .map_err(|_| "FAL consumer Copy conflict setup failed")?;
    let conflict = transport.copy_stream(libfs::client::CopyRequest {
        source,
        destination_parent: second_parent,
        destination_name: "consumer-copy-exists",
        destination_rights: rights,
        deadline,
        cancel: None,
    });
    let Err(conflict) = conflict else {
        return Err("FAL consumer Copy replaced existing target");
    };
    if !matches!(
        conflict.cause,
        libfs::client::CopyCause::TargetCreate(libfs::client::TargetCreationFailure::Rejected {
            status: protocol::Status::Exists,
            ..
        })
    ) || conflict.has_unretired_owners()
    {
        return Err("FAL consumer Copy conflict retained an owner");
    }
    existing
        .delete_if_current(transport, deadline)
        .map_err(|_| "FAL consumer Copy conflict cleanup failed")?;

    let event = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT,
    )
    .map_err(|_| "FAL consumer Copy cancellation source failed")?;
    // SAFETY: NotificationCreate transferred this unique read owner.
    let cancel = unsafe { Capability::from_raw(event.owner) };
    notification::signal(event.peer, 1)
        .map_err(|_| "FAL consumer Copy cancellation signal failed")?;
    let stopped = transport.copy_stream(libfs::client::CopyRequest {
        source,
        destination_parent: second_parent,
        destination_name: "consumer-copy-cancel",
        destination_rights: rights,
        deadline,
        cancel: Some(&cancel),
    });
    let Err(stopped) = stopped else {
        return Err("FAL consumer Copy ignored pre-cancellation");
    };
    if !matches!(stopped.cause, libfs::client::CopyCause::Cancelled)
        || stopped.has_unretired_owners()
        || stopped.target().is_some()
    {
        return Err("FAL consumer Copy pre-cancellation retained a target");
    }
    cancel
        .close()
        .map_err(|_| "FAL consumer Copy cancellation owner close failed")?;
    // SAFETY: this function still uniquely owns the Notification signal endpoint.
    unsafe { rinlib::ipc::object::close(event.peer) }
        .map_err(|_| "FAL consumer Copy cancellation peer close failed")?;

    let local = transport
        .copy_stream(libfs::client::CopyRequest {
            source,
            destination_parent: first_parent,
            destination_name: "consumer-copy-within",
            destination_rights: rights,
            deadline,
            cancel: None,
        })
        .map_err(|_| "FAL consumer same-provider Copy failed")?;
    if local.bytes == 0 || local.target_result.accepted != local.bytes {
        return Err("FAL consumer same-provider Copy progress invalid");
    }
    local
        .target
        .delete_if_current(transport, deadline)
        .map_err(|_| "FAL consumer same-provider Copy cleanup failed")?;

    let empty = {
        let mut client = Client::new();
        let info = client
            .call(
                first_parent.endpoint(),
                &Request::Create {
                    name: "consumer-copy-empty-source",
                    kind: NodeKind::Stream,
                    rights,
                    value: &[],
                },
                deadline,
            )
            .map_err(|_| "FAL consumer empty Copy source Create failed")?;
        let (_, Response::Node(info)) = protocol::decode_response(&info.payload)
            .map_err(|_| "FAL consumer empty Copy source reply invalid")?
        else {
            return Err("FAL consumer empty Copy source shape invalid");
        };
        info
    };
    let empty_position = resolve::resolve(
        transport,
        namespace,
        "/consumer-copy-empty-source",
        protocol::ResolvePolicy::FollowAll,
    )
    .map_err(|_| "FAL consumer empty Copy source resolve failed")?;
    let copied = transport
        .copy_stream(libfs::client::CopyRequest {
            source: &empty_position,
            destination_parent: second_parent,
            destination_name: "consumer-copy-empty-target",
            destination_rights: rights,
            deadline,
            cancel: None,
        })
        .map_err(|_| "FAL consumer empty Copy failed")?;
    if copied.bytes != 0
        || copied.source_result.outcome != protocol::Status::Ok
        || copied.target_result.outcome != protocol::Status::Ok
    {
        return Err("FAL consumer empty Copy reported data");
    }
    copied
        .target
        .delete_if_current(transport, deadline)
        .map_err(|_| "FAL consumer empty Copy target cleanup failed")?;
    let mut client = Client::new();
    client
        .call(
            first_parent.endpoint(),
            &Request::Delete {
                name: "consumer-copy-empty-source",
                expected: protocol::Expected {
                    identity: empty.identity,
                    version: empty.version,
                },
            },
            deadline,
        )
        .map_err(|_| "FAL consumer empty Copy source cleanup failed")?;
    Ok(())
}

fn exercise_copy_after_progress_cancel(
    transport: &mut Transport,
    namespace: &PrefixTable<Arc<MailboxSender>>,
    first_parent: &Grant,
    second_parent: &Grant,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let rights = FalRights::READ_STREAM | FalRights::WRITE_STREAM;
    let mut client = Client::new();
    let created = client
        .call(
            first_parent.endpoint(),
            &Request::Create {
                name: "consumer-copy-mid-source",
                kind: NodeKind::Stream,
                rights,
                value: &[],
            },
            deadline,
        )
        .map_err(|_| "FAL consumer midflight source Create failed")?;
    let (_, Response::Node(_)) = protocol::decode_response(&created.payload)
        .map_err(|_| "FAL consumer midflight source reply invalid")?
    else {
        return Err("FAL consumer midflight source shape invalid");
    };
    let position = resolve::resolve(
        transport,
        namespace,
        "/consumer-copy-mid-source",
        protocol::ResolvePolicy::FollowAll,
    )
    .map_err(|_| "FAL consumer midflight source resolve failed")?;
    let mut data = Vec::new();
    data.try_reserve_exact(65_536)
        .map_err(|_| "FAL consumer midflight source allocation failed")?;
    for index in 0..65_536 {
        data.push((index % 251) as u8);
    }
    let mut write = transport
        .open_stream(
            &position,
            protocol::StreamDirection::Write,
            0,
            Some(data.len() as u64),
            deadline,
        )
        .map_err(|_| "FAL consumer midflight source Open failed")?;
    write
        .write_until(&data, deadline)
        .map_err(|_| "FAL consumer midflight source Write failed")?;
    write
        .end_write()
        .map_err(|_| "FAL consumer midflight source EOF failed")?;
    let info = transport
        .finish_stream(&write, deadline)
        .map_err(|_| "FAL consumer midflight source Finish failed")?;
    if info.outcome != protocol::Status::Ok || info.accepted != data.len() as u64 {
        return Err("FAL consumer midflight source incomplete");
    }
    write
        .close()
        .map_err(|_| "FAL consumer midflight source close failed")?;

    let event = notification::create(
        Rights::READ | Rights::WAIT | Rights::MANAGE,
        Rights::SIGNAL | Rights::TRANSIT,
    )
    .map_err(|_| "FAL consumer midflight cancellation source failed")?;
    // SAFETY: NotificationCreate transferred this unique read owner.
    let cancel = unsafe { Capability::from_raw(event.owner) };
    let peer = event.peer;
    let observer = second_parent.clone();
    let watcher = rinlib::thread::Builder::new()
        .spawn(move || -> Result<(), SystemCallError> {
            let mut observer_client = Client::new();
            loop {
                match observer_client.call(
                    observer.endpoint(),
                    &Request::Lookup {
                        path: "consumer-copy-mid-target",
                    },
                    deadline,
                ) {
                    Ok(reply) => {
                        if let Ok((_, Response::Node(node))) =
                            protocol::decode_response(&reply.payload)
                            && node.size >= 4096
                        {
                            notification::signal(peer, 1)?;
                            break;
                        }
                    }
                    Err(libfal::client::ClientError::Status(protocol::Status::NotFound)) => {}
                    Err(_) => return Err(SystemCallError::InternalError),
                }
                if time::expired(deadline)? {
                    return Err(SystemCallError::DeadlineExpired);
                }
                time::sleep_until(time::timeout_millis(1)?)?;
            }
            // SAFETY: watcher uniquely owns this signal endpoint.
            unsafe { rinlib::ipc::object::close(peer) }
        })
        .map_err(|_| "FAL consumer midflight watcher spawn failed")?;
    let result = transport.copy_stream(libfs::client::CopyRequest {
        source: &position,
        destination_parent: second_parent,
        destination_name: "consumer-copy-mid-target",
        destination_rights: rights,
        deadline,
        cancel: Some(&cancel),
    });
    watcher
        .join()
        .map_err(|_| "FAL consumer midflight watcher join failed")?;
    let Err(mut result) = result else {
        return Err("FAL consumer midflight Copy ignored cancellation");
    };
    let cancelled_in_pump = matches!(result.cause, libfs::client::CopyCause::Pump(_));
    let cancelled_after_pump = matches!(result.cause, libfs::client::CopyCause::Cancelled)
        && result.target_bytes == data.len() as u64;
    if !(cancelled_in_pump || cancelled_after_pump)
        || result.target_bytes < 4096
        || result.target_bytes > data.len() as u64
        || result.has_unretired_owners()
    {
        return Err("FAL consumer midflight Copy progress or owner mismatch");
    }
    result
        .take_target()
        .ok_or("FAL consumer midflight target missing")?
        .delete_if_current(transport, deadline)
        .map_err(|_| "FAL consumer midflight target cleanup failed")?;
    let current = client
        .call(
            first_parent.endpoint(),
            &Request::Lookup {
                path: "consumer-copy-mid-source",
            },
            deadline,
        )
        .map_err(|_| "FAL consumer midflight source lookup failed")?;
    let (_, Response::Node(current)) = protocol::decode_response(&current.payload)
        .map_err(|_| "FAL consumer midflight source lookup reply invalid")?
    else {
        return Err("FAL consumer midflight source lookup shape invalid");
    };
    loop {
        match client.call(
            first_parent.endpoint(),
            &Request::Delete {
                name: "consumer-copy-mid-source",
                expected: protocol::Expected {
                    identity: current.identity,
                    version: current.version,
                },
            },
            deadline,
        ) {
            Ok(_) => break,
            Err(libfal::client::ClientError::Status(protocol::Status::Busy)) => {
                if time::expired(deadline)
                    .map_err(|_| "FAL consumer midflight cleanup clock failed")?
                {
                    return Err("FAL consumer midflight source cleanup timed out");
                }
                time::sleep_until(
                    time::timeout_millis(10)
                        .map_err(|_| "FAL consumer midflight cleanup retry deadline failed")?,
                )
                .map_err(|_| "FAL consumer midflight cleanup retry sleep failed")?;
            }
            Err(_) => return Err("FAL consumer midflight source cleanup failed"),
        }
    }
    cancel
        .close()
        .map_err(|_| "FAL consumer midflight cancel close failed")?;
    Ok(())
}

fn exercise_copy_during_provider_shutdown(
    transport: &mut Transport,
    namespace: &PrefixTable<Arc<MailboxSender>>,
    first_parent: &Grant,
    second_parent: &Grant,
    report_signaler: Handle,
    deadline: time::Deadline,
) -> Result<(), &'static str> {
    let rights = FalRights::READ_STREAM | FalRights::WRITE_STREAM;
    let mut client = Client::new();
    let created = client
        .call(
            first_parent.endpoint(),
            &Request::Create {
                name: "consumer-copy-mid-source",
                kind: NodeKind::Stream,
                rights,
                value: &[],
            },
            deadline,
        )
        .map_err(|_| "FAL consumer midflight source Create failed")?;
    let (_, Response::Node(_)) = protocol::decode_response(&created.payload)
        .map_err(|_| "FAL consumer midflight source reply invalid")?
    else {
        return Err("FAL consumer midflight source shape invalid");
    };
    let position = resolve::resolve(
        transport,
        namespace,
        "/consumer-copy-mid-source",
        protocol::ResolvePolicy::FollowAll,
    )
    .map_err(|_| "FAL consumer midflight source resolve failed")?;
    let mut data = Vec::new();
    data.try_reserve_exact(65_536)
        .map_err(|_| "FAL consumer midflight source allocation failed")?;
    for index in 0..65_536 {
        data.push((index % 251) as u8);
    }
    let mut write = transport
        .open_stream(
            &position,
            protocol::StreamDirection::Write,
            0,
            Some(data.len() as u64),
            deadline,
        )
        .map_err(|_| "FAL consumer midflight source Open failed")?;
    write
        .write_until(&data, deadline)
        .map_err(|_| "FAL consumer midflight source Write failed")?;
    write
        .end_write()
        .map_err(|_| "FAL consumer midflight source EOF failed")?;
    let info = transport
        .finish_stream(&write, deadline)
        .map_err(|_| "FAL consumer midflight source Finish failed")?;
    if info.outcome != protocol::Status::Ok || info.accepted != data.len() as u64 {
        return Err("FAL consumer midflight source incomplete");
    }
    write
        .close()
        .map_err(|_| "FAL consumer midflight source close failed")?;

    let observer = second_parent.clone();
    let watcher = rinlib::thread::Builder::new()
        .spawn(move || -> Result<(), SystemCallError> {
            let mut observer_client = Client::new();
            loop {
                match observer_client.call(
                    observer.endpoint(),
                    &Request::Lookup {
                        path: "consumer-copy-mid-target",
                    },
                    deadline,
                ) {
                    Ok(reply) => {
                        if let Ok((_, Response::Node(node))) =
                            protocol::decode_response(&reply.payload)
                            && node.size >= 4096
                        {
                            notification::signal(report_signaler, report::COPY_ARMED)?;
                            break Ok(());
                        }
                    }
                    Err(libfal::client::ClientError::Status(protocol::Status::NotFound)) => {}
                    Err(_) => return Err(SystemCallError::InternalError),
                }
                if time::expired(deadline)? {
                    return Err(SystemCallError::DeadlineExpired);
                }
                time::sleep_until(time::timeout_millis(1)?)?;
            }
        })
        .map_err(|_| "FAL consumer midflight watcher spawn failed")?;
    let result = transport.copy_stream(libfs::client::CopyRequest {
        source: &position,
        destination_parent: second_parent,
        destination_name: "consumer-copy-mid-target",
        destination_rights: rights,
        deadline,
        cancel: None,
    });
    watcher
        .join()
        .map_err(|_| "FAL consumer midflight watcher join failed")?;
    let Err(mut result) = result else {
        return Err("FAL consumer midflight Copy ignored provider shutdown");
    };
    if result.target_bytes < 4096 || result.target_bytes > data.len() as u64 {
        return Err("FAL consumer midflight Copy progress or owner mismatch");
    }
    let _target = result
        .take_target()
        .ok_or("FAL consumer midflight target missing")?;
    let current = client
        .call(
            first_parent.endpoint(),
            &Request::Lookup {
                path: "consumer-copy-mid-source",
            },
            deadline,
        )
        .map_err(|_| "FAL consumer midflight source lookup failed")?;
    let (_, Response::Node(current)) = protocol::decode_response(&current.payload)
        .map_err(|_| "FAL consumer midflight source lookup reply invalid")?
    else {
        return Err("FAL consumer midflight source lookup shape invalid");
    };
    loop {
        match client.call(
            first_parent.endpoint(),
            &Request::Delete {
                name: "consumer-copy-mid-source",
                expected: protocol::Expected {
                    identity: current.identity,
                    version: current.version,
                },
            },
            deadline,
        ) {
            Ok(_) => break,
            Err(libfal::client::ClientError::Status(protocol::Status::Busy)) => {
                if time::expired(deadline)
                    .map_err(|_| "FAL consumer midflight cleanup clock failed")?
                {
                    return Err("FAL consumer midflight source cleanup timed out");
                }
                time::sleep_until(
                    time::timeout_millis(10)
                        .map_err(|_| "FAL consumer midflight cleanup retry deadline failed")?,
                )
                .map_err(|_| "FAL consumer midflight cleanup retry sleep failed")?;
            }
            Err(_) => return Err("FAL consumer midflight source cleanup failed"),
        }
    }
    Ok(())
}
fn run() -> Result<(), &'static str> {
    let (primary, primary_desc) =
        MailboxSender::from_capability(startup_capability(startup::PRIMARY_ROOT)?)
            .map_err(|_| "FAL consumer primary grant invalid")?;
    let (directory, directory_desc) =
        MailboxSender::from_capability(startup_capability(startup::SERVICE_DIRECTORY)?)
            .map_err(|_| "FAL consumer service directory invalid")?;
    if primary_desc.rights.contains(Rights::GRANT) || directory_desc.rights.contains(Rights::GRANT)
    {
        return Err("FAL consumer received process grant authority");
    }
    let command_owner = startup_capability(startup::COMMAND_OWNER)?;
    let report_signaler = startup_capability(startup::REPORT_SIGNALER)?;
    let deadline = time::timeout_millis(120_000).map_err(|_| "FAL consumer deadline invalid")?;
    let mut client = Client::new();
    let primary_lookup = client
        .call(&primary, &Request::Lookup { path: "" }, deadline)
        .map_err(|_| "FAL consumer primary invocation failed")?;
    let (_, Response::Node(primary_info)) = protocol::decode_response(&primary_lookup.payload)
        .map_err(|_| "FAL consumer primary reply invalid")?
    else {
        return Err("FAL consumer primary reply shape invalid");
    };
    if primary_info.kind != NodeKind::Directory
        || !primary_info.rights.contains(FalRights::TRAVERSE)
    {
        return Err("FAL consumer primary root invalid");
    }
    let watch = client
        .subscribe(
            &directory,
            "",
            protocol::WatchMask::CREATE | protocol::WatchMask::DELETE,
            deadline,
        )
        .map_err(|_| "FAL consumer discovery subscription failed")?;
    notification::signal(report_signaler.as_handle(), report::DISCOVERY_ARMED)
        .map_err(|_| "FAL consumer discovery arm signal failed")?;
    match watch
        .wait(deadline)
        .map_err(|_| "FAL consumer discovery watch failed")?
    {
        SubscriptionEvent::Events(mask) if mask.contains(protocol::WatchMask::CREATE) => {}
        _ => return Err("FAL consumer secondary publication missing"),
    }
    drop(watch);
    let mut response = client
        .call(
            &directory,
            &Request::Read {
                path: "fs.secondary",
            },
            deadline,
        )
        .map_err(|_| "FAL consumer service Record read failed")?;
    let (header, Response::Value(value)) = protocol::decode_response(&response.payload)
        .map_err(|_| "FAL consumer service Record invalid")?
    else {
        return Err("FAL consumer service Record shape invalid");
    };
    if header.status != protocol::Status::Ok || response.handles.remaining() != 1 {
        return Err("FAL consumer service Record export incomplete");
    }
    let record = ServiceRecord::decode(value).map_err(|_| "FAL consumer Record decode failed")?;
    if record.protocol != protocol::ID || record.version != protocol::VERSION as u32 {
        return Err("FAL consumer service Record protocol mismatch");
    }
    let (secondary, description) = MailboxSender::from_capability(
        response
            .handles
            .take(0)
            .map_err(|_| "FAL consumer derived grant missing")?,
    )
    .map_err(|_| "FAL consumer derived grant invalid")?;
    if description.related_object_id == primary_desc.related_object_id {
        return Err("FAL consumer providers share a mailbox");
    }
    let lookup = client
        .call(&secondary, &Request::Lookup { path: "" }, deadline)
        .map_err(|_| "FAL consumer secondary invocation failed")?;
    let (_, Response::Node(info)) = protocol::decode_response(&lookup.payload)
        .map_err(|_| "FAL consumer secondary reply invalid")?
    else {
        return Err("FAL consumer secondary reply shape invalid");
    };
    if info.kind != NodeKind::Directory || !info.rights.contains(FalRights::TRAVERSE) {
        return Err("FAL consumer secondary root invalid");
    }
    drop(lookup);
    drop(primary_lookup);
    drop(response);
    notification::signal(report_signaler.as_handle(), report::SECONDARY_DISCOVERED)
        .map_err(|_| "FAL consumer secondary report failed")?;
    let continue_deadline =
        time::timeout_millis(240_000).map_err(|_| "FAL consumer business deadline invalid")?;
    let ready = wait::wait_until(
        &[WaitItem::new(
            command_owner.as_handle(),
            ObjectSignals::READABLE | ObjectSignals::CLOSED,
            1,
        )],
        continue_deadline,
    )
    .map_err(|_| "FAL consumer continuation wait failed")?;
    if !ready.observed.intersects(ObjectSignals::READABLE)
        || notification::take(command_owner.as_handle(), u64::MAX)
            .map_err(|_| "FAL consumer continuation take failed")?
            != command::CONTINUE
    {
        return Err("FAL consumer continuation missing");
    }
    let shutdown_grant;
    {
        for grant in [&primary, &secondary] {
            exercise_existing_values(&mut client, grant, deadline)?;
        }
        debug!("FAL2 independent property Copy and stream ReadAt passed");
        let deadline =
            time::timeout_millis(120_000).map_err(|_| "FAL consumer copy deadline invalid")?;
        client
            .copy_property(
                &primary,
                "probe-property",
                &secondary,
                "cross-provider-property-copy",
                FalRights::READ_PROPERTY,
                deadline,
            )
            .map_err(|_| "FAL consumer cross-provider property copy failed")?;
        let copied = client
            .call(
                &secondary,
                &Request::Read {
                    path: "cross-provider-property-copy",
                },
                deadline,
            )
            .map_err(|_| "FAL consumer copied property read failed")?;
        let (_, Response::Value(copied)) = protocol::decode_response(&copied.payload)
            .map_err(|_| "FAL consumer copied property reply invalid")?
        else {
            return Err("FAL consumer copied property reply shape invalid");
        };
        let expected = Value::Blob(b"updated");
        let mut bytes =
            alloc::vec![0; expected.encoded_len().ok_or("FAL consumer value length invalid")?];
        let used = expected
            .encode(&mut bytes)
            .map_err(|_| "FAL consumer value encode failed")?;
        if copied != &bytes[..used] {
            return Err("FAL consumer copied property value mismatch");
        }
        debug!("FAL2 independent property copy passed");
        if !matches!(
            client.move_entry(
                &primary,
                &secondary,
                libfal::client::MoveEntry {
                    source_parent: "",
                    source_name: "move-source",
                    destination_name: "cross-device",
                    expected: protocol::Expected::NONE,
                },
                deadline,
            ),
            Err(libfal::client::ClientError::Status(
                protocol::Status::CrossDevice
            ))
        ) {
            return Err("FAL consumer cross-provider Move did not return CrossDevice");
        }
        debug!("FAL2 independent cross-provider Move rejection passed");
        for grant in [&primary, &secondary] {
            exercise_directory_move_and_enumerate(&mut client, grant, deadline)?;
        }
        debug!("FAL2 independent enumeration and same-provider Move passed");
        exercise_unattached_offer(&primary, deadline)?;
        debug!("FAL2 independent unattached Open expired and retired");
        exercise_discarded_offer(&primary, deadline)?;
        debug!("FAL2 independent Open offer discarded");
        exercise_prestart_eof(&primary, deadline)?;
        debug!("FAL2 independent pre-Start EOF gate passed");
        if !matches!(
            client.call(&directory, &Request::Lookup { path: "second" }, deadline,),
            Err(libfal::client::ClientError::Status(
                protocol::Status::NotFound
            ))
        ) {
            return Err("FAL consumer discovery root reached memory route");
        }
        let primary = Arc::new(primary);
        let secondary = Arc::new(secondary);
        shutdown_grant = secondary.clone();
        let mut namespace = PrefixTable::<Arc<MailboxSender>>::new();
        namespace
            .mount("/", DirectoryGrant::new(primary.clone()))
            .map_err(|_| "FAL consumer primary namespace mount failed")?;
        let mut second_namespace = PrefixTable::<Arc<MailboxSender>>::new();
        second_namespace
            .mount("/", DirectoryGrant::new(secondary.clone()))
            .map_err(|_| "FAL consumer secondary namespace mount failed")?;
        let mut transport = Transport::new(deadline);
        let root = resolve::resolve(
            &mut transport,
            &namespace,
            "/",
            protocol::ResolvePolicy::FollowAll,
        )
        .map_err(|_| "FAL consumer primary namespace resolve failed")?;
        let delegated_root = resolve::resolve(
            &mut transport,
            &namespace,
            "/second",
            protocol::ResolvePolicy::FollowAll,
        )
        .map_err(|_| "FAL consumer delegated root resolve failed")?;
        let delegated_property = resolve::resolve(
            &mut transport,
            &namespace,
            "/second/f2-dir/leaf",
            protocol::ResolvePolicy::FollowAll,
        )
        .map_err(|_| "FAL consumer delegated remaining path resolve failed")?;
        if root.info.kind != NodeKind::Directory
            || delegated_root.info.kind != NodeKind::Directory
            || delegated_root.info.rights != (FalRights::TRAVERSE | FalRights::ENUMERATE)
            || delegated_property.info.kind != NodeKind::Property
        {
            return Err("FAL consumer delegated metadata or remaining path invalid");
        }
        let restricted = resolve::resolve(
            &mut transport,
            &namespace,
            "/second/probe-stream",
            protocol::ResolvePolicy::FollowAll,
        )
        .map_err(|_| "FAL consumer delegated stream resolve failed")?;
        let failure = match transport.open_stream(
            &restricted,
            protocol::StreamDirection::Read,
            3,
            Some(6),
            deadline,
        ) {
            Ok(stream) => {
                stream
                    .close()
                    .map_err(|_| "FAL consumer unexpected Stream close failed")?;
                return Err("FAL consumer restricted Open unexpectedly succeeded");
            }
            Err(failure) => failure,
        };
        let mut failure = failure;
        if !matches!(
            &failure,
            libfs::client::StreamOpenFailure::Call(libfal::client::ClientError::Status(
                protocol::Status::Permission
            ))
        ) {
            failure
                .retry_cleanup()
                .map_err(|_| "FAL consumer unexpected Open cleanup failed")?;
            return Err("FAL consumer restricted Open returned unexpected failure");
        }
        failure
            .retry_cleanup()
            .map_err(|_| "FAL consumer restricted Open cleanup failed")?;
        debug!("FAL2 independent Delegate and restricted Open passed");
        let primary_stream = resolve::resolve(
            &mut transport,
            &namespace,
            "/probe-stream",
            protocol::ResolvePolicy::FollowAll,
        )
        .map_err(|_| "FAL consumer primary stream resolve failed")?;
        let secondary_stream = resolve::resolve(
            &mut transport,
            &second_namespace,
            "/probe-stream",
            protocol::ResolvePolicy::FollowAll,
        )
        .map_err(|_| "FAL consumer secondary stream resolve failed")?;
        let secondary_parent = DirectoryGrant::new(secondary.clone());
        exercise_copy_variants(
            &mut transport,
            &namespace,
            &primary_stream,
            &DirectoryGrant::new(primary.clone()),
            &secondary_parent,
            deadline,
        )?;
        debug!("FAL2 independent Copy conflict and empty variants passed");
        exercise_copy_after_progress_cancel(
            &mut transport,
            &namespace,
            &DirectoryGrant::new(primary.clone()),
            &secondary_parent,
            deadline,
        )?;
        debug!("FAL2 independent Copy conflict, empty and cancellation variants passed");
        exercise_cross_provider_copy(&mut transport, &primary_stream, &secondary_parent, deadline)?;
        debug!("FAL2 independent cross-provider stream Copy passed");
        exercise_created_target(&mut transport, &secondary_parent, deadline)?;
        debug!("FAL2 independent exclusive target Create and pinned cleanup passed");
        let too_long = [b'x'; 4096];
        let name = core::str::from_utf8(&too_long).map_err(|_| "FAL consumer name invalid")?;
        if !matches!(
            transport.create_stream_target(
                &secondary_parent,
                name,
                FalRights::READ_STREAM | FalRights::WRITE_STREAM,
                deadline,
            ),
            Err(libfs::client::TargetCreationFailure::NoRequest(_))
        ) {
            return Err("FAL consumer Copy target local encoding was classified as unknown");
        }
        exercise_create_uncertainty(&secondary, deadline)?;
        debug!("FAL2 independent Create uncertainty and reply isolation passed");
        for position in [&primary_stream, &secondary_stream] {
            exercise_bounded_stream_read(&mut transport, position, deadline)?;
        }
        debug!("FAL2 independent primary and secondary bounded Open read passed");
        exercise_conditional_open(&mut transport, &namespace, &primary, deadline)?;
        debug!("FAL2 independent conditional Open identity and pin passed");
        exercise_abandoned_open_reply(&primary, deadline)?;
        debug!("FAL2 independent Open reply authority retired after abandonment");
        exercise_stream_contents(&mut transport, &primary_stream, deadline)?;
        debug!("FAL2 independent stream content and frozen read passed");
        exercise_resource_quota_recovery(
            &mut client,
            &mut transport,
            &primary_stream,
            &primary,
            deadline,
        )?;
        debug!("FAL2 resource quota saturation and recovery passed");
        let watch = client
            .subscribe(&primary, "", protocol::WatchMask::CREATE, deadline)
            .map_err(|_| "FAL consumer Watch subscription failed")?;
        let foreign_watch_context = client
            .derive(
                &primary,
                "",
                FalRights::TRAVERSE | FalRights::WATCH,
                deadline,
            )
            .map_err(|_| "FAL consumer foreign Watch context derive failed")?;
        let foreign_query = client.call(
            &foreign_watch_context,
            &Request::QuerySubscription {
                id: watch.info().id,
            },
            deadline,
        );
        let rejected = matches!(
            foreign_query,
            Err(libfal::client::ClientError::Status(
                protocol::Status::Permission
            ))
        );
        drop(foreign_watch_context);
        client
            .unsubscribe(&watch, deadline)
            .map_err(|_| "FAL consumer Watch cancellation failed")?;
        if !rejected {
            return Err("FAL consumer Watch accepted a foreign subscription context");
        }
        debug!("FAL2 independent Watch authorization passed");
        for grant in [&primary, &secondary] {
            for _ in 0..2 {
                let mut repeated = client
                    .call(
                        grant,
                        &Request::Read {
                            path: "probe-repeatable-handle",
                        },
                        deadline,
                    )
                    .map_err(|_| "FAL consumer repeatable handle read failed")?;
                if repeated.handles.remaining() != 1 {
                    return Err("FAL consumer repeatable handle reply layout invalid");
                }
                let capability = repeated
                    .handles
                    .take(0)
                    .map_err(|_| "FAL consumer repeatable handle missing")?;
                let (sender, _) = MailboxSender::from_capability(capability)
                    .map_err(|_| "FAL consumer repeatable handle role invalid")?;
                sender
                    .close()
                    .map_err(|_| "FAL consumer repeatable handle close failed")?;
            }
        }
        debug!("FAL2 independent repeatable handle read passed");
        for grant in [&primary, &secondary] {
            exercise_affine_take(&mut client, grant, deadline)?;
        }
        debug!("FAL2 independent affine Take recovery passed");
        for grant in [&primary, &secondary] {
            exercise_property_watch(&mut client, grant, deadline)?;
        }
        debug!("FAL2 independent Create Modify and Delete Watch passed");
        exercise_copy_during_provider_shutdown(
            &mut transport,
            &namespace,
            &DirectoryGrant::new(primary.clone()),
            &secondary_parent,
            report_signaler.as_handle(),
            deadline,
        )?;
        debug!("FAL2 in-flight Copy provider shutdown passed");
    }
    drop(directory);
    notification::signal(report_signaler.as_handle(), report::COMPLETE)
        .map_err(|_| "FAL consumer completion report failed")?;
    let shutdown_deadline =
        time::timeout_millis(120_000).map_err(|_| "FAL consumer shutdown deadline invalid")?;
    let ready = wait::wait_until(
        &[WaitItem::new(
            command_owner.as_handle(),
            ObjectSignals::READABLE | ObjectSignals::CLOSED,
            1,
        )],
        shutdown_deadline,
    )
    .map_err(|_| "FAL consumer shutdown command wait failed")?;
    if !ready.observed.intersects(ObjectSignals::READABLE)
        || notification::take(command_owner.as_handle(), u64::MAX)
            .map_err(|_| "FAL consumer shutdown command take failed")?
            != command::OBSERVE_SHUTDOWN
    {
        return Err("FAL consumer shutdown command missing");
    }
    let observed = wait::wait_until(
        &[WaitItem::new(
            shutdown_grant.as_handle(),
            ObjectSignals::CLOSED,
            1,
        )],
        shutdown_deadline,
    )
    .map_err(|_| "FAL consumer provider close wait failed")?;
    if !observed.observed.intersects(ObjectSignals::CLOSED) {
        return Err("FAL consumer did not observe provider close");
    }
    if let Err(ClientCallFailure::Unsent(mut failure)) = client.call_classified(
        &shutdown_grant,
        &Request::Lookup { path: "probe-stream" },
        shutdown_deadline,
        None,
    ) {
        if !failure.has_unretired_owners() || failure.take_unsent_request().is_none() {
            return Err("FAL consumer closed provider request owner missing");
        }
    } else {
        return Err("FAL consumer closed provider accepted a new request");
    }
    drop(shutdown_grant);
    notification::signal(report_signaler.as_handle(), report::PROVIDER_CLOSED)
        .map_err(|_| "FAL consumer provider close report failed")?;
    debug!("FAL2 independent consumer shutdown passed");
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        panic!("test_fal acceptance failed: {error}");
    }
}
