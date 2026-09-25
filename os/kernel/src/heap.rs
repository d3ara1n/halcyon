//! 内核堆（见 notes/impls/internals.md「堆分配器」、notes/impls/mm.md「帧池」）。
//!
//! talc 经 [`SystemSource`] 按需消费启动期隔离并预清零的固定 system tickets。
//! `Source::acquire` 在 allocator 忙碌时执行，因此该路径只做 O(1) ticket pop 与
//! claim，不进入 user FramePool、不扫描区间，也不执行大块清零。ticket 用尽后普通
//! metadata 分配明确 OOM；recovery 子预算不能作为回退。

use core::{
    alloc::Layout,
    sync::atomic::{AtomicUsize, Ordering},
};

use talc::{
    DefaultBinning,
    base::{Counters, Talc, binning::Binning},
    source::Source,
    sync::TalcLock,
};

use crate::{
    frame, mm,
    sync::{RankedRawSpinlock, ranks},
};

struct SystemSource;

static SOURCE_FAILURES: AtomicUsize = AtomicUsize::new(0);
static LAST_FAILED_LAYOUT_SIZE: AtomicUsize = AtomicUsize::new(0);
static LAST_FAILED_LAYOUT_ALIGN: AtomicUsize = AtomicUsize::new(0);
static MAX_FAILED_LAYOUT_SIZE: AtomicUsize = AtomicUsize::new(0);
static PEAK_ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
static MIN_AVAILABLE_BYTES: AtomicUsize = AtomicUsize::new(usize::MAX);
static PEAK_FRAGMENT_COUNT: AtomicUsize = AtomicUsize::new(0);

fn update_max(target: &AtomicUsize, value: usize) {
    let mut current = target.load(Ordering::Relaxed);
    while current < value {
        match target.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
}

fn record_source_failure(layout: Layout) {
    SOURCE_FAILURES.fetch_add(1, Ordering::Relaxed);
    LAST_FAILED_LAYOUT_SIZE.store(layout.size(), Ordering::Relaxed);
    LAST_FAILED_LAYOUT_ALIGN.store(layout.align(), Ordering::Relaxed);
    update_max(&MAX_FAILED_LAYOUT_SIZE, layout.size());
}

fn record_counters(counters: &Counters) {
    update_max(&PEAK_ALLOCATED_BYTES, counters.allocated_bytes);
    let mut current = MIN_AVAILABLE_BYTES.load(Ordering::Relaxed);
    while current > counters.available_bytes {
        match MIN_AVAILABLE_BYTES.compare_exchange_weak(
            current,
            counters.available_bytes,
            Ordering::Relaxed,
            Ordering::Relaxed,
        ) {
            Ok(_) => break,
            Err(observed) => current = observed,
        }
    }
    update_max(&PEAK_FRAGMENT_COUNT, counters.fragment_count);
}

impl core::fmt::Debug for SystemSource {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SystemSource")
    }
}

// SAFETY: acquire 只消费独立的固定 system ticket；区间在启动期已经清零，
// 不重入 TalcLock，也不接触 user FramePool。
unsafe impl Source for SystemSource {
    fn acquire<B: Binning>(talc: &mut Talc<Self, B>, layout: Layout) -> Result<(), ()> {
        let ticket = match frame::take_heap_chunk() {
            Some(ticket) => ticket,
            None => {
                record_source_failure(layout);
                return Err(());
            }
        };
        let range = ticket.range();
        let base = mm::phys_to_virt(range.start());
        // SAFETY: ticket 表达该预清零连续区间的唯一所有权；成功 claim 后消费
        // ticket，把所有权永久转移给不支持归还的全局堆。
        if unsafe { talc.claim(base as *mut u8, range.len()) }.is_none() {
            record_source_failure(layout);
            return Err(());
        }
        let _ = ticket.into_range();
        Ok(())
    }
}

/// 全局分配器：talc，锁注入带锁序秩的内核自研 [`RankedRawSpinlock`]，
/// 内存源只接物理隔离的 system heap tickets。
#[global_allocator]
static HEAP: TalcLock<RankedRawSpinlock<{ ranks::HEAP.0 }>, SystemSource, DefaultBinning> =
    TalcLock::new(SystemSource);

pub(crate) fn sample() {
    let counters = *HEAP.lock().counters();
    record_counters(&counters);
}

pub(crate) fn reset_peaks() {
    let counters = *HEAP.lock().counters();
    PEAK_ALLOCATED_BYTES.store(counters.allocated_bytes, Ordering::Relaxed);
    MIN_AVAILABLE_BYTES.store(counters.available_bytes, Ordering::Relaxed);
    PEAK_FRAGMENT_COUNT.store(counters.fragment_count, Ordering::Relaxed);
    SOURCE_FAILURES.store(0, Ordering::Relaxed);
    LAST_FAILED_LAYOUT_SIZE.store(0, Ordering::Relaxed);
    LAST_FAILED_LAYOUT_ALIGN.store(0, Ordering::Relaxed);
    MAX_FAILED_LAYOUT_SIZE.store(0, Ordering::Relaxed);
}

/// Ready 后在 trap 出口与 Bind/Reset 采样；不能代表 syscall 内短暂峰值。
/// 失败布局只记录 source 取不到 ticket 或 claim 失败的请求；不等于最大连续空闲块。
pub(crate) fn log_snapshot(stage: &str) {
    let counters = *HEAP.lock().counters();
    record_counters(&counters);
    let chunks = frame::consumed_heap_chunks();
    log!(
        Heap,
        "{stage}: claimed {} allocated {} available {} fragments {} chunks {}; peak_allocated {} min_available {} peak_fragments {} source_failures {} last_failed_layout {}+{} max_failed_layout {}",
        counters.claimed_bytes,
        counters.allocated_bytes,
        counters.available_bytes,
        counters.fragment_count,
        chunks,
        PEAK_ALLOCATED_BYTES.load(Ordering::Relaxed),
        MIN_AVAILABLE_BYTES.load(Ordering::Relaxed),
        PEAK_FRAGMENT_COUNT.load(Ordering::Relaxed),
        SOURCE_FAILURES.load(Ordering::Relaxed),
        LAST_FAILED_LAYOUT_SIZE.load(Ordering::Relaxed),
        LAST_FAILED_LAYOUT_ALIGN.load(Ordering::Relaxed),
        MAX_FAILED_LAYOUT_SIZE.load(Ordering::Relaxed)
    );
}
