use work_debt::FairBudget;

#[test]
fn four_pending_classes_each_receive_an_execution_opportunity() {
    let mut budget = FairBudget::new(16, 4, [true; 4]);
    let mut served = [0; 4];

    for (class, service) in served.iter_mut().enumerate() {
        while budget.turn(class) != 0 {
            let work = if class == 0 { budget.turn(class) } else { 1 };
            budget.charge(class, work);
            *service += work;
            if class != 0 {
                break;
            }
        }
    }

    assert_eq!(served, [13, 1, 1, 1]);
    assert_eq!(budget.used(), 16);
}

#[test]
fn absent_entry_classes_do_not_reserve_budget() {
    let mut budget = FairBudget::new(16, 4, [true, false, false, true]);
    let first = budget.remaining(0);
    assert_eq!(first, 15);
    budget.charge(0, first);
    assert_eq!(budget.remaining(1), 0);
    assert_eq!(budget.remaining(2), 0);
    assert_eq!(budget.remaining(3), 1);
    budget.charge(3, 1);
    assert_eq!(budget.used(), 16);
}

#[test]
fn sustained_backlog_repeats_the_same_bounded_service() {
    for _ in 0..64 {
        let mut budget = FairBudget::new(16, 4, [true; 4]);
        for class in 0..4 {
            let work = budget.turn(class).min(1);
            assert_eq!(work, 1);
            budget.charge(class, work);
        }
        assert_eq!(budget.used(), 4);
    }
}
