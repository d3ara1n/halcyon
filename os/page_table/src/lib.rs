//! Sv39/48/57 页表纯逻辑（见 notes/impls/mm.md「页表纯逻辑」）。
//!
//! 本 crate 只管表结构：`TableTree` 经 `TableFrameMemory` 抽象访问表帧，
//! 不直接解引用物理地址，host 与内核 target 复用同一份代码。
//! 匿名内存整备（先取帧再映射）不属于本 crate，由内核 mm 层组合。

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;
use core::fmt;

/// 每张表的项数（RISC-V 标准 9 位索引）。
pub const ENTRIES: usize = 512;
/// 页偏移位数（4KiB 页）。
pub const PAGE_BITS: usize = 12;
/// Sv39/48/57 PTE 可编码的物理页号上界（不含）。
pub const MAX_PPN: usize = 1usize << 44;

// ---------------------------------------------------------------------------
// newtype
// ---------------------------------------------------------------------------

macro_rules! number_newtype {
    ($($name:ident),* $(,)?) => {$(
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
        pub struct $name(pub usize);

        impl $name {
            /// 按字节数地址换算页号。
            pub const fn from_addr(addr: usize) -> Self {
                Self(addr >> PAGE_BITS)
            }
            /// 页号回到字节地址。
            pub const fn addr(self) -> usize {
                self.0 << PAGE_BITS
            }
        }

        impl core::ops::Add<usize> for $name {
            type Output = Self;
            fn add(self, rhs: usize) -> Self {
                Self(self.0 + rhs)
            }
        }
        impl core::ops::Sub<usize> for $name {
            type Output = Self;
            fn sub(self, rhs: usize) -> Self {
                Self(self.0 - rhs)
            }
        }
    )*};
}

number_newtype!(FrameNumber, Vpn, Ppn);

impl Vpn {
    /// 在第 `level` 级表中的索引（level 0 = 叶表）。
    pub const fn index_at(self, level: usize) -> usize {
        (self.0 >> (9 * level)) & 0x1FF
    }
}

/// `LEVELS` 级页表可表示的最大页号（不含）。
pub const fn max_vpn(levels: usize) -> usize {
    1usize << (9 * levels)
}

/// 覆盖 `level` 级一个表项的页数（level 0 = 1 页，level 1 = 2MiB…）。
pub const fn pages_at(level: usize) -> usize {
    512usize.pow(level as u32)
}

// ---------------------------------------------------------------------------
// PTE
// ---------------------------------------------------------------------------

/// 标志位（sv39/48/57 编码一致）。
pub mod flags {
    pub const V: u64 = 1 << 0;
    pub const R: u64 = 1 << 1;
    pub const W: u64 = 1 << 2;
    pub const X: u64 = 1 << 3;
    pub const U: u64 = 1 << 4;
    pub const G: u64 = 1 << 5;
    pub const A: u64 = 1 << 6;
    pub const D: u64 = 1 << 7;

    /// 内核直映射区（含 MMIO）。
    pub const KERNEL_DIRECT: u64 = V | R | W | X | A | D | G;
    /// 内核栈窗口页：RW、不可执行（栈上永不取指；guard 洞不映射）。
    pub const KERNEL_STACK: u64 = V | R | W | A | D | G;
    /// 内核代码/数据（镜像区，不用 G——进程表会拷贝顶层项，粒度到项即可）。
    pub const KERNEL_IMAGE: u64 = V | R | W | X | A | D;
    /// 用户代码。
    pub const USER_CODE: u64 = V | R | X | A | U;
    /// 用户数据（栈/堆）。
    pub const USER_DATA: u64 = V | R | W | A | D | U;
    /// 用户只读数据。
    pub const USER_RODATA: u64 = V | R | A | U;
}

/// 页表项。叶与分支共用 u64 编码：V=1 且 RWX 任一非零为叶，
/// V=1 且 RWX 全零为分支，V=0 无效。
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Pte(u64);

impl Pte {
    pub const fn invalid() -> Self {
        Self(0)
    }

    pub const fn is_valid(self) -> bool {
        self.0 & flags::V != 0
    }

    /// 有效且 RWX 任一非零（叶：mega 或 4KiB）。
    pub const fn is_leaf(self) -> bool {
        self.0 & flags::V != 0 && self.0 & (flags::R | flags::W | flags::X) != 0
    }

    /// 有效且 RWX 全零（指向下一级表）。
    pub const fn is_branch(self) -> bool {
        self.0 & flags::V != 0 && self.0 & (flags::R | flags::W | flags::X) == 0
    }

    /// 构造叶项（4KiB 或 mega，由所在层级决定粒度）。
    pub const fn leaf(ppn: Ppn, flags: u64) -> Self {
        Self(((ppn.0 as u64) << 10) | (flags & 0x3FF))
    }

    /// 构造分支项。
    pub const fn branch(next: FrameNumber) -> Self {
        Self(((next.0 as u64) << 10) | flags::V)
    }

    /// 叶项的目标物理页号。
    pub const fn ppn(self) -> Ppn {
        Ppn((self.0 >> 10) as usize)
    }

    /// 分支项指向的下一级表帧。
    pub const fn next_frame(self) -> FrameNumber {
        FrameNumber((self.0 >> 10) as usize)
    }

    /// 标志位集合。
    pub const fn flags(self) -> u64 {
        self.0 & 0x3FF
    }
}

impl fmt::Debug for Pte {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.is_valid() {
            f.write_str("Pte(invalid)")
        } else if self.is_leaf() {
            write!(
                f,
                "Pte(leaf ppn={:#x} flags={:#x})",
                self.ppn().0,
                self.flags()
            )
        } else {
            write!(f, "Pte(branch frame={:#x})", self.next_frame().0)
        }
    }
}

