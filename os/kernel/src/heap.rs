//! 内核堆（见 notes/internals.md「堆分配器」、notes/mm.md「帧池」）。
//!
//! talc 经 [`FrameSource`] 按需供血：堆耗尽时从帧池取 1 MiB 连续帧块
//! 建立新堆区（talc 支持多块不连续区域）。帧块所有权随 claim 转移给
//! 堆——`Source::acquire` 内不得触碰堆与 TalcLock（talc 契约），故以
//! `mem::forget` 终身持有，不设归还记账。初始化顺序错误（首次分配早于
//! 帧池）会在帧池访问处以 panic 显式暴露。

use core::alloc::Layout;

use talc::{
    base::{binning::Binning, Talc},
    source::Source,
    sync::TalcLock,
    DefaultBinning,
};

use crate::{frame, mm, sync::RawSpinlock};

/// 每次扩堆帧块：1 MiB（256 帧）。
const CHUNK: usize = 1 << 20;

struct FrameSource;

impl core::fmt::Debug for FrameSource {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("FrameSource")
    }
}

// SAFETY: acquire 仅触帧池（独立 Spinlock，不重入 TalcLock、不经全局
// 分配器）；claim 的区域独占、4KiB 对齐、容量远超 talc 元数据需求。
unsafe impl Source for FrameSource {
    fn acquire<B: Binning>(talc: &mut Talc<Self, B>, _layout: Layout) -> Result<(), ()> {
        let tracker = frame::alloc_contiguous(CHUNK / frame::FRAME_SIZE).ok_or(())?;
        let base = mm::phys_to_virt(tracker.base.addr());
        // SAFETY: 区域独占且已清零（分配即清零），claim 成功后帧所有权
        // 转移给堆；失败则 tracker Drop 归还帧池。
        match unsafe { talc.claim(base as *mut u8, CHUNK) } {
            Some(_) => {
                core::mem::forget(tracker);
                Ok(())
            }
            None => Err(()),
        }
    }
}

/// 全局分配器：talc，锁注入内核自研的 [`RawSpinlock`]，内存源接帧池。
#[global_allocator]
static HEAP: TalcLock<RawSpinlock, FrameSource, DefaultBinning> = TalcLock::new(FrameSource);

/// 堆供血自检：首次分配触发 FrameSource claim，验证帧池→堆链路与数据完整性。
pub fn selftest() {
    let before = frame::free_frames();
    let v: alloc::vec::Vec<u32> = (0..8192u32).collect();
    assert_eq!(v.iter().sum::<u32>(), (0..8192u32).sum::<u32>());
    let claimed = before - frame::free_frames();
    assert!(claimed > 0, "heap did not claim frames from the pool");
    log!(Heap, "{} frame(s) claimed, alloc/verify ok", claimed);
}
