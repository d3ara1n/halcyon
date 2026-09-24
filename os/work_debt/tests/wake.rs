use work_debt::{ParkResult, WakeResult, WorkDebts};

type Debts = WorkDebts<u64, 2, 2>;

#[test]
fn parked_work_retains_capacity_without_pending() {
    let mut debts = Debts::new();
    let reservation = debts.reserve().unwrap();
    debts.publish(reservation, 1, 42).unwrap();
    let (token, value) = debts.take(1).unwrap().into_parts();
    let wake = debts.arm_wake(&token).unwrap();
    assert_eq!(debts.park(token, value).unwrap(), ParkResult::Parked);
    assert!(!debts.has_pending(1));
    assert!(debts.take(1).is_none());
    assert_eq!(debts.available(), 1);
    assert_eq!(debts.wake(wake).unwrap(), WakeResult::Runnable { owner: 1 });
    let (token, value) = debts.take(1).unwrap().into_parts();
    assert_eq!(value, 42);
    assert!(debts.finish(token).is_ok());
    assert_eq!(debts.available(), 2);
}

#[test]
fn wake_before_park_is_latched_without_losing_progress() {
    let mut debts = Debts::new();
    let reservation = debts.reserve().unwrap();
    debts.publish(reservation, 0, 7).unwrap();
    let (token, value) = debts.take(0).unwrap().into_parts();
    let wake = debts.arm_wake(&token).unwrap();
    assert_eq!(debts.wake(wake).unwrap(), WakeResult::Latched);
    assert!(!debts.has_pending(0));
    assert!(debts.arm_wake(&token).is_none());
    assert_eq!(debts.park(token, value).unwrap(), ParkResult::Runnable);
    let (token, value) = debts.take(0).unwrap().into_parts();
    assert_eq!(value, 7);
    assert!(debts.finish(token).is_ok());
}

#[test]
fn outstanding_wake_prevents_slot_release_or_reuse() {
    let mut debts = Debts::new();
    let reservation = debts.reserve().unwrap();
    debts.publish(reservation, 0, 10).unwrap();
    let (token, value) = debts.take(0).unwrap().into_parts();
    let wake = debts.arm_wake(&token).unwrap();
    assert!(debts.arm_wake(&token).is_none());
    let token = debts.finish(token).unwrap_err();
    let token = debts.rearm(token).unwrap_err();
    let (token, value) = debts.requeue(token, value).unwrap_err().into_parts();
    assert!(debts.cancel_wake(wake).is_ok());
    assert_eq!(value, 10);
    assert!(debts.finish(token).is_ok());
    assert_eq!(debts.available(), 2);
}

#[test]
fn cancelling_parked_dependency_retains_the_only_wake_owner() {
    let mut debts = Debts::new();
    let reservation = debts.reserve().unwrap();
    debts.publish(reservation, 0, 13).unwrap();
    let (token, value) = debts.take(0).unwrap().into_parts();
    let wake = debts.arm_wake(&token).unwrap();
    assert_eq!(debts.park(token, value).unwrap(), ParkResult::Parked);
    let wake = debts.cancel_wake(wake).unwrap_err();
    assert_eq!(debts.wake(wake).unwrap(), WakeResult::Runnable { owner: 0 });
    let (token, value) = debts.take(0).unwrap().into_parts();
    assert_eq!(value, 13);
    assert!(debts.finish(token).is_ok());
}

#[test]
fn foreign_table_rejects_wake_without_consuming_it() {
    let mut debts = Debts::new();
    let mut other = Debts::new();
    let reservation = debts.reserve().unwrap();
    debts.publish(reservation, 0, 9).unwrap();
    let (token, value) = debts.take(0).unwrap().into_parts();
    let wake = debts.arm_wake(&token).unwrap();
    let wake = other.wake(wake).unwrap_err();
    assert_eq!(debts.park(token, value).unwrap(), ParkResult::Parked);
    assert_eq!(debts.wake(wake).unwrap(), WakeResult::Runnable { owner: 0 });
    let (token, _) = debts.take(0).unwrap().into_parts();
    assert!(debts.finish(token).is_ok());
}

#[test]
fn woke_work_joins_the_tail_without_starving_runnable_peers() {
    let mut debts = Debts::new();
    let reservation = debts.reserve().unwrap();
    debts.publish(reservation, 0, 1).unwrap();
    let (token, value) = debts.take(0).unwrap().into_parts();
    let wake = debts.arm_wake(&token).unwrap();
    assert_eq!(debts.park(token, value).unwrap(), ParkResult::Parked);
    let reservation = debts.reserve().unwrap();
    debts.publish(reservation, 0, 2).unwrap();
    assert_eq!(debts.wake(wake).unwrap(), WakeResult::Runnable { owner: 0 });
    for expected in [2, 1] {
        let (token, value) = debts.take(0).unwrap().into_parts();
        assert_eq!(value, expected);
        assert!(debts.finish(token).is_ok());
    }
}

#[test]
fn unarmed_park_returns_the_payload_and_execution_owner() {
    let mut debts = Debts::new();
    let reservation = debts.reserve().unwrap();
    debts.publish(reservation, 0, 17).unwrap();
    let (token, value) = debts.take(0).unwrap().into_parts();
    let (token, value) = debts.park(token, value).unwrap_err().into_parts();
    assert_eq!(value, 17);
    assert!(debts.finish(token).is_ok());
}
