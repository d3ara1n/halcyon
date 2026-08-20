use core::sync::atomic::{AtomicUsize, Ordering};

pub struct Semaphore {
    count: AtomicUsize,
}

impl Semaphore {
    pub const fn new(init: usize) -> Self {
        Self {
            count: AtomicUsize::new(init),
        }
    }

    pub fn up(&self) {
        self.count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn down(&self) -> bool {
        self.count
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |f| {
                if f > 0 {
                    Some(f - 1)
                } else {
                    None
                }
            })
            .is_ok()
    }
}
