//! 期望来自设备 wire 和供应安全合同；不以 roundtrip 自身作为唯一正确性证据。
use erhino_shared::{
    object::{ObjectKind, Rights},
    time::Deadline,
};
use libbudget::{Budget, Taxonomy};
use libdevice::{
    Error,
    binding::{Phase, Snapshot, Subject},
    funding::Funding,
    identity::{BindingRef, DeviceId},
    protocol::{self, Command, Header, Progress, ProvisionCommand},
    registry::{Access, DeviceInfo, Scope},
    resource::{Descriptor, DeviceResource, Entry, Limits},
};
use librpc::bytes::Writer;

#[test]
fn register_wire_has_exact_fields_and_requires_one_business_capability() {
    let parent = DeviceId::new(7, 2).unwrap();
    let command = Command::Register {
        seq: 3,
        parent: Some(parent),
        name: "xhci",
        identity: &[9, 8],
        limits: Limits {
            entries: 12,
            text_bytes: 256,
        },
        claimable: true,
    };
    let mut bytes = [0; 512];
    let used = command.encode(Deadline::INFINITE, &mut bytes).unwrap();
    assert_eq!(used, 32 + 44 + 4 + 2);
    assert_eq!(&bytes[..4], &[1, 0, 3, 0]);
    assert_eq!(&bytes[32..40], &[3, 0, 0, 0, 0, 0, 0, 0]);
    assert_eq!(&bytes[56..68], &[12, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0]);
    assert_eq!(
        &bytes[68..used],
        &[4, 0, 0, 0, b'x', b'h', b'c', b'i', 2, 0, 0, 0, 9, 8]
    );
    let (_, decoded) = Command::decode(&bytes[..used], 1).unwrap();
    assert!(
        matches!(decoded,Command::Register {seq:3,parent:Some(id),name:"xhci",identity:[9,8],limits:Limits {entries:12,text_bytes:256},claimable:true} if id==parent)
    );
    assert!(matches!(
        Command::decode(&bytes[..used], 0),
        Err(Error::Invalid)
    ));
    assert!(matches!(
        Command::decode(&bytes[..used], 2),
        Err(Error::Invalid)
    ));
    assert!(Command::decode(&bytes[..used - 1], 1).is_err());
    assert!(Command::decode(&bytes[..used + 1], 1).is_err());
    bytes[64] = 2;
    assert!(matches!(
        Command::decode(&bytes[..used], 1),
        Err(Error::Invalid)
    ));
}

#[test]
fn malformed_scope_and_control_payloads_are_rejected() {
    let mut bytes = [0; 128];
    let command = Command::IssueSource {
        scope: Scope::Root,
        access: Access::OBSERVE,
    };
    let used = command.encode(Deadline::INFINITE, &mut bytes).unwrap();
    assert_eq!(
        &bytes[32..52],
        &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0]
    );
    bytes[32] = 1; // incarnation/serial 必须同时为零或同时有效。
    assert!(Command::decode(&bytes[..used], 0).is_err());
    bytes[32] = 0;
    bytes[48] = 0x80;
    assert!(Command::decode(&bytes[..used], 0).is_err());
    let used = Command::StopBinding
        .encode(Deadline::INFINITE, &mut bytes)
        .unwrap();
    assert_eq!(used, 32);
    assert!(matches!(
        Command::decode(&bytes[..used], 1),
        Err(Error::Invalid)
    ));
    bytes[12] = 1;
    assert!(matches!(
        Command::decode(&bytes[..used], 0),
        Err(Error::Invalid)
    ));
    bytes[12] = 0;
    bytes[20] = 1;
    assert!(matches!(
        Command::decode(&bytes[..used], 0),
        Err(Error::Invalid)
    ));
}

