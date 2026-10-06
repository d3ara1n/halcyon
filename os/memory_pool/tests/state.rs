use memory_pool::{LedgerState, MAX_DEPTH, PoolError, PoolId};
use std::sync::{Arc, Barrier, Mutex};

fn id(raw: u64) -> PoolId {
    PoolId::new(raw).unwrap()
}

#[test]
fn charge_commit_and_return_preserve_equation() {
    let mut pool = LedgerState::root(id(1), 32).unwrap();
    let reservation = pool.reserve_charge(7).unwrap();
    assert_eq!(pool.snapshot().available, 25);
    assert_eq!(pool.snapshot().reserved, 7);

    let charge = pool.commit_charge(reservation).unwrap();
    assert_eq!(pool.snapshot().reserved, 0);
    assert_eq!(pool.snapshot().allocated, 7);
    pool.return_charge(charge).unwrap();
    assert!(pool.is_fully_available());
    assert!(pool.snapshot().closes());
}

#[test]
fn rollback_restores_available_credit() {
    let mut pool = LedgerState::root(id(1), 5).unwrap();
    let reservation = pool.reserve_charge(5).unwrap();
    pool.rollback_charge(reservation).unwrap();
    assert!(pool.is_fully_available());
}

#[test]
fn quota_zero_and_maximum_are_checked() {
    assert!(LedgerState::root(id(1), 0).unwrap().is_fully_available());
    let mut pool = LedgerState::root(id(1), u64::MAX).unwrap();
    assert_eq!(pool.reserve_charge(0).unwrap_err(), PoolError::ZeroAmount);
    assert_eq!(pool.reserve_charge(u64::MAX).unwrap().units(), u64::MAX);
    assert_eq!(
        pool.reserve_charge(1).unwrap_err(),
        PoolError::QuotaExceeded
    );
}

#[test]
fn child_credit_returns_to_parent() {
    let mut parent = LedgerState::root(id(1), 16).unwrap();
    let reservation = parent.reserve_delegation(6).unwrap();
    let prepared = LedgerState::prepare_child(id(2), reservation).unwrap();
    let child = parent.commit_child(prepared).unwrap();

    assert_eq!(child.snapshot().parent_identity, Some(id(1)));
    assert_eq!(child.snapshot().depth, 2);
    assert_eq!(parent.snapshot().delegated, 6);
    assert!(child.is_fully_available());

    let credit = child.into_parent_credit().unwrap().unwrap();
    parent.return_delegation(credit).unwrap();
    assert!(parent.is_fully_available());
}

#[test]
fn prepared_child_rollback_discards_the_only_child_state() {
    let mut parent = LedgerState::root(id(1), 8).unwrap();
    let reservation = parent.reserve_delegation(5).unwrap();
    let prepared = LedgerState::prepare_child(id(2), reservation).unwrap();
    parent.rollback_child(prepared).unwrap();
    assert!(parent.is_fully_available());
}

#[test]
fn invalid_child_topology_preserves_the_parent_reservation() {
    let mut parent = LedgerState::root(id(1), 8).unwrap();
    let reservation = parent.reserve_delegation(5).unwrap();
    let error = LedgerState::prepare_child(id(1), reservation).unwrap_err();
    assert_eq!(error.error(), PoolError::InvalidTopology);
    parent.rollback_delegation(error.into_token()).unwrap();
    assert!(parent.is_fully_available());
}

#[test]
fn duplicate_diagnostic_identity_cannot_accept_another_instances_token() {
    let mut owner = LedgerState::root(id(1), 8).unwrap();
    let mut impostor = LedgerState::root(id(1), 8).unwrap();
    let reservation = owner.reserve_charge(3).unwrap();
    let error = impostor.rollback_charge(reservation).unwrap_err();
    assert_eq!(error.error(), PoolError::WrongOwner);
    owner.rollback_charge(error.into_token()).unwrap();
}

#[test]
fn parent_credit_requires_consuming_a_quiescent_child() {
    let mut parent = LedgerState::root(id(1), 8).unwrap();
    let reservation = parent.reserve_delegation(5).unwrap();
    let prepared = LedgerState::prepare_child(id(2), reservation).unwrap();
    let mut child = parent.commit_child(prepared).unwrap();
    let reservation = child.reserve_charge(1).unwrap();
    let charge = child.commit_charge(reservation).unwrap();

    let error = child.into_parent_credit().unwrap_err();
    assert_eq!(error.error(), PoolError::InvariantViolation);
    let mut child = error.into_token();
    child.return_charge(charge).unwrap();
    let credit = child.into_parent_credit().unwrap().unwrap();
    parent.return_delegation(credit).unwrap();
    assert!(parent.is_fully_available());
}

#[test]
fn wrong_owner_returns_the_token_unchanged() {
    let mut first = LedgerState::root(id(1), 8).unwrap();
    let mut second = LedgerState::root(id(2), 8).unwrap();
    let reservation = first.reserve_charge(3).unwrap();
    let error = second.rollback_charge(reservation).unwrap_err();
    assert_eq!(error.error(), PoolError::WrongOwner);
    let reservation = error.into_token();
    assert_eq!(reservation.units(), 3);
    first.rollback_charge(reservation).unwrap();
    assert!(first.is_fully_available());
}

