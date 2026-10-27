//! 区间持有契约：守恒切分、重叠 use、零 use 才摘出、释放不消耗预取。
use memory_space::{BackingError, ReleaseStep, SegmentMap, SegmentOwner};

/// 记录物理起点与页数的假 owner，用于检查切分守恒。
#[derive(Debug, PartialEq, Eq)]
struct Ext(u64, u64);
impl SegmentOwner for Ext {
    fn split_at(self, pages: u64) -> (Self, Self) {
        assert!(pages > 0 && pages < self.1);
        (Ext(self.0, pages), Ext(self.0 + pages, self.1 - pages))
    }
}

fn drain(map: &mut SegmentMap<Ext, ()>, u: memory_space::RangeUse) -> Vec<(u64, u64, Ext)> {
    let mut c = map.begin_release(u).unwrap();
    let mut out = Vec::new();
    loop {
        match map.release_step(&mut c) {
            ReleaseStep::Detached { start, end, owner } => out.push((start, end, owner)),
            ReleaseStep::Retained => {}
            ReleaseStep::Done => return out,
        }
    }
}

fn map_1000() -> SegmentMap<Ext, ()> {
    let mut m = SegmentMap::new(1);
    let mut r = reserve_nodes(&mut m, 1).unwrap();
    m.push_extent(1000, Ext(5000, 1000), &mut r).unwrap();
    m
}

#[test]
fn partial_release_returns_only_unused_pages() {
    // 反例：1000 页只剩中间一小段 view，其余页必须可退。
    let mut m = map_1000();
    let mut r = reserve_nodes(&mut m, 2).unwrap();
    let whole = m.acquire(0, 1000, &mut r).unwrap();
    let mut r = reserve_nodes(&mut m, 2).unwrap();
    let mid = m.acquire(400, 401, &mut r).unwrap();
    let out = drain(&mut m, whole);
    assert_eq!(
        out,
        vec![(0, 400, Ext(5000, 400)), (401, 1000, Ext(5401, 599))]
    );
    let out = drain(&mut m, mid);
    assert_eq!(out, vec![(400, 401, Ext(5400, 1))]);
    assert!(m.is_empty());
}

#[test]
fn acquire_without_credits_fails_without_change() {
    let mut m = map_1000();
    let mut r = reserve_nodes(&mut m, 1).unwrap();
    assert_eq!(
        m.acquire(10, 20, &mut r),
        Err(BackingError::ReserveExhausted)
    );
    assert_eq!(m.segment_count(), 1);
    assert_eq!(r.credits(), 1);
}

#[test]
fn released_hole_cannot_be_reacquired() {
    // 已退款页不得被新 use 复活。
    let mut m = map_1000();
    let mut r = reserve_nodes(&mut m, 2).unwrap();
    let a = m.acquire(0, 500, &mut r).unwrap();
    let mut r = reserve_nodes(&mut m, 2).unwrap();
    let b = m.acquire(500, 1000, &mut r).unwrap();
    drain(&mut m, a);
    let mut r = reserve_nodes(&mut m, 2).unwrap();
    assert_eq!(m.acquire(100, 600, &mut r), Err(BackingError::InvalidRange));
    assert_eq!(m.acquire(100, 200, &mut r), Err(BackingError::InvalidRange));
    drain(&mut m, b);
    assert!(m.is_empty());
}

#[test]
fn foreign_use_is_rejected() {
    let mut a = map_1000();
    let mut b: SegmentMap<Ext, ()> = SegmentMap::new(2);
    let mut r = reserve_nodes(&mut a, 2).unwrap();
    let u = a.acquire(0, 10, &mut r).unwrap();
    let (error, original) = b.begin_release(u).unwrap_err();
    assert_eq!(error, BackingError::ForeignUse);
    assert_eq!(drain(&mut a, original), vec![(0, 10, Ext(5000, 10))]);
}

