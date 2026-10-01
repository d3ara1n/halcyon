//! 内核地址空间：组合 `memory_space` 账本与 `page_table` 树（拆自 task/proc.rs）。
//!
//! 设计见 plans/todo-2026-10-01-memory-redesign.md 设计轮次 3：
//! - 事务：validate → split_at_boundaries → reserve → commit（只移交责任）→
//!   分步 [`ChangeProgress::step`]（ledger `commit_step` 与 PTE `publish_step`
//!   成对推进）→ 最后一次 PTE store 后取 epoch → synchronize → complete。
//! - 空表：只由“最后离开者”在空间锁内经 `detach_empty_table` 摘除，摘出的
//!   owner 进入固定容量退休批次，fence 覆盖后才放帧退款。

use page_table::{PublishCursor, TableTree};

use super::table::TableMem;
use crate::frame::FundedTableFrame;

/// Sv39 页表级数。
pub(crate) const LEVELS: usize = 3;

/// 一个连续预约片段的边界表上限：两端点每个非 root 级至多各 1 张。
pub(crate) const BOUNDARY_TABLES: usize = 2 * (LEVELS - 1);

/// 固定退休批次容量 K。依据：一批须容纳单个连续片段的全部边界表
/// （[`BOUNDARY_TABLES`]），使最后离开者的摘表在一批内完成、无需跨批承载；
/// 再大只增加每次 fence 前的保活帧数与单步成本，不改善正确性。
pub(crate) const RETIRE_BATCH: usize = BOUNDARY_TABLES;

/// 每个维护 step 的单元预算（ledger 节点操作与 PTE 槽单元分别计）。
/// 依据：使一步的最坏工作与 timer 批 qT 同量级；定标归施工门（qM）。
pub(crate) const STEP_UNITS: usize = 16;

/// 固定容量退休批次：摘出的表帧在同步完成前保活，之后锁外放帧退款，
/// 槽位复用，批间不做可失败准入。
pub(crate) struct RetireBatch {
    frames: [Option<FundedTableFrame>; RETIRE_BATCH],
    len: usize,
}

impl RetireBatch {
    pub(crate) const fn new() -> Self {
        Self {
            frames: [const { None }; RETIRE_BATCH],
            len: 0,
        }
    }

    pub(crate) const fn is_full(&self) -> bool {
        self.len == RETIRE_BATCH
    }

    pub(crate) const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 放入一张已摘出的表帧；批满为结构性错误（调用者先检查 `is_full`）。
    pub(crate) fn push(&mut self, frame: FundedTableFrame) {
        assert!(!self.is_full(), "retire batch overflow");
        self.frames[self.len] = Some(frame);
        self.len += 1;
    }

    /// 同步完成后逐张取出、锁外释放；返回 None 时批已清空可复用。
    pub(crate) fn pop(&mut self) -> Option<FundedTableFrame> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        self.frames[self.len].take()
    }
}

/// 一次已提交修改的分步进度：ledger 与 PTE 各一游标，表页储备随事务。
pub(crate) struct ChangeProgress {
    committed: Option<memory_space::CommittedChange>,
    footprint: memory_space::PageRange,
    ledger_done: bool,
    cursors: alloc::vec::Vec<PublishCursor>,
    reserve: alloc::vec::Vec<FundedTableFrame>,
}

impl ChangeProgress {
    pub(crate) fn new(
        committed: memory_space::CommittedChange,
        footprint: memory_space::PageRange,
        cursors: alloc::vec::Vec<PublishCursor>,
        reserve: alloc::vec::Vec<FundedTableFrame>,
    ) -> Self {
        Self {
            committed: Some(committed),
            footprint,
            ledger_done: false,
            cursors,
            reserve,
        }
    }

    /// 本事务的预约片段（最后离开者摘边界表用）。
    pub(crate) const fn footprint(&self) -> memory_space::PageRange {
        self.footprint
    }

    /// 在空间锁内推进一步：先 ledger 至多 [`STEP_UNITS`] 个节点，再 PTE 至多
    /// [`STEP_UNITS`] 个槽。返回是否全部发布（随后取 epoch、发 fence）。
    pub(crate) fn step(
        &mut self,
        ledger: &mut memory_space::MemorySpace,
        tree: &mut TableTree<TableMem, LEVELS>,
    ) -> bool {
        if !self.ledger_done {
            let committed = self.committed.as_mut().expect("change progress lost commit");
            self.ledger_done = ledger.commit_step(committed, STEP_UNITS);
        }
        if let Some(cursor) = self.cursors.last_mut()
            && tree.publish_step(cursor, &mut self.reserve, STEP_UNITS)
        {
            self.cursors.pop();
        }
        self.ledger_done && self.cursors.is_empty()
    }

