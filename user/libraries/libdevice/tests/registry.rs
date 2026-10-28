//! 基于设备计划的权限、Unknown、祖先撤出和身份退休反例。
//! host 使用整数作为 owned payload，不宣称执行了 IPC、设备或 Runtime 任务。

use erhino_shared::object::{HandleDescription, ObjectKind, Rights};
use libbudget::{AccountView, Budget, Taxonomy};
use libdevice::{
    Error,
    binding::{Claim, Commit, Phase, Stop},
    funding::Funding,
    identity::{BindingRef, DeviceId},
    registry::{
        Access, Config, EndpointIdentity, RegisterCommit, Registration, Registry, Scope, SourceIds,
    },
    resource::{DeviceResource, Limits},
};
use std::sync::Arc;

const ROOT: u64 = 10;
const ROOT_ADMIN: u64 = 11;
fn setup() -> (Registry<u64>, Arc<Budget>, AccountView<DeviceResource>) {
    let limits = [
        32,
        32,
        128 * 1024,
        16,
        64,
        32,
        32,
        128 * 1024,
        32,
        32,
        64,
        128 * 1024,
    ];
    let budget = Budget::new(&limits, 1).unwrap();
    let slots: [_; DeviceResource::COUNT] = core::array::from_fn(|n| budget.slot(n).unwrap());
    let payer = budget.account(&limits).unwrap();
    let account = payer.view(&slots).unwrap();
    let funding = Funding::new(
        account.clone(),
        payer
            .view(&[
                budget.slot(DeviceResource::COUNT).unwrap(),
                budget.slot(DeviceResource::COUNT + 1).unwrap(),
            ])
            .unwrap(),
    )
    .unwrap();
    let mut registry = Registry::new(
        42,
        Config {
            sources: 16,
            devices: 32,
            watches: 32,
            functions: 32,
        },
    )
    .unwrap();
    registry
        .bootstrap(
            SourceIds {
                use_context: ROOT,
                admin_context: ROOT_ADMIN,
            },
            1,
            funding,
        )
        .unwrap();
    (registry, budget, account)
}
fn request(seq: u64, parent: Option<DeviceId>, name: &str, claimable: bool) -> Registration<'_> {
    Registration {
        seq,
        parent,
        name,
        identity: b"immutable identity",
        claimable,
        stop_requirement: libdevice::binding::StopRequirement::ProviderOwned,
        limits: Limits {
            entries: 4,
            text_bytes: 256,
        },
        endpoint: EndpointIdentity::from_description(&HandleDescription {
            object_id: 1000 + seq,
            related_object_id: 2000,
            kind: ObjectKind::MailboxSender as u32,
            reserved0: 0,
            rights: Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
            badge: 0,
            reserved: 0,
        })
        .unwrap(),
    }
}
fn register(registry: &mut Registry<u64>, context: u64, request: &Registration<'_>) -> DeviceId {
    let candidate = registry
        .prepare_registration(context, request, request.seq, 7)
        .unwrap();
    match registry.commit_registration(candidate) {
        Ok(RegisterCommit::Created(id)) => id,
        _ => panic!("registration failed"),
    }
}
fn source(registry: &mut Registry<u64>, context: u64, scope: Scope, access: Access) {
    registry
        .derive_source(
            ROOT,
            SourceIds {
                use_context: context,
                admin_context: context + 1,
            },
            3,
            scope,
            access,
        )
        .unwrap();
}

#[test]
fn observer_and_source_admin_do_not_gain_claim_or_registration() {
    let (mut registry, _, _) = setup();
    let id = register(&mut registry, ROOT, &request(1, None, "host", true));
    source(&mut registry, 20, Scope::Subtree(id), Access::OBSERVE);
    let reference = BindingRef::new(id, 1).unwrap();
    assert!(registry.query(20, id).unwrap().present);
    assert_eq!(registry.claim(20, reference), Err(Error::Permission));
    assert_eq!(registry.claim(21, reference), Err(Error::Permission));
    assert!(matches!(
        registry.prepare_registration(20, &request(1, Some(id), "child", true), 1, 2),
        Err(Error::Permission)
    ));
    assert_eq!(
        registry.derive_source(
            20,
            SourceIds {
                use_context: 30,
                admin_context: 31
            },
            3,
            Scope::Subtree(id),
            Access::CLAIM
        ),
        Err(Error::Permission)
    );
    assert_eq!(registry.claim(0, reference), Err(Error::Permission));
    assert_eq!(registry.seal_source(20), Err(Error::Permission));
    assert_eq!(registry.seal_source(21), Ok(3));
}