#[test]
fn retiring_rejects_new_use() {
    let mut m = map_1000();
    m.begin_retire();
    let mut r = reserve_nodes(&mut m, 2).unwrap();
    assert_eq!(m.acquire(0, 10, &mut r), Err(BackingError::Retiring));
}

// root Close 与 View 退休可由不同记录交错推进；Root 完成不应提前退仍有 View 的页。
#[test]
fn root_and_view_release_interleave_without_early_or_duplicate_refund() {
    let mut map = map_1000();
    let mut reserve = reserve_nodes(&mut map, 2).unwrap();
    let root = map.acquire(0, 1000, &mut reserve).unwrap();
    let view = map.acquire(200, 800, &mut reserve).unwrap();
    map.unreserve(reserve);
    map.begin_retire();
    let mut no_credit = reserve_nodes(&mut map, 0).unwrap();
    assert_eq!(
        map.acquire(200, 800, &mut no_credit),
        Err(BackingError::Retiring)
    );
    let mut root = map.begin_release(root).unwrap();
    let mut view = map.begin_release(view).unwrap();
    assert!(matches!(
        map.release_step(&mut root),
        ReleaseStep::Detached {
            start: 0,
            end: 200,
            owner: Ext(5000, 200)
        }
    ));
    assert!(matches!(map.release_step(&mut root), ReleaseStep::Retained));
    // Root 已经过共享区，但尚未结束；View 自己的事务可以独立退还中间段。
    assert!(matches!(
        map.release_step(&mut view),
        ReleaseStep::Detached {
            start: 200,
            end: 800,
            owner: Ext(5200, 600)
        }
    ));
    assert!(matches!(map.release_step(&mut view), ReleaseStep::Done));
    assert!(matches!(
        map.release_step(&mut root),
        ReleaseStep::Detached {
            start: 800,
            end: 1000,
            owner: Ext(5800, 200)
        }
    ));
    assert!(matches!(map.release_step(&mut root), ReleaseStep::Done));
    assert!(map.is_empty());
}

#[test]
fn split_preserves_overlap_and_refunds_only_unshared_subranges() {
    let mut map = map_1000();
    let mut reserve = reserve_nodes(&mut map, 4).unwrap();
    let whole = map.acquire(0, 1000, &mut reserve).unwrap();
    let other = map.acquire(400, 600, &mut reserve).unwrap();
    let (left, rest) = map.split_use(whole, 400, &mut reserve).unwrap();
    let (middle, right) = map.split_use(rest, 600, &mut reserve).unwrap();
    map.unreserve(reserve);
    // 一份原 use 被拆分，不增加段引用，也不能释放另一个视图仍覆盖的物理页。
    assert!(drain(&mut map, middle).is_empty());
    assert_eq!(drain(&mut map, left), vec![(0, 400, Ext(5000, 400))]);
    assert_eq!(drain(&mut map, right), vec![(600, 1000, Ext(5600, 400))]);
    assert_eq!(map.segment_at(500), Some((400, 600, &Ext(5400, 200))));
    assert!(map.segment_at(0).is_none());
    assert_eq!(drain(&mut map, other), vec![(400, 600, Ext(5400, 200))]);
    assert!(map.is_empty());
}

#[test]
fn failed_split_returns_original_owner_and_retiring_split_still_works() {
    let mut map = map_1000();
    let mut reserve = reserve_nodes(&mut map, 0).unwrap();
    let whole = map.acquire(0, 1000, &mut reserve).unwrap();
    let (error, whole) = map.split_use(whole, 20, &mut reserve).unwrap_err();
    assert_eq!(error, BackingError::ReserveExhausted);
    assert_eq!(whole.range(), (0, 1000));
    assert_eq!(map.segment_count(), 1);
    let mut reserve = reserve_nodes(&mut map, 1).unwrap();
    let (error, whole) = map.split_use(whole, 0, &mut reserve).unwrap_err();
    assert_eq!(error, BackingError::InvalidRange);
    assert_eq!(reserve.credits(), 1);
    map.begin_retire();
    let (left, right) = map.split_use(whole, 20, &mut reserve).unwrap();
    assert_eq!(reserve.credits(), 0);
    assert_eq!(drain(&mut map, left), vec![(0, 20, Ext(5000, 20))]);
    assert_eq!(drain(&mut map, right), vec![(20, 1000, Ext(5020, 980))]);
    assert!(map.is_empty());
}

