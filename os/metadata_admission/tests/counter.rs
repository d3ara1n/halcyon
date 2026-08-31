use metadata_admission::{Counter, ReachLimit, SponsoredPermit};
use std::sync::{
    Arc, Barrier,
    atomic::{AtomicBool, Ordering},
};

#[test]
fn exhaustion_and_drop_restore_capacity() {
    let counter = Arc::new(Counter::new(2));
    let first = Counter::try_acquire(&counter).unwrap();
    let second = Counter::try_acquire(&counter).unwrap();
    assert_eq!(counter.used(), 2);
    assert!(matches!(Counter::try_acquire(&counter), Err(ReachLimit)));

    drop(first);
    assert_eq!(counter.used(), 1);
    let replacement = Counter::try_acquire(&counter).unwrap();
    assert_eq!(counter.used(), 2);

    drop((second, replacement));
    assert_eq!(counter.used(), 0);
}

#[test]
fn permit_keeps_counter_alive() {
    let counter = Arc::new(Counter::new(1));
    let permit = Counter::try_acquire(&counter).unwrap();
    let observer = Arc::clone(permit.counter());
    drop(counter);
    assert_eq!(observer.used(), 1);
    drop(permit);
    assert_eq!(observer.used(), 0);
}

#[test]
fn forgotten_permit_leaks_but_never_expands() {
    let counter = Arc::new(Counter::new(1));
    let permit = Counter::try_acquire(&counter).unwrap();
    core::mem::forget(permit);
    assert_eq!(counter.used(), 1);
    assert!(Counter::try_acquire(&counter).is_err());
}

#[test]
fn concurrent_acquisition_never_exceeds_limit() {
    const LIMIT: usize = 8;
    const THREADS: usize = 32;

    let counter = Arc::new(Counter::new(LIMIT));
    let barrier = Arc::new(Barrier::new(THREADS));
    let mut workers = Vec::new();
    for _ in 0..THREADS {
        let counter = Arc::clone(&counter);
        let barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            Counter::try_acquire(&counter).ok()
        }));
    }

    let permits: Vec<_> = workers
        .into_iter()
        .filter_map(|worker| worker.join().unwrap())
        .collect();
    assert_eq!(permits.len(), LIMIT);
    assert_eq!(counter.used(), LIMIT);
    drop(permits);
    assert_eq!(counter.used(), 0);
}

#[test]
fn sponsored_acquisition_rolls_back_partial_failure() {
    let global = Arc::new(Counter::new(1));
    let first_local = Arc::new(Counter::new(1));
    let second_local = Arc::new(Counter::new(1));
    let first_sponsor = Arc::new(());
    let second_sponsor = Arc::new(());

    let first = SponsoredPermit::try_acquire(&first_sponsor, &global, &first_local).unwrap();
    assert!(SponsoredPermit::try_acquire(&second_sponsor, &global, &second_local).is_err());
    assert_eq!(global.used(), 1);
    assert_eq!(first_local.used(), 1);
    assert_eq!(second_local.used(), 0);

    assert!(SponsoredPermit::try_acquire(&first_sponsor, &global, &first_local).is_err());
    assert_eq!(global.used(), 1);
    assert_eq!(first_local.used(), 1);
    drop(first);
    assert_eq!(global.used(), 0);
    assert_eq!(first_local.used(), 0);
}

struct DropSponsor(Arc<AtomicBool>);

impl Drop for DropSponsor {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

#[test]
fn sponsored_permit_keeps_sponsor_alive() {
    let dropped = Arc::new(AtomicBool::new(false));
    let sponsor = Arc::new(DropSponsor(Arc::clone(&dropped)));
    let global = Arc::new(Counter::new(1));
    let local = Arc::new(Counter::new(1));
    let permit = SponsoredPermit::try_acquire(&sponsor, &global, &local).unwrap();

    drop(sponsor);
    assert!(!dropped.load(Ordering::Relaxed));
    drop(permit);
    assert!(dropped.load(Ordering::Relaxed));
    assert_eq!(global.used(), 0);
    assert_eq!(local.used(), 0);
}
