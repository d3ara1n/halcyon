use ready_queue::{Admission, Admitted, ReadyQueue, ReserveError};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    collections::VecDeque,
    sync::{Arc, Barrier, Mutex},
};

// 只观察当前测试线程，避免并行测试和测试框架的分配污染提交段探针。
thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static DENY: Cell<bool> = const { Cell::new(false) };
    static ATTEMPTS: Cell<usize> = const { Cell::new(0) };
}
struct Allocator;

fn refused() -> bool {
    if TRACK.try_with(Cell::get).unwrap_or(false) {
        let _ = ATTEMPTS.try_with(|count| count.set(count.get() + 1));
    }
    DENY.try_with(Cell::get).unwrap_or(false)
}

// SAFETY: 只注入空指针失败；其余分配和释放完整转发给 System。
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if refused() {
            std::ptr::null_mut()
        } else {
            unsafe { System.alloc(layout) }
        }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if refused() {
            std::ptr::null_mut()
        } else {
            unsafe { System.alloc_zeroed(layout) }
        }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if refused() {
            std::ptr::null_mut()
        } else {
            unsafe { System.realloc(pointer, layout, size) }
        }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

fn observe<R>(deny: bool, f: impl FnOnce() -> R) -> (R, usize) {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            TRACK.set(false);
            DENY.set(false);
        }
    }
    ATTEMPTS.set(0);
    TRACK.set(true);
    DENY.set(deny);
    let reset = Reset;
    let result = f();
    drop(reset);
    (result, ATTEMPTS.get())
}

fn without_allocation<R>(f: impl FnOnce() -> R) -> R {
    let (result, attempts) = observe(false, f);
    assert_eq!(attempts, 0, "committed operation attempted allocation");
    result
}

#[test]
fn pending_batches_are_invisible_and_partial_cancellation_is_exact() {
    let mut queue = ReadyQueue::try_new().unwrap();
    let mut first = queue.reserve(4).unwrap();
    let second = queue.reserve(7).unwrap();
    assert!(queue.is_empty());
    assert_eq!(queue.outstanding(), 11);
    without_allocation(|| queue.enqueue(first.admit(1)));
    without_allocation(|| drop(first));
    assert_eq!(queue.outstanding(), 8);
    without_allocation(|| drop(second));
    assert_eq!(queue.outstanding(), 1);
    let running = without_allocation(|| queue.pick().unwrap());
    assert_eq!(*running, 1);
    assert_eq!(queue.outstanding(), 1);
    without_allocation(|| drop(running));
    assert_eq!(queue.outstanding(), 0);
}

#[test]
fn waiting_and_running_capacity_survives_new_births_and_all_wake_paths() {
    let mut queue = ReadyQueue::try_new().unwrap();
    let mut old = queue.reserve(8).unwrap();
    for id in 0..8 {
        without_allocation(|| queue.enqueue(old.admit(id)));
    }
    let mut waiting = Vec::with_capacity(8);
    while let Some(thread) = without_allocation(|| queue.pick()) {
        waiting.push(thread);
    }
    let mut new = queue.reserve(8).unwrap();
    assert!(queue.capacity() >= 16);
    let capacity = queue.capacity();
    without_allocation(|| {
        for id in 8..16 {
            queue.enqueue(new.admit(id));
        }
        // timer、signal、kernel completion 和等待安装错误均只移动已有 owner。
        for thread in waiting {
            queue.enqueue(thread);
        }
        for _ in 0..128 {
            let running = queue.pick().unwrap();
            queue.enqueue(running);
        }
    });
    assert_eq!(queue.len(), 16);
    assert_eq!(queue.capacity(), capacity);
    without_allocation(|| {
        while let Some(thread) = queue.pick() {
            drop(thread);
        }
    });
    assert_eq!(queue.outstanding(), 0);
}

#[test]
fn failed_admission_does_not_borrow_waiters_capacity_or_publish_a_prefix() {
    let mut queue = ReadyQueue::<usize>::try_new().unwrap();
    assert!(matches!(queue.reserve(0), Err(ReserveError::Empty)));
    let mut batch = queue.reserve(8).unwrap();
    let waiting = batch.admit(1);
    drop(batch);
    let before = queue.capacity();
    let (result, attempts) = observe(true, || queue.reserve(before + 1));
    assert!(matches!(result, Err(ReserveError::AllocationFailed)));
    assert!(attempts > 0);
    assert_eq!(queue.outstanding(), 1);
    assert_eq!(queue.capacity(), before);
    assert!(queue.is_empty());
    without_allocation(|| queue.enqueue(waiting));
    without_allocation(|| drop(queue.pick().unwrap()));
    assert_eq!(queue.outstanding(), 0);
}

