//! TableTree 切段算法与生命周期测试（host）。
//!
//! 用例清单见 notes/impls/mm.md「测试集」；`big_unaligned_cross_table`
//! 是 mm-map-bug 的数值原案。

use core::cell::Cell;
use page_table::{
    ENTRIES, FrameExhausted, FrameMemory, FrameNumber, MapError, Ppn, Pte, ReservedTableFrame,
    RootSlotState, TableTree, Vpn, flags,
};
use std::rc::Rc;

/// 共享计数器：树销毁后仍可读，验证帧收支平衡。
#[derive(Default)]
struct Counters {
    live: Cell<usize>,
    deny_alloc: Cell<bool>,
    alloc_budget: Cell<Option<usize>>,
}

struct MockFrames {
    tables: Vec<[Pte; ENTRIES]>,
    counters: Rc<Counters>,
}

impl MockFrames {
    fn new(counters: Rc<Counters>) -> Self {
        Self {
            tables: Vec::new(),
            counters,
        }
    }
}

struct ReservedFrame {
    frame: FrameNumber,
    counters: Rc<Counters>,
    committed: bool,
}

impl ReservedTableFrame for ReservedFrame {
    fn number(&self) -> FrameNumber {
        self.frame
    }

    fn commit(mut self) -> FrameNumber {
        self.committed = true;
        self.frame
    }
}

impl Drop for ReservedFrame {
    fn drop(&mut self) {
        if !self.committed {
            self.counters.live.set(self.counters.live.get() - 1);
        }
    }
}

impl FrameMemory for MockFrames {
    type ReservedFrame = ReservedFrame;

    fn reserve_frame(&mut self) -> Result<Self::ReservedFrame, FrameExhausted> {
        if self.counters.deny_alloc.get() {
            return Err(FrameExhausted);
        }
        if let Some(remaining) = self.counters.alloc_budget.get() {
            if remaining == 0 {
                return Err(FrameExhausted);
            }
            self.counters.alloc_budget.set(Some(remaining - 1));
        }
        self.counters.live.set(self.counters.live.get() + 1);
        self.tables.push([Pte::invalid(); ENTRIES]);
        Ok(ReservedFrame {
            frame: FrameNumber(self.tables.len() - 1),
            counters: self.counters.clone(),
            committed: false,
        })
    }

    fn free_frame(&mut self, _frame: FrameNumber) {
        self.counters.live.set(self.counters.live.get() - 1);
    }

    fn table_mut(&mut self, frame: FrameNumber) -> &mut [Pte; ENTRIES] {
        &mut self.tables[frame.0]
    }
}

type Tree = TableTree<MockFrames, 3>;

fn tree(counters: &Rc<Counters>) -> Tree {
    Tree::new(MockFrames::new(counters.clone())).expect("failed to build tree")
}

fn map(tree: &mut Tree, vpn: Vpn, count: usize, ppn: Ppn, flags: u64) -> Result<(), MapError> {
    let prepared = tree.prepare_map(vpn, count, ppn, flags)?;
    tree.publish(prepared);
    Ok(())
}

fn unmap(tree: &mut Tree, vpn: Vpn, count: usize) -> Result<(), MapError> {
    let prepared = tree.prepare_unmap(vpn, count)?;
    tree.publish(prepared);
    Ok(())
}

fn protect(tree: &mut Tree, vpn: Vpn, count: usize, from: u64, to: u64) -> Result<(), MapError> {
    let prepared = tree.prepare_protect(vpn, count, from, to)?;
    tree.publish(prepared);
    Ok(())
}

