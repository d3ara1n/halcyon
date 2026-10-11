use super::*;
use crate::{
    LinkBoundary, ProviderBoundary,
    info::{self, DIRECTORY},
};

fn frame(op: u16, status: u32, body: &[u8]) -> Vec<u8> {
    // 根据计划线形独立组帧，不用生产编码器构造损坏帧的预期。
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&op.to_le_bytes());
    bytes.extend_from_slice(&status.to_le_bytes());
    bytes.extend_from_slice(&(body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u32.to_le_bytes());
    bytes.extend_from_slice(&0u64.to_le_bytes());
    bytes.extend_from_slice(body);
    bytes
}
fn bytes32(bytes: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
}
fn raw_page(cursor: &[u8], done: u32, count: u32, entries: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    bytes32(cursor, &mut body);
    body.extend_from_slice(&done.to_le_bytes());
    body.extend_from_slice(&count.to_le_bytes());
    body.extend_from_slice(entries);
    body
}

#[test]
fn header_and_lookup_match_fixed_little_endian_wire() {
    // 固定数字/字段顺序/无对齐是外部合同，捕获编码端和解码端共同写错。
    let mut body = Vec::new();
    bytes32("雪/x".as_bytes(), &mut body);
    let expected = frame(1, 0, &body);
    let mut out = [0; 128];
    let request = Request::Lookup { path: "雪/x" };
    let used = encode_request(&request, Deadline::INFINITE, &mut out).unwrap();
    assert_eq!(&out[..used], expected);
    assert_eq!(decode_request(&expected).unwrap().1, request);
    assert_eq!(decode_received_request(&expected, 1).unwrap().1, request);
    for slots in [0, 2, 3, u64::MAX] {
        assert_eq!(
            decode_received_request(&expected, slots),
            Err(Status::Invalid)
        );
    }
    assert_eq!(ID, 0x4f41_4c31);
    assert_eq!(BODY_MAX + HEADER_LEN + librpc::PREFIX_LEN, PAYLOAD_MAX);
}

#[test]
fn headers_keep_unknown_op_for_rejection_but_never_execute_it() {
    // 未知版本/op 返回 Unsupported，可信头仍保留可回显的 op/期限。
    let unknown = frame(u16::MAX, 0, &[]);
    assert_eq!(Header::decode(&unknown).unwrap().0.op, u16::MAX);
    assert_eq!(decode_request(&unknown), Err(Status::Unsupported));
    let mut version = unknown.clone();
    version[..2].copy_from_slice(&2u16.to_le_bytes());
    assert_eq!(decode_request(&version), Err(Status::Unsupported));
    let mut reply = [0; HEADER_LEN];
    assert_eq!(
        encode_response(
            u16::MAX,
            &Response::Error(Status::Unsupported),
            Deadline::INFINITE,
            &mut reply
        ),
        Ok(HEADER_LEN)
    );
    assert_eq!(Header::decode(&reply).unwrap().0.op, u16::MAX);
}

