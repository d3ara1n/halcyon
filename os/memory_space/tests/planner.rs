//! Host 测试需显式指定 `--target aarch64-apple-darwin`。

use memory_space::{
    AddressRange, AllocationKey, AnonymousClass, BackingId, BackingView, ChangeError,
    ExecutableState, FaultClass, LeaseKey, Limits, MapBacking, MapPlacement, MapRequest,
    MemoryObjectState, MemorySpace, ObjectError, ObjectId, PAGE_SIZE, PageRange, ProtectRequest,
    Protection, RangeError, RegionKindView, RegionOwner, SealOutcome, TranslationIntent,
    UnmapRequest, UserWriteLeaseRequest, ValidatedChange,
};

const BASE: usize = 0x1000_0000;
const SPACE_BYTES: usize = 64 * PAGE_SIZE;

fn limits() -> Limits {
    Limits {
        max_regions: 64,
        max_transactions: 8,
        max_pages_per_change: 32,
        max_lease_bytes: 128,
        max_lease_segments: 4,
    }
}

fn space() -> MemorySpace {
    MemorySpace::new(PageRange::new(BASE, SPACE_BYTES).unwrap(), limits()).unwrap()
}

fn backing(id: u64) -> BackingId {
    BackingId::new(id).unwrap()
}

#[allow(clippy::too_many_arguments)]
fn anonymous_map(
    identity: u64,
    usable_start: usize,
    pages: usize,
    guard_before: usize,
    guard_after: usize,
    current: Protection,
    maximum: Protection,
    owner: RegionOwner,
) -> MapRequest {
    MapRequest {
        bytes: pages * PAGE_SIZE,
        guard_before: guard_before * PAGE_SIZE,
        guard_after: guard_after * PAGE_SIZE,
        placement: MapPlacement::FixedEmpty { usable_start },
        current,
        maximum,
        owner,
        backing: MapBacking::Anonymous {
            identity: backing(identity),
            class: if maximum == Protection::ReadExecute {
                AnonymousClass::InitialExecutable
            } else {
                AnonymousClass::Data
            },
        },
        result: None,
    }
}

fn reserve_no_permits(
    space: &mut MemorySpace,
    validated: ValidatedChange,
) -> memory_space::PreparedChange {
    assert!(validated.permit_requirements().is_empty());
    space.reserve(validated, Vec::new()).unwrap()
}

fn complete_prepared(
    space: &mut MemorySpace,
    prepared: memory_space::PreparedChange,
) -> Vec<memory_space::RetiringFragment> {
    let committed = space.commit(prepared);
    let published = space.publish(committed);
    let synchronized = space.synchronize(published);
    let (retiring, mut batch) = space.begin_retire(synchronized);
    let fragments = batch.fragments().to_vec();
    while batch.pop_fragment().is_some() {}
    while batch.pop_permit().is_some() {}
    let retired = space.finish_retire(retiring, &batch);
    space.complete(retired);
    fragments
}

fn commit_map(space: &mut MemorySpace, request: MapRequest) -> memory_space::MapResultLayout {
    let validated = space.validate_map(request).unwrap();
    let layout = validated.map_result().unwrap();
    let prepared = reserve_no_permits(space, validated);
    let batch = complete_prepared(space, prepared);
    assert!(batch.is_empty());
    layout
}

fn region_allocation(space: &MemorySpace) -> AllocationKey {
    space.regions().next().unwrap().allocation
}

#[test]
fn map_establishes_guard_mapping_and_unique_fragment_keys() {
    let mut space = space();
    let layout = commit_map(
        &mut space,
        anonymous_map(
            1,
            BASE + PAGE_SIZE,
            3,
            1,
            1,
            Protection::ReadWrite,
            Protection::ReadWrite,
            RegionOwner::AddressSpace,
        ),
    );
    assert_eq!(
        layout.reservation,
        PageRange::new(BASE, 5 * PAGE_SIZE).unwrap()
    );
    assert_eq!(
        layout.usable,
        PageRange::new(BASE + PAGE_SIZE, 3 * PAGE_SIZE).unwrap()
    );

    let regions: Vec<_> = space.regions().collect();
    assert_eq!(regions.len(), 3);
    assert_eq!(regions[0].allocation, regions[1].allocation);
    assert_eq!(regions[1].allocation, regions[2].allocation);
    assert_ne!(regions[0].key, regions[1].key);
    assert_ne!(regions[1].key, regions[2].key);
    assert_eq!(regions[0].kind, RegionKindView::Guard);
    assert!(matches!(
        regions[1].kind,
        RegionKindView::Mapping {
            current: Protection::ReadWrite,
            maximum: Protection::ReadWrite,
            ..
        }
    ));
    assert_eq!(space.fault_class(BASE), FaultClass::Guard);
    assert_eq!(
        space.fault_class(BASE + PAGE_SIZE),
        FaultClass::Mapping {
            protection: Protection::ReadWrite
        }
    );
    assert_eq!(space.fault_class(BASE + 8 * PAGE_SIZE), FaultClass::Free);
}