/// mm-map-bug 数值原案：未对齐起点跨多张表的 32MB 区间（8192 页）。
#[test]
fn big_unaligned_cross_table() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let (start, count, ppn) = (65usize, 8192usize, 33usize);
    let prepared = t
        .prepare_map(Vpn(start), count, Ppn(ppn), flags::USER_DATA)
        .unwrap();
    assert_eq!(prepared.reserved_frames(), 18);
    t.publish(prepared);

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
    map(
        &mut t,
        Vpn(512 * 3),
        512,
        Ppn(512 * 7),
        flags::KERNEL_DIRECT,
    )
    .unwrap();
    let m = t.translate(Vpn(512 * 3 + 123)).unwrap();
    assert_eq!(m.level, 1, "expected 2MiB mega level");
    assert_eq!(m.ppn.0, 512 * 7 + 123);
}

/// 1GiB root mega（LEVELS=3 顶层叶）。
#[test]
fn one_g_mega() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(
        &mut t,
        Vpn(512 * 512 * 2),
        512 * 512,
        Ppn(512 * 512 * 5),
        flags::KERNEL_DIRECT,
    )
    .unwrap();
    let m = t.translate(Vpn(512 * 512 * 2 + 9999)).unwrap();
    assert_eq!(m.level, 2);
    assert_eq!(m.ppn.0, 512 * 512 * 5 + 9999);
}

/// 未对齐首尾混合。
#[test]
fn mixed_head_tail() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(5), 517, Ppn(9), flags::USER_DATA).unwrap();
    for vpn in 5..5 + 517 {
        assert_eq!(t.translate(Vpn(vpn)).unwrap().ppn.0, 9 + vpn - 5);
    }
}

/// 同 ppn 同 flags → 幂等成功。
#[test]
fn idempotent_same_mapping() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(512), 512, Ppn(512), flags::USER_DATA).unwrap();
    map(&mut t, Vpn(512), 512, Ppn(512), flags::USER_DATA).unwrap();
    // 部分重叠幂等
    map(&mut t, Vpn(512 + 100), 10, Ppn(512 + 100), flags::USER_DATA).unwrap();
}

/// 异 flags 冲突。
#[test]
fn conflict_different_flags() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(0), 16, Ppn(0), flags::USER_DATA).unwrap();
    assert_eq!(
        map(&mut t, Vpn(4), 4, Ppn(4), flags::USER_RODATA),
        Err(MapError::Conflict { vpn: Vpn(4) })
    );
}

/// 同 flags 异 ppn 冲突。
#[test]
fn conflict_different_ppn() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(0), 16, Ppn(0), flags::USER_DATA).unwrap();
    assert_eq!(
        map(&mut t, Vpn(4), 4, Ppn(100), flags::USER_DATA),
        Err(MapError::Conflict { vpn: Vpn(4) })
    );
}

/// mega 槽位下已有更细子树 → 保守冲突。
#[test]
fn conflict_mega_over_subtree() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    // 物理不对齐 → 只能建 4KiB 叶，留下分支链
    map(&mut t, Vpn(512), 512, Ppn(5), flags::USER_DATA).unwrap();
    assert_eq!(t.translate(Vpn(512)).unwrap().level, 0);
    // 再以对齐 ppn 映射同区 → mega 撞上子树
    assert_eq!(
        map(&mut t, Vpn(512), 512, Ppn(512), flags::USER_DATA),
        Err(MapError::Conflict { vpn: Vpn(512) })
    );
}

/// 全量解除后重映射。
#[test]
fn unmap_then_remap() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(100), 100, Ppn(200), flags::USER_DATA).unwrap();
    unmap(&mut t, Vpn(100), 100).unwrap();
    assert!(t.translate(Vpn(150)).is_none());
    map(&mut t, Vpn(100), 100, Ppn(900), flags::USER_CODE).unwrap();
    assert_eq!(t.translate(Vpn(150)).unwrap().ppn.0, 950);
}

