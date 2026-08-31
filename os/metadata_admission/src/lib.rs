#![no_std]
#![forbid(unsafe_code)]

//! 固定容量 metadata admission 计数器。
//!
//! Counter 只表达「最多同时存在多少项」；具体对象负责把 Permit 持有到真实
//! 析构。Permit 不可复制，遗忘只会泄漏额度，不会扩容。

extern crate alloc;

use alloc::sync::Arc;
use core::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReachLimit;

pub struct Counter {
    limit: usize,
    used: AtomicUsize,
}

impl Counter {
    pub const fn new(limit: usize) -> Self {
        assert!(limit != 0, "metadata admission limit must be nonzero");
        Self {
            limit,
            used: AtomicUsize::new(0),
        }
    }

    pub const fn limit(&self) -> usize {
        self.limit
    }

    pub fn used(&self) -> usize {
        self.used.load(Ordering::Relaxed)
    }

    pub fn try_acquire(counter: &Arc<Self>) -> Result<Permit, ReachLimit> {
        let mut current = counter.used.load(Ordering::Relaxed);
        loop {
            if current >= counter.limit {
                return Err(ReachLimit);
            }
            match counter.used.compare_exchange_weak(
                current,
                current + 1,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return Ok(Permit {
                        counter: Arc::clone(counter),
                    });
                }
                Err(observed) => current = observed,
            }
        }
    }
}

#[must_use = "dropping the permit releases its admission slot"]
pub struct Permit {
    counter: Arc<Counter>,
}

impl Permit {
    pub fn counter(&self) -> &Arc<Counter> {
        &self.counter
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let previous = self.counter.used.fetch_sub(1, Ordering::Relaxed);
        assert!(previous != 0, "metadata admission permit underflow");
    }
}

/// 同时持有全局 class slot、每 sponsor class slot 与 sponsor 身份。
/// 任一取得失败都会在返回前释放已取得的另一层额度。
#[must_use = "dropping the permit releases global and sponsor admission slots"]
pub struct SponsoredPermit<S> {
    _global: Permit,
    _local: Permit,
    _sponsor: Arc<S>,
}

impl<S> SponsoredPermit<S> {
    pub fn try_acquire(
        sponsor: &Arc<S>,
        global: &Arc<Counter>,
        local: &Arc<Counter>,
    ) -> Result<Self, ReachLimit> {
        let local = Counter::try_acquire(local)?;
        let global = Counter::try_acquire(global)?;
        Ok(Self {
            _global: global,
            _local: local,
            _sponsor: Arc::clone(sponsor),
        })
    }
}