#[test]
fn anywhere_is_first_fit_and_fixed_empty_never_replaces() {
    let mut space = space();
    let first = commit_map(
        &mut space,
        MapRequest {
            placement: MapPlacement::Anywhere,
            ..anonymous_map(
                1,
                BASE,
                2,
                0,
                0,
                Protection::ReadOnly,
                Protection::ReadOnly,
                RegionOwner::AddressSpace,
            )
        },
    );
    let second = commit_map(
        &mut space,
        MapRequest {
            placement: MapPlacement::Anywhere,
            ..anonymous_map(
                2,
                BASE,
                1,
                0,
                0,
                Protection::ReadOnly,
                Protection::ReadOnly,
                RegionOwner::AddressSpace,
            )
        },
    );
    assert_eq!(first.usable.start(), BASE);
    assert_eq!(second.usable.start(), BASE + 2 * PAGE_SIZE);

    let error = space
        .validate_map(anonymous_map(
            3,
            BASE,
            1,
            0,
            0,
            Protection::ReadOnly,
            Protection::ReadOnly,
            RegionOwner::AddressSpace,
        ))
        .unwrap_err();
    assert_eq!(error, ChangeError::Conflict);
}

#[test]
fn exact_unmap_covers_full_usable_guard_and_middle_cases() {
    // Full reservation.
    let mut full = space();
    let layout = commit_map(
        &mut full,
        anonymous_map(
            1,
            BASE + PAGE_SIZE,
            3,
            1,
            1,
            Protection::ReadWrite,
            Protection::ReadWrite,
            RegionOwner::AddressSpace,
        ),
    );
    let validated = full
        .validate_unmap(UnmapRequest {
            range: layout.reservation,
            authority: RegionOwner::AddressSpace,
        })
        .unwrap();
    let prepared = reserve_no_permits(&mut full, validated);
    let batch = complete_prepared(&mut full, prepared);
    assert_eq!(full.region_count(), 0);
    assert_eq!(batch.len(), 3);

    // Usable only leaves both guards with the original AllocationKey.
    let mut usable = space();
    let layout = commit_map(
        &mut usable,
        anonymous_map(
            2,
            BASE + PAGE_SIZE,
            3,
            1,
            1,
            Protection::ReadWrite,
            Protection::ReadWrite,
            RegionOwner::AddressSpace,
        ),
    );
    let allocation = region_allocation(&usable);
    let validated = usable
        .validate_unmap(UnmapRequest {
            range: layout.usable,
            authority: RegionOwner::AddressSpace,
        })
        .unwrap();
    let prepared = reserve_no_permits(&mut usable, validated);
    complete_prepared(&mut usable, prepared);
    let regions: Vec<_> = usable.regions().collect();
    assert_eq!(regions.len(), 2);
    assert!(
        regions
            .iter()
            .all(|region| region.kind == RegionKindView::Guard)
    );
    assert!(regions.iter().all(|region| region.allocation == allocation));

    // Guard only changes no mapping.
    let mut guard = space();
    let layout = commit_map(
        &mut guard,
        anonymous_map(
            3,
            BASE + PAGE_SIZE,
            3,
            1,
            1,
            Protection::ReadOnly,
            Protection::ReadOnly,
            RegionOwner::AddressSpace,
        ),
    );
    let lower_guard = PageRange::new(layout.reservation.start(), PAGE_SIZE).unwrap();
    let validated = guard
        .validate_unmap(UnmapRequest {
            range: lower_guard,
            authority: RegionOwner::AddressSpace,
        })
        .unwrap();
    let prepared = reserve_no_permits(&mut guard, validated);
    assert!(prepared.translation_intents().is_empty());
    complete_prepared(&mut guard, prepared);
    assert_eq!(guard.region_count(), 2);
    assert_eq!(
        guard.fault_class(layout.usable.start()),
        FaultClass::Mapping {
            protection: Protection::ReadOnly
        }
    );

    // Mapping middle creates exact left/right slices and consumes the old RegionKey.
    let mut middle = space();
    let layout = commit_map(
        &mut middle,
        anonymous_map(
            4,
            BASE + PAGE_SIZE,
            3,
            1,
            1,
            Protection::ReadOnly,
            Protection::ReadOnly,
            RegionOwner::AddressSpace,
        ),
    );
    let old_mapping_key = middle
        .regions()
        .find(|region| matches!(region.kind, RegionKindView::Mapping { .. }))
        .unwrap()
        .key;
    let cut = PageRange::new(layout.usable.start() + PAGE_SIZE, PAGE_SIZE).unwrap();
    let validated = middle
        .validate_unmap(UnmapRequest {
            range: cut,
            authority: RegionOwner::AddressSpace,
        })
        .unwrap();
    let prepared = reserve_no_permits(&mut middle, validated);
    let batch = complete_prepared(&mut middle, prepared);
    let regions: Vec<_> = middle.regions().collect();
    assert_eq!(regions.len(), 4);
    assert!(regions.iter().all(|region| region.key != old_mapping_key));
    assert_eq!(batch.len(), 1);
    assert_ne!(batch[0].key, old_mapping_key);
    assert_eq!(batch[0].range, cut);
}

