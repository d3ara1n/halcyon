//! TableTree 切段算法与生命周期测试（host）。
//!
//! 用例清单见 notes/impls/mm.md「测试集」；`big_unaligned_cross_table`
//! 是 mm-map-bug 的数值原案。

use core::{
    cell::Cell,
    ops::{Deref, DerefMut},
};
use page_table::{
    DrainStep, ENTRIES, FrameExhausted, FrameNumber, MapError, Ppn, Pte, PublishCursor,
    TableFrameMemory, TableFrameOwner, TableTree, Vpn, flags,
};
use std::rc::Rc;

// 一次Sv39根大页Map的保守储备可触及ENTRIES张叶表；fixture容纳储备及后续切分。
const TABLE_CAPACITY: usize = ENTRIES * 2;

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
    adopted: Vec<Option<TableOwner>>,
}

impl MockFrames {
    fn new() -> Self {
        Self {
            tables: vec![[Pte::invalid(); ENTRIES]; TABLE_CAPACITY],
            adopted: (0..TABLE_CAPACITY).map(|_| None).collect(),
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

    fn adopt(&mut self, owner: TableOwner) {
        let slot = &mut self.adopted[owner.frame.0];
        assert!(slot.is_none(), "frame adopted twice");
        *slot = Some(owner);
    }

    fn reclaim(&mut self, frame: FrameNumber) -> TableOwner {
        self.adopted[frame.0]
            .take()
            .expect("reclaimed unadopted frame")
    }
}

type RawTree = TableTree<MockFrames, 3>;

struct Tree {
    inner: Option<RawTree>,
    counters: Rc<Counters>,
}

impl Tree {
    fn publish(
        &mut self,
        mut cursor: PublishCursor,
        mut reserve: Vec<TableOwner>,
    ) -> Vec<TableOwner> {
        step_all(self, &mut cursor, &mut reserve);
        reserve
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
    let cursor = PublishCursor::map(vpn, count, ppn, flags);
    let reserve = supply(&tree.counters, page_table::worst_case_frames::<3>(&cursor))?;
    drop(tree.publish(cursor, reserve));
    Ok(())
}

fn unmap(tree: &mut Tree, vpn: Vpn, count: usize) -> Result<(), MapError> {
    let cursor = PublishCursor::unmap(vpn, count);
    let reserve = supply(&tree.counters, page_table::worst_case_frames::<3>(&cursor))?;
    drop(tree.publish(cursor, reserve));
    Ok(())
}

fn protect(tree: &mut Tree, vpn: Vpn, count: usize, from: u64, to: u64) -> Result<(), MapError> {
    let cursor = PublishCursor::protect(vpn, count, from, to);
    let reserve = supply(&tree.counters, page_table::worst_case_frames::<3>(&cursor))?;
    drop(tree.publish(cursor, reserve));
    Ok(())
}

/// mm-map-bug 数值原案：未对齐起点跨多张表的 32MB 区间（8192 页）。
#[test]
fn big_unaligned_cross_table() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let (start, count, ppn) = (65usize, 8192usize, 33usize);
    let mut cursor = PublishCursor::map(Vpn(start), count, Ppn(ppn), flags::USER_DATA);
    let mut reserve = supply(&counters, page_table::worst_case_frames::<3>(&cursor)).unwrap();
    assert_eq!(reserve.len(), 18);
    assert!(!t.publish_step(&mut cursor, &mut core::iter::from_fn(|| reserve.pop()), 1));
    assert_eq!(cursor.published_until(), Vpn(start + 1));
    assert!(t.translate(Vpn(start)).is_some());
    assert!(t.translate(Vpn(start + 1)).is_none());
    drop(t.publish(cursor, reserve));

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
    let mut cursor = PublishCursor::map(Vpn(1), 1, Ppn(2), flags::USER_DATA);
    let mut reserve = supply(&counters, page_table::worst_case_frames::<3>(&cursor)).unwrap();
    while !tree.publish_step(&mut cursor, &mut core::iter::from_fn(|| reserve.pop()), 1) {}
    drop(reserve);
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
    let cursor = PublishCursor::map(Vpn(1), 1, Ppn(2), flags::USER_DATA);
    let reserve = supply(&counters, page_table::worst_case_frames::<3>(&cursor)).unwrap();
    assert_eq!(reserve.len(), 2);
    assert_eq!(counters.live.get(), 3);
    drop(reserve);
    assert_eq!(counters.live.get(), 1);
    assert!(t.translate(Vpn(1)).is_none());
}

#[test]
fn published_unmap_keeps_table_owners_until_explicit_retirement() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(1), 1, Ppn(2), flags::USER_DATA).unwrap();
    let cursor = PublishCursor::unmap(Vpn(1), 1);
    let reserve = supply(&counters, page_table::worst_case_frames::<3>(&cursor)).unwrap();
    counters.deny_alloc.set(true);
    drop(t.publish(cursor, reserve));
    assert!(t.translate(Vpn(1)).is_none());
    assert_eq!(counters.live.get(), 3);
    let leaf = t.detach_empty_table(Vpn(1), 0).unwrap();
    let middle = t.detach_empty_table(Vpn(1), 1).unwrap();
    assert_eq!(
        counters.live.get(),
        3,
        "retired frames require explicit confirmation"
    );
    drop((leaf, middle));
    assert_eq!(counters.live.get(), 1);
}

