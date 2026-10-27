//! 期望来自设备计划的登记、冻结表及 wire 合同，不使用实现状态生成预期。

use std::{cell::Cell, rc::Rc, sync::Arc};

use erhino_shared::object::{HandleDescription, ObjectKind, Rights};
use libbudget::{AccountView, Budget, Taxonomy};
use libdevice::{
    Error,
    identity::{BindingRef, DeviceId, Ids},
    publication::{Presence, Publications},
    resource::{Collecting, Descriptor, DeviceResource, Entry, Limits},
};
use librpc::bytes::{Reader, Writer};

fn funding(publications: usize, bytes: usize) -> (Arc<Budget>, AccountView<DeviceResource>) {
    let limits = [publications, 4, bytes, 8, 64, 32, 32, bytes, 32, 32];
    let budget = Budget::new(&limits, 1).unwrap();
    let slots: [_; DeviceResource::COUNT] = core::array::from_fn(|n| budget.slot(n).unwrap());
    let account = budget.account(&limits).unwrap().view(&slots).unwrap();
    (budget, account)
}

#[test]
fn withdraw_before_commit_refunds_candidate_and_never_revives_publication() {
    let (budget, account) = funding(2, 4096);
    let mut publications = Publications::new(23, 2).unwrap();
    let candidate = publications.prepare(1, "original", &account).unwrap();
    assert_eq!(publications.withdraw(1), Ok(None));
    let failed = publications.commit(candidate).err().unwrap();
    assert_eq!(failed.error, Error::Closed);
    assert!(!budget.is_empty());
    drop(failed);
    assert!(budget.is_empty());
    assert_eq!(publications.existing(1), Err(Error::Closed));
    assert_eq!(publications.existing(3), Err(Error::Invalid));
    assert_eq!(publications.highest(), 1);
    let candidate = publications.prepare(2, "next", &account).unwrap();
    assert!(publications.commit(candidate).is_ok());
    assert_eq!(publications.existing(2), Ok(Some(&"next")));
    assert_eq!(publications.withdraw(4), Err(Error::Invalid));
    assert_eq!(publications.highest(), 2);
    assert_eq!(publications.withdraw(2), Ok(Some(&"next")));
    assert_eq!(publications.existing(2), Err(Error::Closed));
    let retired = publications.remove_retired(2).unwrap();
    assert_eq!(*retired.value(), "next");
    // 已退出现用索引的实际存储 owner 仍付费，不能在摘索引时提前退款。
    assert!(!budget.is_empty());
    drop(retired);
    assert!(budget.is_empty());
}

#[test]
fn quota_and_foreign_preparation_do_not_consume_sequence() {
    let (budget, account) = funding(1, 4096);
    let mut a = Publications::new(5, 1).unwrap();
    let mut b = Publications::new(6, 1).unwrap();
    let candidate = a.prepare(1, 42, &account).unwrap();
    let failed = b.commit(candidate).err().unwrap();
    assert_eq!(failed.error, Error::Permission);
    assert_eq!(b.highest(), 0);
    assert!(a.commit(failed.prepared).is_ok());
    assert!(matches!(a.prepare(2, 99, &account), Err(Error::Quota)));
    assert_eq!(a.highest(), 1);
    assert_eq!(a.existing(1), Ok(Some(&42)));
    // 清理不依赖新业务槽；已满额时仍能关闭尚未登记的恰好下一轮。
    assert_eq!(a.withdraw(2), Ok(None));
    a.seal();
    assert_eq!(a.existing(1), Err(Error::Closed));
    let (seq, existing) = a.next(0).unwrap();
    assert_eq!(seq, 1);
    assert_eq!(existing.presence(), Presence::Present);
    a.withdraw(seq).unwrap();
    drop(a.remove_retired(seq).unwrap());
    assert!(budget.is_empty());
}

