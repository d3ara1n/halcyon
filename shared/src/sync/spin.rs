use core::{
    hint::spin_loop,
    ptr::null_mut,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
};

use alloc::boxed::Box;
use lock_api::{GuardSend, RawMutex};

pub struct Ticket {
    locked: bool,
    next: *mut Ticket,
}

impl Ticket {
    pub const fn new() -> Self {
        Self {
            locked: true,
            next: null_mut(),
        }
    }
}

pub struct QueueLock {
    tail: AtomicPtr<Ticket>,
    owned: AtomicPtr<Ticket>,
}

impl QueueLock {
    pub const fn new() -> Self {
        Self {
            tail: AtomicPtr::new(null_mut()),
            owned: AtomicPtr::new(null_mut()),
        }
    }
}

unsafe impl RawMutex for QueueLock {
    const INIT: Self = QueueLock::new();

    type GuardMarker = GuardSend;

    fn lock(&self) {
        // 泄露 Ticket 到堆，unlock 的时候回收
        let node = Box::into_raw(Box::new(Ticket::new()));
        let prev = self.tail.swap(node, Ordering::Acquire);
        if !prev.is_null() {
            unsafe {
                (*prev).next = node;
                while (*node).locked {
                    spin_loop()
                }
            }
        }
        self.owned.store(node, Ordering::Relaxed);
    }

    fn try_lock(&self) -> bool {
        let node = Box::into_raw(Box::new(Ticket::new()));
        let prev = self.tail.load(Ordering::Acquire);
        if prev.is_null() {
            if self
                .tail
                .compare_exchange(prev, node, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                self.owned.store(node, Ordering::Relaxed);
                return true;
            }
        }
        unsafe { drop(Box::from_raw(node)) };
        return false;
    }

    unsafe fn unlock(&self) {
        let self_ptr = self.owned.load(Ordering::Relaxed);
        match self
            .tail
            .compare_exchange(self_ptr, null_mut(), Ordering::Release, Ordering::Relaxed)
        {
            Ok(owned) => unsafe {
                drop(Box::from_raw(owned));
            },
            Err(_) => unsafe {
                let owned = &mut (*self_ptr);
                while owned.next.is_null() {
                    spin_loop()
                }
                let succ = &mut (*owned.next);
                drop(Box::from_raw(owned));
                succ.locked = false;
            },
        }
    }
}

pub struct SimpleLock {
    lock: AtomicBool,
}

impl SimpleLock {
    pub const fn new() -> Self {
        Self {
            lock: AtomicBool::new(false),
        }
    }
}

unsafe impl RawMutex for SimpleLock {
    const INIT: Self = SimpleLock::new();

    type GuardMarker = GuardSend;
    fn lock(&self) {
        while self
            .lock
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            while self.is_locked() {
                spin_loop()
            }
        }
    }

    unsafe fn unlock(&self) {
        self.lock.store(false, Ordering::Release);
    }

    fn try_lock(&self) -> bool {
        match self
            .lock
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        {
            Ok(_) => true,
            Err(_) => false,
        }
    }
}