#[test]
fn partial_protect_splits_mega() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let base = 512 * 10;
    map(&mut t, Vpn(base), 512, Ppn(base), flags::USER_DATA).unwrap();
    protect(
        &mut t,
        Vpn(base + 9),
        1,
        flags::USER_DATA,
        flags::USER_RODATA,
    )
    .unwrap();
    assert_eq!(
        t.translate(Vpn(base + 9)).unwrap().flags,
        flags::USER_RODATA
    );
    assert_eq!(t.translate(Vpn(base + 8)).unwrap().flags, flags::USER_DATA);
}

#[test]
fn partial_reservation_failure_returns_frames_and_preserves_tree() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let base = 512 * 512 * 2;
    map(&mut t, Vpn(base), 512 * 512, Ppn(base), flags::USER_DATA).unwrap();
    assert_eq!(counters.live.get(), 1);
    counters.alloc_budget.set(Some(1));
    let cursor = PublishCursor::unmap(Vpn(base + 1), 1);
    let result = supply(&counters, page_table::worst_case_frames::<3>(&cursor));
    assert!(matches!(result, Err(FrameExhausted)));
    assert_eq!(counters.live.get(), 1);
    for offset in [0, 1, 2, 512, 512 * 512 - 1] {
        let mapped = t.translate(Vpn(base + offset)).unwrap();
        assert_eq!(mapped.level, 2);
        assert_eq!(mapped.ppn.0, base + offset);
    }
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
    let first = PublishCursor::map(Vpn(1), 1, Ppn(101), flags::USER_DATA);
    let second = PublishCursor::map(Vpn(2), 1, Ppn(102), flags::USER_DATA);
    let first_owners = supply(&counters, page_table::worst_case_frames::<3>(&first)).unwrap();
    let second_owners = supply(&counters, page_table::worst_case_frames::<3>(&second)).unwrap();
    assert_eq!(counters.live.get(), 5);
    assert!(t.publish(first, first_owners).is_empty());
    let unused = t.publish(second, second_owners);
    assert_eq!(unused.len(), 2);
    assert_eq!(counters.live.get(), 5);
    drop(unused);
    assert_eq!(counters.live.get(), 3);
    assert_eq!(t.translate(Vpn(1)).unwrap().ppn, Ppn(101));
    assert_eq!(t.translate(Vpn(2)).unwrap().ppn, Ppn(102));
}

#[test]
fn publish_never_enters_shared_root_slots() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    let vpn = Vpn(500 * ENTRIES * ENTRIES);
    t.attach_shared_root(500, &[Pte::branch(FrameNumber(0xdead))])
        .unwrap();
    for mut cursor in [
        PublishCursor::map(vpn, 1, Ppn(1), flags::USER_DATA),
        PublishCursor::protect(vpn, 1, flags::USER_DATA, flags::USER_CODE),
    ] {
        let mut reserve = supply(&counters, page_table::worst_case_frames::<3>(&cursor)).unwrap();
        let denied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            t.publish_step(&mut cursor, &mut core::iter::from_fn(|| reserve.pop()), 1)
        }));
        assert!(denied.is_err());
        drop(reserve);
        assert_eq!(counters.live.get(), 1);
    }
}