#[test]
fn protect_splits_then_recoalesces_only_within_allocation() {
    let mut space = space();
    let layout = commit_map(
        &mut space,
        anonymous_map(
            1,
            BASE,
            3,
            0,
            0,
            Protection::ReadWrite,
            Protection::ReadWrite,
            RegionOwner::AddressSpace,
        ),
    );
    let allocation = region_allocation(&space);
    let middle = PageRange::new(BASE + PAGE_SIZE, PAGE_SIZE).unwrap();
    let validated = space
        .validate_protect(ProtectRequest {
            range: middle,
            protection: Protection::ReadOnly,
            authority: RegionOwner::AddressSpace,
        })
        .unwrap();
    let prepared = reserve_no_permits(&mut space, validated);
    let retired = complete_prepared(&mut space, prepared);
    assert_eq!(retired.len(), 1);
    assert_eq!(
        retired[0].backing_retire,
        memory_space::BackingRetire::Retain
    );
    assert_eq!(space.region_count(), 3);

    let validated = space
        .validate_protect(ProtectRequest {
            range: middle,
            protection: Protection::ReadWrite,
            authority: RegionOwner::AddressSpace,
        })
        .unwrap();
    let prepared = reserve_no_permits(&mut space, validated);
    let retired = complete_prepared(&mut space, prepared);
    assert_eq!(retired.len(), 1);
    assert_eq!(
        retired[0].backing_retire,
        memory_space::BackingRetire::Retain
    );
    let regions: Vec<_> = space.regions().collect();
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].allocation, allocation);
    assert_eq!(regions[0].range, layout.usable);

    // An adjacent mapping from another Map has another AllocationKey and stays separate.
    commit_map(
        &mut space,
        anonymous_map(
            2,
            BASE + 3 * PAGE_SIZE,
            1,
            0,
            0,
            Protection::ReadWrite,
            Protection::ReadWrite,
            RegionOwner::AddressSpace,
        ),
    );
    assert_eq!(space.region_count(), 2);
    let allocations: Vec<_> = space.regions().map(|region| region.allocation).collect();
    assert_ne!(allocations[0], allocations[1]);
}

#[test]
fn protection_ceiling_guard_and_lease_authority_are_enforced() {
    let mut space = space();
    let lease = LeaseKey::new(7).unwrap();
    let layout = commit_map(
        &mut space,
        anonymous_map(
            1,
            BASE + PAGE_SIZE,
            1,
            1,
            0,
            Protection::ReadOnly,
            Protection::ReadOnly,
            RegionOwner::Lease(lease),
        ),
    );
    assert_eq!(
        space
            .validate_unmap(UnmapRequest {
                range: layout.usable,
                authority: RegionOwner::AddressSpace,
            })
            .unwrap_err(),
        ChangeError::OwnerDenied
    );
    assert_eq!(
        space
            .validate_protect(ProtectRequest {
                range: layout.usable,
                protection: Protection::ReadWrite,
                authority: RegionOwner::Lease(lease),
            })
            .unwrap_err(),
        ChangeError::PermissionDenied
    );
    let guard = PageRange::new(layout.reservation.start(), PAGE_SIZE).unwrap();
    assert_eq!(
        space
            .validate_protect(ProtectRequest {
                range: guard,
                protection: Protection::ReadOnly,
                authority: RegionOwner::Lease(lease),
            })
            .unwrap_err(),
        ChangeError::Guard
    );
    let validated = space
        .validate_unmap(UnmapRequest {
            range: layout.usable,
            authority: RegionOwner::Lease(lease),
        })
        .unwrap();
    let prepared = reserve_no_permits(&mut space, validated);
    complete_prepared(&mut space, prepared);
}

