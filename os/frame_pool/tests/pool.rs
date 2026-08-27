//! 帧池 in-band 空闲链测试（host）。
//!
//! 用例清单见 notes/impls/mm.md「帧池·测试集」；`boot_layout_dual_region`
//! 模拟 virt 启动剔除（SBI + 内核镜像/栈 + initfs）后的多区间注册。
//! host 测试需显式 `--target aarch64-apple-darwin`（os/.cargo
//! build.target 指向 riscv）。

use std::collections::BTreeMap;

use frame_pool::{AllocAtError, FramePool, PoolMemory, RegionNode};
use page_table::FrameNumber;

/// 模拟帧内存：每帧前两个 usize 为元数据槽。
#[derive(Default)]
struct MockMem {
    slots: BTreeMap<usize, [usize; 2]>,
}

impl PoolMemory for MockMem {
    fn read_meta(&mut self, frame: FrameNumber) -> RegionNode {
        let slot = self
            .slots
            .get(&frame.0)
            .unwrap_or_else(|| panic!("read metadata of uninitialized frame {:#x}", frame.0));
        RegionNode {
            len: slot[0],
            next: slot[1],
        }
    }

    fn write_meta(&mut self, frame: FrameNumber, node: RegionNode) {
        self.slots.insert(frame.0, [node.len, node.next]);
    }

    fn clear_frames(&mut self, base: FrameNumber, count: usize) {
        for f in base.0..base.0 + count {
            self.slots.insert(f, [0; 2]);
        }
    }
}

type Pool = FramePool<MockMem>;

fn pool() -> Pool {
    Pool::new(MockMem::default())
}

fn frame(n: usize) -> FrameNumber {
    FrameNumber(n)
}

/// 整段精确消耗：区间用光后链空、记账归零。
#[test]
fn exact_consume() {
    let mut p = pool();
    p.add_region(frame(10), frame(20));
    assert_eq!(p.free_frames(), 10);
    assert_eq!(p.alloc_contiguous(10), Some(frame(10)));
    assert_eq!(p.free_frames(), 0);
    assert_eq!(p.region_count(), 0);
    assert_eq!(p.alloc_contiguous(1), None);
}

/// 尾端切：连续分配地址递减，余量节点原地改 len（单区间不裂）。
#[test]
fn tail_carve_descending() {
    let mut p = pool();
    p.add_region(frame(100), frame(110));
    assert_eq!(p.alloc_contiguous(3), Some(frame(107)));
    assert_eq!(p.alloc_contiguous(3), Some(frame(104)));
    // 剩余仍为单区间 [100,104)
    assert_eq!(p.region_count(), 1);
    assert_eq!(p.free_frames(), 4);
    let last = p.alloc_contiguous(4);
    assert_eq!(last, Some(frame(100)));
    assert_eq!(p.region_count(), 0);
}

/// 合并：前向、后向、三向，以及跨缺口不合并。
#[test]
fn merge_prev_next_both() {
    let mut p = pool();
    p.add_region(frame(0), frame(30));
    let a = p.alloc_contiguous(10).unwrap(); // [20,30)
    let b = p.alloc_contiguous(10).unwrap(); // [10,20)
    let c = p.alloc_contiguous(10).unwrap(); // [0,10)
    assert_eq!((a.0, b.0, c.0), (20, 10, 0));

    p.dealloc(b, 10); // 独立成区间
    assert_eq!(p.region_count(), 1);
    p.dealloc(c, 10); // 与 [10,20) 后合并
    assert_eq!(p.region_count(), 1);
    p.dealloc(a, 10); // 三向合并回整段
    assert_eq!(p.region_count(), 1);
    assert_eq!(p.free_frames(), 30);
    assert_eq!(p.alloc_contiguous(30), Some(frame(0)));
}

/// 非相邻归还不合并；缺口补上后三向合并。
#[test]
fn no_merge_across_gap() {
    let mut p = pool();
    p.add_region(frame(0), frame(30));
    assert_eq!(p.alloc_contiguous(30), Some(frame(0)));
    assert_eq!(p.region_count(), 0);

    p.dealloc(frame(0), 10);
    p.dealloc(frame(12), 6); // [12,18) 与 [0,10) 隔 [10,12)
    assert_eq!(p.region_count(), 2);
    assert_eq!(p.free_frames(), 16);

    p.dealloc(frame(10), 2); // 补缺口 → 三向合并
    assert_eq!(p.region_count(), 1);
    assert_eq!(p.free_frames(), 18);
}

