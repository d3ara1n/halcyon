//! 内核地址空间：组合 `memory_space` 账本与 `page_table` 树（拆自 task/proc.rs）。
//!
//! 设计见 plans/todo-2026-10-01-memory-redesign.md 设计轮次 3：
//! - 事务：validate → split_at_boundaries → reserve → commit（只移交责任）→
//!   分步 [`ChangeProgress::step`]（ledger `commit_step` 与 PTE `publish_step`
//!   成对推进）→ 最后一次 PTE store 后取 epoch → synchronize → complete。
//! - 空表：只由“最后离开者”在空间锁内经 `detach_empty_table` 摘除，摘出的
//!   owner 进入固定容量退休批次，fence 覆盖后才放帧退款。

use funded_storage::Buffer;
use page_table::{PublishCursor, TableTree};

use super::table::TableMem;
use crate::{
    frame::FundedTableFrame,
    task::resources::{MetadataCharge, MetadataFunding},
};

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

/// 内部空表自底向上扫描；边界留给最后一次原子复查，防止相邻退休互相跳过。
pub(crate) struct TableCleanup {
    range: memory_space::PageRange,
    level: usize,
    next: usize,
}
impl TableCleanup {
    pub(crate) fn new(range: memory_space::PageRange) -> Self {
        let span = page_table::pages_at(1) * memory_space::PAGE_SIZE;
        Self {
            range,
            level: 0,
            next: range.start() / span * span,
        }
    }
    pub(crate) fn range(&self) -> memory_space::PageRange {
        self.range
    }
    fn take(&mut self) -> Option<(usize, usize, usize)> {
        if self.level >= LEVELS - 1 {
            return None;
        }
        let level = self.level;
        let span = page_table::pages_at(level + 1) * memory_space::PAGE_SIZE;
        let address = self.next;
        self.next += span;
        if self.next >= self.range.end() {
            self.level += 1;
            if self.level < LEVELS - 1 {
                let span = page_table::pages_at(self.level + 1) * memory_space::PAGE_SIZE;
                self.next = self.range.start() / span * span;
            }
        }
        Some((address, span, level))
    }
}

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
    committed: Option<memory_space::CommittedChange<MetadataCharge>>,
    footprint: memory_space::PageRange,
    ledger_done: bool,
    cursors: PublishCursors,
    reserve: Buffer<FundedTableFrame, MetadataCharge>,
}

enum PublishCursors {
    Prepared(Buffer<PublishCursor, MetadataCharge>),
    Inline(Option<PublishCursor>),
}
impl PublishCursors {
    fn last_mut(&mut self) -> Option<&mut PublishCursor> {
        match self {
            Self::Prepared(value) => value.last_mut(),
            Self::Inline(value) => value.as_mut(),
        }
    }
    fn pop(&mut self) {
        match self {
            Self::Prepared(value) => {
                value.pop();
            }
            Self::Inline(value) => {
                value.take();
            }
        }
    }
    fn is_empty(&self) -> bool {
        match self {
            Self::Prepared(value) => value.is_empty(),
            Self::Inline(value) => value.is_none(),
        }
    }
}

impl ChangeProgress {
    pub(crate) fn new(
        committed: memory_space::CommittedChange<MetadataCharge>,
        footprint: memory_space::PageRange,
        cursors: Buffer<PublishCursor, MetadataCharge>,
        reserve: Buffer<FundedTableFrame, MetadataCharge>,
    ) -> Self {
        Self {
            committed: Some(committed),
            footprint,
            ledger_done: false,
            cursors: PublishCursors::Prepared(cursors),
            reserve,
        }
    }

