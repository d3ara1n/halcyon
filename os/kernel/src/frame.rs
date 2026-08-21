//! 物理帧池内核侧：真实内存访问 + 全局容器 + 启动注册（见 notes/mm.md「帧池」）。
//!
//! 帧池算法在 os/frame_pool（纯逻辑，host 可测），本模块只做三件事：
//! - [`PhysAccess`]：PoolMemory 的内核实现——经 `mm::phys_to_virt` 访问
//!   （初始化于 mm::init 之后，恒在高半区直映射下工作）；
//! - 全局容器：`Spinlock<Option<FramePool>>`，初始化后只读访问；
//! - 启动注册：DTB memory 段剔除 SBI + 内核镜像/栈 + initfs 占用。

use frame_pool::{FramePool, PoolMemory, RegionNode};
use page_table::{FrameNumber, PAGE_BITS};

use crate::{board::BoardInfo, external, mm, sync::Spinlock};

const PAGE_SIZE: usize = 1 << PAGE_BITS;

/// 帧大小（字节）。堆供血等帧池消费方使用。
pub const FRAME_SIZE: usize = PAGE_SIZE;

/// 帧内存的真实访问：高半区直映射。
struct PhysAccess;

impl PoolMemory for PhysAccess {
    fn read_meta(&mut self, frame: FrameNumber) -> RegionNode {
        // SAFETY: 空闲区间首帧已注册，元数据槽（前两个 usize）有效；
        // 页对齐地址，usize 对齐访问成立。
        let slot = unsafe { &*(mm::phys_to_virt(frame.addr()) as *const [usize; 2]) };
        RegionNode {
            len: slot[0],
            next: slot[1],
        }
    }

    fn write_meta(&mut self, frame: FrameNumber, node: RegionNode) {
        // SAFETY: 同上；写入槽位是池对空闲区间的唯一记账。
        let slot = unsafe { &mut *(mm::phys_to_virt(frame.addr()) as *mut [usize; 2]) };
        *slot = [node.len, node.next];
    }

    fn clear_frames(&mut self, base: FrameNumber, count: usize) {
        // SAFETY: [base, base+count) 刚被切出为已分配，清零不破坏任何
        // 空闲链节点（节点只挂空闲区间首帧）。
        unsafe {
            core::ptr::write_bytes(mm::phys_to_virt(base.addr()) as *mut u8, 0, count * PAGE_SIZE);
        }
    }
}

static POOL: Spinlock<Option<FramePool<PhysAccess>>> = Spinlock::new(None);

/// 持锁访问帧池（初始化前访问为致命错误）。
fn with_pool<R>(f: impl FnOnce(&mut FramePool<PhysAccess>) -> R) -> R {
    f(POOL.lock().as_mut().expect("帧池未初始化"))
}

/// 解析板级信息并初始化帧池：DTB memory 段剔除启动占用后注册。
pub fn init(board: &BoardInfo) {
    let kernel_end = mm::virt_to_phys(external::_kernel_end as *const () as usize);
    // 启动占用最多两洞（镜像/栈 + initfs），固定容量——启动路径零堆依赖
    let mut holes = [(0usize, 0usize); 2];
    holes[0] = (external::sbi_start(), kernel_end);
    let mut hole_count = 1usize;
    if let Some((addr, len)) = board.initfs {
        holes[1] = (addr, addr + len);
        hole_count = 2;
    }

    let mut p = FramePool::new(PhysAccess);
    let mut regions = 0;
    for region in board.memories() {
        subtract(region.start, region.start + region.len, &holes[..hole_count], |s, e| {
            p.add_region(FrameNumber::from_addr(s), FrameNumber::from_addr(e));
            regions += 1;
        });
    }
    assert!(regions > 0, "剔除启动占用后无任何空闲内存段");

    let free = p.free_frames();
    log!(
        Frame,
        "{} region(s), {} frame(s) free ({:#x} bytes)",
        regions,
        free,
        free * PAGE_SIZE
    );
    *POOL.lock() = Some(p);
}

/// 从 `[start, end)` 减去 `holes`，对每个剩余子区间调用 `emit`。
fn subtract(
    start: usize,
    end: usize,
    holes: &[(usize, usize)],
    mut emit: impl FnMut(usize, usize),
) {
    let mut cursor = start;
    for &(hs, he) in holes {
        if he <= cursor || hs >= end {
            continue; // 洞在本区间外
        }
        let hs = hs.max(cursor);
        if hs > cursor {
            emit(cursor, hs);
        }
        cursor = he.min(end);
        if cursor >= end {
            return;
        }
    }
    if cursor < end {
        emit(cursor, end);
    }
}

/// 分配 `count` 个物理连续帧（分配即清零），RAII 归还。
pub fn alloc_contiguous(count: usize) -> Option<FrameTracker> {
    with_pool(|p| p.alloc_contiguous(count)).map(|base| FrameTracker { base, count })
}

/// 归还一段页对齐物理区间（bootstrap 回收用；区间必须此前被剔除）。
pub fn free_range(start_pa: usize, end_pa: usize) {
    assert!(start_pa % PAGE_SIZE == 0 && end_pa % PAGE_SIZE == 0 && start_pa < end_pa);
    with_pool(|p| {
        for frame in (start_pa..end_pa).step_by(PAGE_SIZE) {
            p.dealloc(page_table::FrameNumber::from_addr(frame), 1);
        }
    });
}

/// 帧池剩余空闲帧数。
pub fn free_frames() -> usize {
    with_pool(|p| p.free_frames())
}

/// RAII 帧所有权：Drop 时整批归还。
pub struct FrameTracker {
    pub base: FrameNumber,
    pub count: usize,
}

impl Drop for FrameTracker {
    fn drop(&mut self) {
        with_pool(|p| p.dealloc(self.base, self.count));
    }
}

/// 冒烟：分配→写入→归还→重取验证清零，全程真硬件访问。
pub fn smoke() {
    let t = alloc_contiguous(8).expect("冒烟分配失败");
    let slots = mm::phys_to_virt(t.base.addr()) as *mut usize;
    // SAFETY: 冒烟持有 [base, base+8)，写首 8 槽不越界；高半区直映射下访问。
    unsafe {
        for i in 0..8 {
            slots.add(i).write_volatile(0xDEAD_0000 + i);
        }
    }
    let before = free_frames();
    drop(t);
    assert_eq!(free_frames(), before + 8, "帧归还记账不符");
    let t2 = alloc_contiguous(8).expect("重取失败");
    // SAFETY: 同上；首帧首槽读回验证分配即清零。
    let first = unsafe { *(mm::phys_to_virt(t2.base.addr()) as *const usize) };
    assert!(first == 0, "分配未清零");
    log!(Frame, "smoke: alloc/write/dealloc/re-zero ok");
}
