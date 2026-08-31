use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::Rc,
};

use funded_frame::{
    FundError, Limits, PhysicalClaim, PhysicalSource, QuotaReservation, QuotaSource, fund,
};

#[derive(Debug, Default, PartialEq, Eq)]
struct QuotaState {
    available: usize,
    reserved: usize,
    allocated: usize,
}

#[derive(Clone)]
struct Quota {
    state: Rc<RefCell<QuotaState>>,
    events: Rc<RefCell<Vec<&'static str>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QuotaError {
    Exhausted,
}

struct Reservation {
    state: Rc<RefCell<QuotaState>>,
    events: Rc<RefCell<Vec<&'static str>>>,
    pages: usize,
    active: bool,
}

struct Credit {
    state: Rc<RefCell<QuotaState>>,
    events: Rc<RefCell<Vec<&'static str>>>,
    pages: usize,
}

impl QuotaSource for Quota {
    type Reservation = Reservation;
    type Error = QuotaError;

    fn reserve(&self, pages: usize) -> Result<Self::Reservation, Self::Error> {
        let mut state = self.state.borrow_mut();
        if state.available < pages {
            return Err(QuotaError::Exhausted);
        }
        state.available -= pages;
        state.reserved += pages;
        drop(state);
        Ok(Reservation {
            state: Rc::clone(&self.state),
            events: Rc::clone(&self.events),
            pages,
            active: true,
        })
    }
}

impl QuotaReservation for Reservation {
    type Credit = Credit;