#[test]
fn registration_cannot_admit_a_description_that_query_cannot_return() {
    let mut bytes = vec![0; protocol::HEADER_LEN + protocol::BODY_MAX];
    let name = "r";
    let exact = protocol::BODY_MAX - protocol::DEVICE_FIXED_LEN - name.len();
    let identity = vec![7; exact + 1];
    let valid = Command::Register {
        seq: 1,
        parent: None,
        name,
        identity: &identity[..exact],
        limits: Limits {
            entries: 0,
            text_bytes: 0,
        },
        claimable: false,
    };
    let used = valid.encode(Deadline::INFINITE, &mut bytes).unwrap();
    assert!(Command::decode(&bytes[..used], 1).is_ok());
    let oversized = Command::Register {
        seq: 1,
        parent: None,
        name,
        identity: &identity,
        limits: Limits {
            entries: 0,
            text_bytes: 0,
        },
        claimable: false,
    };
    assert_eq!(
        oversized.encode(Deadline::INFINITE, &mut bytes),
        Err(Error::Invalid)
    );
    let id = DeviceId::new(1, 1).unwrap();
    let info = DeviceInfo {
        id,
        parent: None,
        name,
        identity: &identity[..exact],
        limits: Limits {
            entries: 0,
            text_bytes: 0,
        },
        present: true,
        epoch: None,
        binding: None,
    };
    let used = protocol::encode_device(&info, &mut bytes).unwrap();
    assert_eq!(used, bytes.len());
    assert_eq!(
        protocol::decode_device(&bytes[..used], id, 0)
            .unwrap()
            .identity
            .len(),
        exact
    );
}

#[test]
fn device_query_binds_identity_and_checks_active_binding_shape() {
    let id = DeviceId::new(10, 4).unwrap();
    let binding = Snapshot {
        reference: BindingRef::new(id, 3).unwrap(),
        subject: Subject::from_authenticated_context(17).unwrap(),
        phase: Phase::ResourcesReady,
    };
    let info = DeviceInfo {
        id,
        parent: Some(DeviceId::new(10, 1).unwrap()),
        name: "keyboard",
        identity: &[3, 1, 1],
        limits: Limits {
            entries: 1,
            text_bytes: 32,
        },
        present: true,
        epoch: Some(3),
        binding: Some(binding),
    };
    let mut bytes = [0; 256];
    let used = protocol::encode_device(&info, &mut bytes).unwrap();
    let result = protocol::decode_device(&bytes[..used], id, 0).unwrap();
    assert_eq!(result.binding, Some(binding));
    assert_eq!(result.name, "keyboard");
    assert!(matches!(
        protocol::decode_device(&bytes[..used], DeviceId::new(10, 5).unwrap(), 0),
        Err(Error::StaleInstance)
    ));
    assert!(matches!(
        protocol::decode_device(&bytes[..used], id, 1),
        Err(Error::Invalid)
    ));
    // 当前设备只表达活动绑定；Released 仅属于原控制壳，不能成为新轮当前态。
    bytes[84..88].copy_from_slice(&4u32.to_le_bytes());
    assert!(matches!(
        protocol::decode_device(&bytes[..used], id, 0),
        Err(Error::Invalid)
    ));
}

#[test]
fn provision_result_must_match_operation_epoch_and_capability_count() {
    let mut bytes = [0; 128];
    let used = Progress::Prepared(12)
        .encode(ProvisionCommand::Prepare(9), &mut bytes)
        .unwrap();
    assert_eq!(
        &bytes[32..48],
        &[9, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 12, 0, 0, 0]
    );
    assert_eq!(
        Progress::decode(&bytes[..used], ProvisionCommand::Prepare(9), 0),
        Ok(Progress::Prepared(12))
    );
    assert_eq!(
        Progress::decode(&bytes[..used], ProvisionCommand::Prepare(10), 0),
        Err(Error::StaleEpoch)
    );
    assert_eq!(
        Progress::decode(&bytes[..used], ProvisionCommand::Stop(9), 0),
        Err(Error::Invalid)
    );
    assert_eq!(
        Progress::decode(&bytes[..used], ProvisionCommand::Prepare(9), 1),
        Err(Error::Invalid)
    );
    assert_eq!(
        Progress::Rejected(Error::Unsupported).encode(ProvisionCommand::Stop(9), &mut bytes),
        Err(Error::Invalid)
    );
    let used = Progress::Rejected(Error::Unsupported)
        .encode(ProvisionCommand::Prepare(9), &mut bytes)
        .unwrap();
    bytes[2..4].copy_from_slice(&3u16.to_le_bytes());
    assert_eq!(
        Progress::decode(&bytes[..used], ProvisionCommand::Stop(9), 0),
        Err(Error::Invalid)
    );
}