#[test]
fn user_write_lease_pins_exact_projection_until_commit_or_rollback() {
    let mut space = space();
    commit_map(
        &mut space,
        anonymous_map(
            1,
            BASE,
            1,
            0,
            0,
            Protection::ReadWrite,
            Protection::ReadWrite,
            RegionOwner::AddressSpace,
        ),
    );
    commit_map(
        &mut space,
        anonymous_map(
            2,
            BASE + PAGE_SIZE,
            1,
            0,
            0,
            Protection::ReadWrite,
            Protection::ReadWrite,
            RegionOwner::AddressSpace,
        ),
    );
    let result_range = AddressRange::new(BASE + PAGE_SIZE - 16, 32).unwrap();
    let mut request = anonymous_map(
        3,
        BASE + 3 * PAGE_SIZE,
        1,
        0,
        0,
        Protection::ReadOnly,
        Protection::ReadOnly,
        RegionOwner::AddressSpace,
    );
    request.result = Some(UserWriteLeaseRequest {
        range: result_range,
    });
    let validated = space.validate_map(request).unwrap();
    let prepared = reserve_no_permits(&mut space, validated);
    let lease = prepared.user_write_lease().unwrap();
    assert_eq!(lease.range(), result_range);
    assert_eq!(lease.projection().segments().len(), 2);
    assert_eq!(
        lease.projection().segments()[0].backing_offset,
        PAGE_SIZE - 16
    );
    assert_eq!(lease.projection().segments()[1].backing_offset, 0);

    let first_page = PageRange::new(BASE, PAGE_SIZE).unwrap();
    assert_eq!(
        space
            .validate_unmap(UnmapRequest {
                range: first_page,
                authority: RegionOwner::AddressSpace,
            })
            .unwrap_err(),
        ChangeError::Busy
    );
    assert!(space.rollback(prepared).is_empty());
    assert_eq!(space.transaction_count(), 0);
    assert!(
        space
            .validate_unmap(UnmapRequest {
                range: first_page,
                authority: RegionOwner::AddressSpace,
            })
            .is_ok()
    );

    let mut request = anonymous_map(
        4,
        BASE + 3 * PAGE_SIZE,
        1,
        0,
        0,
        Protection::ReadOnly,
        Protection::ReadOnly,
        RegionOwner::AddressSpace,
    );
    request.result = Some(UserWriteLeaseRequest {
        range: result_range,
    });
    let validated = space.validate_map(request).unwrap();
    let prepared = reserve_no_permits(&mut space, validated);
    let committed = space.commit(prepared);
    // Commit releases the lease; the result slot is no longer needed by the transaction.
    assert!(
        space
            .validate_unmap(UnmapRequest {
                range: first_page,
                authority: RegionOwner::AddressSpace,
            })
            .is_ok()
    );
    let published = space.publish(committed);
    let synchronized = space.synchronize(published);
    let (retiring, mut batch) = space.begin_retire(synchronized);
    while batch.pop_fragment().is_some() {}
    while batch.pop_permit().is_some() {}
    let retired = space.finish_retire(retiring, &batch);
    space.complete(retired);
}

#[test]
fn nonoverlapping_transactions_coexist_and_overlaps_are_busy() {
    let mut space = space();
    let first = space
        .validate_map(anonymous_map(
            1,
            BASE,
            1,
            0,
            0,
            Protection::ReadOnly,
            Protection::ReadOnly,
            RegionOwner::AddressSpace,
        ))
        .unwrap();
    let second = space
        .validate_map(anonymous_map(
            2,
            BASE + 2 * PAGE_SIZE,
            1,
            0,
            0,
            Protection::ReadOnly,
            Protection::ReadOnly,
            RegionOwner::AddressSpace,
        ))
        .unwrap();
    let first = reserve_no_permits(&mut space, first);
    let second = reserve_no_permits(&mut space, second);
    assert_eq!(space.transaction_count(), 2);
    assert_eq!(
        space
            .validate_map(anonymous_map(
                3,
                BASE,
                1,
                0,
                0,
                Protection::ReadOnly,
                Protection::ReadOnly,
                RegionOwner::AddressSpace,
            ))
            .unwrap_err(),
        ChangeError::Busy
    );
    complete_prepared(&mut space, second);
    complete_prepared(&mut space, first);
    assert_eq!(space.transaction_count(), 0);
    assert_eq!(space.region_count(), 2);
}

#[test]
fn stale_validation_and_permit_mismatch_have_zero_ledger_side_effects() {
    let mut space = space();
    let stale = space
        .validate_map(anonymous_map(
            1,
            BASE,
            1,
            0,
            0,
            Protection::ReadOnly,
            Protection::ReadOnly,
            RegionOwner::AddressSpace,
        ))
        .unwrap();
    commit_map(
        &mut space,
        anonymous_map(
            2,
            BASE,
            1,
            0,
            0,
            Protection::ReadOnly,
            Protection::ReadOnly,
            RegionOwner::AddressSpace,
        ),
    );
    let failure = space.reserve(stale, Vec::new()).unwrap_err();
    assert_eq!(failure.error, ChangeError::Stale);
    assert_eq!(space.region_count(), 1);
    assert_eq!(space.transaction_count(), 0);

    let object_id = ObjectId::new(9).unwrap();
    let object = MemoryObjectState::new(object_id, 4);
    let validated = space
        .validate_map(MapRequest {
            bytes: PAGE_SIZE,
            guard_before: 0,
            guard_after: 0,
            placement: MapPlacement::FixedEmpty {
                usable_start: BASE + 2 * PAGE_SIZE,
            },
            current: Protection::ReadWrite,
            maximum: Protection::ReadWrite,
            owner: RegionOwner::AddressSpace,
            backing: MapBacking::Object {
                authorization: object.authorize_view(Protection::ReadWrite).unwrap(),
                offset: 0,
                object_bytes: PAGE_SIZE,
            },
            result: None,
        })
        .unwrap();
    assert_eq!(validated.permit_requirements()[0].count, 1);
    let failure = space.reserve(validated, Vec::new()).unwrap_err();
    assert_eq!(failure.error, ChangeError::PermitMismatch);
    assert_eq!(space.region_count(), 1);
    assert_eq!(space.transaction_count(), 0);
}