/// 跨 512 页子表边界批量解除，必须使用每个子表的真实覆盖基址。
#[test]
fn unmap_crosses_child_table_boundary() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(480), 80, Ppn(1000), flags::USER_DATA).unwrap();
    unmap(&mut t, Vpn(500), 40).unwrap();
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
    map(&mut t, Vpn(base), 512, Ppn(base), flags::USER_DATA).unwrap();
    assert_eq!(t.translate(Vpn(base + 5)).unwrap().level, 1);

    unmap(&mut t, Vpn(base + 100), 1).unwrap();
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
    map(&mut t, Vpn(base), 512, Ppn(base), flags::USER_DATA).unwrap();
    counters.deny_alloc.set(true);
    assert_eq!(
        unmap(&mut t, Vpn(base + 123), 1),
        Err(MapError::FrameExhausted)
    );
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
    map(&mut t, Vpn(512), 512, Ppn(512), flags::USER_DATA).unwrap();
    // 兼容的细粒度重映射（同物理连续性）
    map(&mut t, Vpn(512 + 7), 1, Ppn(512 + 7), flags::USER_DATA).unwrap();
    // 异物理的细粒度重映射
    assert_eq!(
        map(&mut t, Vpn(512 + 8), 1, Ppn(999_999), flags::USER_DATA),
        Err(MapError::Conflict { vpn: Vpn(512 + 8) })
    );
}

/// 超出 sv39 地址宽度。
#[test]
fn out_of_range() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    assert_eq!(
        map(&mut t, Vpn(1 << 27), 1, Ppn(0), flags::USER_DATA),
        Err(MapError::OutOfRange)
    );
    // 溢出
    assert_eq!(
        map(&mut t, Vpn(usize::MAX), 2, Ppn(0), flags::USER_DATA),
        Err(MapError::OutOfRange)
    );
    assert_eq!(
        map(
            &mut t,
            Vpn(0),
            1,
            Ppn(page_table::MAX_PPN),
            flags::USER_DATA
        ),
        Err(MapError::OutOfRange)
    );
}

/// 空区间为 no-op。
#[test]
fn zero_count_noop() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(0), 0, Ppn(0), flags::USER_DATA).unwrap();
    assert!(t.translate(Vpn(0)).is_none());
}

/// Drop 释放全部表帧（多层级压力）。
#[test]
fn drop_frees_all_frames() {
    let counters = Rc::new(Counters::default());
    {
        let mut t = tree(&counters);
        map(&mut t, Vpn(65), 8192, Ppn(33), flags::USER_DATA).unwrap();
        map(
            &mut t,
            Vpn(512 * 100),
            512,
            Ppn(512 * 100),
            flags::USER_CODE,
        )
        .unwrap();
        map(&mut t, Vpn(0), 1, Ppn(1), flags::USER_RODATA).unwrap();
        assert!(counters.live.get() > 1);
    }
    assert_eq!(counters.live.get(), 0, "table frames not fully returned");
}

/// shared root 槽由树内所有权位图标记，Drop 不递归进入外部子树。
#[test]
fn shared_root_is_not_reclaimed() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(0), 1, Ppn(1), flags::USER_DATA).unwrap();
    t.attach_shared_root(500, &[Pte::branch(FrameNumber(0xdead))])
        .unwrap();
    assert_eq!(t.root_slot_state(500), RootSlotState::Shared);
    drop(t);
    assert_eq!(
        counters.live.get(),
        0,
        "shared subtree must not be reclaimed"
    );
}

#[test]
fn dropped_reservation_returns_all_frames() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let prepared = t.prepare_map(Vpn(1), 1, Ppn(2), flags::USER_DATA).unwrap();
    assert_eq!(prepared.reserved_frames(), 2);
    assert_eq!(counters.live.get(), 3);
    drop(prepared);
    assert_eq!(counters.live.get(), 1);
    assert!(t.translate(Vpn(1)).is_none());
}

#[test]
fn existing_page_unmap_reserves_no_frames() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(1), 1, Ppn(2), flags::USER_DATA).unwrap();
    counters.deny_alloc.set(true);
    let prepared = t.prepare_unmap(Vpn(1), 1).unwrap();
    assert_eq!(prepared.reserved_frames(), 0);
    t.publish(prepared);
    assert!(t.translate(Vpn(1)).is_none());
}