// ---------------------------------------------------------------------------
// 帧访问抽象
// ---------------------------------------------------------------------------

/// Eager builder 在发布前持有一张已清零表帧的 affine reservation。
pub trait ReservedTableFrame {
    fn number(&self) -> FrameNumber;
    fn commit(self) -> FrameNumber;
}

/// 未发布 eager tree 的表帧来源；提交后的表帧具有平台级永久寿命。
pub trait EagerFrameMemory {
    type ReservedFrame: ReservedTableFrame;

    fn reserve_frame(&mut self) -> Result<Self::ReservedFrame, FrameExhausted>;
    fn table_mut(&mut self, frame: FrameNumber) -> &mut [Pte; ENTRIES];
}

/// 与 branch PTE 同生灭的 affine 表帧 owner。
pub trait TableFrameOwner {
    fn number(&self) -> FrameNumber;
}

/// 可回收页表树的帧访问与 owner 类型。owner 由调用方在树锁外取得后显式供给。
pub trait TableFrameMemory {
    type FrameOwner: TableFrameOwner;

    fn table_mut(&mut self, frame: FrameNumber) -> &mut [Pte; ENTRIES];
}

/// 表帧耗尽。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FrameExhausted;

// ---------------------------------------------------------------------------
// 映射结果与错误
// ---------------------------------------------------------------------------

/// `translate` 的结果。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mapped {
    pub ppn: Ppn,
    pub flags: u64,
    /// 叶所在层级：0 = 4KiB，1 = 2MiB，2 = 1GiB…
    pub level: usize,
}

/// translation prepare 阶段的可恢复错误。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapError {
    OutOfRange,
    Conflict { vpn: Vpn },
    NotMapped { vpn: Vpn },
    ProtectionMismatch { vpn: Vpn },
    FrameExhausted,
    InvalidFlags,
    AllocationFailed,
}

impl From<FrameExhausted> for MapError {
    fn from(_: FrameExhausted) -> Self {
        Self::FrameExhausted
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SharedRootError {
    OutOfRange,
    InvalidEntry,
    Conflict,
}

/// 一次 translation 结构 preflight。对象只冻结意图与当时精确需求；供给 owner 后
/// 必须由树按当前代次与结构重新验证。
#[derive(Clone, Copy, Debug)]
#[must_use = "translation preflight must be supplied or abandoned"]
pub struct TranslationPreflight {
    plan: TranslationPlan,
    generation: u64,
    required_frames: usize,
    retired_frames: usize,
}

impl TranslationPreflight {
    pub const fn required_frames(&self) -> usize {
        self.required_frames
    }

    pub const fn observed_generation(&self) -> u64 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug)]
enum TranslationPlan {
    Map {
        vpn: Vpn,
        count: usize,
        ppn: Ppn,
        flags: u64,
    },
    Unmap {
        vpn: Vpn,
        count: usize,
    },
    Protect {
        vpn: Vpn,
        count: usize,
        from: u64,
        to: u64,
    },
}

#[derive(Debug)]
#[must_use = "prepared translations must be published or rolled back by dropping them"]
pub struct PreparedTranslation<O: TableFrameOwner> {
    plan: TranslationPlan,
    generation: u64,
    frames: Vec<O>,
    retired: Vec<O>,
}

impl<O: TableFrameOwner> PreparedTranslation<O> {
    pub fn supplied_frames(&self) -> usize {
        self.frames.len()
    }

    fn take_owner(&mut self) -> O {
        self.frames
            .pop()
            .expect("translation preflight undercounted table frames")
    }
}

#[derive(Debug)]
pub struct PrepareFailure<O> {
    pub error: MapError,
    pub owners: Vec<O>,
}

#[derive(Debug)]
#[must_use = "unused and retired table owners must leave the tree lock"]
pub struct PublishOutcome<O> {
    pub unused: Vec<O>,
    pub retired: Vec<O>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DrainCursor<const LEVELS: usize> {
    frames: [Option<FrameNumber>; LEVELS],
    indices: [usize; LEVELS],
    level: usize,
    complete: bool,
}

#[derive(Debug)]
pub enum DrainStep<O> {
    Progress,
    Retired(O),
    Complete,
}

// ---------------------------------------------------------------------------
// 未发布树的 eager builder
// ---------------------------------------------------------------------------

/// 为尚未发布的页表树建立 eager range mapping。
///
/// 每段映射先完成范围与冲突验证，再自动选择尽可能大的叶级发布。中间表由
/// [`EagerFrameMemory`] 提供，因此调用方可以使用固定静态预算而不依赖堆。若表帧在发布
/// 中耗尽，树可能已部分构造；该树仍未发布，调用方必须丢弃或终止启动。
pub struct EagerMapper<'a, M: EagerFrameMemory, const LEVELS: usize> {
    mem: &'a mut M,
    root: FrameNumber,
}

impl<'a, M: EagerFrameMemory, const LEVELS: usize> EagerMapper<'a, M, LEVELS> {
    pub fn new(mem: &'a mut M, root: FrameNumber) -> Self {
        assert!(
            LEVELS >= 2 && LEVELS <= 5,
            "only sv39..sv57 supported (LEVELS 2..5)"
        );
        assert!(root.0 < MAX_PPN, "root frame exceeds PTE encoding");
        Self { mem, root }
    }