#[test]
fn unavailable_not_found_and_error_closed_never_become_safe_closed() {
    let mut bytes = [0; 128];
    for error in [Error::Unavailable, Error::NotFound, Error::Closed] {
        let used = Header {
            op: 3,
            error: Some(error),
            deadline: Deadline::INFINITE,
        }
        .encode(&[], &mut bytes)
        .unwrap();
        assert_eq!(
            Progress::decode(&bytes[..used], ProvisionCommand::Stop(1), 0),
            Err(error)
        );
        assert_eq!(
            Progress::decode(&bytes[..used], ProvisionCommand::Prepare(1), 0),
            Err(Error::Invalid)
        );
    }
    let used = Progress::Failed(Error::Resource)
        .encode(ProvisionCommand::Stop(1), &mut bytes)
        .unwrap();
    assert_eq!(
        Progress::decode(&bytes[..used], ProvisionCommand::Stop(1), 0),
        Ok(Progress::Failed(Error::Resource))
    );
    let used = Progress::Closed
        .encode(ProvisionCommand::Stop(1), &mut bytes)
        .unwrap();
    assert_eq!(
        Progress::decode(&bytes[..used], ProvisionCommand::Stop(1), 0),
        Ok(Progress::Closed)
    );
}

#[test]
fn prepared_resource_reply_pairs_epoch_index_metadata_and_one_capability() {
    let descriptor = Descriptor {
        kind: ObjectKind::MmioUse,
        rights: Rights::READ | Rights::WRITE,
        protocol: 0,
    };
    let entry = Entry {
        index: 2,
        descriptor,
        name: "bar",
        metadata: &[4, 5],
    };
    let mut body = [0; 128];
    let mut w = Writer::new(&mut body);
    w.u64(3);
    let used = entry
        .encode(protocol::RESOURCE_HEADER_LEN, &mut body[8..])
        .unwrap()
        + 8;
    let mut bytes = [0; 256];
    let used = Header {
        op: 2,
        error: None,
        deadline: Deadline::INFINITE,
    }
    .encode(&body[..used], &mut bytes)
    .unwrap();
    let resource = protocol::decode_resource(&bytes[..used], 3, 2, 1).unwrap();
    assert_eq!(resource.name, "bar");
    assert_eq!(resource.metadata, &[4, 5]);
    assert!(matches!(
        protocol::decode_resource(&bytes[..used], 4, 2, 1),
        Err(Error::StaleEpoch)
    ));
    assert!(matches!(
        protocol::decode_resource(&bytes[..used], 3, 1, 1),
        Err(Error::Conflict)
    ));
    assert!(matches!(
        protocol::decode_resource(&bytes[..used], 3, 2, 0),
        Err(Error::Invalid)
    ));
    assert!(matches!(
        protocol::decode_resource(&bytes[..used], 3, 2, 2),
        Err(Error::Invalid)
    ));
}

#[test]
fn funding_never_combines_domain_and_execution_from_different_accounts() {
    let limits = [64; DeviceResource::COUNT + libexecution::ExecutionResource::COUNT];
    let budget = Budget::new(&limits, 2).unwrap();
    let a = budget.account(&limits).unwrap();
    let b = budget.account(&limits).unwrap();
    let domain_slots: [_; DeviceResource::COUNT] =
        core::array::from_fn(|i| budget.slot(i).unwrap());
    let execution_slots: [_; libexecution::ExecutionResource::COUNT] =
        core::array::from_fn(|i| budget.slot(DeviceResource::COUNT + i).unwrap());
    let domain = a.view(&domain_slots).unwrap();
    assert!(matches!(
        Funding::new(domain.clone(), b.view(&execution_slots).unwrap()),
        Err(Error::Permission)
    ));
    let paired = Funding::new(domain, a.view(&execution_slots).unwrap()).unwrap();
    assert_eq!(
        paired.domain().account_id(),
        paired.execution().account_id()
    );
}

#[test]
fn discovery_cursor_and_capability_arity_are_not_interchangeable() {
    use libdevice::registry::Scan;
    let id = DeviceId::new(42, 4).unwrap();
    let mut body = [0; 32];
    protocol::encode_scan(
        Scan {
            end: 8,
            next: 4,
            device: Some(id),
        },
        &mut body,
    )
    .unwrap();
    assert!(protocol::decode_scan(&body, 4, 8).is_err());
    assert!(protocol::decode_scan(&body, 0, 7).is_err());
    assert_eq!(protocol::decode_scan(&body, 0, 8).unwrap().device, Some(id));
    let mut bytes = [0; 256];
    let install = Command::InstallFunction {
        name: "keyboard",
        description: b"boot keyboard",
    };
    let used = install.encode(Deadline::INFINITE, &mut bytes).unwrap();
    assert!(Command::decode(&bytes[..used], 0).is_err());
    assert!(Command::decode(&bytes[..used], 1).is_ok());
    assert!(Command::decode(&bytes[..used], 2).is_err());
    let used = Command::WatchScope { scope: Scope::Root }
        .encode(Deadline::INFINITE, &mut bytes)
        .unwrap();
    assert!(Command::decode(&bytes[..used], 1).is_err());
    assert!(
        Command::AckWatch { sequence: 0 }
            .encode(Deadline::INFINITE, &mut bytes)
            .is_err()
    );
}

