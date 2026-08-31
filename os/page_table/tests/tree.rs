//! TableTree 切段算法与生命周期测试（host）。
//!
//! 用例清单见 notes/impls/mm.md「测试集」；`big_unaligned_cross_table`
//! 是 mm-map-bug 的数值原案。

use core::{
    cell::Cell,
    ops::{Deref, DerefMut},
};
use page_table::{
    DrainStep, ENTRIES, FrameExhausted, FrameNumber, MapError, Ppn, PreparedTranslation, Pte,
    PublishOutcome, TableFrameMemory, TableFrameOwner, TableTree, Vpn, flags,
};
use std::rc::Rc;

const TABLE_CAPACITY: usize = 128;

/// 共享计数器：owner 析构后仍可读，验证帧收支平衡。
#[derive(Default)]
struct Counters {
    live: Cell<usize>,
    next: Cell<usize>,
    deny_alloc: Cell<bool>,
    alloc_budget: Cell<Option<usize>>,
}

struct MockFrames {
    tables: Vec<[Pte; ENTRIES]>,
}

impl MockFrames {
    fn new() -> Self {
        Self {
            tables: vec![[Pte::invalid(); ENTRIES]; TABLE_CAPACITY],
        }
    }
}

struct TableOwner {
    frame: FrameNumber,
    counters: Rc<Counters>,
}

impl TableFrameOwner for TableOwner {
    fn number(&self) -> FrameNumber {
        self.frame
    }
}

impl Drop for TableOwner {
    fn drop(&mut self) {
        self.counters.live.set(self.counters.live.get() - 1);
    }
}

fn allocate_owner(counters: &Rc<Counters>) -> Result<TableOwner, FrameExhausted> {
    if counters.deny_alloc.get() {
        return Err(FrameExhausted);
    }
    if let Some(remaining) = counters.alloc_budget.get() {
        if remaining == 0 {
            return Err(FrameExhausted);
        }
        counters.alloc_budget.set(Some(remaining - 1));
    }
    let frame = counters.next.get();
    if frame == TABLE_CAPACITY {
        return Err(FrameExhausted);
    }
    counters.next.set(frame + 1);
    counters.live.set(counters.live.get() + 1);
    Ok(TableOwner {
        frame: FrameNumber(frame),
        counters: counters.clone(),
    })
}

fn supply(counters: &Rc<Counters>, count: usize) -> Result<Vec<TableOwner>, FrameExhausted> {
    let mut owners = Vec::new();
    owners
        .try_reserve_exact(count)
        .map_err(|_| FrameExhausted)?;
    for _ in 0..count {
        owners.push(allocate_owner(counters)?);
    }
    Ok(owners)
}

impl TableFrameMemory for MockFrames {
    type FrameOwner = TableOwner;

    fn table_mut(&mut self, frame: FrameNumber) -> &mut [Pte; ENTRIES] {
        &mut self.tables[frame.0]
    }
}

type RawTree = TableTree<MockFrames, 3>;

struct Tree {
    inner: Option<RawTree>,
    counters: Rc<Counters>,
}

impl Tree {
    fn prepare_map(
        &mut self,
        vpn: Vpn,
        count: usize,
        ppn: Ppn,
        flags: u64,
    ) -> Result<PreparedTranslation<TableOwner>, MapError> {
        let preflight = self
            .inner
            .as_mut()
            .unwrap()
            .preflight_map(vpn, count, ppn, flags)?;
        let owners = supply(&self.counters, preflight.required_frames())
            .map_err(|_| MapError::FrameExhausted)?;
        self.inner
            .as_mut()
            .unwrap()
            .prepare(preflight, owners)
            .map_err(|failure| failure.error)
    }

    fn prepare_unmap(
        &mut self,
        vpn: Vpn,
        count: usize,
    ) -> Result<PreparedTranslation<TableOwner>, MapError> {
        let preflight = self.inner.as_mut().unwrap().preflight_unmap(vpn, count)?;
        let owners = supply(&self.counters, preflight.required_frames())
            .map_err(|_| MapError::FrameExhausted)?;
        self.inner
            .as_mut()
            .unwrap()
            .prepare(preflight, owners)
            .map_err(|failure| failure.error)
    }

    fn prepare_protect(
        &mut self,
        vpn: Vpn,
        count: usize,
        from: u64,
        to: u64,
    ) -> Result<PreparedTranslation<TableOwner>, MapError> {
        let preflight = self
            .inner
            .as_mut()
            .unwrap()
            .preflight_protect(vpn, count, from, to)?;
        let owners = supply(&self.counters, preflight.required_frames())
            .map_err(|_| MapError::FrameExhausted)?;
        self.inner
            .as_mut()
            .unwrap()
            .prepare(preflight, owners)
            .map_err(|failure| failure.error)
    }

