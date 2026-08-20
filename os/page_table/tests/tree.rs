//! TableTree 切段算法与生命周期测试（host）。
//!
//! 用例清单见 notes/mm.md「测试集」；`big_unaligned_cross_table`
//! 是 mm-map-bug 的数值原案。

use core::cell::Cell;
use page_table::{flags, FrameExhausted, FrameMemory, FrameNumber, MapError, Pte, TableTree, ENTRIES, Vpn, Ppn};
use std::rc::Rc;

/// 共享计数器：树销毁后仍可读，验证帧收支平衡。
#[derive(Default)]
struct Counters {
    live: Cell<usize>,
}

struct MockFrames {
    tables: Vec<[Pte; ENTRIES]>,
    free: Vec<usize>,
    counters: Rc<Counters>,
}

impl MockFrames {
    fn new(counters: Rc<Counters>) -> Self {
        Self {
            tables: Vec::new(),
            free: Vec::new(),
            counters,
        }
    }
}

impl FrameMemory for MockFrames {
    fn alloc_frame(&mut self) -> Result<FrameNumber, FrameExhausted> {
        self.counters.live.set(self.counters.live.get() + 1);
        if let Some(f) = self.free.pop() {
            Ok(FrameNumber(f))
        } else {
            self.tables.push([Pte::invalid(); ENTRIES]);
            Ok(FrameNumber(self.tables.len() - 1))
        }
    }

    fn free_frame(&mut self, frame: FrameNumber) {
        self.counters.live.set(self.counters.live.get() - 1);
        self.free.push(frame.0);
    }

    fn table_mut(&mut self, frame: FrameNumber) -> &mut [Pte; ENTRIES] {
        &mut self.tables[frame.0]
    }
}

type Tree = TableTree<MockFrames, 3>;

fn tree(counters: &Rc<Counters>) -> Tree {
    Tree::new(MockFrames::new(counters.clone())).expect("建树失败")
}

/// mm-map-bug 数值原案：未对齐起点跨多张表的 32MB 区间（8192 页）。
#[test]
fn big_unaligned_cross_table() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let (start, count, ppn) = (65usize, 8192usize, 33usize);
    t.map(Vpn(start), count, Ppn(ppn), flags::USER_DATA).unwrap();

    // 抽查边界与中段：物理连续、全部可译
    for vpn in [65, 66, 511, 512, 513, 4096, 8256, 8257 - 1] {
        let m = t.translate(Vpn(vpn)).expect("应当已映射");
        assert_eq!(m.ppn.0, ppn + (vpn - start), "vpn={}", vpn);
        assert_eq!(m.flags, flags::USER_DATA);
    }
    assert!(t.translate(Vpn(start + count)).is_none());
    drop(t);
    assert_eq!(counters.live.get(), 0, "表帧未全部归还");
}

/// 表对齐 + 物理对齐 → 2MiB mega。
#[test]
fn table_aligned_creates_mega() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    t.map(Vpn(512 * 3), 512, Ppn(512 * 7), flags::KERNEL_DIRECT).unwrap();
    let m = t.translate(Vpn(512 * 3 + 123)).unwrap();
    assert_eq!(m.level, 1, "应落在 2MiB mega");
    assert_eq!(m.ppn.0, 512 * 7 + 123);
}

/// 1GiB root mega（LEVELS=3 顶层叶）。
#[test]
fn one_g_mega() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    t.map(Vpn(512 * 512 * 2), 512 * 512, Ppn(512 * 512 * 5), flags::KERNEL_DIRECT).unwrap();
    let m = t.translate(Vpn(512 * 512 * 2 + 9999)).unwrap();
    assert_eq!(m.level, 2);
    assert_eq!(m.ppn.0, 512 * 512 * 5 + 9999);
}

/// 未对齐首尾混合。
#[test]
fn mixed_head_tail() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    t.map(Vpn(5), 517, Ppn(9), flags::USER_DATA).unwrap();
    for vpn in 5..5 + 517 {
        assert_eq!(t.translate(Vpn(vpn)).unwrap().ppn.0, 9 + vpn - 5);
    }
}

/// 同 ppn 同 flags → 幂等成功。
#[test]
fn idempotent_same_mapping() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    t.map(Vpn(512), 512, Ppn(512), flags::USER_DATA).unwrap();
    t.map(Vpn(512), 512, Ppn(512), flags::USER_DATA).unwrap();
    // 部分重叠幂等
    t.map(Vpn(512 + 100), 10, Ppn(512 + 100), flags::USER_DATA).unwrap();
}