#[test]
fn scopes_only_narrow_and_withdraw_blocks_all_descendants_before_scan() {
    let (mut registry, _, _) = setup();
    let parent = register(&mut registry, ROOT, &request(1, None, "parent", false));
    let sibling = register(&mut registry, ROOT, &request(2, None, "sibling", true));
    source(&mut registry, 20, Scope::Subtree(parent), Access::ALL);
    let child = register(&mut registry, 20, &request(1, Some(parent), "child", true));
    let candidate = registry
        .prepare_registration(20, &request(2, Some(child), "late", true), 2, 9)
        .unwrap();
    assert_eq!(
        registry.derive_source(
            20,
            SourceIds {
                use_context: 30,
                admin_context: 31
            },
            3,
            Scope::Root,
            Access::OBSERVE
        ),
        Err(Error::Permission)
    );
    assert_eq!(
        registry.derive_source(
            20,
            SourceIds {
                use_context: 30,
                admin_context: 31
            },
            3,
            Scope::Subtree(sibling),
            Access::OBSERVE
        ),
        Err(Error::Permission)
    );
    assert!(matches!(
        registry.prepare_registration(20, &request(2, None, "escaped", true), 1, 2),
        Err(Error::Permission)
    ));
    assert!(matches!(
        registry.query(20, sibling),
        Err(Error::Permission)
    ));
    registry.withdraw_publication(ROOT, 1).unwrap();
    assert_eq!(
        registry.claim(20, BindingRef::new(child, 1).unwrap()),
        Err(Error::Closed)
    );
    assert!(!registry.query(20, child).unwrap().present);
    assert_eq!(
        registry.commit_registration(candidate).err().unwrap().error,
        Error::Closed
    );
    assert_eq!(
        registry.derive_source(
            20,
            SourceIds {
                use_context: 30,
                admin_context: 31
            },
            3,
            Scope::Subtree(child),
            Access::OBSERVE
        ),
        Err(Error::Closed)
    );
}

#[test]
fn competing_registration_candidates_recover_one_identity_and_one_charge() {
    let (mut registry, budget, account) = setup();
    let request = request(1, None, "host", true);
    let a = registry.prepare_registration(ROOT, &request, 1, 7).unwrap();
    let b = registry.prepare_registration(ROOT, &request, 1, 8).unwrap();
    assert_eq!(account.usage(DeviceResource::Publication).0, 2);
    let id = match registry.commit_registration(a) {
        Ok(RegisterCommit::Created(id)) => id,
        _ => panic!("first commit failed"),
    };
    assert_eq!(
        registry.commit_registration(b).ok(),
        Some(RegisterCommit::Existing(id))
    );
    assert_eq!(account.usage(DeviceResource::Publication).0, 1);
    assert_eq!(registry.existing_registration(ROOT, &request), Ok(Some(id)));
    let conflicting = Registration {
        identity: b"different",
        ..request
    };
    assert_eq!(
        registry.existing_registration(ROOT, &conflicting),
        Err(Error::Conflict)
    );
    registry.withdraw_publication(ROOT, 1).unwrap();
    registry.control_closed(id).unwrap();
    assert_eq!(registry.retire_device(id), Ok(1));
    registry.role_closed(ROOT).unwrap();
    registry.role_closed(ROOT_ADMIN).unwrap();
    registry.retire_source(ROOT).unwrap();
    assert!(budget.is_empty());
}

#[test]
fn ancestor_seal_retains_old_binding_owner_and_prevents_late_ready() {
    let (mut registry, _, _) = setup();
    let id = register(&mut registry, ROOT, &request(1, None, "host", true));
    source(&mut registry, 20, Scope::Subtree(id), Access::ALL);
    registry
        .derive_source(
            20,
            SourceIds {
                use_context: 30,
                admin_context: 31,
            },
            4,
            Scope::Subtree(id),
            Access::CLAIM.union(Access::OBSERVE),
        )
        .unwrap();
    let reference = BindingRef::new(id, 1).unwrap();
    assert!(matches!(
        registry.commit_claim(30, reference, 55),
        Ok(Commit::Created(_))
    ));
    assert!(matches!(
        registry.commit_claim(30, reference, 99),
        Ok(Commit::Existing(_))
    ));
    registry.seal_source(21).unwrap();
    assert!(matches!(
        registry.claim(30, reference),
        Ok(Claim::Existing(_))
    ));
    assert_eq!(registry.resources_ready(reference), Err(Error::Closed));
    let end = registry.scan_end();
    assert_eq!(
        registry.maintenance(0, end).unwrap(),
        Some((id.serial(), Some(55)))
    );
    assert_eq!(
        registry.query(ROOT, id).unwrap().binding.unwrap().phase,
        Phase::Stopping
    );
    registry.role_closed(30).unwrap();
    registry.role_closed(31).unwrap();
    assert_eq!(registry.retire_source(30), Err(Error::Busy));
    registry.release_binding(reference).unwrap();
    registry.retire_source(30).unwrap();
    assert_eq!(registry.retire_source(20), Err(Error::Busy));
    // 席位仍存在，其他有效来源可取得下一轮；旧主体的数字不复活旧权限。
    assert!(matches!(
        registry.commit_claim(ROOT, BindingRef::new(id, 2).unwrap(), 56),
        Ok(Commit::Created(_))
    ));
}