#[test]
fn seal_is_immediate_even_before_cleanup_reaches_records() {
    let (_, account) = funding(2, 4096);
    let mut p = Publications::new(10, 2).unwrap();
    let first = p.prepare(1, (), &account).unwrap();
    assert!(p.commit(first).is_ok());
    let next = p.prepare(2, (), &account).unwrap();
    p.seal();
    assert_eq!(p.existing(1), Err(Error::Closed));
    assert_eq!(p.commit(next).err().unwrap().error, Error::Closed);
    assert_eq!(p.highest(), 1);
    p.withdraw(1).unwrap();
    drop(p.remove_retired(1).unwrap());
}

fn descriptor() -> Descriptor {
    Descriptor {
        kind: ObjectKind::MmioUse,
        rights: Rights::READ | Rights::WRITE,
        protocol: 0,
    }
}

struct Capability(Rc<Cell<usize>>);
impl Capability {
    fn new(count: &Rc<Cell<usize>>) -> Self {
        count.set(count.get() + 1);
        Self(count.clone())
    }
}
impl Drop for Capability {
    fn drop(&mut self) {
        self.0.set(self.0.get() - 1);
    }
}

#[test]
fn partial_table_is_unpublishable_and_keeps_owned_capabilities_and_fees() {
    let (budget, account) = funding(1, 8192);
    let live = Rc::new(Cell::new(0));
    let mut table = Collecting::reserve(
        Limits {
            entries: 2,
            text_bytes: 12,
        },
        32,
        &account,
    )
    .unwrap();
    table.prepared(2).unwrap();
    let first = Entry {
        index: 0,
        descriptor: descriptor(),
        name: "bar",
        metadata: &[1, 2],
    };
    table.push(&first, Capability::new(&live)).unwrap();
    let mut partial = match table.finish() {
        Err(partial) => partial,
        Ok(_) => panic!("partial table became visible"),
    };
    let duplicate = Entry { index: 1, ..first };
    assert_eq!(
        partial.push(&duplicate, Capability::new(&live)),
        Err(Error::Conflict)
    );
    assert_eq!(live.get(), 1);
    assert!(!budget.is_empty());
    assert_eq!(partial.prepared(1), Err(Error::Conflict));
    drop(partial);
    assert_eq!(live.get(), 0);
    assert!(budget.is_empty());
}

#[test]
fn full_table_uses_declared_capacity_instead_of_transport_handle_limit() {
    let (budget, account) = funding(1, 8192);
    let mut table = Collecting::reserve(
        Limits {
            entries: 12,
            text_bytes: 128,
        },
        32,
        &account,
    )
    .unwrap();
    table.prepared(12).unwrap();
    let paid = account.usage(DeviceResource::Bytes).0;
    for index in 0..12 {
        let name = format!("resource{index}");
        let entry = Entry {
            index,
            descriptor: descriptor(),
            name: &name,
            metadata: &[],
        };
        table.push(&entry, index).unwrap();
        assert_eq!(account.usage(DeviceResource::Bytes).0, paid);
    }
    let table = match table.finish() {
        Ok(table) => table,
        Err(_) => panic!("complete table failed to freeze"),
    };
    assert_eq!(table.len(), 12);
    assert_eq!(*table.get("resource11").unwrap().capability, 11);
    assert!(table.get("resource11/child").is_none());
    drop(table);
    assert!(budget.is_empty());
}

#[test]
fn oversize_metadata_is_rejected_before_changing_partial_table() {
    let (budget, account) = funding(1, 8192);
    let mut table = Collecting::reserve(
        Limits {
            entries: 1,
            text_bytes: 3,
        },
        32,
        &account,
    )
    .unwrap();
    table.prepared(1).unwrap();
    let mut entry = Entry {
        index: 0,
        descriptor: descriptor(),
        name: "bar",
        metadata: &[1],
    };
    assert_eq!(table.push(&entry, ()), Err(Error::Quota));
    entry.metadata = &[];
    table.push(&entry, ()).unwrap();
    assert!(table.finish().is_ok());
    assert!(budget.is_empty());
}

