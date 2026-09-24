//! 锁外安全点的运行停止；启动 Ready/Failed gate 不回退。

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static STOPPED: AtomicBool = AtomicBool::new(false);
static REQUESTED: AtomicBool = AtomicBool::new(false);
/// 固定的物理停驻证据，不承担对象退休或用户结果责任。
static PARKED: AtomicU64 = AtomicU64::new(0);

pub(crate) fn check() {
    if (REQUESTED.load(Ordering::Acquire) || crate::clock::is_failed())
        && !STOPPED.swap(true, Ordering::AcqRel)
    {
        let failed = crate::registry::try_ipi_slots(crate::registry::admitted_mask());
        crate::rt::runtime_stop_report(failed, crate::clock::is_failed());
    }
    if STOPPED.load(Ordering::Acquire) {
        let slot = crate::hart::current().slot();
        debug_assert!(slot < crate::registry::HART_NUM_LIMIT);
        crate::sbi::clear_ssip();
        PARKED.fetch_or(1u64 << slot, Ordering::Release);
        crate::hart::park();
    }
}

pub(crate) fn request() {
    REQUESTED.store(true, Ordering::Release);
    let mask = crate::registry::admitted_mask_if_published();
    if mask != 0 {
        let _ = crate::registry::try_ipi_slots(mask);
    }
}

pub(crate) fn parked_mask() -> u64 {
    PARKED.load(Ordering::Acquire)
}

#[cfg(debug_assertions)]
#[unsafe(no_mangle)]
pub extern "C" fn debug_request_runtime_stop() {
    request();
}

#[cfg(debug_assertions)]
#[unsafe(no_mangle)]
pub extern "C" fn debug_runtime_stop_parked_mask() -> u64 {
    parked_mask()
}

#[cfg(debug_assertions)]
#[used]
static DEBUG_STOP_ENTRY: extern "C" fn() = debug_request_runtime_stop;

#[cfg(debug_assertions)]
#[used]
static DEBUG_MASK_ENTRY: extern "C" fn() -> u64 = debug_runtime_stop_parked_mask;