    fn publish(&mut self, prepared: PreparedTranslation<TableOwner>) -> PublishOutcome<TableOwner> {
        self.inner.as_mut().unwrap().publish(prepared)
    }

    fn publish_batch(
        &mut self,
        prepared: Vec<PreparedTranslation<TableOwner>>,
    ) -> Vec<PublishOutcome<TableOwner>> {
        let mut outcomes = Vec::new();
        outcomes.reserve_exact(prepared.len());
        self.inner
            .as_mut()
            .unwrap()
            .publish_batch(prepared, outcomes)
    }
}

impl Deref for Tree {
    type Target = RawTree;

    fn deref(&self) -> &Self::Target {
        self.inner.as_ref().unwrap()
    }
}

impl DerefMut for Tree {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner.as_mut().unwrap()
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let mut tree = self.inner.take().unwrap();
        let mut cursor = tree.begin_drain();
        loop {
            match tree.drain_step(&mut cursor) {
                DrainStep::Progress => {}
                DrainStep::Retired(owner) => drop(owner),
                DrainStep::Complete => break,
            }
        }
        drop(tree.finish_drain());
    }
}

fn tree(counters: &Rc<Counters>) -> Tree {
    let root = allocate_owner(counters).expect("failed to reserve root");
    Tree {
        inner: Some(RawTree::new(MockFrames::new(), root)),
        counters: counters.clone(),
    }
}

fn map(tree: &mut Tree, vpn: Vpn, count: usize, ppn: Ppn, flags: u64) -> Result<(), MapError> {
    let prepared = tree.prepare_map(vpn, count, ppn, flags)?;
    drop(tree.publish(prepared));
    Ok(())
}

fn unmap(tree: &mut Tree, vpn: Vpn, count: usize) -> Result<(), MapError> {
    let prepared = tree.prepare_unmap(vpn, count)?;
    drop(tree.publish(prepared));
    Ok(())
}

fn protect(tree: &mut Tree, vpn: Vpn, count: usize, from: u64, to: u64) -> Result<(), MapError> {
    let prepared = tree.prepare_protect(vpn, count, from, to)?;
    drop(tree.publish(prepared));
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
    assert_eq!(prepared.supplied_frames(), 18);
    drop(t.publish(prepared));

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

#[test]
fn undrained_tree_drop_is_rejected() {
    let counters = Rc::new(Counters::default());
    let root = allocate_owner(&counters).unwrap();
    let mut tree = RawTree::new(MockFrames::new(), root);
    let preflight = tree
        .preflight_map(Vpn(1), 1, Ppn(2), flags::USER_DATA)
        .unwrap();
    let prepared = tree
        .prepare(
            preflight,
            supply(&counters, preflight.required_frames()).unwrap(),
        )
        .unwrap_or_else(|failure| panic!("prepare failed: {:?}", failure.error));
    drop(tree.publish(prepared));
    let rejected = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| drop(tree)));
    assert!(rejected.is_err());
    assert_eq!(counters.live.get(), 0);
}

/// shared root 槽由树内所有权位图标记，Drop 不递归进入外部子树。
#[test]
fn shared_root_is_not_reclaimed() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(0), 1, Ppn(1), flags::USER_DATA).unwrap();
    t.attach_shared_root(500, &[Pte::branch(FrameNumber(0xdead))])
        .unwrap();
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
    assert_eq!(prepared.supplied_frames(), 2);
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
    assert_eq!(prepared.supplied_frames(), 0);
    let outcome = t.publish(prepared);
    assert!(outcome.unused.is_empty());
    assert_eq!(outcome.retired.len(), 2);
    assert_eq!(
        counters.live.get(),
        3,
        "retired owners released before confirmation"
    );
    drop(outcome);
    assert_eq!(counters.live.get(), 1);
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
    assert_eq!(prepared.supplied_frames(), 1);
    drop(t.publish(prepared));
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
    assert_eq!(t.translate(Vpn(base)).unwrap().level, 2);
    unmap(&mut t, Vpn(base), 512 * 512).unwrap();
    assert!(t.translate(Vpn(base)).is_none());
    t.attach_shared_root(2, &[Pte::branch(FrameNumber(0xbeef))])
        .unwrap();
}