#[test]
fn initial_metadata_failure_is_fallible() {
    let (result, _) = observe(true, ReadyQueue::<usize>::try_new);
    assert!(matches!(result, Err(ReserveError::AllocationFailed)));
}

#[test]
fn impossible_storage_request_preserves_existing_admission() {
    let mut queue = ReadyQueue::<usize>::try_new().unwrap();
    let batch = queue.reserve(1).unwrap();
    assert!(matches!(
        queue.reserve(usize::MAX),
        Err(ReserveError::CapacityOverflow)
    ));
    assert_eq!(queue.outstanding(), 1);
    drop(batch);
    assert_eq!(queue.outstanding(), 0);
}

#[test]
fn foreign_queue_rejects_owner_without_losing_capacity() {
    let mut first = ReadyQueue::<usize>::try_new().unwrap();
    let mut second = ReadyQueue::try_new().unwrap();
    let mut batch = first.reserve(1).unwrap();
    let owner = batch.admit(42);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| second.enqueue(owner)));
    assert!(result.is_err());
    assert_eq!(first.outstanding(), 0);
    assert_eq!(second.outstanding(), 0);
}

#[test]
fn deterministic_container_model_preserves_fifo_and_lifetime_accounting() {
    let mut queue = ReadyQueue::try_new().unwrap();
    let mut expected = VecDeque::new();
    let mut pending: Vec<Admission> = Vec::new();
    let mut waiting: Vec<Admitted<usize>> = Vec::new();
    let mut next_id = 0;
    let mut random = 0x915f_ca71_u32;
    for _ in 0..10_000 {
        random ^= random << 13;
        random ^= random >> 17;
        random ^= random << 5;
        match random % 7 {
            0 if queue.outstanding() < 128 => {
                pending.push(queue.reserve(1 + (random as usize % 4)).unwrap());
            }
            1 => {
                if let Some(mut batch) = pending.pop() {
                    while batch.remaining() != 0 {
                        without_allocation(|| queue.enqueue(batch.admit(next_id)));
                        expected.push_back(next_id);
                        next_id += 1;
                    }
                }
            }
            2 => without_allocation(|| drop(pending.pop())),
            3 => {
                if let Some(thread) = without_allocation(|| queue.pick()) {
                    assert_eq!(Some(*thread), expected.pop_front());
                    waiting.push(thread);
                }
            }
            4 => {
                if let Some(thread) = waiting.pop() {
                    expected.push_back(*thread);
                    without_allocation(|| queue.enqueue(thread));
                }
            }
            5 => without_allocation(|| drop(waiting.pop())),
            _ => {
                if let Some(thread) = without_allocation(|| queue.pick()) {
                    assert_eq!(Some(*thread), expected.pop_front());
                    expected.push_back(*thread);
                    without_allocation(|| queue.enqueue(thread));
                }
            }
        }
        assert_eq!(queue.len(), expected.len());
        let preparing: usize = pending.iter().map(Admission::remaining).sum();
        assert_eq!(queue.outstanding(), queue.len() + waiting.len() + preparing);
        assert!(queue.capacity() >= queue.outstanding());
    }
    without_allocation(|| {
        drop(pending);
        drop(waiting);
    });
    without_allocation(|| {
        while let Some(thread) = queue.pick() {
            drop(thread);
        }
    });
    assert_eq!(queue.outstanding(), 0);
}

#[test]
fn remote_departures_can_race_new_admission_without_taking_queue_lock() {
    let mut queue = ReadyQueue::try_new().unwrap();
    let mut batch = queue.reserve(64).unwrap();
    let groups: Vec<Vec<_>> = (0..4)
        .map(|_| (0..16).map(|i| batch.admit(i)).collect())
        .collect();
    let queue = Arc::new(Mutex::new(queue));
    let barrier = Arc::new(Barrier::new(5));
    std::thread::scope(|scope| {
        for group in groups {
            let barrier = barrier.clone();
            scope.spawn(move || {
                for thread in group {
                    barrier.wait();
                    without_allocation(|| drop(thread));
                    barrier.wait();
                }
            });
        }
        for step in 0..16 {
            barrier.wait();
            let mut queue = queue.lock().unwrap();
            let mut birth = queue.reserve(1).unwrap();
            without_allocation(|| queue.enqueue(birth.admit(100 + step)));
            // 持 Ready 锁等待远端退款，验证离场不反取该锁。
            barrier.wait();
            assert_eq!(queue.outstanding(), 64 - 3 * (step + 1));
            assert!(queue.capacity() >= queue.outstanding());
        }
    });
    let mut queue = queue.lock().unwrap();
    without_allocation(|| {
        while let Some(thread) = queue.pick() {
            drop(thread);
        }
    });
    assert_eq!(queue.outstanding(), 0);
}