    /// 建立物理连续的 eager mapping；`vpn` 与 `ppn` 均以 4 KiB 页计。
    pub fn map_range(
        &mut self,
        vpn: Vpn,
        count: usize,
        ppn: Ppn,
        flags: u64,
    ) -> Result<(), MapError> {
        if !valid_leaf_flags(flags) {
            return Err(MapError::InvalidFlags);
        }
        let end = vpn.0.checked_add(count).ok_or(MapError::OutOfRange)?;
        if end > max_vpn(LEVELS) {
            return Err(MapError::OutOfRange);
        }
        let ppn_end = ppn.0.checked_add(count).ok_or(MapError::OutOfRange)?;
        if ppn_end > MAX_PPN {
            return Err(MapError::OutOfRange);
        }

        let mut vpn_cur = vpn.0;
        let mut ppn_cur = ppn.0;
        while vpn_cur < end {
            let level = largest_segment_level::<LEVELS>(vpn_cur, ppn_cur, end);
            self.validate_segment(Vpn(vpn_cur), level, Ppn(ppn_cur), flags)?;
            let pages = pages_at(level);
            vpn_cur += pages;
            ppn_cur += pages;
        }

        vpn_cur = vpn.0;
        ppn_cur = ppn.0;
        while vpn_cur < end {
            let level = largest_segment_level::<LEVELS>(vpn_cur, ppn_cur, end);
            self.publish_segment(Vpn(vpn_cur), level, Ppn(ppn_cur), flags)?;
            let pages = pages_at(level);
            vpn_cur += pages;
            ppn_cur += pages;
        }
        Ok(())
    }

    fn validate_segment(
        &mut self,
        vpn: Vpn,
        segment_level: usize,
        ppn: Ppn,
        flags: u64,
    ) -> Result<(), MapError> {
        let mut frame = self.root;
        let mut level = LEVELS - 1;
        loop {
            let entry = self.mem.table_mut(frame)[vpn.index_at(level)];
            if level == segment_level {
                return if !entry.is_valid() || self.leaf_matches(entry, ppn, flags) {
                    Ok(())
                } else {
                    Err(MapError::Conflict { vpn })
                };
            }
            if entry.is_branch() {
                frame = entry.next_frame();
                level -= 1;
                continue;
            }
            if entry.is_leaf() && !self.leaf_matches_at(entry, level, vpn, ppn, flags) {
                return Err(MapError::Conflict { vpn });
            }
            return Ok(());
        }
    }

    fn publish_segment(
        &mut self,
        vpn: Vpn,
        segment_level: usize,
        ppn: Ppn,
        flags: u64,
    ) -> Result<(), MapError> {
        let mut frame = self.root;
        let mut level = LEVELS - 1;
        loop {
            let index = vpn.index_at(level);
            let entry = self.mem.table_mut(frame)[index];
            if level == segment_level {
                debug_assert!(!entry.is_valid() || self.leaf_matches(entry, ppn, flags));
                self.mem.table_mut(frame)[index] = Pte::leaf(ppn, flags);
                return Ok(());
            }
            if entry.is_branch() {
                frame = entry.next_frame();
                level -= 1;
                continue;
            }
            if entry.is_leaf() {
                debug_assert!(self.leaf_matches_at(entry, level, vpn, ppn, flags));
                return Ok(());
            }
            let child = self.allocate_table()?;
            self.mem.table_mut(frame)[index] = Pte::branch(child);
            frame = child;
            level -= 1;
        }
    }

    fn allocate_table(&mut self) -> Result<FrameNumber, MapError> {
        let reserved = self.mem.reserve_frame()?;
        let frame = reserved.number();
        assert!(
            frame.0 < MAX_PPN,
            "reserved table frame exceeds PTE encoding"
        );
        self.mem.table_mut(frame).fill(Pte::invalid());
        Ok(reserved.commit())
    }

    fn leaf_matches(&self, entry: Pte, ppn: Ppn, flags: u64) -> bool {
        entry.is_leaf() && entry.ppn() == ppn && entry.flags() == flags
    }

    fn leaf_matches_at(&self, entry: Pte, level: usize, vpn: Vpn, ppn: Ppn, flags: u64) -> bool {
        entry.is_leaf()
            && entry.flags() == flags
            && entry.ppn().0 + vpn.0 % pages_at(level) == ppn.0
    }
}

// ---------------------------------------------------------------------------
// TableTree
// ---------------------------------------------------------------------------

/// 一棵页表树，const 泛型于级数：`TableTree<M, 3>` 即 sv39。
///
/// root 与每张非 shared 分支表都保留 affine owner；硬件 PTE 只保存其几何投影。
/// shared root 槽由显式位图标记，owner ledger 与有界 drain 不进入外部子树。
pub struct TableTree<M: TableFrameMemory, const LEVELS: usize> {
    mem: M,
    root: FrameNumber,
    root_owner: Option<M::FrameOwner>,
    owners: Vec<M::FrameOwner>,
    shared_root: [u64; ENTRIES / 64],
    owned_root: [u64; ENTRIES / 64],
    generation: u64,
}

impl<M: TableFrameMemory, const LEVELS: usize> TableTree<M, LEVELS> {
    /// 以调用方已取得的 root owner 建树；构造不访问帧来源。
    pub fn new(mut mem: M, root_owner: M::FrameOwner) -> Self {
        assert!(
            LEVELS >= 2 && LEVELS <= 5,
            "only sv39..sv57 supported (LEVELS 2..5)"
        );
        let root = root_owner.number();
        assert!(root.0 < MAX_PPN, "root frame exceeds PTE encoding");
        mem.table_mut(root).fill(Pte::invalid());
        Self {
            mem,
            root,
            root_owner: Some(root_owner),
            owners: Vec::new(),
            shared_root: [0; ENTRIES / 64],
            owned_root: [0; ENTRIES / 64],
            generation: 0,
        }
    }

