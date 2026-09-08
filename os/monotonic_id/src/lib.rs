#![no_std]
#![forbid(unsafe_code)]

//! 非零、单调且不回绕的原子身份序列。
//!
//! 每个使用者持有独立 allocator，因此 identity domain 仍由容器定义；本 crate
//! 只统一耗尽语义。最大值可以最后发行一次，同时把内部 next 置零为永久
//! Exhausted；后续申请失败且不改变状态。

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

#[cfg(test)]
extern crate std;

pub struct AtomicId64(AtomicU64);

impl AtomicId64 {
    pub const fn new(first: u64) -> Self {
        assert!(first != 0);
        Self(AtomicU64::new(first))
    }

    pub fn allocate(&self) -> Option<u64> {
        self.0
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                (current != 0).then(|| current.wrapping_add(1))
            })
            .ok()
    }
}

pub struct AtomicIdUsize(AtomicUsize);

impl AtomicIdUsize {
    pub const fn new(first: usize) -> Self {
        assert!(first != 0);
        Self(AtomicUsize::new(first))
    }

    pub fn allocate(&self) -> Option<usize> {
        self.0
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                (current != 0).then(|| current.wrapping_add(1))
            })
            .ok()
    }
}

#[cfg(test)]
mod tests {
    use super::{AtomicId64, AtomicIdUsize};
    use std::{collections::BTreeSet, sync::Arc, thread, vec::Vec};

    #[test]
    fn u64_issues_max_once_then_remains_exhausted() {
        let ids = AtomicId64::new(u64::MAX - 1);
        assert_eq!(ids.allocate(), Some(u64::MAX - 1));
        assert_eq!(ids.allocate(), Some(u64::MAX));
        assert_eq!(ids.allocate(), None);
        assert_eq!(ids.allocate(), None);
    }

    #[test]
    fn usize_issues_max_once_then_remains_exhausted() {
        let ids = AtomicIdUsize::new(usize::MAX);
        assert_eq!(ids.allocate(), Some(usize::MAX));
        assert_eq!(ids.allocate(), None);
        assert_eq!(ids.allocate(), None);
    }

    #[test]
    fn concurrent_allocations_are_unique() {
        const THREADS: usize = 8;
        const PER_THREAD: usize = 1_000;
        let ids = Arc::new(AtomicId64::new(1));
        let workers: Vec<_> = (0..THREADS)
            .map(|_| {
                let ids = Arc::clone(&ids);
                thread::spawn(move || {
                    (0..PER_THREAD)
                        .map(|_| ids.allocate().expect("test identity exhausted"))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let mut observed = BTreeSet::new();
        for worker in workers {
            for id in worker.join().unwrap() {
                assert!(observed.insert(id));
            }
        }
        assert_eq!(observed.len(), THREADS * PER_THREAD);
        assert_eq!(observed.first(), Some(&1));
        assert_eq!(observed.last(), Some(&((THREADS * PER_THREAD) as u64)));
    }
}
