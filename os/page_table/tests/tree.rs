//! TableTree 切段算法与生命周期测试（host）。
//!
//! 用例清单见 notes/impls/mm.md「测试集」；`big_unaligned_cross_table`
//! 是 mm-map-bug 的数值原案。

use core::cell::Cell;
use page_table::{flags, FrameExhausted, FrameMemory, FrameNumber, MapError, Pte, TableTree, ENTRIES, Vpn, Ppn};
use std::rc::Rc;

/// 共享计数器：树销毁后仍可读，验证帧收支平衡。
#[derive(Default)]
struct Counters {
    live: Cell<usize>,
    deny_alloc: Cell<bool>,
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
        if self.counters.deny_alloc.get() {
            return Err(FrameExhausted);
        }
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
    Tree::new(MockFrames::new(counters.clone())).expect("failed to build tree")
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
        let m = t.translate(Vpn(vpn)).expect("expected mapped");
        assert_eq!(m.ppn.0, ppn + (vpn - start), "vpn={}", vpn);
        assert_eq!(m.flags, flags::USER_DATA);
    }
    assert!(t.translate(Vpn(start + count)).is_none());
    drop(t);
    assert_eq!(counters.live.get(), 0, "table frames not fully returned");
}

/// 表对齐 + 物理对齐 → 2MiB mega。
#[test]
fn table_aligned_creates_mega() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    t.map(Vpn(512 * 3), 512, Ppn(512 * 7), flags::KERNEL_DIRECT).unwrap();
    let m = t.translate(Vpn(512 * 3 + 123)).unwrap();
    assert_eq!(m.level, 1, "expected 2MiB mega level");
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
    t.unmap(Vpn(100), 100).unwrap();
    assert!(t.translate(Vpn(150)).is_none());
    t.map(Vpn(100), 100, Ppn(900), flags::USER_CODE).unwrap();
    assert_eq!(t.translate(Vpn(150)).unwrap().ppn.0, 950);
}

/// 跨 512 页子表边界批量解除，必须使用每个子表的真实覆盖基址。
#[test]
fn unmap_crosses_child_table_boundary() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    t.map(Vpn(480), 80, Ppn(1000), flags::USER_DATA).unwrap();
    t.unmap(Vpn(500), 40).unwrap();
    for vpn in 480..500 {
        assert!(t.translate(Vpn(vpn)).is_some(), "left neighbor {vpn}");
    }
    for vpn in 500..540 {
        assert!(t.translate(Vpn(vpn)).is_none(), "stale mapping {vpn}");
    }
    for vpn in 540..560 {
        assert!(t.translate(Vpn(vpn)).is_some(), "right neighbor {vpn}");
    }
}

/// 部分解除 mega：分裂，邻居保留。
#[test]
fn partial_unmap_splits_mega() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let base = 512 * 10;
    t.map(Vpn(base), 512, Ppn(base), flags::USER_DATA).unwrap();
    assert_eq!(t.translate(Vpn(base + 5)).unwrap().level, 1);

    t.unmap(Vpn(base + 100), 1).unwrap();
    assert!(t.translate(Vpn(base + 100)).is_none());
    // 分裂后邻居变 4KiB 叶但映射保持
    for off in [0, 99, 101, 511] {
        let m = t.translate(Vpn(base + off)).unwrap();
        assert_eq!(m.ppn.0, base + off, "off={}", off);
    }
}

/// mega 分裂分配失败必须显式返回错误，原映射保持完整。
#[test]
fn partial_unmap_split_oom_preserves_mapping() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let base = 512 * 10;
    t.map(Vpn(base), 512, Ppn(base), flags::USER_DATA).unwrap();
    counters.deny_alloc.set(true);
    assert_eq!(t.unmap(Vpn(base + 123), 1), Err(MapError::FrameExhausted));
    for off in [0, 122, 123, 124, 511] {
        assert_eq!(t.translate(Vpn(base + off)).unwrap().ppn.0, base + off);
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
    assert_eq!(counters.live.get(), 0, "table frames not fully returned");
}

/// clear_slots 只清槽位、不递归回收：被剥离分支的子树帧仍记账为存活，
/// 随后 Drop 也不得触碰它们（内核共享顶层项剥离的数值契约）。
#[test]
fn clear_slots_detaches_without_recursion() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    // 用户侧一页 + 模拟共享进来的顶层分支项（指向手工构造的外部表）。
    t.map(Vpn(0), 1, Ppn(1), flags::USER_DATA).unwrap();

    // 在 root 空槽挂一个分支项，其下再挂一层叶表与一个叶子映射。
    let sub = t.mem_mut().alloc_frame().unwrap();
    let leaf = t.mem_mut().alloc_frame().unwrap();
    {
        let tables = &mut *t.mem_mut();
        tables.tables[sub.0][7] = Pte::branch(leaf);
        tables.tables[leaf.0][9] = Pte::leaf(Ppn(0x999), flags::KERNEL_DIRECT);
        tables.tables[0][500] = Pte::branch(sub); // root 是 0 号帧
    }

    // 剥离槽位：不归还任何帧，翻译随之消失。
    // 记账：root(1) + 用户映射的中间表/叶表(2) + 手工 sub/leaf(2) = 5。
    t.clear_slots(FrameNumber(0), 500, 501);
    assert_eq!(counters.live.get(), 5, "detach must not return subtree frames");
    assert!(t.translate(Vpn(500 * 512 + 7)).is_none(), "detached slot must not translate");

    // Drop 后只回收用户侧帧；外部子树的 2 帧归调用方所有，仍存活。
    drop(t);
    assert_eq!(counters.live.get(), 2, "Drop must not reclaim detached kernel subtree");
}
