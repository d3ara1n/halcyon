use core::{
    alloc::Layout,
    cell::{Cell, RefCell},
};
use funded_storage::{Buffer, Funding, OwnedBox, StorageError};
use std::rc::Rc;

#[derive(Default)]
struct State {
    live: usize,
    events: Vec<&'static str>,
}
struct Payer {
    state: Rc<RefCell<State>>,
    budget: usize,
}
struct Charge {
    state: Rc<RefCell<State>>,
    bytes: usize,
}
impl Drop for Charge {
    fn drop(&mut self) {
        let mut state = self.state.borrow_mut();
        state.live -= self.bytes;
        state.events.push("refund");
    }
}
impl Funding for Payer {
    type Charge = Charge;
    type Error = ();
    fn charge(&self, layout: Layout) -> Result<Charge, ()> {
        let mut state = self.state.borrow_mut();
        let live = state.live.checked_add(layout.size()).ok_or(())?;
        if live > self.budget {
            return Err(());
        }
        state.live = live;
        state.events.push("charge");
        Ok(Charge {
            state: self.state.clone(),
            bytes: layout.size(),
        })
    }
}
fn payer(budget: usize) -> Payer {
    Payer {
        state: Rc::new(RefCell::new(State::default())),
        budget,
    }
}
struct Value {
    id: usize,
    state: Rc<RefCell<State>>,
}
impl Drop for Value {
    fn drop(&mut self) {
        assert!(
            self.state.borrow().live > 0,
            "value outlived its storage payment"
        );
        self.state.borrow_mut().events.push("value");
    }
}

#[test]
fn fixed_layout_capacity_and_retained_empty_storage_are_paid() {
    let funds = payer(16);
    let mut buffer = Buffer::<u64, Charge>::try_empty(2, &funds).unwrap();
    assert_eq!(buffer.capacity(), 2);
    assert_eq!(funds.state.borrow().live, 16);
    buffer.push(1);
    buffer.push(2);
    assert_eq!(buffer.try_push(3), Err(3));
    buffer.clear();
    assert_eq!(funds.state.borrow().live, 16);
    drop(buffer);
    assert_eq!(funds.state.borrow().live, 0);
}

#[test]
fn failed_charge_does_not_change_live_storage_or_value() {
    let funds = payer(8);
    let mut live = Buffer::<u64, Charge>::try_empty(1, &funds).unwrap();
    live.push(7);
    assert!(matches!(
        Buffer::<u64, Charge>::try_empty(2, &funds),
        Err(StorageError::Funding(()))
    ));
    assert_eq!(&*live, &[7]);
    assert_eq!(funds.state.borrow().live, 8);
    assert!(matches!(
        Buffer::<u64, Charge>::try_empty(usize::MAX, &funds),
        Err(StorageError::Layout)
    ));
    let error = OwnedBox::<u64, Charge>::try_new(99, &funds).err().unwrap();
    assert_eq!(error, (StorageError::Funding(()), 99));
}

#[test]
fn allocation_failure_refunds_accepted_charge_and_preserves_existing_storage() {
    let funds = payer(usize::MAX);
    let mut live = Buffer::<u8, Charge>::try_empty(1, &funds).unwrap();
    live.push(7);
    let baseline = funds.state.borrow().live;
    funds.state.borrow_mut().events.clear();

    // Layout 合法，但 host 地址空间不能容纳 isize::MAX 字节的单块。
    // 不耗尽物理内存、不注入全局 allocator，也不把付款拒绝当分配失败。
    assert!(matches!(
        live.try_reserve_exact(isize::MAX as usize - live.len(), &funds),
        Err(StorageError::Allocation)
    ));
    assert_eq!(&*live, &[7]);
    assert_eq!(live.capacity(), 1);
    assert_eq!(funds.state.borrow().live, baseline);
    assert_eq!(funds.state.borrow().events, ["charge", "refund"]);
    drop(live);
    assert_eq!(funds.state.borrow().live, 0);
}

#[test]
fn explicit_growth_requires_peak_payment_and_retains_empty_capacity() {
    let first = payer(47);
    let second = payer(32);
    let mut live = Buffer::<u64, Charge>::try_empty(2, &first).unwrap();
    live.push(11);
    live.push(22);
    // 16 字节旧块加 32 字节目标块的峰值为 48，禁止先退旧块来绕过预算。
    assert!(matches!(
        live.try_reserve_exact(2, &first),
        Err(StorageError::Funding(()))
    ));
    assert_eq!(&*live, &[11, 22]);
    assert_eq!(live.capacity(), 2);
    assert_eq!(first.state.borrow().live, 16);
    live.try_reserve_exact(2, &second).unwrap();
    assert_eq!(&*live, &[11, 22]);
    assert_eq!(first.state.borrow().live, 0);
    assert_eq!(second.state.borrow().live, 32);
    live.truncate(1);
    assert_eq!(&*live, &[11]);
    live.try_reserve_exact(2, &first).unwrap();
    assert_eq!(first.state.borrow().live, 0);
    live.clear();
    assert_eq!(second.state.borrow().live, 32);
    drop(live);
    assert_eq!(second.state.borrow().live, 0);
}