#[test]
fn wire_identity_has_fixed_little_endian_bytes_and_nonzero_fields() {
    let mut ids = Ids::new(0x0102).unwrap();
    let a = ids.allocate().unwrap();
    let b = ids.allocate().unwrap();
    assert_eq!(ids.scan_end(), 2);
    assert_ne!(a, b);
    let reference = BindingRef::new(a, 0x0304).unwrap();
    let mut bytes = [0; 24];
    reference.encode(&mut Writer::new(&mut bytes));
    assert_eq!(
        bytes,
        [
            2, 1, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 4, 3, 0, 0, 0, 0, 0, 0,
        ]
    );
    assert_eq!(BindingRef::decode(&mut Reader::new(&bytes)), Ok(reference));
    assert_eq!(DeviceId::new(0, 1), Err(Error::Invalid));
    assert_eq!(BindingRef::new(a, 0), Err(Error::InvalidEpoch));
    assert_eq!(
        BindingRef::decode(&mut Reader::new(&bytes[..23])),
        Err(Error::Invalid)
    );
}

#[test]
fn resource_wire_rejects_extra_slots_reserved_fields_tail_and_length_forgery() {
    let entry = Entry {
        index: 2,
        descriptor: descriptor(),
        name: "bar",
        metadata: &[9],
    };
    let mut bytes = [0; 128];
    let len = entry.encode(32, &mut bytes).unwrap();
    assert_eq!(&bytes[..12], &[2, 0, 0, 0, 22, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(Entry::decode(&bytes[..len], 32, 1).unwrap().name, "bar");
    assert!(matches!(
        Entry::decode(&bytes[..len], 32, 0),
        Err(Error::Invalid)
    ));
    assert!(matches!(
        Entry::decode(&bytes[..len], 32, 2),
        Err(Error::Invalid)
    ));
    assert!(matches!(
        Entry::decode(&bytes[..len + 1], 32, 1),
        Err(Error::Invalid)
    ));
    bytes[8] = 1;
    assert!(matches!(
        Entry::decode(&bytes[..len], 32, 1),
        Err(Error::Invalid)
    ));
    bytes[8] = 0;
    bytes[28..32].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(matches!(
        Entry::decode(&bytes[..len], 32, 1),
        Err(Error::Invalid)
    ));
    assert_eq!(entry.encode(usize::MAX, &mut bytes), Err(Error::Invalid));
    let invalid = Entry {
        name: "bar/child",
        ..entry
    };
    assert_eq!(invalid.encoded_len(), Err(Error::Invalid));
}

#[test]
fn fixed_export_rights_do_not_require_giving_clients_duplicate_authority() {
    let descriptor = descriptor();
    let mut actual = HandleDescription {
        object_id: 7,
        related_object_id: 0,
        kind: ObjectKind::MmioUse as u32,
        reserved0: 0,
        rights: Rights::READ | Rights::WRITE | Rights::DUPLICATE | Rights::TRANSIT,
        badge: 0,
        reserved: 0,
    };
    assert_eq!(descriptor.check_handle(&actual), Ok(()));
    actual.rights = Rights::READ | Rights::WRITE | Rights::TRANSIT;
    assert_eq!(descriptor.check_handle(&actual), Err(Error::Permission));
    actual.kind = ObjectKind::MemoryObject as u32;
    assert_eq!(descriptor.check_handle(&actual), Err(Error::Permission));
    assert_eq!(
        Descriptor {
            kind: ObjectKind::TunnelInvitation,
            ..descriptor
        }
        .validate(),
        Err(Error::Unsupported)
    );
}

#[test]
fn collecting_rejects_untransportable_entry_despite_sufficient_domain_quota() {
    use erhino_shared::message::PAYLOAD_MAX;
    let (budget, account) = funding(1, PAYLOAD_MAX * 4);
    let live = Rc::new(Cell::new(0));
    let mut table = Collecting::reserve(
        Limits {
            entries: 1,
            text_bytes: (PAYLOAD_MAX + 1) as u32,
        },
        32,
        &account,
    )
    .unwrap();
    table.prepared(1).unwrap();
    let metadata = vec![0; PAYLOAD_MAX];
    let entry = Entry {
        index: 0,
        descriptor: descriptor(),
        name: "r",
        metadata: &metadata,
    };
    assert_eq!(
        table.push(&entry, Capability::new(&live)),
        Err(Error::Invalid)
    );
    assert_eq!(live.get(), 0);
    let mut table = match table.finish() {
        Err(table) => table,
        Ok(_) => panic!("untransportable entry was frozen"),
    };
    // 真实回复恰好占满载荷时仍可接收，不能借修复加任意保守阈值。
    let exact = PAYLOAD_MAX - librpc::PREFIX_LEN - 32 - Entry::FIXED_LEN - 1;
    let boundary = Entry {
        metadata: &metadata[..exact],
        ..entry
    };
    table.push(&boundary, Capability::new(&live)).unwrap();
    let table = match table.finish() {
        Ok(table) => table,
        Err(_) => panic!("transport boundary was rejected"),
    };
    assert_eq!(table.get("r").unwrap().metadata.len(), exact);
    drop(table);
    assert_eq!(live.get(), 0);
    assert!(budget.is_empty());
}

#[test]
fn frozen_resources_oal_is_named_read_only_and_round_specific() {
    use erhino_shared::time::Deadline;
    use libdevice::resource::oal::Info;
    use liboal::protocol::{self as oal, Request, Response, Status};
    let (_, account) = funding(1, 8192);
    let mut collecting = Collecting::reserve(
        Limits {
            entries: 3,
            text_bytes: 256,
        },
        libdevice::protocol::RESOURCE_HEADER_LEN,
        &account,
    )
    .unwrap();
    collecting.prepared(3).unwrap();
    let descriptor = Descriptor {
        kind: ObjectKind::MemoryObject,
        rights: Rights::READ,
        protocol: 0,
    };
    for (index, name) in ["z", "a", "m"].into_iter().enumerate() {
        collecting
            .push(
                &Entry {
                    index: index as u32,
                    descriptor,
                    name,
                    metadata: b"opaque",
                },
                index,
            )
            .unwrap();
    }
    let table = collecting.finish().ok().unwrap();
    let id = DeviceId::new(31, 9).unwrap();
    let reference = BindingRef::new(id, 1).unwrap();
    let deadline = Deadline::at(100);
    let mut input = [0; oal::HEADER_LEN + oal::BODY_MAX];
    let mut output = [0; oal::HEADER_LEN + oal::BODY_MAX];
    let lookup = Request::Lookup { path: "a" };
    let used = oal::encode_request(&lookup, deadline, &mut input).unwrap();
    let n = table
        .discover(reference, &input[..used], 0, &mut output)
        .unwrap();
    let Response::Found(summary) =
        oal::decode_response(&output[..n], &lookup, deadline, 0).unwrap()
    else {
        panic!("resource leaf missing")
    };
    assert_eq!(
        Info::decode(summary).unwrap(),
        Info {
            reference,
            descriptor: Some(descriptor)
        }
    );
    assert_eq!(
        table.discover(reference, &input[..used], 1, &mut output),
        Err(Status::Invalid)
    );
    let bad = Request::Lookup { path: "a/child" };
    let n = oal::encode_request(&bad, deadline, &mut input).unwrap();
    assert_eq!(
        table.discover(reference, &input[..n], 0, &mut output),
        Err(Status::NotDirectory)
    );
    let mut cursor = Vec::new();
    let mut old_cursor = Vec::new();
    for (round, expected) in ["a", "m", "z"].into_iter().enumerate() {
        let request = Request::Enumerate {
            path: "",
            cursor: &cursor,
            max_entries: 8,
        };
        let used = oal::encode_request(&request, deadline, &mut input).unwrap();
        let n = table
            .discover(reference, &input[..used], 0, &mut output)
            .unwrap();
        let Response::Page(page) =
            oal::decode_response(&output[..n], &request, deadline, 0).unwrap()
        else {
            panic!("resource page missing")
        };
        assert_eq!(page.count(), 1);
        let entry = page.entries().next().unwrap();
        assert_eq!(entry.name, expected);
        assert_eq!(
            Info::decode(entry.info.unwrap()).unwrap().reference,
            reference
        );
        assert_eq!(page.done(), round == 2);
        let next = page.next_cursor().to_vec();
        if round == 0 {
            old_cursor = next.clone();
        }
        cursor = next;
    }
    let request = Request::Enumerate {
        path: "",
        cursor: &old_cursor,
        max_entries: 1,
    };
    let used = oal::encode_request(&request, deadline, &mut input).unwrap();
    assert_eq!(
        table.discover(
            BindingRef::new(id, 2).unwrap(),
            &input[..used],
            0,
            &mut output
        ),
        Err(Status::CursorInvalid)
    );
    let root = Request::Lookup { path: "" };
    let used = oal::encode_request(&root, deadline, &mut input).unwrap();
    let n = table
        .discover(reference, &input[..used], 0, &mut output)
        .unwrap();
    let Response::Found(summary) = oal::decode_response(&output[..n], &root, deadline, 0).unwrap()
    else {
        panic!("resource root missing")
    };
    assert_eq!(
        Info::decode(summary).unwrap(),
        Info {
            reference,
            descriptor: None
        }
    );
    let truncated = &old_cursor[..BindingRef::ENCODED_LEN];
    let request = Request::Enumerate {
        path: "",
        cursor: truncated,
        max_entries: 1,
    };
    let used = oal::encode_request(&request, deadline, &mut input).unwrap();
    assert_eq!(
        table.discover(reference, &input[..used], 0, &mut output),
        Err(Status::CursorInvalid)
    );
}

#[test]
fn resource_names_admitted_at_the_limit_fit_an_oal_page() {
    use libdevice::resource::oal::MAX_RESOURCE_NAME;
    let descriptor = Descriptor {
        kind: ObjectKind::MemoryObject,
        rights: Rights::READ,
        protocol: 0,
    };
    let name = "a".repeat(MAX_RESOURCE_NAME);
    let (_, account) = funding(1, MAX_RESOURCE_NAME + 8192);
    let mut collecting = Collecting::reserve(
        Limits {
            entries: 2,
            text_bytes: (MAX_RESOURCE_NAME + 1) as u32,
        },
        libdevice::protocol::RESOURCE_HEADER_LEN,
        &account,
    )
    .unwrap();
    collecting.prepared(2).unwrap();
    collecting
        .push(
            &Entry {
                index: 0,
                descriptor,
                name: &name,
                metadata: &[],
            },
            0,
        )
        .unwrap();
    collecting
        .push(
            &Entry {
                index: 1,
                descriptor,
                name: "z",
                metadata: &[],
            },
            1,
        )
        .unwrap();
    let too_long = "a".repeat(MAX_RESOURCE_NAME + 1);
    assert_eq!(
        Entry {
            index: 0,
            descriptor,
            name: &too_long,
            metadata: &[]
        }
        .encoded_len(),
        Err(Error::Invalid)
    );
    let table = collecting.finish().ok().unwrap();
    let deadline = erhino_shared::time::Deadline::at(100);
    let request = liboal::protocol::Request::Enumerate {
        path: "",
        cursor: &[],
        max_entries: 1,
    };
    let mut input = [0; liboal::protocol::HEADER_LEN + liboal::protocol::BODY_MAX];
    let mut output = [0; liboal::protocol::HEADER_LEN + liboal::protocol::BODY_MAX];
    let used = liboal::protocol::encode_request(&request, deadline, &mut input).unwrap();
    let reference = BindingRef::new(DeviceId::new(33, 2).unwrap(), 1).unwrap();
    let n = table
        .discover(reference, &input[..used], 0, &mut output)
        .unwrap();
    let liboal::protocol::Response::Page(page) =
        liboal::protocol::decode_response(&output[..n], &request, deadline, 0).unwrap()
    else {
        panic!("resource page missing")
    };
    assert_eq!(page.entries().next().unwrap().name, name);
    assert!(!page.done());
}