#[test]
fn parent_and_scope_identity_outlive_all_child_records_and_controls() {
    let (mut registry, budget, _) = setup();
    let parent = register(&mut registry, ROOT, &request(1, None, "parent", false));
    let child = register(
        &mut registry,
        ROOT,
        &request(2, Some(parent), "child", true),
    );
    source(&mut registry, 20, Scope::Subtree(child), Access::CLAIM);
    registry.withdraw_publication(ROOT, 1).unwrap();
    registry.control_closed(parent).unwrap();
    registry.control_closed(child).unwrap();
    assert_eq!(registry.retire_device(parent), Err(Error::Busy));
    let end = registry.scan_end();
    registry.maintenance(parent.serial(), end).unwrap();
    assert_eq!(registry.retire_device(child), Err(Error::Busy));
    registry.role_closed(20).unwrap();
    registry.role_closed(21).unwrap();
    registry.retire_source(20).unwrap();
    assert_eq!(registry.retire_device(child), Ok(2));
    assert_eq!(registry.retire_device(parent), Ok(1));
    registry.role_closed(ROOT).unwrap();
    registry.role_closed(ROOT_ADMIN).unwrap();
    registry.retire_source(ROOT).unwrap();
    assert!(budget.is_empty());
}

#[test]
fn unknown_registration_cancel_and_name_reuse_never_restore_old_id() {
    let (mut registry, _, _) = setup();
    let old = registry
        .prepare_registration(ROOT, &request(1, None, "host", true), 1, 7)
        .unwrap();
    assert_eq!(registry.withdraw_publication(ROOT, 1), Ok(None));
    assert_eq!(
        registry.commit_registration(old).err().unwrap().error,
        Error::Closed
    );
    let id = register(&mut registry, ROOT, &request(2, None, "host", true));
    registry.withdraw_publication(ROOT, 2).unwrap();
    let next = register(&mut registry, ROOT, &request(3, None, "host", true));
    assert_ne!(id, next);
    assert!(!registry.query(ROOT, id).unwrap().present);
    assert!(registry.query(ROOT, next).unwrap().present);
    assert_eq!(
        registry.stop_claim(ROOT, BindingRef::new(id, 1).unwrap()),
        Ok(Stop::Closed)
    );
    assert_eq!(
        registry.claim(ROOT, BindingRef::new(next, 1).unwrap()),
        Ok(Claim::Vacant)
    );
}

#[test]
fn source_failure_is_atomic_and_does_not_consume_context_or_budget() {
    let limits = [4, 4, 32768, 1, 4, 4, 4, 32768, 4, 4, 64, 32768];
    let budget = Budget::new(&limits, 1).unwrap();
    let slots: [_; DeviceResource::COUNT] = core::array::from_fn(|n| budget.slot(n).unwrap());
    let payer = budget.account(&limits).unwrap();
    let account = payer.view(&slots).unwrap();
    let funding = Funding::new(
        account.clone(),
        payer
            .view(&[
                budget.slot(DeviceResource::COUNT).unwrap(),
                budget.slot(DeviceResource::COUNT + 1).unwrap(),
            ])
            .unwrap(),
    )
    .unwrap();
    let mut registry: Registry<()> = Registry::new(
        42,
        Config {
            sources: 4,
            devices: 4,
            watches: 4,
            functions: 4,
        },
    )
    .unwrap();
    registry
        .bootstrap(
            SourceIds {
                use_context: ROOT,
                admin_context: ROOT_ADMIN,
            },
            1,
            funding,
        )
        .unwrap();
    let paid = account.usage(DeviceResource::Bytes).0;
    assert_eq!(
        registry.derive_source(
            ROOT,
            SourceIds {
                use_context: 20,
                admin_context: 21
            },
            3,
            Scope::Root,
            Access::OBSERVE
        ),
        Err(Error::Quota)
    );
    assert_eq!(account.usage(DeviceResource::Bytes).0, paid);
    assert_eq!(registry.seal_source(21), Err(Error::Permission));
    assert_eq!(
        registry.derive_source(
            ROOT,
            SourceIds {
                use_context: ROOT_ADMIN,
                admin_context: 21
            },
            3,
            Scope::Root,
            Access::OBSERVE
        ),
        Err(Error::Conflict)
    );
}

#[test]
fn vacant_stop_before_maintenance_cannot_issue_epochs_after_effective_withdrawal() {
    for seal_registrar in [false, true] {
        let (mut registry, _, _) = setup();
        let parent = register(&mut registry, ROOT, &request(1, None, "parent", false));
        source(
            &mut registry,
            20,
            Scope::Subtree(parent),
            Access::REGISTER.union(Access::CLAIM).union(Access::OBSERVE),
        );
        let child = register(&mut registry, 20, &request(1, Some(parent), "child", true));
        if seal_registrar {
            registry.seal_source(21).unwrap();
        } else {
            registry.withdraw_publication(ROOT, 1).unwrap();
        }
        let current = BindingRef::new(child, 1).unwrap();
        let future = BindingRef::new(child, 2).unwrap();
        // 故意不调用 maintenance，覆盖准入已闭而本地 Seat 尚未接到扫描的窗口。
        assert_eq!(registry.claim(20, current), Err(Error::Closed));
        assert_eq!(registry.stop_claim(20, current), Ok(Stop::Closed));
        assert_eq!(registry.stop_claim(20, future), Err(Error::InvalidEpoch));
        assert_eq!(registry.query(20, child).unwrap().epoch, Some(1));
    }
}

