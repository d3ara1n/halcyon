//! 内核堆（见 notes/impls/internals.md「堆分配器」、notes/impls/mm.md「帧池」）。
//!
//! talc 经 [`SystemSource`] 按需消费启动期隔离并预清零的固定 system tickets。
//! `Source::acquire` 在 allocator 忙碌时执行，因此该路径只做 O(1) ticket pop 与
//! claim，不进入 user FramePool、不扫描区间，也不执行大块清零。ticket 用尽后普通
//! metadata 分配明确 OOM；recovery 子预算不能作为回退。

use core::alloc::Layout;

use talc::{
    DefaultBinning,
    base::{Talc, binning::Binning},
    source::Source,
    sync::TalcLock,
};

use crate::{
    frame, mm,
    sync::{RankedRawSpinlock, ranks},
};

struct SystemSource;

impl core::fmt::Debug for SystemSource {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("SystemSource")
    }
}

// SAFETY: acquire 只消费独立的固定 system ticket；区间在启动期已经清零，
// 不重入 TalcLock，也不接触 user FramePool。
unsafe impl Source for SystemSource {
    fn acquire<B: Binning>(talc: &mut Talc<Self, B>, _layout: Layout) -> Result<(), ()> {
        let ticket = frame::take_heap_chunk().ok_or(())?;
        let range = ticket.range();
        let base = mm::phys_to_virt(range.start());
        // SAFETY: ticket 表达该预清零连续区间的唯一所有权；成功 claim 后消费
        // ticket，把所有权永久转移给不支持归还的全局堆。
        if unsafe { talc.claim(base as *mut u8, range.len()) }.is_none() {
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

/// 堆供血自检：首次分配触发 system ticket claim，验证容量单向消费与数据完整性。
pub fn selftest() {
    let before = frame::remaining_heap_chunks();
    let v: alloc::vec::Vec<u32> = (0..8192u32).collect();
    assert_eq!(v.iter().sum::<u32>(), (0..8192u32).sum::<u32>());
    let claimed = before - frame::remaining_heap_chunks();
    assert!(claimed > 0, "heap did not consume a system ticket");
    log!(Heap, "{} system chunk(s) claimed, alloc/verify ok", claimed);
}
