//! 内核堆（见 notes/internals.md「堆分配器」）。
//!
//! M0 阶段堆为静态数组（.bss 内，首次分配时惰性 claim）；M2 帧分配器
//! 就位后切换为 DTB 多段内存并支持按需扩堆。

use talc::{source::Claim, sync::TalcLock, DefaultBinning};

use crate::sync::RawSpinlock;

/// 静态堆区大小，与旧内核 HEAP_SIZE 保持一致。
const HEAP_SIZE: usize = 0x800000;

static mut HEAP_ARENA: [u8; HEAP_SIZE] = [0; HEAP_SIZE];

/// 全局分配器：talc，锁注入内核自研的 [`RawSpinlock`]。
#[global_allocator]
static HEAP: TalcLock<RawSpinlock, Claim, DefaultBinning> = TalcLock::new(unsafe {
    // SAFETY: HEAP_ARENA 独占且未作他用；Claim 首次被问询时注册该区域。
    Claim::array(core::ptr::addr_of_mut!(HEAP_ARENA))
});