#[test]
fn allocated_credit_split_and_merge_are_conservative() {
    let mut pool = LedgerState::root(id(1), 12).unwrap();
    let reservation = pool.reserve_charge(12).unwrap();
    let mut charge = pool.commit_charge(reservation).unwrap();
    assert_eq!(charge.split(12).unwrap_err(), PoolError::InvalidSplit);
    let split = charge.split(5).unwrap();
    assert_eq!(charge.units(), 7);
    assert_eq!(split.units(), 5);
    charge.merge(split).unwrap();
    assert_eq!(charge.units(), 12);
    pool.return_charge(charge).unwrap();
    assert!(pool.is_fully_available());
}

#[test]
fn wrong_owner_merge_preserves_both_credits() {
    let mut first = LedgerState::root(id(1), 4).unwrap();
    let mut second = LedgerState::root(id(2), 6).unwrap();
    let first_reservation = first.reserve_charge(4).unwrap();
    let second_reservation = second.reserve_charge(6).unwrap();
    let mut first_credit = first.commit_charge(first_reservation).unwrap();
    let second_credit = second.commit_charge(second_reservation).unwrap();

    let error = first_credit.merge(second_credit).unwrap_err();
    assert_eq!(error.error(), PoolError::WrongOwner);
    let second_credit = error.into_token();
    first.return_charge(first_credit).unwrap();
    second.return_charge(second_credit).unwrap();
}

#[test]
fn depth_limit_is_owned_by_the_state_machine() {
    let mut states = vec![LedgerState::root(id(1), 1).unwrap()];
    for raw in 2..=MAX_DEPTH as u64 {
        let parent = states.last_mut().unwrap();
        let reservation = parent.reserve_delegation(1).unwrap();
        let prepared = LedgerState::prepare_child(id(raw), reservation).unwrap();
        let child = parent.commit_child(prepared).unwrap();
        states.push(child);
    }
    assert_eq!(states.last().unwrap().snapshot().depth, MAX_DEPTH);
    assert_eq!(
        states
            .last_mut()
            .unwrap()
            .reserve_delegation(1)
            .unwrap_err(),
        PoolError::DepthLimit
    );

    while states.len() > 1 {
        let child = states.pop().unwrap();
        assert!(child.is_fully_available());
        let credit = child.into_parent_credit().unwrap().unwrap();
        states
            .last_mut()
            .unwrap()
            .return_delegation(credit)
            .unwrap();
    }
    assert!(states[0].is_fully_available());
}

#[test]
fn forgotten_reservation_only_leaks_quota() {
    let mut pool = LedgerState::root(id(1), 2).unwrap();
    let _forgotten_reservation = pool.reserve_charge(1).unwrap();
    assert_eq!(pool.snapshot().available, 1);
    assert_eq!(pool.snapshot().reserved, 1);
    assert!(pool.reserve_charge(2).is_err());
}

#[test]
fn concurrent_reservations_never_overdraw() {
    const TOTAL: u64 = 16;
    const THREADS: usize = 64;

    let pool = Arc::new(Mutex::new(LedgerState::root(id(1), TOTAL).unwrap()));
    let barrier = Arc::new(Barrier::new(THREADS));
    let mut workers = Vec::new();
    for _ in 0..THREADS {
        let pool = Arc::clone(&pool);
        let barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            pool.lock().unwrap().reserve_charge(1).ok()
        }));
    }

    let reservations: Vec<_> = workers
        .into_iter()
        .filter_map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(reservations.len(), TOTAL as usize);
    let mut pool = pool.lock().unwrap();
    assert_eq!(pool.snapshot().available, 0);
    for reservation in reservations {
        pool.rollback_charge(reservation).unwrap();
    }
    assert!(pool.is_fully_available());
}

#[test]
fn independent_dimensions_cannot_exchange_same_pool_identity_tokens() {
    let mut pages = LedgerState::root(id(1), 4).unwrap();
    let mut metadata = LedgerState::root(id(1), 4).unwrap();
    let reservation = pages.reserve_charge(1).unwrap();
    let failure = metadata.commit_charge(reservation).unwrap_err();
    assert_eq!(failure.error(), PoolError::WrongOwner);
    pages.rollback_charge(failure.into_token()).unwrap();
    assert!(pages.is_fully_available() && metadata.is_fully_available());
}

#[test]
fn zero_dimension_delegates_identity_without_minting_units() {
    let mut parent = LedgerState::root(id(1), 0).unwrap();
    let reservation = parent.reserve_delegation(0).unwrap();
    let prepared = LedgerState::prepare_child(id(2), reservation).unwrap();
    let mut child = parent.commit_child(prepared).unwrap();
    assert_eq!(child.snapshot().total, 0);
    assert_eq!(child.snapshot().depth, 2);
    assert_eq!(
        child.reserve_charge(1).unwrap_err(),
        PoolError::QuotaExceeded
    );
    parent
        .return_delegation(child.into_parent_credit().unwrap().unwrap())
        .unwrap();
    assert!(parent.is_fully_available());
}
