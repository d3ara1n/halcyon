use core::{
    hint::spin_loop,
    sync::atomic::{AtomicUsize, Ordering, AtomicBool, AtomicPtr}, ptr::null_mut,
};

use alloc::boxed::Box;
use erhino_shared::sync::InteriorLock;

use crate::hart;

pub struct HartLock {
    lock: AtomicUsize,
}

impl HartLock {
    pub const fn new() -> Self {
        Self {
            lock: AtomicUsize::new(usize::MAX),
        }
    }
}

impl InteriorLock for HartLock {
    fn is_locked(&self) -> bool {
        let hartid = hart::hartid();
        let locked = self.lock.load(Ordering::Relaxed);
        locked != usize::MAX && locked != hartid
    }

    fn lock(&self) {
        let hartid = hart::hartid();
        while self
            .lock
            .compare_exchange_weak(usize::MAX, hartid, Ordering::Acquire, Ordering::Relaxed)
            .is_err_and(|c| c != hartid)
        {
            while self.is_locked() {
                spin_loop()
            }
        }
    }

    fn unlock(&self) {
        self.lock.store(usize::MAX, Ordering::Relaxed);
    }

    fn try_lock(&self) -> bool {
        let hartid = hart::hartid();
        match self
            .lock
            .compare_exchange(usize::MAX, hartid, Ordering::Acquire, Ordering::Relaxed)
        {
            Ok(_) => true,
            Err(current) => current == hartid,
        }
    }
}

pub struct Ticket {
    locked: AtomicBool,
    next: AtomicPtr<Ticket>,
}

impl Ticket {
    pub const fn new() -> Self {
        Self {
            locked: AtomicBool::new(true),
            next: AtomicPtr::new(null_mut()),
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

impl InteriorLock for QueueLock {
    fn is_locked(&self) -> bool {
        todo!()
    }

    fn lock(&self) {
        // 泄露 Ticket 到堆，unlock 的时候回收
        let node = Box::into_raw(Box::new(Ticket::new()));
        let prev = self.tail.swap(node, Ordering::Acquire);
        if !prev.is_null() {
            unsafe {
                (*prev).next.store(node, Ordering::Relaxed);
                while (*node).locked.load(Ordering::Acquire) {
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

    fn unlock(&self) {
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
                let mut next: *mut Ticket = null_mut();
                while next.is_null() {
                    next = owned.next.load(Ordering::Acquire);
                    spin_loop()
                }
                let succ = &mut (*next);
                drop(Box::from_raw(owned));
                succ.locked.store(false, Ordering::Relaxed);
            },
        }
    }
}