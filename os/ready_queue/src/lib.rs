//! 跨 Ready/Running/Waiting 寿命的队列存储准入。
//!
//! 容器通过外部锁串行化 reserve/enqueue/pick；在队列外的执行 owner
//! 保留不可复制的 credit，只有最终离场才归还。退款仅访问原子计数。

#![no_std]
#![feature(allocator_api)]

extern crate alloc;

use alloc::{collections::VecDeque, sync::Arc};
use core::{
    ops::Deref,
    sync::atomic::{AtomicUsize, Ordering},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReserveError {
    Empty,
    CapacityOverflow,
    AllocationFailed,
}

struct Capacity {
    outstanding: AtomicUsize,
}

/// 一批已支付但尚未交付的容量；取消只归还剩余 credit，不触碰队列锁。
#[must_use = "ready admission must be published or cancelled"]
pub struct Admission {
    capacity: Arc<Capacity>,
    remaining: usize,
}

impl Admission {
    pub fn remaining(&self) -> usize {
        self.remaining
    }

    /// 消耗一个 credit，形成可跨执行容器移动的唯一 owner。
    pub fn admit<T>(&mut self, value: T) -> Admitted<T> {
        assert!(self.remaining > 0, "ready admission batch is exhausted");
        self.remaining -= 1;
        Admitted {
            value,
            admission: Admission {
                capacity: self.capacity.clone(),
                remaining: 1,
            },
        }
    }
}

impl Drop for Admission {
    fn drop(&mut self) {
        if self.remaining != 0 {
            let previous = self
                .capacity
                .outstanding
                .fetch_sub(self.remaining, Ordering::Relaxed);
            assert!(
                previous >= self.remaining,
                "ready admission count underflow"
            );
        }
    }
}

/// 值与整个可调度寿命的容量责任。没有 Clone，也没有释放 credit 后重新入队的入口。
///
/// 借用值的复制不产生另一个可入队 owner：
/// ```compile_fail
/// use ready_queue::ReadyQueue;
/// let mut queue = ReadyQueue::<usize>::try_new().unwrap();
/// let mut admission = queue.reserve(1).unwrap();
/// let owner = admission.admit(42);
/// queue.enqueue(owner.clone());
/// ```
#[must_use = "admitted owners must be transferred or retired"]
pub struct Admitted<T> {
    value: T,
    admission: Admission,
}

impl<T> Deref for Admitted<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.value
    }
}

/// FIFO 就绪容器。容量不变量：storage.capacity ≥ outstanding ≥ storage.len。
/// 容量计数包含所有在途出生预留，以及 Ready/Running/Waiting 中的 owner。
pub struct ReadyQueue<T> {
    capacity: Arc<Capacity>,
    storage: VecDeque<Admitted<T>>,
}

impl<T> ReadyQueue<T> {
    pub fn try_new() -> Result<Self, ReserveError> {
        Ok(Self {
            capacity: Arc::try_new(Capacity {
                outstanding: AtomicUsize::new(0),
            })
            .map_err(|_| ReserveError::AllocationFailed)?,
            storage: VecDeque::new(),
        })
    }

    /// 在独占队列访问下为整个出生批次支付存储；失败不增加任何责任。
    pub fn reserve(&mut self, count: usize) -> Result<Admission, ReserveError> {
        if count == 0 {
            return Err(ReserveError::Empty);
        }
        let outstanding = self.capacity.outstanding.load(Ordering::Relaxed);
        let required = outstanding
            .checked_add(count)
            .ok_or(ReserveError::CapacityOverflow)?;
        let additional = required
            .checked_sub(self.storage.len())
            .expect("ready queue contains unadmitted entries");
        self.storage
            .try_reserve(additional)
            .map_err(|_| ReserveError::AllocationFailed)?;
        // reserve 由 &mut self 串行化；并发方只能归还 credit，因此实际存量
        // 不大于预留快照。此后 enqueue 不再分配，队列也不主动 shrink。
        let previous = self
            .capacity
            .outstanding
            .fetch_add(count, Ordering::Relaxed);
        assert!(
            previous <= outstanding,
            "ready admission escaped queue serialization"
        );
        Ok(Admission {
            capacity: self.capacity.clone(),
            remaining: count,
        })
    }

    pub fn enqueue(&mut self, value: Admitted<T>) {
        assert!(
            Arc::ptr_eq(&self.capacity, &value.admission.capacity),
            "ready admission belongs to another queue"
        );
        assert!(
            self.storage.len() < self.storage.capacity(),
            "ready capacity was not reserved"
        );
        self.storage.push_back(value);
    }

    pub fn pick(&mut self) -> Option<Admitted<T>> {
        self.storage.pop_front()
    }

    pub fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }

    pub fn len(&self) -> usize {
        self.storage.len()
    }

    pub fn capacity(&self) -> usize {
        self.storage.capacity()
    }

    pub fn outstanding(&self) -> usize {
        self.capacity.outstanding.load(Ordering::Relaxed)
    }
}