#[test]
fn later_nonoverlapping_publish_returns_now_unused_frames() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let first_preflight = t
        .preflight_map(Vpn(1), 1, Ppn(101), flags::USER_DATA)
        .unwrap();
    let second_preflight = t
        .preflight_map(Vpn(2), 1, Ppn(102), flags::USER_DATA)
        .unwrap();
    let first_owners = supply(&counters, first_preflight.required_frames()).unwrap();
    let second_owners = supply(&counters, second_preflight.required_frames()).unwrap();
    assert_eq!(first_owners.len(), 2);
    assert_eq!(second_owners.len(), 2);
    assert_eq!(counters.live.get(), 5);

    let first = t
        .prepare(first_preflight, first_owners)
        .unwrap_or_else(|failure| panic!("first prepare failed: {:?}", failure.error));
    let second = t
        .prepare(second_preflight, second_owners)
        .unwrap_or_else(|failure| panic!("second prepare failed: {:?}", failure.error));
    let mut outcomes = t.publish_batch(vec![first, second]);
    let second_outcome = outcomes.pop().unwrap();
    let first_outcome = outcomes.pop().unwrap();
    assert!(outcomes.is_empty());
    assert!(first_outcome.unused.is_empty());
    assert!(first_outcome.retired.is_empty());
    drop(first_outcome);
    assert_eq!(counters.live.get(), 5);
    assert_eq!(second_outcome.unused.len(), 2);
    assert!(second_outcome.retired.is_empty());
    assert_eq!(counters.live.get(), 5);
    drop(second_outcome);
    assert_eq!(counters.live.get(), 3);
    assert_eq!(t.translate(Vpn(1)).unwrap().ppn, Ppn(101));
    assert_eq!(t.translate(Vpn(2)).unwrap().ppn, Ppn(102));
}

#[test]
fn stale_preflight_is_rechecked_before_prepare() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let stale = t
        .preflight_map(Vpn(2), 1, Ppn(102), flags::USER_DATA)
        .unwrap();
    assert_eq!(stale.required_frames(), 2);

    let first = t
        .prepare_map(Vpn(1), 1, Ppn(101), flags::USER_DATA)
        .unwrap();
    drop(t.publish(first));

    let owners = supply(&t.counters, stale.required_frames()).unwrap();
    let prepared = t
        .prepare(stale, owners)
        .unwrap_or_else(|failure| panic!("stale preflight prepare failed: {:?}", failure.error));
    assert_eq!(prepared.supplied_frames(), 2);
    let outcome = t.publish(prepared);
    assert_eq!(outcome.unused.len(), 2);
    assert!(outcome.retired.is_empty());
    drop(outcome);

    assert_eq!(t.counters.live.get(), 3);
    assert_eq!(t.translate(Vpn(1)).unwrap().ppn, Ppn(101));
    assert_eq!(t.translate(Vpn(2)).unwrap().ppn, Ppn(102));
}

#[test]
fn map_and_protect_reject_shared_root_slots() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let root_slot_pages = ENTRIES * ENTRIES;
    let vpn = Vpn(500 * root_slot_pages);
    t.attach_shared_root(500, &[Pte::branch(FrameNumber(0xdead))])
        .unwrap();

    assert!(matches!(
        t.preflight_map(vpn, 1, Ppn(1), flags::USER_DATA),
        Err(MapError::Conflict { vpn: conflict }) if conflict == vpn
    ));
    assert!(matches!(
        t.preflight_protect(vpn, 1, flags::USER_DATA, flags::USER_CODE),
        Err(MapError::Conflict { vpn: conflict }) if conflict == vpn
    ));
}

#[test]
fn map_after_pruning_unmap_is_detected_as_stale() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(0), 1, Ppn(10), flags::USER_DATA).unwrap();

    let map_next = t.prepare_map(Vpn(1), 1, Ppn(11), flags::USER_DATA).unwrap();
    let unmap_first = t.prepare_unmap(Vpn(0), 1).unwrap();
    let retired = t.publish(unmap_first);
    assert_eq!(retired.retired.len(), 2);
    assert!(!t.prepared_is_current(&map_next));
    drop(map_next);
    drop(retired);
}

#[test]
fn second_unmap_after_first_publish_is_detected_as_stale() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(0), 2, Ppn(10), flags::USER_DATA).unwrap();

    let first = t.prepare_unmap(Vpn(0), 1).unwrap();
    let second = t.prepare_unmap(Vpn(1), 1).unwrap();
    let first_outcome = t.publish(first);
    assert!(first_outcome.retired.is_empty());
    assert!(!t.prepared_is_current(&second));
    drop(second);
    drop(first_outcome);
}