#[test]
fn object_write_permit_retires_only_after_synchronization_and_finishes_seal() {
    let mut space = space();
    let object_id = ObjectId::new(1).unwrap();
    let mut object = MemoryObjectState::new(object_id, 8);
    let validated = space
        .validate_map(MapRequest {
            bytes: 2 * PAGE_SIZE,
            guard_before: 0,
            guard_after: 0,
            placement: MapPlacement::FixedEmpty { usable_start: BASE },
            current: Protection::ReadWrite,
            maximum: Protection::ReadWrite,
            owner: RegionOwner::AddressSpace,
            backing: MapBacking::Object {
                authorization: object.authorize_view(Protection::ReadWrite).unwrap(),
                offset: PAGE_SIZE,
                object_bytes: 4 * PAGE_SIZE,
            },
            result: None,
        })
        .unwrap();
    let permits = object.reserve_writes(1).unwrap();
    let prepared = space.reserve(validated, permits).unwrap();
    let region_key = prepared
        .mapped_region_key()
        .expect("object Map must publish one usable region identity");
    complete_prepared(&mut space, prepared);
    assert_eq!(object.permit_count(), 1);
    assert!(
        space.regions().any(|region| region.key == region_key),
        "prepared region identity must survive Commit"
    );
    assert_eq!(object.seal(Some(77)).unwrap(), SealOutcome::Waiting);
    assert_eq!(object.state(), ExecutableState::Sealing);
    assert_eq!(object.reserve_writes(1), Err(ObjectError::PermitDenied));

    let validated = space
        .validate_unmap(UnmapRequest {
            range: PageRange::new(BASE, 2 * PAGE_SIZE).unwrap(),
            authority: RegionOwner::AddressSpace,
        })
        .unwrap();
    let prepared = reserve_no_permits(&mut space, validated);
    let committed = space.commit(prepared);
    assert_eq!(object.permit_count(), 1);
    let published = space.publish(committed);
    assert_eq!(object.permit_count(), 1);
    let synchronized = space.synchronize(published);
    assert_eq!(object.permit_count(), 1);
    let (retiring, mut batch) = space.begin_retire(synchronized);
    let fragment = batch.pop_fragment().expect("object fragment must retire");
    assert_eq!(
        fragment.backing_retire,
        memory_space::BackingRetire::Release
    );
    let permit = batch.pop_permit().expect("write permit must retire");
    assert_eq!(object.retire_write(permit), Some(77));
    assert!(batch.is_empty());
    let retired = space.finish_retire(retiring, &batch);
    assert_eq!(object.state(), ExecutableState::Executable);
    assert_eq!(object.permit_count(), 0);
    space.complete(retired);
    assert!(object.authorize_view(Protection::ReadExecute).is_ok());
    assert_eq!(
        object.authorize_view(Protection::ReadWrite),
        Err(ObjectError::ViewDenied)
    );
}