#[test]
fn binding_release_before_maintenance_does_not_issue_withdrawn_future_epoch() {
    for seal_registrar in [false, true] {
        let (mut registry, _, _) = setup();
        let parent = register(&mut registry, ROOT, &request(1, None, "parent", false));
        source(
            &mut registry,
            20,
            Scope::Subtree(parent),
            Access::REGISTER.union(Access::CLAIM).union(Access::OBSERVE),
        );
        let child = register(&mut registry, 20, &request(1, Some(parent), "child", true));
        let current = BindingRef::new(child, 1).unwrap();
        registry.commit_claim(20, current, 55).unwrap();
        registry.resources_ready(current).unwrap();
        registry.stop_claim(20, current).unwrap();
        if seal_registrar {
            registry.seal_source(21).unwrap();
        } else {
            registry.withdraw_publication(ROOT, 1).unwrap();
        }
        // Stop 后、实际 Closed 到来前撤祖先，Release 本身也必须复核有效撤出。
        assert_eq!(
            registry.release_binding(current).unwrap().phase,
            Phase::Released
        );
        assert_eq!(registry.query(20, child).unwrap().epoch, Some(1));
        assert_eq!(
            registry.stop_claim(20, BindingRef::new(child, 2).unwrap()),
            Err(Error::InvalidEpoch)
        );
    }
}

#[test]
fn scan_has_a_finite_end_and_never_crosses_authority_scope() {
    let (mut registry, _, _) = setup();
    let a = register(&mut registry, ROOT, &request(1, None, "a", true));
    let b = register(&mut registry, ROOT, &request(2, None, "b", true));
    source(&mut registry, 20, Scope::Subtree(a), Access::OBSERVE);
    let first = registry.enumerate(ROOT, Scope::Root, 0, 0).unwrap();
    assert_eq!(first.device, Some(a));
    let c = register(&mut registry, ROOT, &request(3, None, "c", true));
    let second = registry
        .enumerate(ROOT, Scope::Root, first.next, first.end)
        .unwrap();
    assert_eq!(second.device, Some(b));
    assert!(
        registry
            .enumerate(ROOT, Scope::Root, second.next, first.end)
            .unwrap()
            .device
            .is_none()
    );
    assert_eq!(
        registry.enumerate(20, Scope::Root, 0, 0),
        Err(Error::Permission)
    );
    assert_eq!(
        registry.enumerate(20, Scope::Subtree(b), 0, 0),
        Err(Error::Permission)
    );
    assert_eq!(
        registry
            .enumerate(20, Scope::Subtree(a), 0, 0)
            .unwrap()
            .device,
        Some(a)
    );
    assert!(
        registry
            .enumerate(ROOT, Scope::Root, b.serial(), 0)
            .is_err()
    );
    assert!(
        registry
            .enumerate(ROOT, Scope::Root, 0, c.serial() + 1)
            .is_err()
    );
}

#[test]
fn scoped_watch_covers_changes_after_query_and_withdrawn_binding_release() {
    use libdevice::{registry::WatchTarget, watch::Reason};
    let (mut registry, _, _) = setup();
    let a = register(&mut registry, ROOT, &request(1, None, "a", true));
    let b = register(&mut registry, ROOT, &request(2, None, "b", true));
    source(&mut registry, 20, Scope::Subtree(a), Access::OBSERVE);
    let initial = registry
        .create_watch(20, WatchTarget::Scope(Scope::Subtree(a)), 100, 10)
        .unwrap();
    registry.ack_watch(100, initial.sequence).unwrap();
    registry.control_withdraw(b).unwrap();
    assert_eq!(
        registry.query_watch(100).unwrap().sequence,
        initial.sequence
    );
    let reference = BindingRef::new(a, 1).unwrap();
    registry.commit_claim(ROOT, reference, 20).unwrap();
    let preparing = registry.query_watch(100).unwrap();
    registry.resources_ready(reference).unwrap();
    assert_ne!(
        registry.ack_watch(100, preparing.sequence).unwrap().pending,
        0
    );
    registry.control_withdraw(a).unwrap();
    let stopping = registry.query_watch(100).unwrap();
    assert_eq!(stopping.reason, Reason::Active);
    registry.release_binding(reference).unwrap();
    assert!(registry.query_watch(100).unwrap().sequence > stopping.sequence);
    registry.seal_source(21).unwrap();
    let terminal = registry.query_watch(100).unwrap();
    assert_eq!(terminal.reason, Reason::Revoked);
    assert_eq!(
        registry.ack_watch(100, terminal.sequence).unwrap().pending,
        0
    );
    registry.retire_watch(100).unwrap();
}