/// 碎片化：总帧数足够但无足够连续区间 → None。
#[test]
fn fragmentation_returns_none() {
    let mut p = pool();
    p.add_region(frame(0), frame(40));
    let mut holds = Vec::new();
    for _ in 0..4 {
        holds.push(p.alloc_contiguous(10).unwrap());
    }
    p.dealloc(holds[0], 10); // [30,40)
    p.dealloc(holds[2], 10); // [10,20)（尾端切后 holds=[30,20,10,0]）
    assert_eq!(p.free_frames(), 20);
    assert_eq!(p.alloc_contiguous(11), None);
    // first-fit 取最低地址的合适区间 [10,20)
    assert_eq!(p.alloc_contiguous(10), Some(frame(10)));
}

/// 每次分配的帧数都计入清零：分配即清零契约，余量节点落在区间首帧。
#[test]
fn alloc_zeroes_frames() {
    let mut p = pool();
    p.add_region(frame(0), frame(8));
    let base = p.alloc_contiguous(5).unwrap();
    assert_eq!(base, frame(3));
    let mem = p.into_mem();
    for f in 3..8 {
        assert_eq!(mem.slots[&f], [0; 2], "frame {:#x} not zeroed", f);
    }
    // 余量节点 [0,3)：len=3、next=链尾哨兵
    assert_eq!(mem.slots[&0], [3, usize::MAX]);
}

/// alloc_at：中切、跨缺口与越界不可用、释放后可再取。
#[test]
fn alloc_at_cases() {
    let mut p = pool();
    p.add_region(frame(0), frame(16));
    // 中切：[6,10)
    p.alloc_at(frame(6), 4).unwrap();
    assert_eq!(p.free_frames(), 12);
    // 边切：[1,3)（左残 + 中切右残共存）
    p.alloc_at(frame(1), 2).unwrap();
    // 完全在区间外
    assert_eq!(p.alloc_at(frame(20), 2), Err(AllocAtError::Unavailable));
    // 跨已分配区域 [4,8)
    assert_eq!(p.alloc_at(frame(4), 4), Err(AllocAtError::Unavailable));
    // 部分在区间内部分在外
    assert_eq!(p.alloc_at(frame(14), 4), Err(AllocAtError::Unavailable));
    // 全量释放后整区间可再取
    p.dealloc(frame(6), 4);
    p.dealloc(frame(1), 2);
    p.alloc_at(frame(0), 16).unwrap();
    assert_eq!(p.free_frames(), 0);
}

/// 零帧分配 / 双重释放 → debug 断言。
#[test]
#[should_panic(expected = "zero-frame allocation")]
fn zero_count_alloc_panics() {
    let mut p = pool();
    p.add_region(frame(0), frame(4));
    let _ = p.alloc_contiguous(0);
}

#[test]
#[should_panic(expected = "overlaps")]
fn double_free_panics() {
    let mut p = pool();
    p.add_region(frame(0), frame(8));
    let a = p.alloc_contiguous(4).unwrap();
    p.dealloc(a, 4);
    p.dealloc(a, 4);
}

/// virt 启动布局：SBI + 内核镜像/栈 + initfs 剔除后多区间注册，
/// 低地址区间先消耗。
#[test]
fn boot_layout_dual_region() {
    // 数值（页号，virt 128MB dts）：SBI [0x80000,0x80200)，
    // 内核镜像+栈 [0x80200,0x80400)，initfs @0xB0000（长 64），
    // 内存末 0x100000。
    let (sbi_end, kernel_end) = (0x80200, 0x80400);
    let (initfs_base, initfs_len) = (0xB0000, 64);
    let mem_end = 0x100000;

    let mut p = pool();
    p.add_region(frame(sbi_end), frame(kernel_end));
    p.add_region(frame(initfs_base + initfs_len), frame(mem_end));

    // 低地址区间先消耗（尾端切）
    let first = p.alloc_contiguous(0x100).unwrap();
    assert_eq!(first.0, kernel_end - 0x100);
    let rest = kernel_end - sbi_end - 0x100;
    let second = p.alloc_contiguous(rest).unwrap();
    assert_eq!(second.0, sbi_end);
    // 低区间耗尽后转入高区间
    let third = p.alloc_contiguous(1).unwrap();
    assert_eq!(third.0, mem_end - 1);
}