#[test]
fn watch_errors_keep_their_status_and_terminal_sequence_cannot_be_forged() {
    use libdevice::watch::{Info, Reason, TERMINAL};
    let mut bytes = [0; 128];
    for op in [15, 17] {
        for error in [Error::Quota, Error::Permission, Error::Closed] {
            let used = Header {
                op,
                error: Some(error),
                deadline: Deadline::INFINITE,
            }
            .encode(&[], &mut bytes)
            .unwrap();
            assert_eq!(protocol::decode_watch(&bytes[..used], op, 0), Err(error));
            assert_eq!(
                protocol::decode_watch(&bytes[..used], op, 2),
                Err(Error::Invalid)
            );
        }
    }
    let forged = Info {
        sequence: 7,
        pending: TERMINAL,
        reason: Reason::Exhausted,
    };
    let mut body = [0; Info::ENCODED_LEN];
    forged.encode(&mut body).unwrap();
    let used = Header {
        op: 17,
        error: None,
        deadline: Deadline::INFINITE,
    }
    .encode(&body, &mut bytes)
    .unwrap();
    assert_eq!(
        protocol::decode_watch(&bytes[..used], 17, 0),
        Err(Error::Invalid)
    );
}

#[test]
fn source_stop_and_claim_stop_do_not_confuse_errors_with_safe_closed() {
    use libdevice::{binding::Stop, registry::SourceStop};
    let reference = BindingRef::new(DeviceId::new(7, 2).unwrap(), 3).unwrap();
    let snapshot = Snapshot {
        reference,
        subject: Subject::from_authenticated_context(10).unwrap(),
        phase: Phase::Stopping,
    };
    let mut body = [0; 64];
    let mut wire = [0; 128];
    let used = protocol::encode_stop_claim(Stop::Binding(snapshot), reference, &mut body).unwrap();
    assert_eq!(used, 40);
    let used = Header {
        op: 7,
        error: None,
        deadline: Deadline::INFINITE,
    }
    .encode(&body[..used], &mut wire)
    .unwrap();
    assert_eq!(
        protocol::decode_stop_claim(&wire[..used], reference, 0),
        Ok(Stop::Binding(snapshot))
    );
    assert!(protocol::decode_stop_claim(&wire[..used], reference, 1).is_err());
    let old = BindingRef::new(reference.device(), 2).unwrap();
    assert_eq!(
        protocol::decode_stop_claim(&wire[..used], old, 0),
        Err(Error::StaleEpoch)
    );
    let used = protocol::encode_stop_claim(Stop::Closed, reference, &mut body).unwrap();
    assert_eq!(used, 28);
    let used = Header {
        op: 7,
        error: None,
        deadline: Deadline::INFINITE,
    }
    .encode(&body[..used], &mut wire)
    .unwrap();
    assert_eq!(
        protocol::decode_stop_claim(&wire[..used], reference, 0),
        Ok(Stop::Closed)
    );
    for op in [7, 25] {
        let used = Header {
            op,
            error: Some(Error::Closed),
            deadline: Deadline::INFINITE,
        }
        .encode(&[], &mut wire)
        .unwrap();
        if op == 7 {
            assert_eq!(
                protocol::decode_stop_claim(&wire[..used], reference, 0),
                Err(Error::Closed)
            );
        } else {
            assert_eq!(
                protocol::decode_source_stop(&wire[..used], 0),
                Err(Error::Closed)
            );
        }
    }
    for (code, status) in [
        (0u32, SourceStop::Open),
        (1, SourceStop::Stopping),
        (2, SourceStop::Closed),
    ] {
        let used = Header {
            op: 25,
            error: None,
            deadline: Deadline::INFINITE,
        }
        .encode(&code.to_le_bytes(), &mut wire)
        .unwrap();
        assert_eq!(protocol::decode_source_stop(&wire[..used], 0), Ok(status));
    }
    let used = Header {
        op: 7,
        error: None,
        deadline: Deadline::INFINITE,
    }
    .encode(&[], &mut wire)
    .unwrap();
    assert!(protocol::decode_stop_claim(&wire[..used], reference, 0).is_err());
}