/// 异 flags 冲突。
#[test]
fn conflict_different_flags() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    t.map(Vpn(0), 16, Ppn(0), flags::USER_DATA).unwrap();
    assert_eq!(
        t.map(Vpn(4), 4, Ppn(4), flags::USER_RODATA),
        Err(MapError::Conflict { vpn: Vpn(4) })
    );
}

/// 同 flags 异 ppn 冲突。
#[test]
fn conflict_different_ppn() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    t.map(Vpn(0), 16, Ppn(0), flags::USER_DATA).unwrap();
    assert_eq!(
        t.map(Vpn(4), 4, Ppn(100), flags::USER_DATA),
        Err(MapError::Conflict { vpn: Vpn(4) })
    );
}

/// mega 槽位下已有更细子树 → 保守冲突。
#[test]
fn conflict_mega_over_subtree() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    // 物理不对齐 → 只能建 4KiB 叶，留下分支链
    t.map(Vpn(512), 512, Ppn(5), flags::USER_DATA).unwrap();
    assert_eq!(t.translate(Vpn(512)).unwrap().level, 0);
    // 再以对齐 ppn 映射同区 → mega 撞上子树
    assert_eq!(
        t.map(Vpn(512), 512, Ppn(512), flags::USER_DATA),
        Err(MapError::Conflict { vpn: Vpn(512) })
    );
}

/// 全量解除后重映射。
#[test]
fn unmap_then_remap() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    t.map(Vpn(100), 100, Ppn(200), flags::USER_DATA).unwrap();
    t.unmap(Vpn(100), 100);
    assert!(t.translate(Vpn(150)).is_none());
    t.map(Vpn(100), 100, Ppn(900), flags::USER_CODE).unwrap();
    assert_eq!(t.translate(Vpn(150)).unwrap().ppn.0, 950);
}

/// 部分解除 mega：分裂，邻居保留。
#[test]
fn partial_unmap_splits_mega() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let base = 512 * 10;
    t.map(Vpn(base), 512, Ppn(base), flags::USER_DATA).unwrap();
    assert_eq!(t.translate(Vpn(base + 5)).unwrap().level, 1);

    t.unmap(Vpn(base + 100), 1);
    assert!(t.translate(Vpn(base + 100)).is_none());
    // 分裂后邻居变 4KiB 叶但映射保持
    for off in [0, 99, 101, 511] {
        let m = t.translate(Vpn(base + off)).unwrap();
        assert_eq!(m.ppn.0, base + off, "off={}", off);
    }
}

/// 在 mega 内做细粒度映射：物理连续 + 同 flags → 分裂后幂等成功；
/// 异物理 → 冲突。
#[test]
fn finer_map_over_mega() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    t.map(Vpn(512), 512, Ppn(512), flags::USER_DATA).unwrap();
    // 兼容的细粒度重映射（同物理连续性）
    t.map(Vpn(512 + 7), 1, Ppn(512 + 7), flags::USER_DATA).unwrap();
    // 异物理的细粒度重映射
    assert_eq!(
        t.map(Vpn(512 + 8), 1, Ppn(999_999), flags::USER_DATA),
        Err(MapError::Conflict { vpn: Vpn(512 + 8) })
    );
}

/// 超出 sv39 地址宽度。
#[test]
fn out_of_range() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    assert_eq!(t.map(Vpn(1 << 27), 1, Ppn(0), flags::USER_DATA), Err(MapError::OutOfRange));
    // 溢出
    assert_eq!(t.map(Vpn(usize::MAX), 2, Ppn(0), flags::USER_DATA), Err(MapError::OutOfRange));
}

/// 空区间为 no-op。
#[test]
fn zero_count_noop() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    t.map(Vpn(0), 0, Ppn(0), flags::USER_DATA).unwrap();
    assert!(t.translate(Vpn(0)).is_none());
}

/// Drop 释放全部表帧（多层级压力）。
#[test]
fn drop_frees_all_frames() {
    let counters = Rc::new(Counters::default());
    {
        let mut t = tree(&counters);
        t.map(Vpn(65), 8192, Ppn(33), flags::USER_DATA).unwrap();
        t.map(Vpn(512 * 100), 512, Ppn(512 * 100), flags::USER_CODE).unwrap();
        t.map(Vpn(0), 1, Ppn(1), flags::USER_RODATA).unwrap();
        assert!(counters.live.get() > 1);
    }
    assert_eq!(counters.live.get(), 0, "表帧未全部归还");
}
