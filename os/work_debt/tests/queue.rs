use work_debt::{ReserveError, WorkDebts};

type Debts = WorkDebts<u64, 3, 4>;

#[test]
fn reservation_is_global_and_rollback_is_exact() {
    let mut debts = Debts::new();
    let reservations = [
        debts.reserve().unwrap(),
        debts.reserve().unwrap(),
        debts.reserve().unwrap(),
        debts.reserve().unwrap(),
    ];
    assert_eq!(debts.reserve(), Err(ReserveError::Full));
    assert_eq!(debts.available(), 0);
    for reservation in reservations {
        assert!(debts.cancel(reservation).is_ok());
    }
    assert_eq!(debts.available(), 4);
}

#[test]
fn owner_queues_are_fifo_and_independent() {
    let mut debts = Debts::new();
    let first = debts.reserve().unwrap();
    let second = debts.reserve().unwrap();
    let other = debts.reserve().unwrap();
    debts.publish(first, 1, 10).unwrap();
    debts.publish(other, 2, 30).unwrap();
    debts.publish(second, 1, 20).unwrap();

    let (token, value) = debts.take(1).unwrap().into_parts();
    assert_eq!(value, 10);
    assert!(debts.finish(token).is_ok());
    let (token, value) = debts.take(1).unwrap().into_parts();
    assert_eq!(value, 20);
    assert!(debts.finish(token).is_ok());
    let (token, value) = debts.take(2).unwrap().into_parts();
    assert_eq!(value, 30);
    assert!(debts.finish(token).is_ok());
}

#[test]
fn minimum_budget_requeues_without_starving_peers() {
    let mut debts = Debts::new();
    let long = debts.reserve().unwrap();
    let short = debts.reserve().unwrap();
    debts.publish(long, 0, 3).unwrap();
    debts.publish(short, 0, 100).unwrap();

    let mut completed = Vec::new();
    while debts.has_pending(0) {
        let (token, remaining) = debts.take(0).unwrap().into_parts();
        if remaining == 1 || remaining == 100 {
            completed.push(remaining);
            assert!(debts.finish(token).is_ok());
        } else {
            debts.requeue(token, remaining - 1).unwrap();
        }
    }
    assert_eq!(completed, vec![100, 1]);
    assert_eq!(debts.available(), 4);
}

#[test]
fn pending_level_survives_missing_and_duplicate_doorbells() {
    let mut debts = Debts::new();
    let reservation = debts.reserve().unwrap();
    debts.publish(reservation, 2, 7).unwrap();
    assert!(debts.has_pending(2));
    assert!(debts.has_pending(2));

    let (token, value) = debts.take(2).unwrap().into_parts();
    assert_eq!(value, 7);
    assert!(!debts.has_pending(2));
    debts.requeue(token, value).unwrap();
    assert!(debts.has_pending(2));
}

#[test]
fn rearm_preserves_the_affine_capacity_owner() {
    let mut debts = Debts::new();
    let reservation = debts.reserve().unwrap();
    let generation = reservation.generation();
    debts.publish(reservation, 0, 1).unwrap();
    let (token, value) = debts.take(0).unwrap().into_parts();
    assert_eq!(value, 1);

    let reservation = debts.rearm(token).unwrap();
    assert_eq!(reservation.generation(), generation);
    assert_eq!(debts.available(), 3);
    debts.publish(reservation, 1, 2).unwrap();
    let (token, value) = debts.take(1).unwrap().into_parts();
    assert_eq!(value, 2);
    assert!(debts.finish(token).is_ok());
    assert_eq!(debts.available(), 4);
}

#[test]
fn foreign_table_rejects_tokens_without_losing_owners() {
    let mut first: WorkDebts<u32, 1, 1> = WorkDebts::new_with_id(work_debt::TableId::new(41));
    let mut second: WorkDebts<u32, 1, 1> = WorkDebts::new_with_id(work_debt::TableId::new(42));

    let reservation = first.reserve().unwrap();
    let reservation = second.cancel(reservation).unwrap_err();
    assert!(first.cancel(reservation).is_ok());

    let reservation = first.reserve().unwrap();
    let error = second.publish(reservation, 0, 7).unwrap_err();
    let (reservation, value) = error.into_parts();
    assert_eq!(value, 7);
    assert!(first.cancel(reservation).is_ok());

    let reservation = first.reserve().unwrap();
    first.publish(reservation, 0, 9).unwrap();
    let (token, value) = first.take(0).unwrap().into_parts();
    let error = second.requeue(token, value).unwrap_err();
    let (token, value) = error.into_parts();
    assert_eq!(value, 9);
    first.requeue(token, value).unwrap();
    let (token, value) = first.take(0).unwrap().into_parts();
    assert_eq!(value, 9);
    let token = second.finish(token).unwrap_err();
    assert!(first.finish(token).is_ok());
}

#[test]
fn cancellation_storm_preserves_capacity_and_fifo_progress() {
    let mut debts = Debts::new();
    let long = debts.reserve().unwrap();
    debts.publish(long, 0, 64).unwrap();

    for _ in 0..1_024 {
        let cancelled = debts.reserve().unwrap();
        assert!(debts.cancel(cancelled).is_ok());
    }
    for _ in 0..64 {
        let (token, remaining) = debts.take(0).unwrap().into_parts();
        if remaining == 1 {
            assert!(debts.finish(token).is_ok());
        } else {
            debts.requeue(token, remaining - 1).unwrap();
        }
    }
    assert!(!debts.has_pending(0));
    assert_eq!(debts.available(), 4);
}

#[test]
fn generation_advances_before_slot_reuse() {
    let mut debts = Debts::new();
    let reservation = debts.reserve_in(0..1).unwrap();
    let generation = reservation.generation();
    debts.publish(reservation, 0, 1).unwrap();
    let (token, _) = debts.take(0).unwrap().into_parts();
    assert_eq!(token.generation(), generation);
    assert!(debts.finish(token).is_ok());

    let next = debts.reserve_in(0..1).unwrap();
    assert_ne!(next.generation(), generation);
    assert!(debts.cancel(next).is_ok());
}