    pub fn root_frame(&self) -> FrameNumber {
        self.root
    }

    /// 组装 satp 的 PPN 字段值（模式位由调用方拼）。
    pub fn satp_ppn(&self) -> usize {
        self.root.0
    }

    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// 锁内只读结构 preflight：精确计算当前树所需的新表页与退休表页。
    pub fn preflight_map(
        &mut self,
        vpn: Vpn,
        count: usize,
        ppn: Ppn,
        flags: u64,
    ) -> Result<TranslationPreflight, MapError> {
        self.preflight_plan(TranslationPlan::Map {
            vpn,
            count,
            ppn,
            flags,
        })
    }

    pub fn preflight_unmap(
        &mut self,
        vpn: Vpn,
        count: usize,
    ) -> Result<TranslationPreflight, MapError> {
        self.preflight_plan(TranslationPlan::Unmap { vpn, count })
    }

    pub fn preflight_protect(
        &mut self,
        vpn: Vpn,
        count: usize,
        from: u64,
        to: u64,
    ) -> Result<TranslationPreflight, MapError> {
        self.preflight_plan(TranslationPlan::Protect {
            vpn,
            count,
            from,
            to,
        })
    }

    /// 供给锁外取得且已清零的 owner，并按当前代次和结构重新验证。
    /// 并行发布若减少需求，多余 owner 保留到 PublishOutcome 显式返回。
    pub fn prepare(
        &mut self,
        preflight: TranslationPreflight,
        owners: Vec<M::FrameOwner>,
    ) -> Result<PreparedTranslation<M::FrameOwner>, PrepareFailure<M::FrameOwner>> {
        let current = match self.preflight_plan(preflight.plan) {
            Ok(current) => current,
            Err(error) => return Err(PrepareFailure { error, owners }),
        };
        if owners.len() < current.required_frames {
            return Err(PrepareFailure {
                error: MapError::FrameExhausted,
                owners,
            });
        }
        if self.owners.try_reserve(current.required_frames).is_err() {
            return Err(PrepareFailure {
                error: MapError::AllocationFailed,
                owners,
            });
        }
        let mut retired = Vec::new();
        if retired.try_reserve_exact(current.retired_frames).is_err() {
            return Err(PrepareFailure {
                error: MapError::AllocationFailed,
                owners,
            });
        }
        for owner in &owners {
            assert!(
                owner.number().0 < MAX_PPN,
                "supplied table frame exceeds PTE encoding"
            );
        }
        Ok(PreparedTranslation {
            plan: current.plan,
            generation: current.generation,
            frames: owners,
            retired,
        })
    }

    /// 判断 Prepared 是否仍对应当前树代次；调用方必须在 Commit 前于同一树锁内复检。
    pub fn prepared_is_current(&self, prepared: &PreparedTranslation<M::FrameOwner>) -> bool {
        prepared.generation == self.generation
    }

    /// 同一事务内发布一组 Prepared。多项批次只接受同代次 Map：前项只会减少后项
    /// 的表页需求，调用方须在 Commit 前持树锁确认该代次仍为当前代次。
    pub fn publish_batch(
        &mut self,
        prepared: Vec<PreparedTranslation<M::FrameOwner>>,
        mut outcomes: Vec<PublishOutcome<M::FrameOwner>>,
    ) -> Vec<PublishOutcome<M::FrameOwner>> {
        assert!(!prepared.is_empty(), "translation batch must be nonempty");
        assert!(
            outcomes.is_empty(),
            "translation outcome buffer must be empty"
        );
        assert!(
            outcomes.capacity() >= prepared.len(),
            "translation outcome buffer was not preallocated"
        );
        assert!(
            prepared
                .iter()
                .all(|translation| translation.generation == self.generation),
            "prepared translation batch is stale"
        );
        assert!(
            prepared.len() == 1
                || prepared
                    .iter()
                    .all(|translation| matches!(translation.plan, TranslationPlan::Map { .. })),
            "multi-translation batch must contain only Maps"
        );
        for translation in prepared {
            outcomes.push(self.publish_inner(translation));
        }
        self.advance_generation();
        outcomes
    }

    /// Publish 只消费 Prepared owner、改写 PTE 与移动 owner，不分配且不可失败。
    pub fn publish(
        &mut self,
        prepared: PreparedTranslation<M::FrameOwner>,
    ) -> PublishOutcome<M::FrameOwner> {
        assert_eq!(
            prepared.generation, self.generation,
            "prepared translation is stale"
        );
        let outcome = self.publish_inner(prepared);
        self.advance_generation();
        outcome
    }

    fn publish_inner(
        &mut self,
        mut prepared: PreparedTranslation<M::FrameOwner>,
    ) -> PublishOutcome<M::FrameOwner> {
        match prepared.plan {
            TranslationPlan::Map {
                vpn,
                count,
                ppn,
                flags,
            } => self.publish_map(vpn, count, ppn, flags, &mut prepared),
            TranslationPlan::Unmap { vpn, count } => {
                let end = vpn.0 + count;
                let _root_empty =
                    self.unmap_range(self.root, LEVELS - 1, 0, vpn.0, end, &mut prepared);
            }
            TranslationPlan::Protect {
                vpn,
                count,
                from,
                to,
            } => {
                let end = vpn.0 + count;
                self.protect_range(
                    self.root,
                    LEVELS - 1,
                    0,
                    vpn.0,
                    end,
                    from,
                    to,
                    &mut prepared,
                );
            }
        }
        PublishOutcome {
            unused: prepared.frames,
            retired: prepared.retired,
        }
    }

    fn advance_generation(&mut self) {
        self.generation = self
            .generation
            .checked_add(1)
            .expect("page-table generation exhausted");
    }