#[test]
fn partial_protect_splits_mega() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let base = 512 * 10;
    map(&mut t, Vpn(base), 512, Ppn(base), flags::USER_DATA).unwrap();
    let prepared = t
        .prepare_protect(Vpn(base + 9), 1, flags::USER_DATA, flags::USER_RODATA)
        .unwrap();
    assert_eq!(prepared.reserved_frames(), 1);
    t.publish(prepared);
    assert_eq!(
        t.translate(Vpn(base + 9)).unwrap().flags,
        flags::USER_RODATA
    );
    assert_eq!(t.translate(Vpn(base + 8)).unwrap().flags, flags::USER_DATA);
}

#[test]
fn protect_validation_has_zero_side_effects() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(10), 2, Ppn(20), flags::USER_DATA).unwrap();
    let live = counters.live.get();
    assert_eq!(
        protect(&mut t, Vpn(10), 2, flags::USER_RODATA, flags::USER_CODE,),
        Err(MapError::ProtectionMismatch { vpn: Vpn(10) })
    );
    assert_eq!(counters.live.get(), live);
    assert_eq!(t.translate(Vpn(10)).unwrap().flags, flags::USER_DATA);
}

#[test]
fn partial_reservation_failure_returns_frames_and_preserves_tree() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let base = 512 * 512 * 2;
    map(&mut t, Vpn(base), 512 * 512, Ppn(base), flags::USER_DATA).unwrap();
    assert_eq!(counters.live.get(), 1);
    counters.alloc_budget.set(Some(1));
    let result = t.prepare_unmap(Vpn(base + 1), 1);
    assert!(matches!(result, Err(MapError::FrameExhausted)));
    assert_eq!(counters.live.get(), 1);
    for offset in [0, 1, 2, 512, 512 * 512 - 1] {
        let mapped = t.translate(Vpn(base + offset)).unwrap();
        assert_eq!(mapped.level, 2);
        assert_eq!(mapped.ppn.0, base + offset);
    }
}

#[test]
fn invalid_leaf_flags_are_rejected_before_reservation() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    assert!(matches!(
        t.prepare_map(Vpn(0), 1, Ppn(0), flags::V),
        Err(MapError::InvalidFlags)
    ));
    map(&mut t, Vpn(0), 1, Ppn(0), flags::USER_DATA).unwrap();
    assert!(matches!(
        t.prepare_protect(Vpn(0), 1, flags::USER_DATA, flags::V),
        Err(MapError::InvalidFlags)
    ));
    assert_eq!(counters.live.get(), 3);
}

#[test]
fn detached_root_slot_can_be_reused_as_shared() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let base = 512 * 512 * 2;
    map(&mut t, Vpn(base), 512 * 512, Ppn(base), flags::USER_DATA).unwrap();
    assert_eq!(t.root_slot_state(2), RootSlotState::Leaf);
    unmap(&mut t, Vpn(base), 512 * 512).unwrap();
    assert_eq!(t.root_slot_state(2), RootSlotState::Empty);
    t.attach_shared_root(2, &[Pte::branch(FrameNumber(0xbeef))])
        .unwrap();
    assert_eq!(t.root_slot_state(2), RootSlotState::Shared);
}

#[test]
fn later_nonoverlapping_publish_returns_now_unused_frames() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let first = t
        .prepare_map(Vpn(1), 1, Ppn(101), flags::USER_DATA)
        .unwrap();
    let second = t
        .prepare_map(Vpn(2), 1, Ppn(102), flags::USER_DATA)
        .unwrap();
    assert_eq!(first.reserved_frames(), 2);
    assert_eq!(second.reserved_frames(), 2);
    assert_eq!(counters.live.get(), 5);

    t.publish(first);
    assert_eq!(counters.live.get(), 5);
    t.publish(second);
    assert_eq!(counters.live.get(), 3);
    assert_eq!(t.translate(Vpn(1)).unwrap().ppn, Ppn(101));
    assert_eq!(t.translate(Vpn(2)).unwrap().ppn, Ppn(102));
}
