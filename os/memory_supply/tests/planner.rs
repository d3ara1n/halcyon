use memory_supply::{PlanError, Planner, Range, Requirements};

const PAGE: usize = 4096;

fn range(start: usize, end: usize) -> Range {
    Range::new(start, end).unwrap()
}

fn requirements(heap_chunks: usize, recovery_tickets: usize) -> Requirements {
    Requirements {
        page_size: PAGE,
        metadata_bytes: 6000,
        heap_chunk_size: 1 << 20,
        heap_chunk_count: heap_chunks,
        recovery_ticket_size: PAGE * 2,
        recovery_ticket_count: recovery_tickets,
    }
}

#[test]
fn classification_closes_and_prioritizes_permanent_over_boot() {
    let managed = [range(0x1000_0000, 0x1100_0000)];
    let permanent = [range(0x1000_0000, 0x1020_0000)];
    let boot = [
        range(0x1010_0000, 0x1030_0000),
        range(0x10f0_0000, 0x1100_0000),
    ];
    let mut planner = Planner::<32, 2, 1>::new();

    let plan = planner
        .plan(&managed, &permanent, &boot, requirements(2, 1))
        .unwrap();
    let (inventory, mut system) = plan.into_parts();

    assert_eq!(inventory.permanent(), &permanent);
    assert_eq!(
        inventory.boot_held(),
        &[
            range(0x1020_0000, 0x1030_0000),
            range(0x10f0_0000, 0x1100_0000),
        ]
    );
    assert_eq!(
        inventory.managed_bytes(),
        inventory.permanent_bytes()
            + inventory.boot_held_bytes()
            + inventory.system_bytes()
            + inventory.user_free_bytes()
    );
    assert_eq!(system.metadata().range(), range(0x1030_0000, 0x1030_2000));
    assert_eq!(
        system.take_heap_chunk().unwrap().range(),
        range(0x1040_0000, 0x1050_0000)
    );
    assert_eq!(
        system.take_heap_chunk().unwrap().range(),
        range(0x1050_0000, 0x1060_0000)
    );
    assert_eq!(
        system.take_recovery_ticket().unwrap().range(),
        range(0x1030_2000, 0x1030_4000)
    );
    assert_eq!(system.remaining_heap_chunks(), 0);
    assert_eq!(system.remaining_recovery_tickets(), 0);
}

#[test]
fn fragmented_supply_places_each_heap_chunk_independently() {
    let managed = [
        range(0x1000_0000, 0x1020_0000),
        range(0x2000_0000, 0x2020_0000),
    ];
    let permanent = [range(0x1000_2000, 0x1010_0000)];
    let mut planner = Planner::<32, 2, 0>::new();

    let plan = planner
        .plan(&managed, &permanent, &[], requirements(2, 0))
        .unwrap();
    let (_, mut system) = plan.into_parts();

    assert_eq!(
        system.take_heap_chunk().unwrap().range(),
        range(0x1010_0000, 0x1020_0000)
    );
    assert_eq!(
        system.take_heap_chunk().unwrap().range(),
        range(0x2000_0000, 0x2010_0000)
    );
}

#[test]
fn insufficient_subbudget_fails_without_partial_plan() {
    let managed = [range(0x1000_0000, 0x1010_0000)];
    let mut planner = Planner::<16, 2, 0>::new();
    assert!(matches!(
        planner.plan(&managed, &[], &[], requirements(2, 0)),
        Err(PlanError::InsufficientHeap)
    ));
}

#[test]
fn failed_workspace_can_be_replanned_atomically() {
    let mut planner = Planner::<16, 2, 0>::new();
    assert!(
        planner
            .plan(
                &[range(0x1000_0000, 0x1010_0000)],
                &[],
                &[],
                requirements(2, 0),
            )
            .is_err()
    );
    let plan = planner
        .plan(
            &[range(0x1000_0000, 0x1040_0000)],
            &[],
            &[],
            requirements(2, 0),
        )
        .unwrap();
    assert_eq!(plan.into_parts().1.remaining_heap_chunks(), 2);
}

#[test]
fn fixed_output_capacity_is_enforced() {
    let managed = [
        range(0x1000, 0x2000),
        range(0x3000, 0x4000),
        range(0x5000, 0x6000),
    ];
    let mut planner = Planner::<2, 0, 0>::new();
    assert!(matches!(
        planner.plan(&managed, &[], &[], requirements(0, 0)),
        Err(PlanError::CapacityExhausted)
    ));
}

#[test]
fn ticket_consumption_is_monotonic_and_purpose_specific() {
    let managed = [range(0x4000_0000, 0x4080_0000)];
    let mut planner = Planner::<16, 3, 2>::new();
    let plan = planner
        .plan(&managed, &[], &[], requirements(3, 2))
        .unwrap();
    let (_, mut system) = plan.into_parts();

    let heap: Vec<_> = (0..3)
        .map(|_| system.take_heap_chunk().unwrap().range())
        .collect();
    assert!(heap.windows(2).all(|pair| pair[0].end() <= pair[1].start()));
    assert!(system.take_heap_chunk().is_none());

    let recovery: Vec<_> = (0..2)
        .map(|_| system.take_recovery_ticket().unwrap().range())
        .collect();
    assert!(
        recovery
            .windows(2)
            .all(|pair| pair[0].end() <= pair[1].start())
    );
    assert!(system.take_recovery_ticket().is_none());
}

#[test]
fn malformed_or_overlapping_memory_is_rejected() {
    let mut planner = Planner::<8, 0, 0>::new();
    assert_eq!(
        planner
            .plan(
                &[range(0x1000, 0x4000), range(0x3000, 0x5000)],
                &[],
                &[],
                requirements(0, 0),
            )
            .err(),
        Some(PlanError::MemoryOverlap)
    );
    assert_eq!(
        planner
            .plan(&[range(0x1001, 0x4000)], &[], &[], requirements(0, 0),)
            .err(),
        Some(PlanError::InvalidRange)
    );
}