    /// 把外部所有的顶层项挂入 root；失败时不修改任何槽。
    pub fn attach_shared_root(
        &mut self,
        start: usize,
        entries: &[Pte],
    ) -> Result<(), SharedRootError> {
        let end = start
            .checked_add(entries.len())
            .ok_or(SharedRootError::OutOfRange)?;
        if end > ENTRIES {
            return Err(SharedRootError::OutOfRange);
        }
        if entries.iter().any(|entry| !entry.is_valid()) {
            return Err(SharedRootError::InvalidEntry);
        }
        if (start..end).any(|slot| {
            self.is_shared_root(slot)
                || self.is_owned_root(slot)
                || self.mem.table_mut(self.root)[slot].is_valid()
        }) {
            return Err(SharedRootError::Conflict);
        }
        for (slot, entry) in (start..end).zip(entries.iter().copied()) {
            self.mem.table_mut(self.root)[slot] = entry;
            self.set_shared_root(slot);
        }
        self.generation = self
            .generation
            .checked_add(1)
            .expect("page-table generation exhausted");
        Ok(())
    }

    /// 建立层级无关的固定宽 drain 游标；不改动树。
    pub fn begin_drain(&self) -> DrainCursor<LEVELS> {
        let mut frames = [None; LEVELS];
        frames[LEVELS - 1] = Some(self.root);
        DrainCursor {
            frames,
            indices: [0; LEVELS],
            level: LEVELS - 1,
            complete: false,
        }
    }

    /// 推进一个固定 work unit：扫描/清除一个槽，下降一级，或交出一张已摘除表帧。
    pub fn drain_step(&mut self, cursor: &mut DrainCursor<LEVELS>) -> DrainStep<M::FrameOwner> {
        if cursor.complete {
            return DrainStep::Complete;
        }
        let level = cursor.level;
        let frame = cursor.frames[level].expect("drain cursor lost its current frame");
        let index = cursor.indices[level];
        if index < ENTRIES {
            if level == LEVELS - 1 && self.is_shared_root(index) {
                cursor.indices[level] += 1;
                return DrainStep::Progress;
            }
            let entry = self.mem.table_mut(frame)[index];
            cursor.indices[level] += 1;
            if entry.is_branch() {
                assert!(level > 0, "level-0 PTE cannot be a branch");
                let child_level = level - 1;
                cursor.frames[child_level] = Some(entry.next_frame());
                cursor.indices[child_level] = 0;
                cursor.level = child_level;
                return DrainStep::Progress;
            }
            if entry.is_valid() {
                self.mem.table_mut(frame)[index] = Pte::invalid();
                if level == LEVELS - 1 {
                    self.clear_owned_root(index);
                }
            } else if level == LEVELS - 1 {
                assert!(
                    !self.is_owned_root(index),
                    "owned root slot contains an invalid PTE"
                );
            }
            return DrainStep::Progress;
        }

        if level == LEVELS - 1 {
            cursor.complete = true;
            return DrainStep::Complete;
        }
        let parent_level = level + 1;
        let parent = cursor.frames[parent_level].expect("drain cursor lost its parent frame");
        let parent_index = cursor.indices[parent_level]
            .checked_sub(1)
            .expect("drain cursor parent index underflow");
        let parent_entry = self.mem.table_mut(parent)[parent_index];
        assert!(
            parent_entry.is_branch() && parent_entry.next_frame() == frame,
            "drain cursor parent branch changed"
        );
        self.mem.table_mut(parent)[parent_index] = Pte::invalid();
        if parent_level == LEVELS - 1 {
            self.clear_owned_root(parent_index);
        }
        cursor.frames[level] = None;
        cursor.indices[level] = 0;
        cursor.level = parent_level;
        DrainStep::Retired(self.take_owner(frame))
    }

    /// drain 完成后交出 root owner；调用方可在树锁外归还。
    pub fn finish_drain(mut self) -> M::FrameOwner {
        assert!(
            self.owned_root.iter().all(|word| *word == 0),
            "owned root slot remains at drain completion"
        );
        assert!(
            self.owners.is_empty(),
            "branch owner remains at drain completion"
        );
        self.root_owner
            .take()
            .expect("page-table root owner already removed")
    }

    pub fn translate(&mut self, vpn: Vpn) -> Option<Mapped> {
        if vpn.0 >= max_vpn(LEVELS) {
            return None;
        }
        let mut frame = self.root;
        for level in (0..LEVELS).rev() {
            let entry = self.mem.table_mut(frame)[vpn.index_at(level)];
            if entry.is_leaf() {
                let base = entry.ppn().0;
                // mega 内偏移换算
                let off = vpn.0 % pages_at(level);
                return Some(Mapped {
                    ppn: Ppn(base + off),
                    flags: entry.flags(),
                    level,
                });
            }
            if entry.is_branch() {
                frame = entry.next_frame();
            } else {
                return None;
            }
        }
        None
    }

    // -- 内部 ---------------------------------------------------------------

    fn validate_range(&self, vpn: Vpn, count: usize) -> Result<usize, MapError> {
        let end = vpn.0.checked_add(count).ok_or(MapError::OutOfRange)?;
        if end > max_vpn(LEVELS) {
            return Err(MapError::OutOfRange);
        }
        Ok(end)
    }