#[test]
fn binding_watch_terminates_on_its_own_release_and_does_not_follow_next_epoch() {
    use libdevice::{registry::WatchTarget, watch::Reason};
    let (mut registry, _, _) = setup();
    let a = register(&mut registry, ROOT, &request(1, None, "a", true));
    let reference = BindingRef::new(a, 1).unwrap();
    registry.commit_claim(ROOT, reference, 20).unwrap();
    let snapshot = registry.binding_snapshot(reference).unwrap();
    registry
        .create_watch(ROOT, WatchTarget::Binding(snapshot), 100, 10)
        .unwrap();
    registry.stop_claim(ROOT, reference).unwrap();
    assert!(!registry.query_watch(100).unwrap().terminal());
    registry.release_binding(reference).unwrap();
    let terminal = registry.query_watch(100).unwrap();
    assert_eq!(terminal.reason, Reason::Released);
    registry
        .commit_claim(ROOT, BindingRef::new(a, 2).unwrap(), 21)
        .unwrap();
    assert_eq!(registry.query_watch(100).unwrap(), terminal);
}

#[test]
fn released_control_can_create_terminal_watch_after_registry_identity_retires() {
    use libdevice::watch::Reason;
    let (mut registry, _, _) = setup();
    let id = register(&mut registry, ROOT, &request(1, None, "late-watch", true));
    let reference = BindingRef::new(id, 1).unwrap();
    registry.commit_claim(ROOT, reference, 20).unwrap();
    // 原控制壳保留稳定回执及付款者，不应为了后续观察而保活供应 payload。
    let funding = registry.funding(ROOT).unwrap().clone();
    registry.stop_claim(ROOT, reference).unwrap();
    let released = registry.release_binding(reference).unwrap();
    registry.withdraw_publication(ROOT, 1).unwrap();
    registry.control_closed(id).unwrap();
    assert_eq!(registry.retire_device(id), Ok(1));
    let info = registry
        .create_binding_watch(released, 100, 10, &funding)
        .unwrap();
    assert_eq!(info.reason, Reason::Released);
    assert!(info.terminal());
    assert_ne!(info.pending, 0);
    assert_eq!(registry.ack_watch(100, info.sequence).unwrap().pending, 0);
    registry.retire_watch(100).unwrap();
}

#[test]
fn child_registrar_is_bound_to_parent_epoch_and_cannot_register_outside_it() {
    let (mut registry, _, _) = setup();
    let host = register(&mut registry, ROOT, &request(1, None, "host", true));
    source(&mut registry, 20, Scope::Root, Access::CLAIM);
    let reference = BindingRef::new(host, 1).unwrap();
    registry.commit_claim(20, reference, 20).unwrap();
    registry.resources_ready(reference).unwrap();
    assert_eq!(
        registry.derive_child_source(
            20,
            reference,
            SourceIds {
                use_context: 100,
                admin_context: 101
            },
            30
        ),
        Err(Error::Permission)
    );
    registry.stop_claim(20, reference).unwrap();
    registry.release_binding(reference).unwrap();
    let reference = BindingRef::new(host, 2).unwrap();
    registry.commit_claim(ROOT, reference, 21).unwrap();
    registry.resources_ready(reference).unwrap();
    registry
        .derive_child_source(
            ROOT,
            reference,
            SourceIds {
                use_context: 100,
                admin_context: 101,
            },
            30,
        )
        .unwrap();
    assert!(matches!(
        registry.prepare_registration(100, &request(1, None, "escape", true), 1, 31),
        Err(Error::Permission)
    ));
    let child = register(&mut registry, 100, &request(1, Some(host), "child", true));
    registry.stop_claim(ROOT, reference).unwrap();
    assert!(!registry.query(ROOT, child).unwrap().present);
    assert!(matches!(
        registry.prepare_registration(100, &request(2, Some(host), "late", true), 1, 32),
        Err(Error::Closed)
    ));
    registry.release_binding(reference).unwrap();
    let next = BindingRef::new(host, 3).unwrap();
    registry.commit_claim(ROOT, next, 22).unwrap();
    registry.resources_ready(next).unwrap();
    assert!(matches!(
        registry.prepare_registration(100, &request(2, Some(host), "revived", true), 1, 32),
        Err(Error::Closed)
    ));
}

