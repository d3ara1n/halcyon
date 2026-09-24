use work_debt::{ReserveError, WorkDebts};

#[test]
fn exhausted_partition_cannot_borrow_protected_capacity() {
    let mut debts: WorkDebts<u64, 1, 6> = WorkDebts::new();
    let first = debts.reserve_in(0..2).unwrap();
    let second = debts.reserve_in(0..2).unwrap();
    assert_eq!(debts.reserve_in(0..2), Err(ReserveError::Full));
    let protected = debts.reserve_in(2..4).unwrap();
    let other = debts.reserve_in(4..6).unwrap();
    for reservation in [first, second, protected, other] {
        assert!(debts.cancel(reservation).is_ok());
    }
    assert_eq!(debts.available(), 6);
}

#[test]
fn invalid_partitions_do_not_change_admission() {
    let mut debts: WorkDebts<u64, 1, 6> = WorkDebts::new();
    assert_eq!(debts.reserve_in(0..0), Err(ReserveError::Full));
    assert_eq!(debts.reserve_in(0..7), Err(ReserveError::Full));
    let start = 5;
    let end = 2;
    assert_eq!(debts.reserve_in(start..end), Err(ReserveError::Full));
    assert_eq!(debts.available(), 6);
}
