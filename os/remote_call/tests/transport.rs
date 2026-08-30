use remote_call::{RemoteCalls, ReserveError};

type Calls = RemoteCalls<u64, 3, 2>;

#[test]
fn reserve_is_fixed_per_target_and_rollback_is_exact() {
    let mut calls = Calls::new();
    let first = calls.reserve(1).unwrap();
    let second = calls.reserve(1).unwrap();
    assert_eq!(calls.reserve(1), Err(ReserveError::Full));
    assert_eq!(calls.available(1), Some(0));
    assert!(calls.cancel(first));
    assert_eq!(calls.available(1), Some(1));
    assert!(calls.cancel(second));
    assert_eq!(calls.available(1), Some(2));
    assert_eq!(calls.reserve(3), Err(ReserveError::InvalidTarget));
}

#[test]
fn pending_level_survives_missing_and_duplicate_doorbells() {
    let mut calls = Calls::new();
    let reservation = calls.reserve(2).unwrap();
    calls.publish(reservation, 41).unwrap();
    assert!(calls.has_pending(2));
    assert!(calls.has_pending(2));

    let taken = calls.take(2).unwrap();
    assert!(!calls.has_pending(2));
    let (token, value) = taken.into_parts();
    assert_eq!(value, 41);
    assert!(calls.finish(token));
    assert_eq!(calls.available(2), Some(2));
}

#[test]
fn targets_and_completion_order_are_independent() {
    let mut calls = Calls::new();
    let zero = calls.reserve(0).unwrap();
    let two = calls.reserve(2).unwrap();
    calls.publish(zero, 10).unwrap();
    calls.publish(two, 12).unwrap();

    let (two_token, two_value) = calls.take(2).unwrap().into_parts();
    assert_eq!(two_value, 12);
    assert!(calls.finish(two_token));
    assert!(calls.has_pending(0));

    let (zero_token, zero_value) = calls.take(0).unwrap().into_parts();
    assert_eq!(zero_value, 10);
    assert!(calls.finish(zero_token));
}

#[test]
fn slot_generation_advances_before_reuse() {
    let mut calls = Calls::new();
    let first = calls.reserve(0).unwrap();
    let first_generation = first.generation();
    calls.publish(first, 1).unwrap();
    let (finish, _) = calls.take(0).unwrap().into_parts();
    assert_eq!(finish.generation(), first_generation);
    assert!(calls.finish(finish));

    let second = calls.reserve(0).unwrap();
    assert_ne!(second.generation(), first_generation);
    assert!(calls.cancel(second));
}

#[test]
fn taking_is_bounded_by_caller_budget() {
    let mut calls = Calls::new();
    let first = calls.reserve(1).unwrap();
    let second = calls.reserve(1).unwrap();
    calls.publish(first, 1).unwrap();
    calls.publish(second, 2).unwrap();

    let (token, value) = calls.take(1).unwrap().into_parts();
    assert_eq!(value, 1);
    assert!(calls.finish(token));
    assert!(calls.has_pending(1));
}