    /// 完整基础页 MMIO 片段删除使用 Work 内联游标，不需要拆大页供给或堆数组。
    pub(super) fn forced_remove(
        committed: memory_space::CommittedChange<MetadataCharge>,
        footprint: memory_space::PageRange,
    ) -> Self {
        Self {
            committed: Some(committed),
            footprint,
            ledger_done: false,
            cursors: PublishCursors::Inline(Some(PublishCursor::unmap(
                page_table::Vpn(footprint.start() / memory_space::PAGE_SIZE),
                footprint.pages(),
            ))),
            reserve: Buffer::new(),
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
        ledger: &mut memory_space::MemorySpace<crate::task::resources::MetadataCharge>,
        tree: &mut TableTree<TableMem, LEVELS>,
    ) -> bool {
        if !self.ledger_done {
            let committed = self
                .committed
                .as_mut()
                .expect("change progress lost commit");
            self.ledger_done = ledger.commit_step(committed, STEP_UNITS);
        }
        if let Some(cursor) = self.cursors.last_mut()
            && tree.publish_step(
                cursor,
                &mut core::iter::from_fn(|| self.reserve.pop()),
                STEP_UNITS,
            )
        {
            self.cursors.pop();
        }
        self.ledger_done && self.cursors.is_empty()
    }

    /// 发布完成后交出已提交责任（供 synchronize）与未用表页储备（锁外退款）。
    pub(crate) fn finish(
        mut self,
    ) -> (
        memory_space::CommittedChange<MetadataCharge>,
        Buffer<FundedTableFrame, MetadataCharge>,
    ) {
        assert!(
            self.ledger_done && self.cursors.is_empty(),
            "change not fully published"
        );
        let committed = self.committed.take().expect("change progress lost commit");
        (committed, core::mem::take(&mut self.reserve))
    }
}

/// 整空间退休的一个单元结果。
pub(crate) enum DrainUnit {
    Progress,
    Fragment(
        memory_space::RetiringFragment,
        Option<memory_space::WritePermit>,
    ),
    Table(FundedTableFrame),
    Complete,
}

/// 内核地址空间：账本、页表树与固定空间账户（表页同源）。
pub(crate) struct AnySpace {
    ledger: memory_space::MemorySpace<crate::task::resources::MetadataCharge>,
    tree: TableTree<TableMem, LEVELS>,
    account: alloc::sync::Arc<crate::task::memory_pool::MemoryPool>,
    /// 同一实物来源账本；View 从 PTE 发布前保留到翻译同步后，覆盖几何账本的分步空窗。
    views: alloc::sync::Arc<super::views::Views>,
}

impl AnySpace {
    pub(crate) fn empty(
        account: alloc::sync::Arc<crate::task::memory_pool::MemoryPool>,
    ) -> Result<Self, erhino_shared::call::SystemCallError> {
        use erhino_shared::call::SystemCallError;
        let bounds = memory_space::PageRange::new(0, erhino_shared::proc::PROCESS_USER_TOP)
            .expect("invalid architectural user range");
        let ledger = memory_space::MemorySpace::new(
            bounds,
            memory_space::Limits {
                max_regions: crate::task::resources::REGION_SLOTS_PER_ADDRESS_SPACE,
                max_transactions: crate::task::resources::MEMORY_CHANGES_PER_ADDRESS_SPACE,
                max_pages_per_change: (256 << 20) / memory_space::PAGE_SIZE,
                max_lease_bytes: crate::uaccess::MAX_USER_ACCESS,
                max_lease_segments: 64,
            },
            &crate::task::resources::MetadataFunding(&account),
        )
        .map_err(|error| match error {
            memory_space::SpaceCreateError::BadLimits => SystemCallError::IllegalArgument,
            memory_space::SpaceCreateError::Storage(error) => {
                crate::task::resources::map_storage_error(error)
            }
        })?;
        let root = crate::frame::fund_user_table_frame(&account).map_err(|error| match error {
            funded_frame::FundError::Quota(error) => {
                crate::task::memory_pool::map_pool_error(error)
            }
            _ => SystemCallError::OutOfMemory,
        })?;
        let views = super::views::Views::try_new(&account)?;
        Ok(Self::new(ledger, root, account, views))
    }

    /// 与 SUM 访问同持空间锁，逐页检查 RAM 来源及用户权限，不建立第二套页表遍历。
    pub(crate) fn check_range(
        &mut self,
        ptr: usize,
        len: usize,
        writable: bool,
    ) -> Result<(), crate::uaccess::AccessError> {
        use crate::uaccess::AccessError;
        use erhino_shared::proc::{PROCESS_PAGE_SIZE, PROCESS_USER_TOP};
        use page_table::{Vpn, flags};
        let end = ptr.checked_add(len).ok_or(AccessError::BadRange)?;
        if ptr >= PROCESS_USER_TOP || end > PROCESS_USER_TOP {
            return Err(AccessError::BadRange);
        }
        if len == 0 {
            return Ok(());
        }
        let required = flags::U | if writable { flags::W } else { flags::R };
        for vpn in ptr / PROCESS_PAGE_SIZE..=(end - 1) / PROCESS_PAGE_SIZE {
            if !self.views.is_ram_at(vpn * PROCESS_PAGE_SIZE) {
                return Err(AccessError::Permission);
            }
            let mapped = self
                .tree
                .translate(Vpn(vpn))
                .ok_or(AccessError::NotMapped)?;
            if mapped.flags & required != required {
                return Err(AccessError::Permission);
            }
        }
        Ok(())
    }

    /// 软件页表的真实发布事实；调用者持Vm空间锁。
    pub(crate) fn translate(&mut self, vpn: page_table::Vpn) -> Option<page_table::Mapped> {
        self.tree.translate(vpn)
    }

