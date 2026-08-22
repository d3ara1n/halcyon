use core::{hint::spin_loop, sync::atomic::{AtomicBool, Ordering}};

use lock_api::{GuardSend, RawMutex};

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
