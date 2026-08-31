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
        assert!(debts.cancel(reservation));
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
    assert!(debts.finish(token));
    let (token, value) = debts.take(1).unwrap().into_parts();
    assert_eq!(value, 20);
    assert!(debts.finish(token));
    let (token, value) = debts.take(2).unwrap().into_parts();
    assert_eq!(value, 30);
    assert!(debts.finish(token));
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
            assert!(debts.finish(token));
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
fn generation_advances_before_slot_reuse() {
    let mut debts = Debts::new();
    let reservation = debts.reserve().unwrap();
    let generation = reservation.generation();
    debts.publish(reservation, 0, 1).unwrap();
    let (token, _) = debts.take(0).unwrap().into_parts();
    assert_eq!(token.generation(), generation);
    assert!(debts.finish(token));

    let next = debts.reserve().unwrap();
    assert_ne!(next.generation(), generation);
    assert!(debts.cancel(next));
}