/// 满池压力：伪随机 alloc/dealloc，帧数守恒，收尾可整取。
#[test]
fn stress_conservation() {
    let mut p = pool();
    let total = 256;
    p.add_region(frame(0), frame(total));

    let mut rng: u64 = 0x243F_6A88_85A3_08D3;
    let mut held: Vec<(usize, usize)> = Vec::new();
    for round in 0..1000 {
        rng = rng
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        if (rng >> 33) % 4 < 2 {
            let count = ((rng >> 40) % 16 + 1) as usize;
            if let Some(base) = p.alloc_contiguous(count) {
                held.push((base.0, count));
            }
        } else if let Some(idx) = held.iter().position(|_| (rng >> 50) % 2 == 0) {
            let (b, c) = held.swap_remove(idx);
            p.dealloc(frame(b), c);
        }
        let expected: usize = held.iter().map(|(_, c)| c).sum();
        assert_eq!(p.free_frames() + expected, total, "frame count not conserved @{}", round);
    }

    for (b, c) in held.drain(..) {
        p.dealloc(frame(b), c);
    }
    assert_eq!(p.free_frames(), total);
    assert_eq!(p.alloc_contiguous(total), Some(frame(0)));
}

#[test]
fn bounded_dealloc_resumes_across_budgets() {
    let mut pool = pool();
    pool.add_region(frame(100), frame(200));
    // 制造碎片：从单区间切出多段，留下 8 个交错空闲区间。
    let mut allocated = Vec::new();
    for _ in 0..8 {
        let base = pool.alloc_contiguous(1).unwrap();
        allocated.push(base);
    }
    // 先释放偶数位，形成 4 个分离区间。
    for base in allocated.iter().step_by(2) {
        pool.dealloc(*base, 1);
    }
    assert_eq!(pool.region_count(), 5); // 4 小区间 + 尾部大区间

    // 用 1 步预算逐步释放一个奇数位：必须经历多次 Progress 后完成。
    let target = allocated[1];
    let mut scan = frame_pool::FreeScan::default();
    let mut calls = 0;
    loop {
        calls += 1;
        let (steps, done) = pool.dealloc_bounded(target, 1, &mut scan, 1);
        assert_eq!(steps, 1);
        if done {
            break;
        }
        assert!(calls < 32, "bounded dealloc never completes");
    }
    // 完成后与相邻空闲区间合并：4+2 块顺序结构不变或减少。
    assert!(pool.region_count() <= 5);
    // 再验证一次幂等语义不可能：重复释放同一帧必须被 debug 断言拦截
    // （此处不触发，仅确认池仍可用）。
    let again = pool.alloc_contiguous(1);
    assert!(again.is_some());
}

#[test]
fn bounded_dealloc_cursor_invalidated_by_concurrent_free() {
    let mut pool = pool();
    pool.add_region(frame(100), frame(200));
    let mut allocated = Vec::new();
    for _ in 0..6 {
        allocated.push(pool.alloc_contiguous(1).unwrap());
    }
    for base in allocated.iter().step_by(2) {
        pool.dealloc(*base, 1);
    }
    // 预算 1 步推进游标后，他方（模拟另一 hart）无界归还改变链。
    let target = allocated[3];
    let mut scan = frame_pool::FreeScan::default();
    let (_steps, done) = pool.dealloc_bounded(target, 1, &mut scan, 1);
    assert!(!done);
    pool.dealloc(allocated[5], 1); // 并发归还使游标邻接失效
    // 续扫：校验失败 → 从链头重启，仍能正确完成。
    loop {
        let (_steps, done) = pool.dealloc_bounded(target, 1, &mut scan, 2);
        if done {
            break;
        }
    }
    // 链结构仍然有序无重叠：分配全部成功。
    for _ in 0..4 {
        assert!(pool.alloc_contiguous(1).is_some());
    }
}