#[test]
fn rollback_returns_reserved_permit_and_executable_object_rejects_reenable_write() {
    let mut first_space = space();
    let object_id = ObjectId::new(12).unwrap();
    let mut object = MemoryObjectState::new(object_id, 4);
    let validated = first_space
        .validate_map(MapRequest {
            bytes: PAGE_SIZE,
            guard_before: 0,
            guard_after: 0,
            placement: MapPlacement::FixedEmpty { usable_start: BASE },
            current: Protection::ReadWrite,
            maximum: Protection::ReadWrite,
            owner: RegionOwner::AddressSpace,
            backing: MapBacking::Object {
                authorization: object.authorize_view(Protection::ReadWrite).unwrap(),
                offset: 0,
                object_bytes: PAGE_SIZE,
            },
            result: None,
        })
        .unwrap();
    let permits = object.reserve_writes(1).unwrap();
    let prepared = first_space.reserve(validated, permits).unwrap();
    assert_eq!(object.seal(Some(91)).unwrap(), SealOutcome::Waiting);
    let permits = first_space.rollback(prepared);
    assert_eq!(object.cancel_writes(permits), Some(91));
    assert_eq!(object.state(), ExecutableState::Executable);
    assert_eq!(first_space.region_count(), 0);
    assert_eq!(first_space.transaction_count(), 0);

    let mut second_space = space();
    let mutable_id = ObjectId::new(13).unwrap();
    let mut executable = MemoryObjectState::new(mutable_id, 4);
    let validated = second_space
        .validate_map(MapRequest {
            bytes: PAGE_SIZE,
            guard_before: 0,
            guard_after: 0,
            placement: MapPlacement::FixedEmpty { usable_start: BASE },
            current: Protection::ReadOnly,
            maximum: Protection::ReadWrite,
            owner: RegionOwner::AddressSpace,
            backing: MapBacking::Object {
                authorization: executable.authorize_view(Protection::ReadWrite).unwrap(),
                offset: 0,
                object_bytes: PAGE_SIZE,
            },
            result: None,
        })
        .unwrap();
    let prepared = reserve_no_permits(&mut second_space, validated);
    complete_prepared(&mut second_space, prepared);
    assert_eq!(executable.seal(None).unwrap(), SealOutcome::Complete);
    let validated = second_space
        .validate_protect(ProtectRequest {
            range: PageRange::new(BASE, PAGE_SIZE).unwrap(),
            protection: Protection::ReadWrite,
            authority: RegionOwner::AddressSpace,
        })
        .unwrap();
    assert_eq!(validated.permit_requirements()[0].object, mutable_id);
    assert_eq!(executable.reserve_writes(1), Err(ObjectError::PermitDenied));
}

#[test]
fn abandoned_seal_waiter_does_not_revert_state() {
    let object_id = ObjectId::new(2).unwrap();
    let mut object = MemoryObjectState::new(object_id, 2);
    let permits = object.reserve_writes(1).unwrap();
    assert_eq!(object.seal(Some(11)).unwrap(), SealOutcome::Waiting);
    assert!(object.abandon_waiter(11));
    assert_eq!(object.state(), ExecutableState::Sealing);
    assert_eq!(object.retire_writes(permits), None);
    assert_eq!(object.state(), ExecutableState::Executable);
    assert_eq!(object.seal(Some(12)).unwrap(), SealOutcome::Complete);
}

