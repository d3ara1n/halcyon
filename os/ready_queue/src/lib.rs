//! 跨 Ready/Running/Waiting 寿命的存储准入；固定环保留完整付款 owner。
//! 准备存储在队列锁外，reserve/enqueue/pick 由外部锁串行化；离场仅原子退 credit。

#![no_std]
#![feature(allocator_ext)]

extern crate alloc;

use alloc::sync::Arc;
use core::{
    ops::Deref,
    sync::atomic::{AtomicUsize, Ordering},
};
use funded_storage::{Buffer, Funding, StorageError};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReserveError {
    Empty,
    CapacityOverflow,
    CapacityChanged,
    AllocationFailed,
}

// 固定协调对象；内核在 boot/root seed 之前创建，其实际占用由 boot 供给拥有。
// 它不含存储 charge，远端退准入不会获取资金或队列锁。
struct Capacity {
    outstanding: AtomicUsize,
}

#[must_use = "ready admission must be published or cancelled"]
pub struct Admission {
    capacity: Arc<Capacity>,
    remaining: usize,
}
impl Admission {
    pub fn remaining(&self) -> usize {
        self.remaining
    }
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

/// 借用值的复制不产生可入队 owner：
/// ```compile_fail
/// fn duplicate<T>(owner: ready_queue::Admitted<T>) -> ready_queue::Admitted<T> {
///     owner.clone()
/// }
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

/// 已付款的固定环；候选/换下旧环使用同一种 owner，调用者锁外销毁。
pub struct ReadyStorage<T, C> {
    cells: Buffer<Option<Admitted<T>>, C>,
    head: usize,
    len: usize,
}
impl<T, C> ReadyStorage<T, C> {
    pub const fn new() -> Self {
        Self {
            cells: Buffer::new(),
            head: 0,
            len: 0,
        }
    }
    pub fn try_empty<F: Funding<Charge = C>>(
        capacity: usize,
        payer: &F,
    ) -> Result<Self, StorageError<F::Error>> {
        let mut cells = Buffer::try_empty(capacity, payer)?;
        // 所有槽在准备期初始化；业务提交只替换 Option，不触碰 allocator。
        for _ in 0..capacity {
            cells.push(None);
        }
        Ok(Self {
            cells,
            head: 0,
            len: 0,
        })
    }
    pub fn capacity(&self) -> usize {
        self.cells.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn push(&mut self, value: Admitted<T>) {
        let capacity = self.capacity();
        assert!(self.len < capacity, "ready capacity was not reserved");
        let remainder = capacity - self.head;
        let index = if self.len >= remainder {
            self.len - remainder
        } else {
            self.head + self.len
        };
        assert!(self.cells[index].is_none(), "ready ring slot is occupied");
        self.cells[index] = Some(value);
        self.len += 1;
    }
    fn pop(&mut self) -> Option<Admitted<T>> {
        if self.len == 0 {
            return None;
        }
        let value = self.cells[self.head]
            .take()
            .expect("ready ring slot is empty");
        self.head += 1;
        if self.head == self.capacity() {
            self.head = 0;
        }
        self.len -= 1;
        Some(value)
    }
    fn replace(&mut self, prepared: &mut Self) {
        assert!(
            prepared.is_empty(),
            "prepared ready storage contains owners"
        );
        assert!(
            prepared.capacity() >= self.len,
            "prepared ready storage is too small"
        );
        while let Some(value) = self.pop() {
            prepared.push(value);
        }
        core::mem::swap(self, prepared);
    }
}
impl<T, C> Default for ReadyStorage<T, C> {
    fn default() -> Self {
        Self::new()
    }
}

/// storage.capacity ≥ outstanding ≥ len；所有执行容器保留同一 Capacity 身份。
pub struct ReadyQueue<T, C> {
    capacity: Arc<Capacity>,
    storage: ReadyStorage<T, C>,
}

pub enum Scan<T> {
    Empty,
    Retained,
    Taken(Admitted<T>),
}

impl<T, C> ReadyQueue<T, C> {
    pub fn try_new() -> Result<Self, ReserveError> {
        Ok(Self {
            capacity: Arc::try_new(Capacity {
                outstanding: AtomicUsize::new(0),
            })
            .map_err(|_| ReserveError::AllocationFailed)?,
            storage: ReadyStorage::new(),
        })
    }

    /// 只给出布局需求，不接受责任。2*旧容量+count 留下有证明的竞争重试空间。
    pub fn growth_capacity(&self, count: usize) -> Result<usize, ReserveError> {
        if count == 0 {
            return Err(ReserveError::Empty);
        }
        self.outstanding()
            .checked_add(count)
            .ok_or(ReserveError::CapacityOverflow)?;
        self.capacity()
            .checked_mul(2)
            .and_then(|v| v.checked_add(count))
            .ok_or(ReserveError::CapacityOverflow)
    }

    /// 不分配、不退存储付款；不足时双方原样保留，调用者锁外准备后重试。
    pub fn reserve(
        &mut self,
        count: usize,
        prepared: &mut ReadyStorage<T, C>,
    ) -> Result<Admission, ReserveError> {
        if count == 0 {
            return Err(ReserveError::Empty);
        }
        let outstanding = self.outstanding();
        let required = outstanding
            .checked_add(count)
            .ok_or(ReserveError::CapacityOverflow)?;
        if required > self.capacity() {
            if prepared.capacity() < required {
                return Err(ReserveError::CapacityChanged);
            }
            self.storage.replace(prepared);
        }
        // 唯一增加方经 &mut self 串行化；其他方只做原子退款，存量至多为快照。
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
        self.storage.push(value);
    }
    pub fn pick(&mut self) -> Option<Admitted<T>> {
        self.storage.pop()
    }
    pub fn scan_front_if(&mut self, predicate: impl FnOnce(&T) -> bool) -> Scan<T> {
        let Some(entry) = self.storage.pop() else {
            return Scan::Empty;
        };
        if predicate(&entry.value) {
            Scan::Taken(entry)
        } else {
            self.storage.push(entry);
            Scan::Retained
        }
    }
    pub fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }
    pub fn len(&self) -> usize {
        self.storage.len
    }
    pub fn capacity(&self) -> usize {
        self.storage.capacity()
    }
    pub fn outstanding(&self) -> usize {
        self.capacity.outstanding.load(Ordering::Relaxed)
    }
}