    /// 发布完成后交出已提交责任（供 synchronize）与未用表页储备（锁外退款）。
    pub(crate) fn finish(
        mut self,
    ) -> (memory_space::CommittedChange, alloc::vec::Vec<FundedTableFrame>) {
        assert!(self.ledger_done && self.cursors.is_empty(), "change not fully published");
        let committed = self.committed.take().expect("change progress lost commit");
        (committed, core::mem::take(&mut self.reserve))
    }
}

/// 准备阶段失败：账本已回滚，归还的写许可与已取得的表页储备交回调用者锁外释放。
pub(crate) struct PrepareFailure {
    pub(crate) error: memory_space::ChangeError,
    pub(crate) permits: alloc::vec::Vec<memory_space::WritePermit>,
    pub(crate) reserve: alloc::vec::Vec<FundedTableFrame>,
}

/// 整空间退休的一个单元结果。
pub(crate) enum DrainUnit {
    Progress,
    Fragment(memory_space::RetiringFragment, Option<memory_space::WritePermit>),
    Table(FundedTableFrame),
    Complete,
}

/// 内核地址空间：账本、页表树与固定空间账户（表页同源）。
pub(crate) struct AnySpace {
    ledger: memory_space::MemorySpace,
    tree: TableTree<TableMem, LEVELS>,
    account: alloc::sync::Arc<crate::task::memory_pool::MemoryPool>,
}

impl AnySpace {
    pub(crate) fn new(
        ledger: memory_space::MemorySpace,
        root: FundedTableFrame,
        account: alloc::sync::Arc<crate::task::memory_pool::MemoryPool>,
    ) -> Self {
        Self {
            ledger,
            tree: TableTree::new(TableMem::new(), root),
            account,
        }
    }

    pub(crate) fn account(&self) -> &alloc::sync::Arc<crate::task::memory_pool::MemoryPool> {
        &self.account
    }

    pub(crate) fn ledger(&self) -> &memory_space::MemorySpace {
        &self.ledger
    }

    pub(crate) fn ledger_mut(&mut self) -> &mut memory_space::MemorySpace {
        &mut self.ledger
    }

    /// 整空间退休：建立页表 drain 游标（不改树）。
    pub(crate) fn begin_drain(&self) -> page_table::DrainCursor<LEVELS> {
        self.tree.begin_drain()
    }

    /// 整空间退休一个固定单元：账本先逐 region 摘除，再页表逐槽清除/交出表帧。
    pub(crate) fn drain_step(
        &mut self,
        cursor: &mut page_table::DrainCursor<LEVELS>,
    ) -> DrainUnit {
        if let Some((fragment, permit)) = self.ledger.drain_one() {
            return DrainUnit::Fragment(fragment, permit);
        }
        match self.tree.drain_step(cursor) {
            page_table::DrainStep::Progress => DrainUnit::Progress,
            page_table::DrainStep::Retired(frame) => DrainUnit::Table(frame),
            page_table::DrainStep::Complete => DrainUnit::Complete,
        }
    }

    /// drain 完成后交出 root 表帧与账户（锁外放帧退款）。
    pub(crate) fn finish_drain(
        self,
    ) -> (FundedTableFrame, alloc::sync::Arc<crate::task::memory_pool::MemoryPool>) {
        (self.tree.finish_drain(), self.account)
    }

    /// 由账本 intent 推导 PTE 游标与几何最坏表页数。`resolve` 把 Install 的
    /// backing 起点解析为物理页号（backing 层所有，调用者已持其区间使用权）。
    pub(crate) fn plan_cursors(
        intents: &[memory_space::TranslationIntent],
        mut resolve: impl FnMut(&memory_space::BackingView) -> page_table::Ppn,
    ) -> (alloc::vec::Vec<PublishCursor>, usize) {
        use memory_space::TranslationIntent as T;
        use page_table::Vpn;
        let vpn = |r: memory_space::PageRange| Vpn(r.start() / memory_space::PAGE_SIZE);
        let mut cursors = alloc::vec::Vec::new();
        let mut frames = 0;
        for intent in intents {
            let cursor = match *intent {
                T::Install { range, ref backing, protection } => PublishCursor::map(
                    vpn(range),
                    range.pages(),
                    resolve(backing),
                    protection_flags(protection),
                ),
                T::Remove { range } => PublishCursor::unmap(vpn(range), range.pages()),
                T::Protect { range, from, to } => PublishCursor::protect(
                    vpn(range),
                    range.pages(),
                    protection_flags(from),
                    protection_flags(to),
                ),
            };
            frames += page_table::worst_case_frames::<LEVELS>(&cursor);
            cursors.push(cursor);
        }
        (cursors, frames)
    }