#[test]
fn object_view_offsets_follow_exact_middle_split() {
    let mut space = space();
    let object_id = ObjectId::new(3).unwrap();
    let object = MemoryObjectState::new(object_id, 2);
    let validated = space
        .validate_map(MapRequest {
            bytes: 3 * PAGE_SIZE,
            guard_before: 0,
            guard_after: 0,
            placement: MapPlacement::FixedEmpty { usable_start: BASE },
            current: Protection::ReadOnly,
            maximum: Protection::ReadOnly,
            owner: RegionOwner::AddressSpace,
            backing: MapBacking::Object {
                authorization: object.authorize_view(Protection::ReadOnly).unwrap(),
                offset: PAGE_SIZE,
                object_bytes: 8 * PAGE_SIZE,
            },
            result: None,
        })
        .unwrap();
    let prepared = reserve_no_permits(&mut space, validated);
    complete_prepared(&mut space, prepared);
    let cut = PageRange::new(BASE + PAGE_SIZE, PAGE_SIZE).unwrap();
    let validated = space
        .validate_unmap(UnmapRequest {
            range: cut,
            authority: RegionOwner::AddressSpace,
        })
        .unwrap();
    let prepared = reserve_no_permits(&mut space, validated);
    let batch = complete_prepared(&mut space, prepared);
    let regions: Vec<_> = space.regions().collect();
    assert_eq!(regions.len(), 2);
    let offsets: Vec<_> = regions
        .iter()
        .map(|region| match region.kind {
            RegionKindView::Mapping {
                backing: BackingView::Object { offset, .. },
                ..
            } => offset,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(offsets, [PAGE_SIZE, 3 * PAGE_SIZE]);
    assert!(matches!(
        batch[0].kind,
        RegionKindView::Mapping {
            backing: BackingView::Object { offset, .. },
            ..
        } if offset == 2 * PAGE_SIZE
    ));
}

#[test]
fn capacity_geometry_and_backing_failures_are_atomic() {
    let mut tight_limits = limits();
    tight_limits.max_regions = 2;
    tight_limits.max_pages_per_change = 2;
    let page_limited =
        MemorySpace::new(PageRange::new(BASE, SPACE_BYTES).unwrap(), tight_limits).unwrap();
    assert_eq!(
        page_limited
            .validate_map(anonymous_map(
                1,
                BASE + PAGE_SIZE,
                1,
                1,
                1,
                Protection::ReadOnly,
                Protection::ReadOnly,
                RegionOwner::AddressSpace,
            ))
            .unwrap_err(),
        ChangeError::PageLimit
    );
    assert_eq!(page_limited.region_count(), 0);

    tight_limits.max_pages_per_change = 8;
    let region_limited =
        MemorySpace::new(PageRange::new(BASE, SPACE_BYTES).unwrap(), tight_limits).unwrap();
    assert_eq!(
        region_limited
            .validate_map(anonymous_map(
                2,
                BASE + PAGE_SIZE,
                1,
                1,
                1,
                Protection::ReadOnly,
                Protection::ReadOnly,
                RegionOwner::AddressSpace,
            ))
            .unwrap_err(),
        ChangeError::RegionLimit
    );

    let normal = space();
    let object_id = ObjectId::new(4).unwrap();
    let object = MemoryObjectState::new(object_id, 2);
    assert_eq!(
        normal
            .validate_map(MapRequest {
                bytes: 2 * PAGE_SIZE,
                guard_before: 0,
                guard_after: 0,
                placement: MapPlacement::FixedEmpty { usable_start: BASE },
                current: Protection::ReadOnly,
                maximum: Protection::ReadOnly,
                owner: RegionOwner::AddressSpace,
                backing: MapBacking::Object {
                    authorization: object.authorize_view(Protection::ReadOnly).unwrap(),
                    offset: PAGE_SIZE,
                    object_bytes: 2 * PAGE_SIZE,
                },
                result: None,
            })
            .unwrap_err(),
        ChangeError::BackingOutOfRange
    );
    assert_eq!(normal.region_count(), 0);
}

#[test]
fn translation_intents_follow_typed_change_stages() {
    let mut space = space();
    let validated = space
        .validate_map(anonymous_map(
            1,
            BASE,
            1,
            0,
            0,
            Protection::ReadOnly,
            Protection::ReadOnly,
            RegionOwner::AddressSpace,
        ))
        .unwrap();
    let prepared = reserve_no_permits(&mut space, validated);
    assert!(matches!(
        prepared.translation_intents(),
        [TranslationIntent::Install { range, .. }] if *range == PageRange::new(BASE, PAGE_SIZE).unwrap()
    ));
    let committed = space.commit(prepared);
    assert_eq!(space.transaction_count(), 1);
    let published = space.publish(committed);
    let synchronized = space.synchronize(published);
    let (retiring, batch) = space.begin_retire(synchronized);
    assert!(batch.is_empty());
    let retired = space.finish_retire(retiring, &batch);
    space.complete(retired);
    assert_eq!(space.transaction_count(), 0);
}

#[test]
fn range_geometry_is_checked_and_page_covering_is_exact() {
    assert_eq!(
        AddressRange::new(usize::MAX - 2, 4),
        Err(RangeError::Overflow)
    );
    assert_eq!(
        PageRange::new(BASE + 1, PAGE_SIZE),
        Err(RangeError::Unaligned)
    );
    assert_eq!(
        PageRange::rounded(BASE, PAGE_SIZE + 1).unwrap(),
        PageRange::new(BASE, 2 * PAGE_SIZE).unwrap()
    );
    assert_eq!(
        PageRange::rounded(BASE, usize::MAX),
        Err(RangeError::Overflow)
    );
}

#[test]
fn transaction_and_lease_capacity_limits_are_explicit() {
    let mut bounded = limits();
    bounded.max_transactions = 1;
    let mut space = MemorySpace::new(PageRange::new(BASE, SPACE_BYTES).unwrap(), bounded).unwrap();
    let first = space
        .validate_map(anonymous_map(
            1,
            BASE,
            1,
            0,
            0,
            Protection::ReadOnly,
            Protection::ReadOnly,
            RegionOwner::AddressSpace,
        ))
        .unwrap();
    let second = space
        .validate_map(anonymous_map(
            2,
            BASE + 2 * PAGE_SIZE,
            1,
            0,
            0,
            Protection::ReadOnly,
            Protection::ReadOnly,
            RegionOwner::AddressSpace,
        ))
        .unwrap();
    let first = reserve_no_permits(&mut space, first);
    let failure = space.reserve(second, Vec::new()).unwrap_err();
    assert_eq!(failure.error, ChangeError::TransactionLimit);
    assert!(space.rollback(first).is_empty());

    let mut lease_bounded = limits();
    lease_bounded.max_lease_segments = 1;
    let mut space =
        MemorySpace::new(PageRange::new(BASE, SPACE_BYTES).unwrap(), lease_bounded).unwrap();
    commit_map(
        &mut space,
        anonymous_map(
            3,
            BASE,
            1,
            0,
            0,
            Protection::ReadWrite,
            Protection::ReadWrite,
            RegionOwner::AddressSpace,
        ),
    );
    commit_map(
        &mut space,
        anonymous_map(
            4,
            BASE + PAGE_SIZE,
            1,
            0,
            0,
            Protection::ReadWrite,
            Protection::ReadWrite,
            RegionOwner::AddressSpace,
        ),
    );
    let mut request = anonymous_map(
        5,
        BASE + 3 * PAGE_SIZE,
        1,
        0,
        0,
        Protection::ReadOnly,
        Protection::ReadOnly,
        RegionOwner::AddressSpace,
    );
    request.result = Some(UserWriteLeaseRequest {
        range: AddressRange::new(BASE + PAGE_SIZE - 16, 32).unwrap(),
    });
    assert_eq!(
        space.validate_map(request).unwrap_err(),
        ChangeError::LeaseTooLarge
    );

    let mut request = anonymous_map(
        6,
        BASE + 3 * PAGE_SIZE,
        1,
        0,
        0,
        Protection::ReadOnly,
        Protection::ReadOnly,
        RegionOwner::AddressSpace,
    );
    request.result = Some(UserWriteLeaseRequest {
        range: AddressRange::new(BASE, 129).unwrap(),
    });
    assert_eq!(
        space.validate_map(request).unwrap_err(),
        ChangeError::LeaseTooLarge
    );
}

#[test]
fn deterministic_model_preserves_coverage_and_nonoverlap() {
    const PAGES: usize = 64;
    let mut space = space();
    let mut occupied = [false; PAGES];
    let mut state = 0x6a09_e667_f3bc_c909_u64;
    let mut identity = 1_u64;

    for _ in 0..2_000 {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let start = (state as usize) % PAGES;
        let pages = (((state >> 17) as usize) % 4 + 1).min(PAGES - start);
        let range = PageRange::new(BASE + start * PAGE_SIZE, pages * PAGE_SIZE).unwrap();
        if state & (1 << 63) == 0 {
            let expected = occupied[start..start + pages].iter().all(|page| !*page);
            let result = space.validate_map(anonymous_map(
                identity,
                range.start(),
                pages,
                0,
                0,
                Protection::ReadOnly,
                Protection::ReadOnly,
                RegionOwner::AddressSpace,
            ));
            identity += 1;
            if expected {
                let prepared = reserve_no_permits(&mut space, result.unwrap());
                complete_prepared(&mut space, prepared);
                occupied[start..start + pages].fill(true);
            } else {
                assert_eq!(result.unwrap_err(), ChangeError::Conflict);
            }
        } else {
            let expected = occupied[start..start + pages].iter().all(|page| *page);
            let result = space.validate_unmap(UnmapRequest {
                range,
                authority: RegionOwner::AddressSpace,
            });
            if expected {
                let prepared = reserve_no_permits(&mut space, result.unwrap());
                complete_prepared(&mut space, prepared);
                occupied[start..start + pages].fill(false);
            } else {
                assert_eq!(result.unwrap_err(), ChangeError::NotCovered);
            }
        }

        let regions: Vec<_> = space.regions().collect();
        assert!(
            regions
                .windows(2)
                .all(|pair| pair[0].range.end() <= pair[1].range.start())
        );
        for (page, expected) in occupied.iter().copied().enumerate() {
            let address = BASE + page * PAGE_SIZE;
            assert_eq!(
                !matches!(space.fault_class(address), FaultClass::Free),
                expected
            );
        }
    }
}

#[test]
fn drain_one_removes_exactly_one_region_without_allocation() {
    let mut space = space();
    commit_map(
        &mut space,
        anonymous_map(
            90,
            BASE + PAGE_SIZE,
            2,
            1,
            1,
            Protection::ReadWrite,
            Protection::ReadWrite,
            RegionOwner::AddressSpace,
        ),
    );
    assert_eq!(space.region_count(), 3);
    let allocation = region_allocation(&space);
    for remaining in (0..3).rev() {
        let (fragment, permit) = space.drain_one().unwrap();
        assert_eq!(fragment.allocation, allocation);
        assert!(permit.is_none());
        assert_eq!(space.region_count(), remaining);
    }
    assert!(space.drain_one().is_none());
}

#[test]
#[should_panic(expected = "memory change cannot leave Retiring with live retire owners")]
fn retiring_stage_cannot_complete_with_live_owners() {
    let mut space = space();
    let layout = commit_map(
        &mut space,
        anonymous_map(
            99,
            BASE,
            1,
            0,
            0,
            Protection::ReadWrite,
            Protection::ReadWrite,
            RegionOwner::AddressSpace,
        ),
    );
    let validated = space
        .validate_unmap(UnmapRequest {
            range: layout.usable,
            authority: RegionOwner::AddressSpace,
        })
        .unwrap();
    let prepared = reserve_no_permits(&mut space, validated);
    let committed = space.commit(prepared);
    let published = space.publish(committed);
    let synchronized = space.synchronize(published);
    let (retiring, batch) = space.begin_retire(synchronized);
    assert!(!batch.is_empty());
    let _ = space.finish_retire(retiring, &batch);
}
