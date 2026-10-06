use core::{alloc::Layout, cell::Cell};
use std::rc::Rc;

use funded_storage::{Buffer, Funding, StorageError};
use memory_space::{
    AnonymousClass, BackingId, ChangeRequest, ChangeStorage, Limits, MapBacking, MapPlacement,
    MapRequest, MemorySpace, PAGE_SIZE, Protection, RegionOwner, RegionStorage, RollbackStep,
};

#[derive(Debug, Default)]
struct Ledger {
    live: Cell<usize>,
    gate: Cell<bool>,
    refund_in_gate: Cell<bool>,
    remaining: Cell<Option<usize>>,
}

#[derive(Clone)]
struct Payer(Rc<Ledger>);

#[derive(Debug)]
struct Charge {
    ledger: Rc<Ledger>,
    bytes: usize,
}
impl Drop for Charge {
    fn drop(&mut self) {
        if self.ledger.gate.get() {
            self.ledger.refund_in_gate.set(true);
        }
        self.ledger.live.set(self.ledger.live.get() - self.bytes);
    }
}
impl Funding for Payer {
    type Charge = Charge;
    type Error = ();
    fn charge(&self, layout: Layout) -> Result<Charge, ()> {
        assert!(
            !self.0.gate.get(),
            "funding attempted inside the ledger gate"
        );
        if let Some(left) = self.0.remaining.get() {
            if left == 0 {
                return Err(());
            }
            self.0.remaining.set(Some(left - 1));
        }
        self.0.live.set(self.0.live.get() + layout.size());
        Ok(Charge {
            ledger: self.0.clone(),
            bytes: layout.size(),
        })
    }
}

fn limits() -> Limits {
    Limits {
        max_regions: 32,
        max_transactions: 2,
        max_pages_per_change: 16,
        max_lease_segments: 2,
        max_lease_bytes: PAGE_SIZE,
    }
}
fn request() -> ChangeRequest {
    ChangeRequest::Map(MapRequest {
        bytes: PAGE_SIZE,
        guard_before: PAGE_SIZE,
        guard_after: PAGE_SIZE,
        placement: MapPlacement::FixedEmpty {
            usable_start: 0x1000_1000,
        },
        current: Protection::ReadWrite,
        maximum: Protection::ReadWrite,
        owner: RegionOwner::AddressSpace,
        backing: MapBacking::Anonymous {
            identity: BackingId::new(1).unwrap(),
            class: AnonymousClass::Data,
        },
        result: None,
    })
}

#[test]
fn rollback_and_complete_transfer_payment_outside_the_gate() {
    let facts = Rc::new(Ledger::default());
    let payer = Payer(facts.clone());
    let mut space = MemorySpace::new(
        memory_space::PageRange::new(0x1000_0000, 16 * PAGE_SIZE).unwrap(),
        limits(),
        &payer,
    )
    .unwrap();
    for rollback in [true, false] {
        let request = request();
        let shape = space.validation_capacity(&request).unwrap();
        let mut scratch = ChangeStorage::default();
        scratch.prepare_validation(shape, &payer).unwrap();
        facts.gate.set(true);
        let mut validated = space.validate(&request, &mut scratch).unwrap();
        facts.gate.set(false);
        validated.prepare_reservation(&payer).unwrap();
        let count = space.change_storage_capacity(&validated).unwrap();
        let mut regions = RegionStorage::try_empty(count, &payer).unwrap();
        facts.gate.set(true);
        let mut permits = Buffer::new();
        let prepared = space
            .reserve(validated, &mut permits, &mut regions)
            .unwrap();
        facts.gate.set(false);
        drop(regions);
        let before = facts.live.get();
        facts.gate.set(true);
        let storage = if rollback {
            space.rollback(prepared)
        } else {
            let mut committed = space.commit(prepared);
            while !space.commit_step(&mut committed, 1) {}
            let retiring = space.synchronize(committed);
            assert!(retiring.is_empty());
            space.complete(retiring)
        };
        assert_eq!(
            facts.live.get(),
            before,
            "stage transition refunded live storage"
        );
        facts.gate.set(false);
        let mut storage = storage;
        loop {
            match storage.rollback_step() {
                RollbackStep::Progress => {}
                RollbackStep::Permit(_) => {
                    panic!("anonymous mapping acquired an object write permit")
                }
                RollbackStep::Done => break,
            }
        }
        drop(storage);
        assert!(
            facts.live.get() < before,
            "returned plan storage did not refund"
        );
        assert!(!facts.refund_in_gate.get());
    }
    drop(space);
    assert_eq!(facts.live.get(), 0);
}

#[test]
fn partially_funded_validation_keeps_its_owner_until_caller_release() {
    let facts = Rc::new(Ledger::default());
    let payer = Payer(facts.clone());
    let space = MemorySpace::new(
        memory_space::PageRange::new(0x1000_0000, 16 * PAGE_SIZE).unwrap(),
        limits(),
        &payer,
    )
    .unwrap();
    let baseline = facts.live.get();
    let shape = space.validation_capacity(&request()).unwrap();

    // 先完成相同公开请求，取得付款调用数；不把内部数组数量复制成测试真值。
    facts.remaining.set(Some(usize::MAX));
    let mut complete = ChangeStorage::default();
    complete.prepare_validation(shape, &payer).unwrap();
    let charges = usize::MAX - facts.remaining.get().unwrap();
    assert!(charges > 0, "validation storage was not funded");
    drop(complete);
    assert_eq!(facts.live.get(), baseline);

    // 每个已接受付款前缀后拒绝下一次：尚未提交的空间不变，部分存储只随调用者退款。
    for accepted in 0..charges {
        facts.remaining.set(Some(accepted));
        let mut scratch = ChangeStorage::default();
        assert!(matches!(
            scratch.prepare_validation(shape, &payer),
            Err(StorageError::Funding(()))
        ));
        if accepted > 0 {
            assert!(
                facts.live.get() > baseline,
                "partial preparation lost its paid owner"
            );
        }
        assert_eq!(space.transaction_count(), 0);
        assert_eq!(space.region_count(), 0);
        drop(scratch);
        assert_eq!(facts.live.get(), baseline);
    }
    drop(space);
    assert_eq!(facts.live.get(), 0);
}