#[test]
fn rejected_extent_preserves_physical_owner_and_reserve() {
    let mut map = map_1000();
    let mut reserve = reserve_nodes(&mut map, 1).unwrap();
    let (error, owner) = map
        .push_extent(u64::MAX, Ext(7000, 10), &mut reserve)
        .unwrap_err();
    assert_eq!(error, BackingError::InvalidRange);
    assert_eq!(owner, Ext(7000, 10));
    assert_eq!(reserve.credits(), 1);
    map.begin_retire();
    let (error, owner) = map.push_extent(10, owner, &mut reserve).unwrap_err();
    assert_eq!(error, BackingError::Retiring);
    assert_eq!(owner, Ext(7000, 10));
    assert_eq!(map.pages(), 1000);
}

struct TestFunding;
impl funded_storage::Funding for TestFunding {
    type Charge = ();
    type Error = ();
    fn charge(&self, _: core::alloc::Layout) -> Result<(), ()> {
        Ok(())
    }
}
fn reserve_nodes(
    map: &mut SegmentMap<Ext, ()>,
    n: usize,
) -> Result<memory_space::NodeReserve, BackingError> {
    let mut storage =
        memory_space::SegmentStorage::try_empty(map.storage_capacity(n)?, &TestFunding).unwrap();
    map.reserve(n, &mut storage)
}

#[test]
fn incremental_acquire_survives_source_seal_and_keeps_only_the_requested_pages() {
    let mut map = SegmentMap::new(19);
    let mut reserve = reserve_nodes(&mut map, 4).unwrap();
    for page in [100, 200, 300, 400] {
        map.push_extent(2, Ext(page, 2), &mut reserve).unwrap();
    }
    map.unreserve(reserve);
    let mut reserve = reserve_nodes(&mut map, 2).unwrap();
    let root = map.acquire(0, 8, &mut reserve).unwrap();
    let mut pin = map.begin_acquire(1, 7, &mut reserve).unwrap();
    assert!(!map.acquire_step(&mut pin).unwrap());
    map.begin_retire(); // 外层保留 root use，封口不取消已经接受的取得责任。
    while !map.acquire_step(&mut pin).unwrap() {}
    let pin = map.finish_acquire(pin).unwrap();
    assert_eq!(pin.range(), (1, 7));
    let released = drain(&mut map, root);
    assert_eq!(released.iter().map(|(a, b, _)| b - a).sum::<u64>(), 2);
    let released = drain(&mut map, pin);
    assert_eq!(released.iter().map(|(a, b, _)| b - a).sum::<u64>(), 6);
    assert!(map.is_empty());
}

#[test]
fn cancelled_incremental_acquire_releases_exactly_its_acquired_prefix() {
    let mut map = SegmentMap::new(20);
    let mut reserve = reserve_nodes(&mut map, 3).unwrap();
    for page in [100, 200, 300] {
        map.push_extent(2, Ext(page, 2), &mut reserve).unwrap();
    }
    map.unreserve(reserve);
    let mut reserve = reserve_nodes(&mut map, 2).unwrap();
    let root = map.acquire(0, 6, &mut reserve).unwrap();
    let mut pin = map.begin_acquire(1, 5, &mut reserve).unwrap();
    assert!(!map.acquire_step(&mut pin).unwrap());
    let prefix = map.finish_acquire(pin).unwrap();
    assert_eq!(prefix.range(), (1, 2));
    let released = drain(&mut map, root);
    assert_eq!(released.iter().map(|(a, b, _)| b - a).sum::<u64>(), 5);
    assert_eq!(drain(&mut map, prefix), vec![(1, 2, Ext(101, 1))]);
    assert!(map.is_empty());
}