#[test]
fn malformed_headers_utf8_and_body_lengths_are_invalid() {
    // 拒绝截断、尾差、保留位、非法 Deadline、非零请求 status、非法 Text。
    let good = frame(1, 0, &0u32.to_le_bytes());
    for len in 0..good.len() {
        assert_eq!(decode_request(&good[..len]), Err(Status::Invalid));
    }
    let mut tail = good.clone();
    tail.push(0);
    assert_eq!(decode_request(&tail), Err(Status::Invalid));
    for offset in [4, 12, 16, 20, 24] {
        let mut bad = good.clone();
        bad[offset] = 3;
        assert_eq!(decode_request(&bad), Err(Status::Invalid));
    }
    let mut bad = good.clone();
    bad[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(decode_request(&bad), Err(Status::Invalid));
    let mut body = Vec::new();
    bytes32(&[0xff], &mut body);
    assert_eq!(decode_request(&frame(1, 0, &body)), Err(Status::Invalid));
}

#[test]
fn enumerate_initial_and_continuation_requests_are_exclusive() {
    // 续页按原入口+opaque cursor，不携旧 path；max_entries 不能为零。
    for request in [
        Request::Enumerate {
            path: "long/path",
            cursor: b"",
            max_entries: 1,
        },
        Request::Enumerate {
            path: "",
            cursor: b"opaque",
            max_entries: 4,
        },
        Request::Enumerate {
            path: "",
            cursor: b"",
            max_entries: 4,
        },
    ] {
        let mut out = [0; 128];
        let used = encode_request(&request, Deadline::at(123), &mut out).unwrap();
        assert_eq!(decode_request(&out[..used]).unwrap().1, request);
    }
    for (path, cursor, max_entries) in [
        ("a", b"cursor".as_slice(), 1u32),
        ("", b"".as_slice(), 0u32),
    ] {
        let mut body = Vec::new();
        bytes32(path.as_bytes(), &mut body);
        bytes32(cursor, &mut body);
        body.extend_from_slice(&max_entries.to_le_bytes());
        assert_eq!(decode_request(&frame(2, 0, &body)), Err(Status::Invalid));
        let request = Request::Enumerate {
            path,
            cursor,
            max_entries,
        };
        assert_eq!(
            encode_request(&request, Deadline::INFINITE, &mut [0; 128]),
            Err(Status::Invalid)
        );
    }
}

#[test]
fn pages_validate_progress_shape_and_boundary_item_without_export() {
    // 非终页要有项和 cursor；done 必须 0/1，终页无 cursor，边界项 info=0。
    let mut entry = Vec::new();
    bytes32(b"name", &mut entry);
    bytes32(b"", &mut entry);
    let page_bytes = raw_page(b"next", 0, 1, &entry);
    let page = Page::decode(&page_bytes).unwrap();
    assert_eq!(
        page.entries().collect::<Vec<_>>(),
        [DirectoryEntry {
            name: "name",
            info: None
        }]
    );
    let request = Request::Enumerate {
        path: "a",
        cursor: b"",
        max_entries: 1,
    };
    let mut body = 0u32.to_le_bytes().to_vec();
    body.extend_from_slice(&page_bytes);
    let response = frame(2, 0, &body);
    assert!(matches!(
        decode_response(&response, &request, Deadline::INFINITE, 0),
        Ok(Response::Page(_))
    ));
    for (cursor, done, count, entries) in [
        (b"".as_slice(), 2, 0, b"".as_slice()),
        (b"next".as_slice(), 1, 1, entry.as_slice()),
        (b"".as_slice(), 0, 1, entry.as_slice()),
        (b"next".as_slice(), 0, 0, b"".as_slice()),
        (b"".as_slice(), 1, u32::MAX, b"".as_slice()),
    ] {
        assert_eq!(
            Page::decode(&raw_page(cursor, done, count, entries)),
            Err(Status::Invalid)
        );
    }
    assert!(Page::decode(&raw_page(b"", 1, 0, b"")).unwrap().done);
    let mut tail = page_bytes.clone();
    tail.push(0);
    assert_eq!(Page::decode(&tail), Err(Status::Invalid));
    for len in 0..page_bytes.len() {
        assert_eq!(Page::decode(&page_bytes[..len]), Err(Status::Invalid));
    }
}

#[test]
fn responses_check_role_slots_operation_status_and_deadline() {
    // 只 ProviderBoundary 输出槽0；错误/面貌/页面/Link 均无业务能力。
    let request = Request::Lookup { path: "a/b" };
    let provider = Response::Boundary(Boundary::Provider(ProviderBoundary {
        consumed_bytes: 1,
        remaining: "b",
    }));
    let mut out = [0; 128];
    let used = encode_response(1, &provider, Deadline::at(123), &mut out).unwrap();
    assert_eq!(
        decode_response(&out[..used], &request, Deadline::at(123), 1),
        Ok(provider)
    );
    for slots in [0, 2, 3, 1 << MESSAGE_HANDLE_MAX, u64::MAX] {
        assert_eq!(
            decode_response(&out[..used], &request, Deadline::at(123), slots),
            Err(Status::Invalid)
        );
    }
    assert_eq!(
        decode_response(&out[..used], &request, Deadline::at(124), 1),
        Err(Status::Invalid)
    );
    out[2..4].copy_from_slice(&2u16.to_le_bytes());
    assert_eq!(
        decode_response(&out[..used], &request, Deadline::at(123), 1),
        Err(Status::Invalid)
    );
    let used = encode_response(
        1,
        &Response::Error(Status::NotFound),
        Deadline::INFINITE,
        &mut out,
    )
    .unwrap();
    assert_eq!(
        decode_response(&out[..used], &request, Deadline::INFINITE, 0),
        Ok(Response::Error(Status::NotFound))
    );
    assert_eq!(
        decode_response(&out[..used], &request, Deadline::INFINITE, 1),
        Err(Status::Invalid)
    );
    out[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        decode_response(&out[..used], &request, Deadline::INFINITE, 0),
        Err(Status::Invalid)
    );
    assert_eq!(
        decode_response(&frame(1, 1, &[0]), &request, Deadline::INFINITE, 0),
        Err(Status::Invalid)
    );
    let mut info_bytes = [0; 64];
    let len = info::encode(&[DIRECTORY], &[], &mut info_bytes).unwrap();
    let info = ObjectInfo::decode(&info_bytes[..len]).unwrap();
    let used = encode_response(1, &Response::Found(info), Deadline::INFINITE, &mut out).unwrap();
    assert_eq!(
        decode_response(&out[..used], &request, Deadline::INFINITE, 0),
        Ok(Response::Found(info))
    );
    assert_eq!(
        decode_response(&out[..used], &request, Deadline::INFINITE, 1),
        Err(Status::Invalid)
    );
    assert_eq!(
        encode_response(2, &Response::Found(info), Deadline::INFINITE, &mut out),
        Err(Status::Invalid)
    );
}

#[test]
fn continuations_reject_terminal_lookup_link_and_cursor_boundary() {
    // Lookup 末项是对象面貌；续页不能携旧 cursor 换入口。
    let link = Response::Boundary(Boundary::Link(LinkBoundary {
        parent_bytes: 0,
        target: "../new",
        remaining: "",
    }));
    let mut out = [0; 128];
    let used = encode_response(1, &link, Deadline::INFINITE, &mut out).unwrap();
    assert_eq!(
        decode_response(
            &out[..used],
            &Request::Lookup { path: "link" },
            Deadline::INFINITE,
            0
        ),
        Err(Status::Invalid)
    );
    let used = encode_response(2, &link, Deadline::INFINITE, &mut out).unwrap();
    assert_eq!(
        decode_response(
            &out[..used],
            &Request::Enumerate {
                path: "link",
                cursor: b"",
                max_entries: 1
            },
            Deadline::INFINITE,
            0
        ),
        Ok(link)
    );
    assert_eq!(
        decode_response(
            &out[..used],
            &Request::Enumerate {
                path: "",
                cursor: b"old",
                max_entries: 1
            },
            Deadline::INFINITE,
            0
        ),
        Err(Status::Invalid)
    );
    let bytes = raw_page(b"old", 0, 1, &[1, 0, 0, 0, b'a', 0, 0, 0, 0]);
    let page = Page::decode(&bytes).unwrap();
    let used = encode_response(2, &Response::Page(page), Deadline::INFINITE, &mut out).unwrap();
    assert_eq!(
        decode_response(
            &out[..used],
            &Request::Enumerate {
                path: "",
                cursor: b"old",
                max_entries: 1
            },
            Deadline::INFINITE,
            0
        ),
        Err(Status::Invalid)
    );
}

#[test]
fn capacity_includes_result_cursor_names_and_summary_prefixes() {
    // D-F1/D-F2 合同：首项和下一次空 path 请求均要完整可编码。
    let name = "a".repeat(BODY_MAX - 28);
    let mut out = vec![0; BODY_MAX];
    assert_eq!(
        encode_page(
            b"next",
            false,
            &[DirectoryEntry {
                name: &name,
                info: None
            }],
            &mut out
        ),
        Ok(BODY_MAX - 4)
    );
    let over = format!("{name}a");
    assert_eq!(
        encode_page(
            b"next",
            false,
            &[DirectoryEntry {
                name: &over,
                info: None
            }],
            &mut out
        ),
        Err(Status::EntryTooLarge)
    );
    let cursor = vec![0; BODY_MAX - 12];
    let request = Request::Enumerate {
        path: "",
        cursor: &cursor,
        max_entries: 1,
    };
    assert_eq!(request.encoded_len(), Ok(HEADER_LEN + BODY_MAX));
    let too_large = vec![0; cursor.len() + 1];
    assert_eq!(
        Request::Enumerate {
            path: "",
            cursor: &too_large,
            max_entries: 1
        }
        .encoded_len(),
        Err(Status::Invalid)
    );
    let max_path = "a".repeat(BODY_MAX - 4);
    assert_eq!(
        Request::Lookup { path: &max_path }.encoded_len(),
        Ok(HEADER_LEN + BODY_MAX)
    );
    assert_eq!(
        Request::Lookup {
            path: &format!("{max_path}a")
        }
        .encoded_len(),
        Err(Status::Invalid)
    );
}
