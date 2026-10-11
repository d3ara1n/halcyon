use super::*;
use crate::value::Value;

fn roundtrip(request: Request<'_>, op: u16, expected: usize) {
    let mut bytes = [0; PAYLOAD_MAX];
    let used = encode_request(&request, Deadline::at(99), &mut bytes).unwrap();
    assert_eq!(used, HEADER_LEN + expected);
    assert_eq!(&bytes[2..4], &op.to_le_bytes());
    let (h, decoded) = decode_request(&bytes[..used], request.request_slots()).unwrap();
    assert_eq!(h.op, op);
    assert_eq!(decoded.path(), request.path());
    assert_eq!(decoded.body_len().unwrap(), expected);
    for n in 0..used {
        assert!(decode_request(&bytes[..n], request.request_slots()).is_err());
    }
    assert!(decode_request(&bytes[..used], request.request_slots() | 4).is_err());
}
#[test]
fn all_twenty_one_opcodes_have_exact_body_lengths_and_slots() {
    // 对照计划逐项独立规定长度；捕获 opcode 漂移、漏字段、u64 缩成 u32。
    let mut value = [0; 32];
    let len = Value::Bool(true).encode(&mut value).unwrap();
    let value = &value[..len];
    let open = OpenOptions {
        direction: Direction::Read,
        offset: 4,
        length: Some(8),
        session_deadline: Deadline::at(90),
        tunnel_bytes: 0,
    };
    roundtrip(Request::Get { path: "x" }, 1, 5);
    roundtrip(Request::Set { path: "x", value }, 2, 9 + value.len());
    roundtrip(
        Request::ReadAt {
            path: "x",
            offset: 0,
            count: 1,
        },
        3,
        17,
    );
    roundtrip(
        Request::WriteAt {
            path: "x",
            offset: 0,
            data: b"abc",
        },
        4,
        20,
    );
    roundtrip(
        Request::Open {
            path: "x",
            options: open,
        },
        5,
        49,
    );
    roundtrip(
        Request::CreateOpen {
            path: "x",
            options: OpenOptions {
                direction: Direction::Write,
                offset: 0,
                length: None,
                ..open
            },
            rights: FsRights::WRITE_STREAM,
        },
        6,
        53,
    );
    roundtrip(
        Request::Create {
            path: "x",
            kind: NodeKind::Property,
            rights: FsRights::READ_PROPERTY,
            initial: value,
        },
        7,
        17 + value.len(),
    );
    roundtrip(Request::Remove { path: "x" }, 8, 5);
    roundtrip(
        Request::Rename {
            source: "x",
            destination_name: "y",
            replace: true,
        },
        9,
        14,
    );
    roundtrip(Request::ReadLink { path: "x" }, 10, 5);
    roundtrip(
        Request::SetLength {
            path: "x",
            length: 8,
        },
        11,
        13,
    );
    roundtrip(Request::Sync { path: "x" }, 12, 5);
    roundtrip(
        Request::DeriveDirectory {
            path: "x",
            rights: FsRights::DESCRIBE,
            sender_transport: Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
            output_transport: Rights::READ | Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        },
        13,
        25,
    );
    roundtrip(
        Request::Watch {
            path: "x",
            mask: WATCH_CREATE,
        },
        14,
        13,
    );
    roundtrip(Request::Start, 15, 0);
    roundtrip(Request::QueryStream, 16, 0);
    roundtrip(Request::Finish, 17, 0);
    roundtrip(Request::Cancel, 18, 0);
    roundtrip(Request::QueryWatch, 19, 0);
    roundtrip(Request::AckWatch { sequence: 7 }, 20, 8);
    roundtrip(Request::CancelWatch, 21, 0);
}
#[test]
fn widened_transport_masks_reject_high_unknown_bits_and_route_lengths_are_exact() {
    // 高位必须实际读取再拒绝，不能截断后当作合法小 mask；Bind=20+path。
    let request = Request::DeriveDirectory {
        path: "x",
        rights: FsRights::DERIVE,
        sender_transport: Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        output_transport: Rights::NONE,
    };
    let mut bytes = [0; 128];
    let used = encode_request(&request, Deadline::INFINITE, &mut bytes).unwrap();
    bytes[HEADER_LEN + 17..HEADER_LEN + 25].copy_from_slice(&(1u64 << 32).to_le_bytes());
    assert!(matches!(
        decode_request(&bytes[..used], 1),
        Err(Status::Invalid)
    ));
    let bind = crate::route::Request::Bind {
        path: "x",
        max_rights: FsRights::DESCRIBE,
        output_transport: Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        replace: false,
    };
    let used = crate::route::encode_request(&bind, Deadline::INFINITE, &mut bytes).unwrap();
    assert_eq!(used, HEADER_LEN + 21);
    assert!(crate::route::decode_request(&bytes[..used], 3).is_ok());
    bytes[HEADER_LEN + 9..HEADER_LEN + 17].copy_from_slice(&(1u64 << 32).to_le_bytes());
    assert!(matches!(
        crate::route::decode_request(&bytes[..used], 3),
        Err(Status::Invalid)
    ));
}
#[test]
fn boundary_and_io_preserve_uncommitted_path_and_partial_progress_contracts() {
    // Boundary 无数据/副作用，只有换入口带槽0；短 I/O 错误保留已完成数量。
    let request = Request::ReadAt {
        path: "a/x",
        offset: 0,
        count: 8,
    };
    let mut bytes = [0; 128];
    let response = Response::Boundary(Boundary::Provider(liboal::ProviderBoundary {
        consumed_bytes: 1,
        remaining: "x",
    }));
    let used = encode_response(3, &response, Deadline::INFINITE, &mut bytes).unwrap();
    assert!(matches!(
        decode_response(&bytes[..used], &request, Deadline::INFINITE, 1),
        Ok(Response::Boundary(_))
    ));
    assert!(decode_response(&bytes[..used], &request, Deadline::INFINITE, 0).is_err());
    let response = Response::Io(IoResult {
        count: 3,
        end: IoEnd::Error,
        cause: Status::Quota,
        data: b"abc",
    });
    let used = encode_response(3, &response, Deadline::INFINITE, &mut bytes).unwrap();
    let Response::Io(io) =
        decode_response(&bytes[..used], &request, Deadline::INFINITE, 0).unwrap()
    else {
        panic!("unexpected response")
    };
    assert_eq!(io.count, 3);
    assert_eq!(io.cause, Status::Quota);
    assert_eq!(io.data, b"abc");
    assert!(decode_response(&bytes[..used], &request, Deadline::INFINITE, 1).is_err());
    let response = Response::Io(IoResult {
        count: 0,
        end: IoEnd::Continue,
        cause: Status::Ok,
        data: b"",
    });
    let used = encode_response(3, &response, Deadline::INFINITE, &mut bytes).unwrap();
    assert!(decode_response(&bytes[..used], &request, Deadline::INFINITE, 0).is_err());
}
#[test]
fn stream_and_watch_fixed_shapes_validate_states_and_final_causes() {
    // 48/40/24 是线形合同，终态原因与 outcome 配对且 reserved 必须为零。
    let mut bytes = [0; 128];
    let offer = StreamOffer {
        session_id: 7,
        direction: Direction::Read,
        tunnel_bytes: 8192,
        start: 1,
        read_end: 9,
        offer_deadline: Deadline::at(20),
    };
    let used = encode_response(5, &Response::Offer(offer), Deadline::INFINITE, &mut bytes).unwrap();
    assert_eq!(used, HEADER_LEN + 4 + 48);
    let request = Request::Open {
        path: "x",
        options: OpenOptions {
            direction: Direction::Read,
            offset: 1,
            length: Some(8),
            session_deadline: Deadline::INFINITE,
            tunnel_bytes: 0,
        },
    };
    assert!(
        matches!(decode_response(&bytes[..used],&request,Deadline::INFINITE,3),Ok(Response::Offer(v))if v==offer)
    );
    let stream = StreamInfo {
        session_id: 7,
        state: StreamState::Terminal,
        outcome: Status::Ok,
        reason: StreamReason::Completed,
        accepted: 8,
        transported: 8,
    };
    let used = encode_response(
        17,
        &Response::Stream(stream),
        Deadline::INFINITE,
        &mut bytes,
    )
    .unwrap();
    assert_eq!(used, HEADER_LEN + 40);
    assert!(
        matches!(decode_response(&bytes[..used],&Request::Finish,Deadline::INFINITE,0),Ok(Response::Stream(v))if v==stream)
    );
    bytes[HEADER_LEN + 20] = 1;
    assert!(decode_response(&bytes[..used], &Request::Finish, Deadline::INFINITE, 0).is_err());
    assert!(
        encode_response(
            17,
            &Response::Stream(StreamInfo {
                reason: StreamReason::Cancelled,
                ..stream
            }),
            Deadline::INFINITE,
            &mut bytes
        )
        .is_err()
    );
    let watch = WatchInfo {
        sequence: 1,
        pending: WATCH_INITIAL,
        state: WatchState::Active,
        reason: WatchReason::Active,
    };
    let used =
        encode_response(19, &Response::Watch(watch), Deadline::INFINITE, &mut bytes).unwrap();
    assert_eq!(used, HEADER_LEN + 24);
    assert!(
        matches!(decode_response(&bytes[..used],&Request::QueryWatch,Deadline::INFINITE,0),Ok(Response::Watch(v))if v==watch)
    );
}