#[test]
fn function_boundary_uses_same_authority_and_stops_with_original_binding() {
    use erhino_shared::time::Deadline;
    use libdevice::registry::FunctionRegistration;
    use liboal::protocol as oal;
    let (mut registry, _, _) = setup();
    let host = register(&mut registry, ROOT, &request(1, None, "host", true));
    source(&mut registry, 20, Scope::Subtree(host), Access::OBSERVE);
    source(
        &mut registry,
        30,
        Scope::Subtree(host),
        Access::OBSERVE.union(Access::FUNCTIONS),
    );
    let reference = BindingRef::new(host, 1).unwrap();
    registry.commit_claim(ROOT, reference, 20).unwrap();
    registry.resources_ready(reference).unwrap();
    let endpoint = request(1, None, "unused", false).endpoint;
    registry
        .install_function(
            reference,
            FunctionRegistration {
                name: "keyboard",
                description: b"boot keyboard",
                endpoint,
            },
            99,
        )
        .unwrap();
    assert!(matches!(
        registry.open_function(20, reference, "keyboard"),
        Err(Error::Permission)
    ));
    assert_eq!(
        *registry
            .open_function(30, reference, "keyboard")
            .unwrap()
            .entry,
        99
    );
    let query = oal::Request::Lookup {
        path: "functions/keyboard/events",
    };
    let mut bytes = [0; oal::HEADER_LEN + oal::BODY_MAX];
    let used = oal::encode_request(&query, Deadline::INFINITE, &mut bytes).unwrap();
    let mut out = [0; oal::HEADER_LEN + oal::BODY_MAX];
    assert!(matches!(
        registry.discover(20, &bytes[..used], 0, &mut out),
        Err(oal::Status::Permission)
    ));
    let result = registry.discover(30, &bytes[..used], 0, &mut out).unwrap();
    assert_eq!(result.entry, Some(99));
    let oal::Response::Boundary(boundary) =
        oal::decode_response(&out[..result.used], &query, Deadline::INFINITE, 1).unwrap()
    else {
        panic!("function must return its provider boundary");
    };
    boundary.validate_for("functions/keyboard/events").unwrap();
    registry.stop_claim(ROOT, reference).unwrap();
    assert!(registry.open_function(30, reference, "keyboard").is_err());
    assert!(registry.discover(30, &bytes[..used], 0, &mut out).is_err());
    registry.release_binding(reference).unwrap();
    let next = BindingRef::new(host, 2).unwrap();
    registry.commit_claim(ROOT, next, 21).unwrap();
    registry.resources_ready(next).unwrap();
    assert!(matches!(
        registry.open_function(30, next, "keyboard"),
        Err(Error::NotFound)
    ));
}

#[test]
fn oal_name_cursor_survives_deleted_entry_and_static_names_do_not_shadow_children() {
    use erhino_shared::time::Deadline;
    use liboal::protocol as oal;
    let (mut registry, _, _) = setup();
    let a = register(&mut registry, ROOT, &request(1, None, "a", true));
    register(&mut registry, ROOT, &request(2, None, "b", true));
    register(&mut registry, ROOT, &request(3, Some(a), "binding", true));
    let mut bytes = [0; oal::HEADER_LEN + oal::BODY_MAX];
    let mut out = [0; oal::HEADER_LEN + oal::BODY_MAX];
    let mut views = Vec::new();
    for path in ["a/binding", "a/children/binding"] {
        let query = oal::Request::Lookup { path };
        let used = oal::encode_request(&query, Deadline::INFINITE, &mut bytes).unwrap();
        let result = registry
            .discover(ROOT, &bytes[..used], 0, &mut out)
            .unwrap();
        let oal::Response::Found(info) =
            oal::decode_response(&out[..result.used], &query, Deadline::INFINITE, 0).unwrap()
        else {
            panic!("expected distinct local views");
        };
        views.push(info.as_bytes().to_vec());
    }
    assert_ne!(views[0], views[1]);
    let query = oal::Request::Enumerate {
        path: "",
        cursor: &[],
        max_entries: 1,
    };
    let used = oal::encode_request(&query, Deadline::INFINITE, &mut bytes).unwrap();
    let result = registry
        .discover(ROOT, &bytes[..used], 0, &mut out)
        .unwrap();
    let oal::Response::Page(page) =
        oal::decode_response(&out[..result.used], &query, Deadline::INFINITE, 0).unwrap()
    else {
        panic!("expected page");
    };
    assert_eq!(page.entries().next().unwrap().name, "a");
    let cursor = page.next_cursor().to_vec();
    registry.control_withdraw(a).unwrap();
    let query = oal::Request::Enumerate {
        path: "",
        cursor: &cursor,
        max_entries: 1,
    };
    let used = oal::encode_request(&query, Deadline::INFINITE, &mut bytes).unwrap();
    let result = registry
        .discover(ROOT, &bytes[..used], 0, &mut out)
        .unwrap();
    let oal::Response::Page(page) =
        oal::decode_response(&out[..result.used], &query, Deadline::INFINITE, 0).unwrap()
    else {
        panic!("expected page");
    };
    assert_eq!(page.entries().next().unwrap().name, "b");
    assert!(page.done());
}

#[test]
fn oal_continuation_restores_nonroot_view_and_rechecks_scope() {
    use erhino_shared::time::Deadline;
    use liboal::protocol as oal;
    let (mut registry, _, _) = setup();
    let a = register(&mut registry, ROOT, &request(1, None, "a", true));
    let b = register(&mut registry, ROOT, &request(2, None, "b", true));
    source(&mut registry, 20, Scope::Subtree(b), Access::OBSERVE);
    let mut bytes = [0; oal::HEADER_LEN + oal::BODY_MAX];
    let mut out = [0; oal::HEADER_LEN + oal::BODY_MAX];
    let mut cursor = Vec::new();
    let mut names = Vec::new();
    for round in 0..3 {
        let query = oal::Request::Enumerate {
            path: if round == 0 { "a" } else { "" },
            cursor: &cursor,
            max_entries: 8,
        };
        let used = oal::encode_request(&query, Deadline::INFINITE, &mut bytes).unwrap();
        if round != 0 {
            assert!(matches!(
                registry.discover(20, &bytes[..used], 0, &mut out),
                Err(oal::Status::Permission)
            ));
        }
        let projection = registry
            .discover(ROOT, &bytes[..used], 0, &mut out)
            .unwrap();
        let oal::Response::Page(page) =
            oal::decode_response(&out[..projection.used], &query, Deadline::INFINITE, 0).unwrap()
        else {
            panic!("expected directory page");
        };
        names.push(page.entries().next().unwrap().name.to_owned());
        assert_eq!(page.done(), round == 2);
        cursor = page.next_cursor().to_vec();
    }
    assert_eq!(names, ["binding", "children", "functions"]);
    assert_ne!(a, b);
}