    fn commit(mut self) -> Self::Credit {
        let mut state = self.state.borrow_mut();
        state.reserved -= self.pages;
        state.allocated += self.pages;
        drop(state);
        self.events.borrow_mut().push("commit");
        self.active = false;
        Credit {
            state: Rc::clone(&self.state),
            events: Rc::clone(&self.events),
            pages: self.pages,
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.active {
            self.events.borrow_mut().push("quota-reservation");
            let mut state = self.state.borrow_mut();
            state.reserved -= self.pages;
            state.available += self.pages;
        }
    }
}

impl Drop for Credit {
    fn drop(&mut self) {
        self.events.borrow_mut().push("quota");
        let mut state = self.state.borrow_mut();
        state.allocated -= self.pages;
        state.available += self.pages;
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct InventoryState {
    free: usize,
    claimed: usize,
    cleared: usize,
}

#[derive(Clone)]
struct Inventory {
    state: Rc<RefCell<InventoryState>>,
    script: Rc<RefCell<VecDeque<usize>>>,
    calls_before_failure: Rc<Cell<Option<usize>>>,
    next_claim_override: Rc<Cell<Option<usize>>>,
    events: Rc<RefCell<Vec<&'static str>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PhysicalError {
    Exhausted,
}

struct Claim {
    state: Rc<RefCell<InventoryState>>,
    events: Rc<RefCell<Vec<&'static str>>>,
    pages: usize,
}

impl PhysicalSource for Inventory {
    type Claim = Claim;
    type Error = PhysicalError;

    fn claim_largest(&self, max_pages: usize) -> Result<Self::Claim, Self::Error> {
        if let Some(remaining) = self.calls_before_failure.get() {
            if remaining == 0 {
                return Err(PhysicalError::Exhausted);
            }
            self.calls_before_failure.set(Some(remaining - 1));
        }
        let pages = self.next_claim_override.take().unwrap_or_else(|| {
            self.script
                .borrow_mut()
                .pop_front()
                .unwrap_or(max_pages)
                .min(max_pages)
        });
        let mut state = self.state.borrow_mut();
        if state.free < pages {
            return Err(PhysicalError::Exhausted);
        }
        state.free -= pages;
        state.claimed += pages;
        drop(state);
        Ok(Claim {
            state: Rc::clone(&self.state),
            events: Rc::clone(&self.events),
            pages,
        })
    }
}

impl PhysicalClaim for Claim {
    fn pages(&self) -> usize {
        self.pages
    }

    fn clear(&mut self) {
        self.state.borrow_mut().cleared += self.pages;
        self.events.borrow_mut().push("clear");
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.events.borrow_mut().push("physical");
        let mut state = self.state.borrow_mut();
        state.claimed -= self.pages;
        state.free += self.pages;
    }
}

fn fixtures(total: usize, script: &[usize]) -> (Quota, Inventory) {
    let events = Rc::new(RefCell::new(Vec::new()));
    (
        Quota {
            state: Rc::new(RefCell::new(QuotaState {
                available: total,
                ..QuotaState::default()
            })),
            events: Rc::clone(&events),
        },
        Inventory {
            state: Rc::new(RefCell::new(InventoryState {
                free: total,
                ..InventoryState::default()
            })),
            script: Rc::new(RefCell::new(script.iter().copied().collect())),
            calls_before_failure: Rc::new(Cell::new(None)),
            next_claim_override: Rc::new(Cell::new(None)),
            events,
        },
    )
}

const LIMITS: Limits = Limits {
    max_pages: 64,
    max_extents: 4,
};

#[test]
fn success_clears_every_extent_before_commit_and_restores_both_accounts() {
    let (quota, inventory) = fixtures(32, &[4, 2]);
    let funded = fund::<_, _, 4>(&quota, &inventory, 6, LIMITS).unwrap();

    assert_eq!(funded.pages(), 6);
    assert_eq!(funded.extent_count(), 2);
    assert_eq!(funded.claims().map(PhysicalClaim::pages).sum::<usize>(), 6);
    assert_eq!(
        *quota.state.borrow(),
        QuotaState {
            available: 26,
            reserved: 0,
            allocated: 6,
        }
    );
    assert_eq!(
        *inventory.state.borrow(),
        InventoryState {
            free: 26,
            claimed: 6,
            cleared: 6,
        }
    );

    drop(funded);
    assert_eq!(
        *quota.state.borrow(),
        QuotaState {
            available: 32,
            reserved: 0,
            allocated: 0,
        }
    );
    assert_eq!(inventory.state.borrow().free, 32);
    assert_eq!(
        inventory.events.borrow().as_slice(),
        ["clear", "clear", "commit", "physical", "physical", "quota"]
    );
}

#[test]
fn quota_failure_never_touches_inventory() {
    let (quota, inventory) = fixtures(2, &[]);
    let result = fund::<_, _, 4>(&quota, &inventory, 3, LIMITS);
    assert!(matches!(
        result,
        Err(FundError::Quota(QuotaError::Exhausted))
    ));
    assert_eq!(inventory.state.borrow().free, 2);
    assert_eq!(quota.state.borrow().available, 2);
}

#[test]
fn physical_failure_rolls_back_claims_before_quota() {
    let (quota, inventory) = fixtures(16, &[4, 4]);
    inventory.calls_before_failure.set(Some(1));
    let result = fund::<_, _, 4>(&quota, &inventory, 8, LIMITS);
    assert!(matches!(
        result,
        Err(FundError::Physical(PhysicalError::Exhausted))
    ));
    assert_eq!(inventory.state.borrow().free, 16);
    assert_eq!(inventory.state.borrow().cleared, 0);
    assert_eq!(quota.state.borrow().available, 16);
    assert_eq!(quota.state.borrow().reserved, 0);
    assert_eq!(
        inventory.events.borrow().as_slice(),
        ["physical", "quota-reservation"]
    );
}

#[test]
fn extent_limit_abandons_uncleared_claims_and_refunds_quota() {
    let (quota, inventory) = fixtures(16, &[4, 2]);
    let result = fund::<_, _, 4>(
        &quota,
        &inventory,
        6,
        Limits {
            max_pages: 64,
            max_extents: 1,
        },
    );
    assert!(matches!(result, Err(FundError::ExtentLimit)));
    assert_eq!(inventory.state.borrow().free, 16);
    assert_eq!(inventory.state.borrow().cleared, 0);
    assert_eq!(quota.state.borrow().available, 16);
    assert_eq!(
        inventory.events.borrow().as_slice(),
        ["physical", "quota-reservation"]
    );
}

#[test]
fn invalid_claim_rolls_back_both_resources_without_clearing() {
    for invalid_pages in [0, 5] {
        let (quota, inventory) = fixtures(16, &[]);
        inventory.next_claim_override.set(Some(invalid_pages));
        let result = fund::<_, _, 4>(&quota, &inventory, 4, LIMITS);
        assert!(matches!(result, Err(FundError::InvalidClaim)));
        assert_eq!(inventory.state.borrow().free, 16);
        assert_eq!(inventory.state.borrow().cleared, 0);
        assert_eq!(quota.state.borrow().available, 16);
        assert_eq!(
            inventory.events.borrow().as_slice(),
            ["physical", "quota-reservation"]
        );
    }
}

#[test]
fn request_limits_fail_before_reserving_either_resource() {
    let (quota, inventory) = fixtures(16, &[]);
    assert!(matches!(
        fund::<_, _, 4>(&quota, &inventory, 0, LIMITS),
        Err(FundError::ZeroPages)
    ));
    assert!(matches!(
        fund::<_, _, 4>(
            &quota,
            &inventory,
            9,
            Limits {
                max_pages: 8,
                max_extents: 4,
            }
        ),
        Err(FundError::PageLimit)
    ));
    assert!(matches!(
        fund::<_, _, 4>(
            &quota,
            &inventory,
            1,
            Limits {
                max_pages: 8,
                max_extents: 5,
            }
        ),
        Err(FundError::ExtentLimit)
    ));
    assert!(matches!(
        fund::<_, _, 0>(
            &quota,
            &inventory,
            1,
            Limits {
                max_pages: 8,
                max_extents: 0,
            }
        ),
        Err(FundError::ExtentLimit)
    ));
    assert_eq!(quota.state.borrow().available, 16);
    assert_eq!(inventory.state.borrow().free, 16);
}