    pub(crate) fn validate_initial_context(
        &mut self,
        entry: usize,
        stack: usize,
    ) -> Result<(), erhino_shared::call::SystemCallError> {
        use erhino_shared::{
            call::SystemCallError,
            proc::{PROCESS_PAGE_SIZE, PROCESS_USER_TOP},
        };
        use page_table::{Vpn, flags};
        if entry >= PROCESS_USER_TOP
            || entry & 1 != 0
            || stack == 0
            || stack > PROCESS_USER_TOP
            || !stack.is_multiple_of(16)
        {
            return Err(SystemCallError::IllegalArgument);
        }
        for (address, required) in [
            (entry, flags::U | flags::X),
            (stack - 1, flags::U | flags::W),
        ] {
            if !self.views.is_ram_at(address) {
                return Err(SystemCallError::MemoryNotAccessible);
            }
            let mapped = self
                .tree
                .translate(Vpn(address / PROCESS_PAGE_SIZE))
                .ok_or(SystemCallError::MemoryNotAccessible)?;
            if mapped.flags & required != required {
                return Err(SystemCallError::MemoryNotAccessible);
            }
        }
        Ok(())
    }

    pub(crate) fn page_pa(&mut self, va: usize) -> Option<usize> {
        let page = erhino_shared::proc::PROCESS_PAGE_SIZE;
        self.tree
            .translate(page_table::Vpn(va / page))
            .map(|mapped| mapped.ppn.0 * page)
    }

    pub(crate) fn satp_ppn(&self) -> usize {
        self.tree.satp_ppn()
    }

    pub(crate) fn new(
        ledger: memory_space::MemorySpace<crate::task::resources::MetadataCharge>,
        root: FundedTableFrame,
        account: alloc::sync::Arc<crate::task::memory_pool::MemoryPool>,
        views: alloc::sync::Arc<super::views::Views>,
    ) -> Self {
        assert!(
            root.charge_pool_is(&account),
            "address space root funded by a different pool"
        );
        assert!(
            ledger.bounds().end() <= erhino_shared::proc::PROCESS_USER_TOP,
            "user ledger crosses the kernel address domain"
        );
        let mut tree = TableTree::new(TableMem::new(), root);
        // 共享 direct-map/stack-window 根项是借用；TableTree 的 shared-root 标记
        // 保证整空间 drain 不回收内核所有的下级表。用户树出生即具备 trap 所需映射。
        crate::mm::install_kernel_top_level(&mut tree);
        Self {
            ledger,
            tree,
            account,
            views,
        }
    }

    pub(crate) fn views(&self) -> &alloc::sync::Arc<super::views::Views> {
        &self.views
    }

    pub(crate) fn account(&self) -> &alloc::sync::Arc<crate::task::memory_pool::MemoryPool> {
        &self.account
    }

    pub(crate) fn ledger(
        &self,
    ) -> &memory_space::MemorySpace<crate::task::resources::MetadataCharge> {
        &self.ledger
    }

    pub(crate) fn ledger_mut(
        &mut self,
    ) -> &mut memory_space::MemorySpace<crate::task::resources::MetadataCharge> {
        &mut self.ledger
    }

    /// 整空间退休：建立页表 drain 游标（不改树）。
    pub(crate) fn begin_drain(&self) -> page_table::DrainCursor<LEVELS> {
        self.tree.begin_drain()
    }

    /// 整空间退休一个固定单元：账本先逐 region 摘除，再页表逐槽清除/交出表帧。
    pub(crate) fn drain_step(&mut self, cursor: &mut page_table::DrainCursor<LEVELS>) -> DrainUnit {
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
    ) -> (
        FundedTableFrame,
        alloc::sync::Arc<crate::task::memory_pool::MemoryPool>,
    ) {
        (self.tree.finish_drain(), self.account)
    }