#[test]
fn sealing_only_claim_source_publishes_function_removal_before_maintenance() {
    use libdevice::registry::{FunctionRegistration, WatchTarget};
    let (mut registry, _, account) = setup();
    let id = register(&mut registry, ROOT, &request(1, None, "host", true));
    source(&mut registry, 20, Scope::Subtree(id), Access::CLAIM);
    source(
        &mut registry,
        30,
        Scope::Subtree(id),
        Access::OBSERVE.union(Access::FUNCTIONS),
    );
    let reference = BindingRef::new(id, 1).unwrap();
    registry.commit_claim(20, reference, 20).unwrap();
    registry.resources_ready(reference).unwrap();
    registry
        .install_function(
            reference,
            FunctionRegistration {
                name: "keyboard",
                description: b"boot",
                endpoint: request(1, None, "unused", false).endpoint,
            },
            99,
        )
        .unwrap();
    registry
        .create_watch(30, WatchTarget::Scope(Scope::Subtree(id)), 100, 30)
        .unwrap();
    let read = registry.query_watch(100).unwrap();
    assert_eq!(
        *registry
            .open_function(30, reference, "keyboard")
            .unwrap()
            .entry,
        99
    );
    registry.seal_source(21).unwrap();
    assert!(registry.query(ROOT, id).unwrap().present);
    assert_eq!(
        registry.binding_snapshot(reference).unwrap().phase,
        Phase::ResourcesReady
    );
    assert!(registry.open_function(30, reference, "keyboard").is_err());
    let acknowledged = registry.ack_watch(100, read.sequence).unwrap();
    assert!(acknowledged.sequence > read.sequence);
    assert_ne!(acknowledged.pending, 0);
    assert_eq!(account.usage(DeviceResource::Function).0, 0);
}

#[test]
fn sealed_registrar_waits_for_other_claim_sources_and_descendants_before_maintenance() {
    use libdevice::registry::SourceStop;
    let (mut registry, _, _) = setup();
    source(
        &mut registry,
        20,
        Scope::Root,
        Access::REGISTER.union(Access::OBSERVE),
    );
    source(
        &mut registry,
        30,
        Scope::Root,
        Access::CLAIM.union(Access::OBSERVE),
    );
    source(
        &mut registry,
        40,
        Scope::Root,
        Access::REGISTER.union(Access::OBSERVE),
    );
    let parent = register(&mut registry, 20, &request(1, None, "parent", false));
    let child = register(&mut registry, 40, &request(1, Some(parent), "child", true));
    let grandchild = register(
        &mut registry,
        40,
        &request(2, Some(child), "grandchild", true),
    );
    let child_ref = BindingRef::new(child, 1).unwrap();
    let grand_ref = BindingRef::new(grandchild, 1).unwrap();
    registry.commit_claim(30, child_ref, 20).unwrap();
    registry.commit_claim(30, grand_ref, 21).unwrap();
    registry.resources_ready(grand_ref).unwrap();
    assert_eq!(registry.query_source_stop(21), Ok(SourceStop::Open));
    registry.seal_source(21).unwrap();
    // 未执行 maintenance，其他来源的 Preparing/Ready 仍必须阻止来源停止完成。
    assert_eq!(
        registry.binding_snapshot(child_ref).unwrap().phase,
        Phase::Preparing
    );
    assert_eq!(registry.query_source_stop(21), Ok(SourceStop::Stopping));
    assert_eq!(registry.query_source_stop(31), Ok(SourceStop::Open));
    assert_eq!(registry.query_source_stop(20), Err(Error::Permission));
    assert_eq!(registry.query_source_stop(ROOT), Err(Error::Permission));
    registry.retain_device(child).unwrap();
    registry.stop_claim(30, child_ref).unwrap();
    registry.release_binding(child_ref).unwrap();
    assert_eq!(registry.query_source_stop(21), Ok(SourceStop::Stopping));
    registry.stop_claim(30, grand_ref).unwrap();
    registry.release_binding(grand_ref).unwrap();
    // 旧身份/观察壳仍付费保活，但物理绑定都已安全 Released。
    assert_eq!(registry.query_source_stop(21), Ok(SourceStop::Closed));
    registry.release_device(child).unwrap();
}