#[test]
fn replacement_preserves_indices_and_separate_payment_until_retirement() {
    let first = payer(24);
    let second = payer(40);
    let mut live = Buffer::<u64, Charge>::try_empty(3, &first).unwrap();
    live.push(1);
    live.push(2);
    live.push(3);
    let mut next = Buffer::<u64, Charge>::try_empty(5, &second).unwrap();
    live.replace_storage(&mut next).unwrap();
    assert_eq!(&*live, &[1, 2, 3]);
    assert!(next.is_empty());
    assert_eq!(live.capacity(), 5);
    assert_eq!(next.capacity(), 3);
    assert_eq!(first.state.borrow().live, 24);
    assert_eq!(second.state.borrow().live, 40);
    drop(next);
    assert_eq!(first.state.borrow().live, 0);
    assert_eq!(second.state.borrow().live, 40);
    drop(live);
    assert_eq!(second.state.borrow().live, 0);
}

#[test]
fn insufficient_replacement_leaves_both_owners_unchanged() {
    let funds = payer(24);
    let mut live = Buffer::<u64, Charge>::try_empty(2, &funds).unwrap();
    live.push(11);
    live.push(22);
    let mut next = Buffer::<u64, Charge>::try_empty(1, &funds).unwrap();
    assert!(live.replace_storage(&mut next).is_err());
    assert_eq!(&*live, &[11, 22]);
    assert!(next.is_empty());
    assert_eq!(funds.state.borrow().live, 24);
}

#[test]
fn removal_and_partial_iterator_drop_each_value_once_before_refund() {
    let funds = payer(4096);
    let mut live = Buffer::<Value, Charge>::try_empty(4, &funds).unwrap();
    for id in 0..4 {
        live.push(Value {
            id,
            state: funds.state.clone(),
        });
    }
    let removed = live.remove(1);
    assert_eq!(removed.id, 1);
    assert_eq!(live.iter().map(|v| v.id).collect::<Vec<_>>(), [0, 2, 3]);
    drop(removed);
    let mut iter = live.into_iter();
    assert_eq!(iter.len(), 3);
    let front = iter.next().unwrap();
    let back = iter.next_back().unwrap();
    assert_eq!(front.id, 0);
    assert_eq!(back.id, 3);
    drop(front);
    drop(back);
    drop(iter);
    let state = funds.state.borrow();
    assert_eq!(state.events.iter().filter(|e| **e == "value").count(), 4);
    assert_eq!(state.events.last(), Some(&"refund"));
    assert_eq!(state.live, 0);
}

#[test]
fn fixed_box_returns_value_on_failure_and_refunds_after_value_drop() {
    let funds = payer(4096);
    let boxed = OwnedBox::<Value, Charge>::try_new(
        Value {
            id: 1,
            state: funds.state.clone(),
        },
        &funds,
    )
    .map_err(|_| ())
    .unwrap();
    drop(boxed);
    assert_eq!(funds.state.borrow().events, ["charge", "value", "refund"]);
    let boxed = OwnedBox::<u64, Charge>::try_new(7, &funds).unwrap();
    assert_eq!(boxed.into_inner(), 7);
    assert_eq!(funds.state.borrow().live, 0);
}

#[test]
fn zero_sized_values_need_no_storage_charge_and_still_drop() {
    thread_local! { static DROPS: Cell<usize> = const { Cell::new(0) }; }
    struct Zero;
    impl Drop for Zero {
        fn drop(&mut self) {
            DROPS.with(|n| n.set(n.get() + 1));
        }
    }
    let funds = payer(0);
    let mut values = Buffer::<Zero, Charge>::try_empty(3, &funds).unwrap();
    for _ in 0..3 {
        values.push(Zero);
    }
    let mut next = Buffer::<Zero, Charge>::try_empty(4, &funds).unwrap();
    values.replace_storage(&mut next).unwrap();
    drop(next);
    drop(values);
    assert_eq!(DROPS.with(Cell::get), 3);
    assert!(funds.state.borrow().events.is_empty());
}