    /// 按实际连续物理段拆 Install 游标；未发布定义可失败，不能假定整个对象连续。
    pub(crate) fn plan_cursors(
        intents: &[memory_space::TranslationIntent],
        payer: &MetadataFunding<'_>,
        mut resolve: impl FnMut(&memory_space::MappingOrigin, usize) -> Option<(page_table::Ppn, usize)>,
    ) -> Result<(Buffer<PublishCursor, MetadataCharge>, usize), erhino_shared::call::SystemCallError>
    {
        use erhino_shared::call::SystemCallError;
        use memory_space::{PAGE_SIZE, TranslationIntent as T};
        use page_table::Vpn;
        let mut cursors = Buffer::new();
        let mut frames = 0usize;
        for intent in intents {
            match *intent {
                T::Install {
                    range,
                    ref backing,
                    protection,
                } => {
                    let mut done = 0;
                    while done < range.pages() {
                        let (ppn, available) =
                            resolve(backing, done).ok_or(SystemCallError::IllegalArgument)?;
                        let count = available.min(range.pages() - done);
                        if count == 0 {
                            return Err(SystemCallError::IllegalArgument);
                        }
                        let cursor = PublishCursor::map(
                            Vpn(range.start() / PAGE_SIZE + done),
                            count,
                            ppn,
                            protection_flags(protection),
                            if backing.is_ram() {
                                page_table::LeafSize::Largest
                            } else {
                                page_table::LeafSize::BasePage
                            },
                        );
                        frames = frames
                            .checked_add(page_table::worst_case_frames::<LEVELS>(&cursor))
                            .ok_or(SystemCallError::ReachLimit)?;
                        cursors
                            .try_reserve(1, payer)
                            .map_err(crate::task::resources::map_storage_error)?;
                        cursors.push(cursor);
                        done += count;
                    }
                }
                T::Remove { range } | T::Protect { range, .. } => {
                    let cursor = match *intent {
                        T::Remove { .. } => {
                            PublishCursor::unmap(Vpn(range.start() / PAGE_SIZE), range.pages())
                        }
                        T::Protect { from, to, .. } => PublishCursor::protect(
                            Vpn(range.start() / PAGE_SIZE),
                            range.pages(),
                            protection_flags(from),
                            protection_flags(to),
                        ),
                        _ => unreachable!(),
                    };
                    frames = frames
                        .checked_add(page_table::worst_case_frames::<LEVELS>(&cursor))
                        .ok_or(SystemCallError::ReachLimit)?;
                    cursors
                        .try_reserve(1, payer)
                        .map_err(crate::task::resources::map_storage_error)?;
                    cursors.push(cursor);
                }
            }
        }
        Ok((cursors, frames))
    }

    /// 所有已取得表帧立即落入操作 owner，失败也不在此处批量析构。
    pub(crate) fn fund_reserve(
        account: &alloc::sync::Arc<crate::task::memory_pool::MemoryPool>,
        frames: usize,
        payer: &MetadataFunding<'_>,
        reserve: &mut Buffer<FundedTableFrame, MetadataCharge>,
    ) -> Result<(), erhino_shared::call::SystemCallError> {
        reserve
            .try_reserve_exact(frames, payer)
            .map_err(crate::task::resources::map_storage_error)?;
        for _ in 0..frames {
            reserve.push(crate::frame::fund_user_table_frame(account).map_err(
                |error| match error {
                    funded_frame::FundError::Quota(error) => {
                        crate::task::memory_pool::map_pool_error(error)
                    }
                    _ => erhino_shared::call::SystemCallError::OutOfMemory,
                },
            )?);
        }
        Ok(())
    }

    /// 空间锁内推进一步已提交修改。
    pub(crate) fn step(&mut self, progress: &mut ChangeProgress) -> bool {
        progress.step(&mut self.ledger, &mut self.tree)
    }

    /// 最后离开者：自身仍持 RetiringChange，在同一锁内忽略自己、复查其他预约，
    /// 自底向上摘除两个端点的空表，随后立即 complete。一个连续片段至多
    /// [`BOUNDARY_TABLES`] 张，故批次（容量 K）总能容纳；返回是否摘出了表
    /// （需新 epoch 与 fence 覆盖后才放帧）。
    pub(crate) fn detach_boundary_tables(
        &mut self,
        owner: memory_space::ChangeKey,
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
                if self.ledger.span_reserved_by_other(owner, table) {
                    break;
                }
                if let Some(frame) = self.tree.detach_empty_table(vpn, level) {
                    batch.push(frame);
                    detached = true;
                }
            }
        }
        detached
    }

    /// 每步检查固定数目的内部表跨度，批满先 fence/退款再继续；返回内部扫描已耗尽。
    pub(crate) fn detach_interior_tables(
        &mut self,
        owner: memory_space::ChangeKey,
        cursor: &mut TableCleanup,
        batch: &mut RetireBatch,
    ) -> bool {
        for _ in 0..STEP_UNITS {
            if batch.is_full() {
                return false;
            }
            let Some((address, span, level)) = cursor.take() else {
                return true;
            };
            if address <= cursor.range.start() || address + span >= cursor.range.end() {
                continue;
            }
            let range =
                memory_space::AddressRange::new(address, span).expect("table span overflow");
            if self.ledger.span_reserved_by_other(owner, range) {
                continue;
            }
            if let Some(frame) = self
                .tree
                .detach_empty_table(page_table::Vpn(address / memory_space::PAGE_SIZE), level)
            {
                batch.push(frame);
            }
        }
        false
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
