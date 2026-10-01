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

fn drain(map: &mut SegmentMap<Ext>, u: memory_space::RangeUse) -> Vec<(u64, u64, Ext)> {
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

fn map_1000() -> SegmentMap<Ext> {
    let mut m = SegmentMap::new(1);
    let mut r = m.reserve(1).unwrap();
    m.push_extent(1000, Ext(5000, 1000), &mut r).unwrap();
    m
}

#[test]
fn partial_release_returns_only_unused_pages() {
    // 反例：1000 页只剩中间一小段 view，其余页必须可退。
    let mut m = map_1000();
    let mut r = m.reserve(2).unwrap();
    let whole = m.acquire(0, 1000, &mut r).unwrap();
    let mut r = m.reserve(2).unwrap();
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
    let mut r = m.reserve(1).unwrap();
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
    let mut r = m.reserve(2).unwrap();
    let a = m.acquire(0, 500, &mut r).unwrap();
    let mut r = m.reserve(2).unwrap();
    let b = m.acquire(500, 1000, &mut r).unwrap();
    drain(&mut m, a);
    let mut r = m.reserve(2).unwrap();
    assert_eq!(m.acquire(100, 600, &mut r), Err(BackingError::InvalidRange));
    assert_eq!(m.acquire(100, 200, &mut r), Err(BackingError::InvalidRange));
    drain(&mut m, b);
    assert!(m.is_empty());
}

#[test]
fn foreign_use_is_rejected() {
    let mut a = map_1000();
    let mut b: SegmentMap<Ext> = SegmentMap::new(2);
    let mut r = a.reserve(2).unwrap();
    let u = a.acquire(0, 10, &mut r).unwrap();
    assert!(matches!(b.begin_release(u), Err(BackingError::ForeignUse)));
}

#[test]
fn retiring_rejects_new_use() {
    let mut m = map_1000();
    m.begin_retire();
    let mut r = m.reserve(2).unwrap();
    assert_eq!(m.acquire(0, 10, &mut r), Err(BackingError::Retiring));
}
