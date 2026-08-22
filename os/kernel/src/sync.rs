//! 锁原语（见 notes/internals.md「锁原语」）。
//!
//! - [`RawSpinlock`]：CAS 自旋锁，不含中断处理，实现 `lock_api::RawMutex`
//!   供 talc 等外部泛型使用（协作式内核内 SIE 恒 0，无需关中断）。
//! - [`Spinlock<T>`]：内核自用容器，获取期间额外关闭本地中断——
//!   中断处理路径若触碰本 hart 持有的锁会同核死锁，关中断是正确性要求。

use core::{
    cell::UnsafeCell,
    fmt,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicU32, Ordering},
};

use crate::csr::SSTATUS_SIE;

/// 获取前的本地中断状态。
#[must_use]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct InterruptState {
    sie_was_enabled: bool,
}

/// 关闭本地中断（sstatus.SIE 清零），返回此前的状态。
pub fn disable_interrupts() -> InterruptState {
    // SAFETY: 仅读写 sstatus.SIE 位；S 态特权指令。
    let old: usize;
    unsafe {
        core::arch::asm!(
            "csrrc {old}, sstatus, {mask}",
            old = out(reg) old,
            mask = in(reg) SSTATUS_SIE,
            options(nomem)
        )
    }
    InterruptState {
        sie_was_enabled: old & SSTATUS_SIE != 0,
    }
}

/// 恢复本地中断到 `state` 记录的状态。
pub fn restore_interrupts(state: InterruptState) {
    if state.sie_was_enabled {
        // SAFETY: 仅置回 sstatus.SIE 位。
        unsafe {
            core::arch::asm!(
                "csrrs x0, sstatus, {mask}",
                mask = in(reg) SSTATUS_SIE,
                options(nomem)
            )
        }
    }
}

/// CAS 自旋锁（无中断语义）。内存序由原子语义背书：acquire 成功路径
/// Acquire、release Release，跨核临界区可见性不依赖手写栅栏。
pub struct RawSpinlock {
    locked: AtomicU32,
}

impl RawSpinlock {
    pub const fn new() -> Self {
        Self {
            locked: AtomicU32::new(0),
        }
    }

    /// 自旋直到获取。
    fn acquire(&self) {
        while self
            .locked
            .compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    /// 释放。仅允许持有者调用。
    fn release(&self) {
        self.locked.store(0, Ordering::Release);
    }

    /// 尝试获取，成功返回 true。
    fn try_lock(&self) -> bool {
        self.locked
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }
}
unsafe impl lock_api::RawMutex for RawSpinlock {
    const INIT: Self = Self::new();
    type GuardMarker = lock_api::GuardNoSend;

    #[inline]
    fn lock(&self) {
        self.acquire();
    }

    #[inline]
    fn try_lock(&self) -> bool {
        Self::try_lock(self)
    }

    #[inline]
    unsafe fn unlock(&self) {
        self.release();
    }
}

/// 内核自旋锁容器：获取期间关本地中断。
pub struct Spinlock<T> {
    raw: RawSpinlock,
    data: UnsafeCell<T>,
}

/// SAFETY: 数据仅能在持有锁（本 hart 关中断 + 自旋互斥）时访问。
unsafe impl<T: Send> Sync for Spinlock<T> {}
unsafe impl<T: Send> Send for Spinlock<T> {}

/// [`Spinlock`] 的守卫，drop 时释放锁并恢复中断状态。
pub struct SpinlockGuard<'a, T> {
    lock: &'a Spinlock<T>,
    state: InterruptState,
}

impl<T> Spinlock<T> {
    pub const fn new(value: T) -> Self {
        Self {
            raw: RawSpinlock::new(),
            data: UnsafeCell::new(value),
        }
    }

    /// 获取锁：关本地中断后自旋等待。
    pub fn lock(&self) -> SpinlockGuard<'_, T> {
        let state = disable_interrupts();
        self.raw.acquire();
        SpinlockGuard { lock: self, state }
    }

    /// 尝试获取，失败返回 `None`（同样关中断，drop 时恢复）。
    pub fn try_lock(&self) -> Option<SpinlockGuard<'_, T>> {
        let state = disable_interrupts();
        if self.raw.try_lock() {
            Some(SpinlockGuard { lock: self, state })
        } else {
            restore_interrupts(state);
            None
        }
    }}

impl<T> Deref for SpinlockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: 持有锁，独占访问。
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinlockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: 持有锁，独占访问。
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinlockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.raw.release();
        restore_interrupts(self.state);
    }
}

impl<T: fmt::Debug> fmt::Debug for Spinlock<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.try_lock() {
            Some(guard) => guard.fmt(f),
            None => f.write_str("<locked>"),
        }
    }
}