#[test]
fn stream_offer_must_match_open_direction_offset_range_and_deadline() {
    // Offer 自身合法不足以证明兑现本次 Open；方向、offset、范围和期限可独立反例。
    let request = Request::Open {
        path: "x",
        options: OpenOptions {
            direction: Direction::Read,
            offset: 100,
            length: Some(8),
            session_deadline: Deadline::at(30),
            tunnel_bytes: 0,
        },
    };
    let valid = StreamOffer {
        session_id: 1,
        direction: Direction::Read,
        tunnel_bytes: 8192,
        start: 100,
        read_end: 108,
        offer_deadline: Deadline::at(20),
    };
    let mut bytes = [0; 128];
    for offer in [
        StreamOffer {
            direction: Direction::Write,
            start: 0,
            read_end: 0,
            ..valid
        },
        StreamOffer { start: 99, ..valid },
        StreamOffer {
            read_end: 109,
            ..valid
        },
        StreamOffer {
            offer_deadline: Deadline::at(31),
            ..valid
        },
    ] {
        let used =
            encode_response(5, &Response::Offer(offer), Deadline::at(25), &mut bytes).unwrap();
        assert!(decode_response(&bytes[..used], &request, Deadline::at(25), 3).is_err());
    }
    let used = encode_response(5, &Response::Offer(valid), Deadline::at(25), &mut bytes).unwrap();
    assert!(decode_response(&bytes[..used], &request, Deadline::at(25), 3).is_ok());
    let create = Request::CreateOpen {
        path: "x",
        options: OpenOptions {
            direction: Direction::Write,
            offset: 0,
            length: None,
            session_deadline: Deadline::INFINITE,
            tunnel_bytes: 0,
        },
        rights: FsRights::WRITE_STREAM,
    };
    let used = encode_response(6, &Response::Offer(valid), Deadline::INFINITE, &mut bytes).unwrap();
    assert!(decode_response(&bytes[..used], &create, Deadline::INFINITE, 3).is_err());
}