    fn preflight_plan(&mut self, plan: TranslationPlan) -> Result<TranslationPreflight, MapError> {
        let (required_frames, retired_frames) = match plan {
            TranslationPlan::Map {
                vpn,
                count,
                ppn,
                flags,
            } => (self.preflight_map_requirements(vpn, count, ppn, flags)?, 0),
            TranslationPlan::Unmap { vpn, count } => {
                let end = self.validate_range(vpn, count)?;
                let (required, retired, _) =
                    self.preflight_unmap_range(self.root, LEVELS - 1, 0, vpn.0, end)?;
                (required, retired)
            }
            TranslationPlan::Protect {
                vpn,
                count,
                from,
                to,
            } => {
                if !valid_leaf_flags(from) || !valid_leaf_flags(to) {
                    return Err(MapError::InvalidFlags);
                }
                let end = self.validate_range(vpn, count)?;
                (
                    self.preflight_range(self.root, LEVELS - 1, 0, vpn.0, end, Some(from))?,
                    0,
                )
            }
        };
        Ok(TranslationPreflight {
            plan,
            generation: self.generation,
            required_frames,
            retired_frames,
        })
    }

    fn preflight_map_requirements(
        &mut self,
        vpn: Vpn,
        count: usize,
        ppn: Ppn,
        flags: u64,
    ) -> Result<usize, MapError> {
        if !valid_leaf_flags(flags) {
            return Err(MapError::InvalidFlags);
        }
        let end = self.validate_range(vpn, count)?;
        let ppn_end = ppn.0.checked_add(count).ok_or(MapError::OutOfRange)?;
        if ppn_end > MAX_PPN {
            return Err(MapError::OutOfRange);
        }
        let max_missing = count
            .checked_mul(LEVELS - 1)
            .ok_or(MapError::AllocationFailed)?;
        let mut missing = Vec::new();
        missing
            .try_reserve_exact(max_missing)
            .map_err(|_| MapError::AllocationFailed)?;
        let mut vpn_cur = vpn.0;
        let mut ppn_cur = ppn.0;
        while vpn_cur < end {
            let seg_level = largest_segment_level::<LEVELS>(vpn_cur, ppn_cur, end);
            self.validate_map_segment(Vpn(vpn_cur), seg_level, Ppn(ppn_cur), flags, &mut missing)?;
            let pages = pages_at(seg_level);
            vpn_cur += pages;
            ppn_cur += pages;
        }
        missing.sort_unstable();
        missing.dedup();
        Ok(missing.len())
    }

    fn validate_map_segment(
        &mut self,
        vpn: Vpn,
        seg_level: usize,
        ppn: Ppn,
        flags: u64,
        missing: &mut Vec<(usize, usize)>,
    ) -> Result<(), MapError> {
        let mut frame = self.root;
        let mut level = LEVELS - 1;
        loop {
            let index = vpn.index_at(level);
            if level == LEVELS - 1 && self.is_shared_root(index) {
                return Err(MapError::Conflict { vpn });
            }
            let entry = self.mem.table_mut(frame)[index];
            if level == seg_level {
                return if !entry.is_valid() || self.leaf_matches(entry, ppn, flags) {
                    Ok(())
                } else {
                    Err(MapError::Conflict { vpn })
                };
            }
            if entry.is_branch() {
                frame = entry.next_frame();
                level -= 1;
                continue;
            }
            if entry.is_leaf() {
                if !self.leaf_matches_at(entry, level, vpn, ppn, flags) {
                    return Err(MapError::Conflict { vpn });
                }
                for table_level in seg_level..level {
                    missing.push((table_level, vpn.0 / pages_at(table_level + 1)));
                }
                return Ok(());
            }
            for table_level in seg_level..level {
                missing.push((table_level, vpn.0 / pages_at(table_level + 1)));
            }
            return Ok(());
        }
    }

    fn publish_map(
        &mut self,
        vpn: Vpn,
        count: usize,
        ppn: Ppn,
        flags: u64,
        prepared: &mut PreparedTranslation<M::FrameOwner>,
    ) {
        let end = vpn.0 + count;
        let mut vpn_cur = vpn.0;
        let mut ppn_cur = ppn.0;
        while vpn_cur < end {
            let seg_level = largest_segment_level::<LEVELS>(vpn_cur, ppn_cur, end);
            self.publish_map_segment(Vpn(vpn_cur), seg_level, Ppn(ppn_cur), flags, prepared);
            let pages = pages_at(seg_level);
            vpn_cur += pages;
            ppn_cur += pages;
        }
    }

    fn publish_map_segment(
        &mut self,
        vpn: Vpn,
        seg_level: usize,
        ppn: Ppn,
        flags: u64,
        prepared: &mut PreparedTranslation<M::FrameOwner>,
    ) {
        let mut frame = self.root;
        let mut level = LEVELS - 1;
        loop {
            let idx = vpn.index_at(level);
            assert!(
                level != LEVELS - 1 || !self.is_shared_root(idx),
                "prepared Map entered a shared root slot"
            );
            let entry = self.mem.table_mut(frame)[idx];
            if level == seg_level {
                assert!(
                    !entry.is_valid() || self.leaf_matches(entry, ppn, flags),
                    "prepared map became conflicting before publish"
                );
                if frame == self.root {
                    self.set_owned_root(idx);
                }
                self.mem.table_mut(frame)[idx] = Pte::leaf(ppn, flags);
                return;
            }
            if entry.is_branch() {
                frame = entry.next_frame();
                level -= 1;
                continue;
            }
            if entry.is_leaf() {
                assert!(
                    self.leaf_matches_at(entry, level, vpn, ppn, flags),
                    "prepared map became conflicting before publish"
                );
                frame = self.split_mega(frame, idx, level, prepared);
                level -= 1;
                continue;
            }
            if frame == self.root {
                self.set_owned_root(idx);
            }
            let owner = prepared.take_owner();
            let new_frame = self.install_owner(owner);
            self.mem.table_mut(frame)[idx] = Pte::branch(new_frame);
            frame = new_frame;
            level -= 1;
        }
    }