    /// 锁外预取 `frames` 张同源表页（可失败，发生在预约可见之前）。
    pub(crate) fn fund_reserve(
        account: &alloc::sync::Arc<crate::task::memory_pool::MemoryPool>,
        frames: usize,
    ) -> Result<alloc::vec::Vec<FundedTableFrame>, memory_space::ChangeError> {
        let mut reserve = alloc::vec::Vec::new();
        reserve
            .try_reserve_exact(frames)
            .map_err(|_| memory_space::ChangeError::AllocationFailed)?;
        for _ in 0..frames {
            let frame = crate::frame::fund_user_table_frame(account)
                .map_err(|_| memory_space::ChangeError::AllocationFailed)?;
            reserve.push(frame);
        }
        Ok(reserve)
    }

    /// 空间锁内：端点物化 → 预约 → 不可逆提交（只移交责任）。表页储备须已
    /// 同源；失败时账本回滚，储备与许可原样交回。
    pub(crate) fn prepare_and_commit(
        &mut self,
        validated: memory_space::ValidatedChange,
        footprint: memory_space::PageRange,
        permits: alloc::vec::Vec<memory_space::WritePermit>,
        cursors: alloc::vec::Vec<PublishCursor>,
        reserve: alloc::vec::Vec<FundedTableFrame>,
    ) -> Result<ChangeProgress, PrepareFailure> {
        if reserve
            .iter()
            .any(|frame| !self.tree_mem_source_ok(frame))
        {
            return Err(PrepareFailure {
                error: memory_space::ChangeError::PermitMismatch,
                permits,
                reserve,
            });
        }
        if let Err(error) = self.ledger.split_at_boundaries(footprint) {
            return Err(PrepareFailure { error, permits, reserve });
        }
        let prepared = match self.ledger.reserve(validated, permits) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let (error, _validated, permits) = failure.into_parts();
                return Err(PrepareFailure { error, permits, reserve });
            }
        };
        let committed = self.ledger.commit(prepared);
        Ok(ChangeProgress::new(committed, footprint, cursors, reserve))
    }

    fn tree_mem_source_ok(&self, frame: &FundedTableFrame) -> bool {
        frame.charge_pool_is(&self.account)
    }

    /// 空间锁内推进一步已提交修改。
    pub(crate) fn step(&mut self, progress: &mut ChangeProgress) -> bool {
        progress.step(&mut self.ledger, &mut self.tree)
    }

    /// 最后离开者：自身记录已 complete/rollback 后，在同一锁内对片段两端点
    /// 自底向上摘除未被其他预约覆盖的空表，放入固定批次。一个连续片段至多
    /// [`BOUNDARY_TABLES`] 张，故批次（容量 K）总能容纳；返回是否摘出了表
    /// （需新 epoch 与 fence 覆盖后才放帧）。
    pub(crate) fn detach_boundary_tables(
        &mut self,
        range: memory_space::PageRange,
        batch: &mut RetireBatch,
    ) -> bool {
        let mut detached = false;
        let last = range.end() - memory_space::PAGE_SIZE;
        for address in [range.start(), last] {
            let vpn = page_table::Vpn(address / memory_space::PAGE_SIZE);
            for level in 0..LEVELS - 1 {
                let span = page_table::pages_at(level + 1) * memory_space::PAGE_SIZE;
                let table_start = address / span * span;
                let Ok(table) = memory_space::AddressRange::new(table_start, span) else {
                    break;
                };
                if self.ledger.span_reserved(table) {
                    break;
                }
                match self.tree.detach_empty_table(vpn, level) {
                    Some(frame) => {
                        batch.push(frame);
                        detached = true;
                    }
                    None => break,
                }
            }
        }
        debug_assert_eq!(self.tree.branch_count(), self.tree_held_pages());
        detached
    }

    fn tree_held_pages(&self) -> usize {
        self.tree.branch_count()
    }
}

fn protection_flags(protection: memory_space::Protection) -> u64 {
    use memory_space::Protection;
    use page_table::flags;
    match protection {
        Protection::ReadOnly => flags::V | flags::U | flags::A | flags::R,
        Protection::ReadWrite => flags::V | flags::U | flags::A | flags::R | flags::W | flags::D,
        Protection::ReadExecute => flags::V | flags::U | flags::A | flags::R | flags::X,
    }
}