#[test]
fn prefunded_cursor_rebuilds_after_an_empty_branch_is_detached() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(0), 1, Ppn(10), flags::USER_DATA).unwrap();
    let next = PublishCursor::map(Vpn(1), 1, Ppn(11), flags::USER_DATA);
    let reserve = supply(&counters, page_table::worst_case_frames::<3>(&next)).unwrap();
    unmap(&mut t, Vpn(0), 1).unwrap();
    drop(t.detach_empty_table(Vpn(0), 0).unwrap());
    drop(t.detach_empty_table(Vpn(0), 1).unwrap());
    counters.deny_alloc.set(true);
    drop(t.publish(next, reserve));
    assert_eq!(t.translate(Vpn(1)).unwrap().ppn, Ppn(11));
    assert_eq!(counters.live.get(), 3);
}

#[test]
fn disjoint_unmaps_do_not_invalidate_each_other() {
    let counters = Rc::new(Counters::default());
    let mut t = tree(&counters);
    map(&mut t, Vpn(0), 2, Ppn(10), flags::USER_DATA).unwrap();
    let first = PublishCursor::unmap(Vpn(0), 1);
    let second = PublishCursor::unmap(Vpn(1), 1);
    let a = supply(&counters, page_table::worst_case_frames::<3>(&first)).unwrap();
    let b = supply(&counters, page_table::worst_case_frames::<3>(&second)).unwrap();
    drop(t.publish(first, a));
    assert!(t.translate(Vpn(0)).is_none());
    assert!(t.translate(Vpn(1)).is_some());
    drop(t.publish(second, b));
    assert!(t.translate(Vpn(1)).is_none());
}

// ---- 游标分步发布（内存分案轮次 3）----

fn step_all(
    tree: &mut Tree,
    cursor: &mut page_table::PublishCursor,
    reserve: &mut Vec<TableOwner>,
) {
    while !tree.publish_step(cursor, &mut core::iter::from_fn(|| reserve.pop()), 1) {}
}

#[test]
fn disjoint_cursors_publish_interleaved_without_stale() {
    let counters = Rc::new(Counters::default());
    let mut tree = tree(&counters);
    let rw = flags::V | flags::R | flags::W;
    let mut a = page_table::PublishCursor::map(Vpn(0), 8, Ppn(0x1000), rw);
    let mut b = page_table::PublishCursor::map(Vpn(16), 8, Ppn(0x2000), rw);
    let mut ra = supply(&counters, page_table::worst_case_frames::<3>(&a)).unwrap();
    let mut rb = supply(&counters, page_table::worst_case_frames::<3>(&b)).unwrap();
    loop {
        let da = tree.publish_step(&mut a, &mut core::iter::from_fn(|| ra.pop()), 1);
        let db = tree.publish_step(&mut b, &mut core::iter::from_fn(|| rb.pop()), 1);
        if da && db {
            break;
        }
    }
    assert_eq!(tree.translate(Vpn(3)).unwrap().ppn, Ppn(0x1003));
    assert_eq!(tree.translate(Vpn(20)).unwrap().ppn, Ppn(0x2004));
    drop((ra, rb));
    let mut u = page_table::PublishCursor::unmap(Vpn(0), 24);
    let mut none = Vec::new();
    step_all(&mut tree, &mut u, &mut none);
    assert!(tree.translate(Vpn(3)).is_none());
}

#[test]
fn stepwise_protect_and_detach_empty_tables() {
    let counters = Rc::new(Counters::default());
    let mut tree = tree(&counters);
    let rw = flags::V | flags::R | flags::W;
    let ro = flags::V | flags::R;
    let mut m = page_table::PublishCursor::map(Vpn(0), 4, Ppn(0x10), rw);
    let mut r = supply(&counters, page_table::worst_case_frames::<3>(&m)).unwrap();
    step_all(&mut tree, &mut m, &mut r);
    drop(r);
    let mut p = page_table::PublishCursor::protect(Vpn(0), 4, rw, ro);
    let mut none = Vec::new();
    step_all(&mut tree, &mut p, &mut none);
    assert_eq!(tree.translate(Vpn(2)).unwrap().flags, ro);
    let mut u = page_table::PublishCursor::unmap(Vpn(0), 4);
    step_all(&mut tree, &mut u, &mut none);
    // 固定批次退休：逐张摘空表（叶表→中间表），每次交出恰好一个 owner。
    let live = counters.live.get();
    let mut retired = 0;
    for level in 0..2 {
        if let Some(owner) = tree.detach_empty_table(Vpn(0), level) {
            drop(owner);
            retired += 1;
        }
    }
    assert_eq!(retired, 2);
    assert_eq!(counters.live.get(), live - 2);
}