    fn install_owner(&mut self, owner: M::FrameOwner) -> FrameNumber {
        let frame = owner.number();
        self.owners.push(owner);
        frame
    }

    fn take_owner(&mut self, frame: FrameNumber) -> M::FrameOwner {
        let index = self
            .owners
            .iter()
            .position(|owner| owner.number() == frame)
            .expect("branch PTE has no committed table owner");
        self.owners.swap_remove(index)
    }

    fn leaf_matches(&self, entry: Pte, ppn: Ppn, flags: u64) -> bool {
        entry.is_leaf() && entry.ppn() == ppn && entry.flags() == flags
    }

    fn leaf_matches_at(&self, entry: Pte, level: usize, vpn: Vpn, ppn: Ppn, flags: u64) -> bool {
        if !entry.is_leaf() || entry.flags() != flags {
            return false;
        }
        let mega_pages = pages_at(level);
        entry.ppn().0 + vpn.0 % mega_pages == ppn.0
    }

    fn split_mega(
        &mut self,
        frame: FrameNumber,
        idx: usize,
        level: usize,
        prepared: &mut PreparedTranslation<M::FrameOwner>,
    ) -> FrameNumber {
        assert!(level >= 1, "level-0 leaf cannot split further");
        let entry = self.mem.table_mut(frame)[idx];
        assert!(entry.is_leaf(), "only a leaf can be split");
        let mega_pages = pages_at(level);
        let base_ppn = entry.ppn().0 - entry.ppn().0 % mega_pages;
        let flags = entry.flags();
        let owner = prepared.take_owner();
        let new_frame = self.install_owner(owner);
        let table = self.mem.table_mut(new_frame);
        let sub_pages = pages_at(level - 1);
        for (index, slot) in table.iter_mut().enumerate() {
            *slot = Pte::leaf(Ppn(base_ppn + index * sub_pages), flags);
        }
        self.mem.table_mut(frame)[idx] = Pte::branch(new_frame);
        new_frame
    }

    fn preflight_range(
        &mut self,
        frame: FrameNumber,
        level: usize,
        table_base: usize,
        vpn_start: usize,
        vpn_end: usize,
        expected_flags: Option<u64>,
    ) -> Result<usize, MapError> {
        let mut required = 0usize;
        let slot_pages = pages_at(level);
        for index in 0..ENTRIES {
            let slot_base = table_base + index * slot_pages;
            let slot_end = slot_base + slot_pages;
            if slot_base >= vpn_end || slot_end <= vpn_start {
                continue;
            }
            if level == LEVELS - 1 && self.is_shared_root(index) {
                return Err(MapError::Conflict {
                    vpn: Vpn(slot_base.max(vpn_start)),
                });
            }
            let entry = self.mem.table_mut(frame)[index];
            if !entry.is_valid() {
                if expected_flags.is_some() {
                    return Err(MapError::NotMapped {
                        vpn: Vpn(slot_base.max(vpn_start)),
                    });
                }
                continue;
            }
            if entry.is_leaf() {
                if let Some(expected) = expected_flags
                    && entry.flags() != expected
                {
                    return Err(MapError::ProtectionMismatch {
                        vpn: Vpn(slot_base.max(vpn_start)),
                    });
                }
                required = required
                    .checked_add(split_count_for_leaf(level, slot_base, vpn_start, vpn_end))
                    .ok_or(MapError::AllocationFailed)?;
            } else {
                required = required
                    .checked_add(self.preflight_range(
                        entry.next_frame(),
                        level - 1,
                        slot_base,
                        vpn_start,
                        vpn_end,
                        expected_flags,
                    )?)
                    .ok_or(MapError::AllocationFailed)?;
            }
        }
        Ok(required)
    }

    fn preflight_unmap_range(
        &mut self,
        frame: FrameNumber,
        level: usize,
        table_base: usize,
        vpn_start: usize,
        vpn_end: usize,
    ) -> Result<(usize, usize, bool), MapError> {
        let mut required = 0usize;
        let mut retired = 0usize;
        let mut live = false;
        let slot_pages = pages_at(level);
        for index in 0..ENTRIES {
            if level == LEVELS - 1 && self.is_shared_root(index) {
                live = true;
                continue;
            }
            let slot_base = table_base + index * slot_pages;
            let slot_end = slot_base + slot_pages;
            let entry = self.mem.table_mut(frame)[index];
            if slot_base >= vpn_end || slot_end <= vpn_start {
                live |= entry.is_valid();
                continue;
            }
            if !entry.is_valid() {
                continue;
            }
            if entry.is_leaf() {
                if slot_base >= vpn_start && slot_end <= vpn_end {
                    continue;
                }
                required = required
                    .checked_add(split_count_for_leaf(level, slot_base, vpn_start, vpn_end))
                    .ok_or(MapError::AllocationFailed)?;
                live = true;
                continue;
            }
            let (child_required, child_retired, child_empty) = self.preflight_unmap_range(
                entry.next_frame(),
                level - 1,
                slot_base,
                vpn_start,
                vpn_end,
            )?;
            required = required
                .checked_add(child_required)
                .ok_or(MapError::AllocationFailed)?;
            retired = retired
                .checked_add(child_retired)
                .and_then(|value| value.checked_add(usize::from(child_empty)))
                .ok_or(MapError::AllocationFailed)?;
            live |= !child_empty;
        }
        Ok((required, retired, !live))
    }