#[test]
fn source_closed_projection_cannot_be_followed_by_late_commits() {
    use libdevice::registry::SourceStop;
    let (mut registry, _, _) = setup();
    source(&mut registry, 20, Scope::Root, Access::ALL);
    registry
        .derive_source(
            20,
            SourceIds {
                use_context: 30,
                admin_context: 31,
            },
            3,
            Scope::Root,
            Access::CLAIM,
        )
        .unwrap();
    let device = register(&mut registry, ROOT, &request(1, None, "existing", true));
    let reference = BindingRef::new(device, 1).unwrap();
    assert_eq!(registry.claim(30, reference), Ok(Claim::Vacant));
    let publication = request(1, None, "late", true);
    let candidate = registry
        .prepare_registration(20, &publication, 1, 7)
        .unwrap();
    registry.seal_source(21).unwrap();
    assert_eq!(registry.query_source_stop(21), Ok(SourceStop::Closed));
    assert_eq!(registry.query_source_stop(31), Ok(SourceStop::Closed));
    // 管理者即使尚未处理封口提示、只清了本地终态选择，下一次新Claim仍由此门拒绝。
    assert_eq!(registry.claim(30, reference), Err(Error::Closed));
    assert_eq!(registry.commit_claim(30, reference, 8), Err(Error::Closed));
    assert!(
        matches!(registry.commit_registration(candidate), Err(failure) if failure.error == Error::Closed)
    );
    assert_eq!(
        registry.derive_source(
            20,
            SourceIds {
                use_context: 40,
                admin_context: 41
            },
            4,
            Scope::Root,
            Access::OBSERVE
        ),
        Err(Error::Closed)
    );
}

#[test]
fn parent_release_waits_for_its_binding_dependencies_not_static_topology() {
    let (mut registry, _, _) = setup();
    source(
        &mut registry,
        20,
        Scope::Root,
        Access::CLAIM.union(Access::CHILD_REGISTER),
    );
    source(&mut registry, 30, Scope::Root, Access::CLAIM);
    let parent = register(&mut registry, ROOT, &request(1, None, "parent", true));
    let independent = register(
        &mut registry,
        ROOT,
        &request(2, Some(parent), "independent", true),
    );
    let parent_ref = BindingRef::new(parent, 1).unwrap();
    registry.commit_claim(20, parent_ref, 100).unwrap();
    registry.resources_ready(parent_ref).unwrap();
    registry
        .derive_child_source(
            20,
            parent_ref,
            SourceIds {
                use_context: 40,
                admin_context: 41,
            },
            101,
        )
        .unwrap();
    let dependent = register(
        &mut registry,
        40,
        &request(1, Some(parent), "dependent", true),
    );
    let descendant = register(
        &mut registry,
        ROOT,
        &request(3, Some(dependent), "descendant", true),
    );
    let dependent_ref = BindingRef::new(dependent, 1).unwrap();
    let descendant_ref = BindingRef::new(descendant, 1).unwrap();
    let independent_ref = BindingRef::new(independent, 1).unwrap();
    registry.commit_claim(30, dependent_ref, 102).unwrap();
    registry.commit_claim(30, descendant_ref, 103).unwrap();
    registry.resources_ready(descendant_ref).unwrap();
    registry.commit_claim(30, independent_ref, 104).unwrap();
    registry.resources_ready(independent_ref).unwrap();
    registry.stop_claim(20, parent_ref).unwrap();
    // 父申请来源仍开放，子维护尚未运行；释放门仍看见 Preparing/Ready 的实际依赖。
    assert_eq!(
        registry.binding_snapshot(dependent_ref).unwrap().phase,
        Phase::Preparing
    );
    assert_eq!(registry.release_binding(parent_ref), Err(Error::Busy));
    assert_eq!(
        registry.binding_snapshot(parent_ref).unwrap().phase,
        Phase::Stopping
    );
    let end = registry.scan_end();
    assert_eq!(
        registry.maintenance(0, end).unwrap(),
        Some((parent.serial(), Some(100)))
    );
    registry.stop_claim(30, dependent_ref).unwrap();
    registry.retain_device(dependent).unwrap();
    registry.release_binding(dependent_ref).unwrap();
    assert_eq!(registry.release_binding(parent_ref), Err(Error::Busy));
    registry.stop_claim(30, descendant_ref).unwrap();
    registry.release_binding(descendant_ref).unwrap();
    // 独立静态子节点仍 Ready，旧控制壳仍保留；两者都不是本父轮的供应依赖。
    assert_eq!(
        registry.binding_snapshot(independent_ref).unwrap().phase,
        Phase::ResourcesReady
    );
    registry.release_binding(parent_ref).unwrap();
    registry.release_device(dependent).unwrap();
    let next = BindingRef::new(parent, 2).unwrap();
    registry.commit_claim(20, next, 105).unwrap();
    registry.resources_ready(next).unwrap();
    registry
        .derive_child_source(
            20,
            next,
            SourceIds {
                use_context: 50,
                admin_context: 51,
            },
            106,
        )
        .unwrap();
    let fresh = register(&mut registry, 50, &request(1, Some(parent), "fresh", true));
    registry
        .commit_claim(30, BindingRef::new(fresh, 1).unwrap(), 107)
        .unwrap();
    assert!(registry.binding_dependents_released(parent_ref).unwrap());
    assert!(!registry.binding_dependents_released(next).unwrap());
}