    fn unmap_range(
        &mut self,
        frame: FrameNumber,
        level: usize,
        table_base: usize,
        vpn_start: usize,
        vpn_end: usize,
        prepared: &mut PreparedTranslation<M::FrameOwner>,
    ) -> bool {
        let mut live = false;
        let slot_pages = pages_at(level);
        for index in 0..ENTRIES {
            if level == LEVELS - 1 && self.is_shared_root(index) {
                live = true;
                continue;
            }
            let slot_base = table_base + index * slot_pages;
            let slot_end = slot_base + slot_pages;
            let entry = self.mem.table_mut(frame)[index];
            if slot_base >= vpn_end || slot_end <= vpn_start {
                live |= entry.is_valid();
                continue;
            }
            if !entry.is_valid() {
                continue;
            }
            if entry.is_leaf() {
                if slot_base >= vpn_start && slot_end <= vpn_end {
                    self.mem.table_mut(frame)[index] = Pte::invalid();
                    if frame == self.root {
                        self.clear_owned_root(index);
                    }
                } else {
                    let sub = self.split_mega(frame, index, level, prepared);
                    let empty =
                        self.unmap_range(sub, level - 1, slot_base, vpn_start, vpn_end, prepared);
                    assert!(!empty, "partial mega unmap removed the complete leaf");
                    live = true;
                }
                continue;
            }
            let child = entry.next_frame();
            if self.unmap_range(child, level - 1, slot_base, vpn_start, vpn_end, prepared) {
                self.mem.table_mut(frame)[index] = Pte::invalid();
                if frame == self.root {
                    self.clear_owned_root(index);
                }
                prepared.retired.push(self.take_owner(child));
            } else {
                live = true;
            }
        }
        !live
    }

    #[allow(clippy::too_many_arguments)]
    fn protect_range(
        &mut self,
        frame: FrameNumber,
        level: usize,
        table_base: usize,
        vpn_start: usize,
        vpn_end: usize,
        from: u64,
        to: u64,
        prepared: &mut PreparedTranslation<M::FrameOwner>,
    ) {
        let slot_pages = pages_at(level);
        for index in 0..ENTRIES {
            let slot_base = table_base + index * slot_pages;
            let slot_end = slot_base + slot_pages;
            if slot_base >= vpn_end || slot_end <= vpn_start {
                continue;
            }
            assert!(
                level != LEVELS - 1 || !self.is_shared_root(index),
                "prepared Protect entered a shared root slot"
            );
            let entry = self.mem.table_mut(frame)[index];
            assert!(entry.is_valid(), "prepared protect lost its mapping");
            if entry.is_leaf() {
                assert_eq!(
                    entry.flags(),
                    from,
                    "prepared protect flags changed before publish"
                );
                if slot_base >= vpn_start && slot_end <= vpn_end {
                    self.mem.table_mut(frame)[index] = Pte::leaf(entry.ppn(), to);
                } else {
                    let sub = self.split_mega(frame, index, level, prepared);
                    self.protect_range(
                        sub,
                        level - 1,
                        slot_base,
                        vpn_start,
                        vpn_end,
                        from,
                        to,
                        prepared,
                    );
                }
            } else {
                self.protect_range(
                    entry.next_frame(),
                    level - 1,
                    slot_base,
                    vpn_start,
                    vpn_end,
                    from,
                    to,
                    prepared,
                );
            }
        }
    }

    fn is_owned_root(&self, slot: usize) -> bool {
        self.owned_root[slot / 64] & (1u64 << (slot % 64)) != 0
    }

    fn set_owned_root(&mut self, slot: usize) {
        self.owned_root[slot / 64] |= 1u64 << (slot % 64);
    }

    fn clear_owned_root(&mut self, slot: usize) {
        self.owned_root[slot / 64] &= !(1u64 << (slot % 64));
    }

    fn is_shared_root(&self, slot: usize) -> bool {
        self.shared_root[slot / 64] & (1u64 << (slot % 64)) != 0
    }

    fn set_shared_root(&mut self, slot: usize) {
        self.shared_root[slot / 64] |= 1u64 << (slot % 64);
    }
}

impl<M: TableFrameMemory, const LEVELS: usize> Drop for TableTree<M, LEVELS> {
    fn drop(&mut self) {
        if self.root_owner.is_none() {
            return;
        }
        assert!(
            self.owned_root.iter().all(|word| *word == 0) && self.owners.is_empty(),
            "page-table tree dropped before explicit drain"
        );
    }
}

fn largest_segment_level<const LEVELS: usize>(vpn: usize, ppn: usize, end: usize) -> usize {
    for level in (1..LEVELS).rev() {
        let pages = pages_at(level);
        if vpn.is_multiple_of(pages) && ppn.is_multiple_of(pages) && end - vpn >= pages {
            return level;
        }
    }
    0
}

fn split_count_for_leaf(level: usize, slot_base: usize, vpn_start: usize, vpn_end: usize) -> usize {
    let slot_end = slot_base + pages_at(level);
    if slot_base >= vpn_start && slot_end <= vpn_end {
        return 0;
    }
    assert!(
        level > 0,
        "page-aligned range cannot partially cover a level-0 leaf"
    );
    let child_level = level - 1;
    let child_pages = pages_at(child_level);
    let mut count = 1usize;
    for index in 0..ENTRIES {
        let child_base = slot_base + index * child_pages;
        let child_end = child_base + child_pages;
        if child_base >= vpn_end || child_end <= vpn_start {
            continue;
        }
        count += split_count_for_leaf(child_level, child_base, vpn_start, vpn_end);
    }
    count
}

fn valid_leaf_flags(value: u64) -> bool {
    value & !0x3ff == 0
        && value & flags::V != 0
        && value & (flags::R | flags::W | flags::X) != 0
        && (value & flags::W == 0 || value & flags::R != 0)
}
