//! Sv39/48/57 页表纯逻辑（见 notes/impls/mm.md「页表纯逻辑」）。
//!
//! 本 crate 只管表结构：`TableTree` 经 `FrameMemory` 抽象访问表帧，
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

/// Commit 前唯一持有一张已清零表帧的 affine token。
pub trait ReservedTableFrame {
    fn number(&self) -> FrameNumber;
    fn commit(self) -> FrameNumber;
}

/// 表帧访问与 reservation 来源。树只拥有已 Commit 到分支 PTE 的表帧。
pub trait FrameMemory {
    type ReservedFrame: ReservedTableFrame;

    /// 取得一张尚未发布的表帧；token 丢弃时必须自动归还。
    fn reserve_frame(&mut self) -> Result<Self::ReservedFrame, FrameExhausted>;
    /// 释放已从树中摘除的表帧。
    fn free_frame(&mut self, frame: FrameNumber);
    /// 访问指定表帧。
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RootSlotState {
    Empty,
    Shared,
    Leaf,
    Branch(FrameNumber),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlotState {
    Empty,
    Leaf,
    Branch(FrameNumber),
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
pub struct PreparedTranslation<F: ReservedTableFrame> {
    plan: TranslationPlan,
    frames: Vec<F>,
}

impl<F: ReservedTableFrame> PreparedTranslation<F> {
    pub fn reserved_frames(&self) -> usize {
        self.frames.len()
    }

    fn take_frame(&mut self) -> FrameNumber {
        self.frames
            .pop()
            .expect("translation preflight undercounted table frames")
            .commit()
    }
}

// ---------------------------------------------------------------------------
// 未发布树的 eager builder
// ---------------------------------------------------------------------------

/// 为尚未发布的页表树建立 eager range mapping。
///
/// 每段映射先完成范围与冲突验证，再自动选择尽可能大的叶级发布。中间表由
/// [`FrameMemory`] 提供，因此调用方可以使用固定静态预算而不依赖堆。若表帧在发布
/// 中耗尽，树可能已部分构造；该树仍未发布，调用方必须丢弃或终止启动。
pub struct EagerMapper<'a, M: FrameMemory, const LEVELS: usize> {
    mem: &'a mut M,
    root: FrameNumber,
}

impl<'a, M: FrameMemory, const LEVELS: usize> EagerMapper<'a, M, LEVELS> {
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
/// 树拥有 root 和全部非 shared 分支表帧；叶数据帧永不属于树。shared root
/// 槽由显式位图标记，Drop 与有界 drain 都不会递归进入这些外部子树。
pub struct TableTree<M: FrameMemory, const LEVELS: usize> {
    mem: M,
    root: FrameNumber,
    shared_root: [u64; ENTRIES / 64],
    owned_root: [u64; ENTRIES / 64],
    root_owned: bool,
}

impl<M: FrameMemory, const LEVELS: usize> TableTree<M, LEVELS> {
    /// 建树：root 在构造期经 reservation 取得并清零。
    pub fn new(mut mem: M) -> Result<Self, FrameExhausted> {
        assert!(
            LEVELS >= 2 && LEVELS <= 5,
            "only sv39..sv57 supported (LEVELS 2..5)"
        );
        let reserved = mem.reserve_frame()?;
        let root = reserved.number();
        assert!(root.0 < MAX_PPN, "root frame exceeds PTE encoding");
        mem.table_mut(root).fill(Pte::invalid());
        let root = reserved.commit();
        Ok(Self {
            mem,
            root,
            shared_root: [0; ENTRIES / 64],
            owned_root: [0; ENTRIES / 64],
            root_owned: true,
        })
    }

    pub fn root_frame(&self) -> FrameNumber {
        self.root
    }

    /// 组装 satp 的 PPN 字段值（模式位由调用方拼）。
    pub fn satp_ppn(&self) -> usize {
        self.root.0
    }

    /// Validate 映射意图并精确预留 Publish 所需的全部中间表帧。
    pub fn prepare_map(
        &mut self,
        vpn: Vpn,
        count: usize,
        ppn: Ppn,
        flags: u64,
    ) -> Result<PreparedTranslation<M::ReservedFrame>, MapError> {
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

        Ok(PreparedTranslation {
            plan: TranslationPlan::Map {
                vpn,
                count,
                ppn,
                flags,
            },
            frames: self.reserve_frames(missing.len())?,
        })
    }

    /// Validate 宽松 Unmap，并只为实际发生的部分 mega split 预留表帧。
    pub fn prepare_unmap(
        &mut self,
        vpn: Vpn,
        count: usize,
    ) -> Result<PreparedTranslation<M::ReservedFrame>, MapError> {
        let end = self.validate_range(vpn, count)?;
        let required = self.preflight_range(self.root, LEVELS - 1, 0, vpn.0, end, None)?;
        Ok(PreparedTranslation {
            plan: TranslationPlan::Unmap { vpn, count },
            frames: self.reserve_frames(required)?,
        })
    }

    /// Validate Protect：目标必须完整映射且当前 flags 全部等于 `from`。
    pub fn prepare_protect(
        &mut self,
        vpn: Vpn,
        count: usize,
        from: u64,
        to: u64,
    ) -> Result<PreparedTranslation<M::ReservedFrame>, MapError> {
        if !valid_leaf_flags(from) || !valid_leaf_flags(to) {
            return Err(MapError::InvalidFlags);
        }
        let end = self.validate_range(vpn, count)?;
        let required = self.preflight_range(self.root, LEVELS - 1, 0, vpn.0, end, Some(from))?;
        Ok(PreparedTranslation {
            plan: TranslationPlan::Protect {
                vpn,
                count,
                from,
                to,
            },
            frames: self.reserve_frames(required)?,
        })
    }

    /// Commit 后 Publish：只消费已清零 reservation 并写 PTE，不分配且不可失败。
    pub fn publish(&mut self, mut prepared: PreparedTranslation<M::ReservedFrame>) {
        match prepared.plan {
            TranslationPlan::Map {
                vpn,
                count,
                ppn,
                flags,
            } => self.publish_map(vpn, count, ppn, flags, &mut prepared),
            TranslationPlan::Unmap { vpn, count } => {
                let end = vpn.0 + count;
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
        // 其它先发布的非重叠变更可能已建立共享路径；剩余 token 随
        // prepared 在此处 Drop 并自动归还。
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
        Ok(())
    }

    pub fn root_slot_state(&mut self, slot: usize) -> RootSlotState {
        assert!(slot < ENTRIES, "root slot out of range");
        if self.is_shared_root(slot) {
            return RootSlotState::Shared;
        }
        if !self.is_owned_root(slot) {
            assert!(
                !self.mem.table_mut(self.root)[slot].is_valid(),
                "unowned root slot contains a valid PTE"
            );
            return RootSlotState::Empty;
        }
        match slot_state(self.mem.table_mut(self.root)[slot]) {
            SlotState::Empty => panic!("owned root slot contains an invalid PTE"),
            SlotState::Leaf => RootSlotState::Leaf,
            SlotState::Branch(frame) => RootSlotState::Branch(frame),
        }
    }

    pub fn slot_state(&mut self, frame: FrameNumber, slot: usize) -> SlotState {
        assert!(slot < ENTRIES, "table slot out of range");
        slot_state(self.mem.table_mut(frame)[slot])
    }

    /// 摘除非 root 表中的 branch；叶与空槽保持不动。
    pub fn detach_branch(&mut self, frame: FrameNumber, slot: usize) -> Option<FrameNumber> {
        let entry = self.mem.table_mut(frame)[slot];
        if !entry.is_branch() {
            return None;
        }
        self.mem.table_mut(frame)[slot] = Pte::invalid();
        Some(entry.next_frame())
    }

    /// 摘除一个 owned root 槽；shared 槽不可经此入口处理。
    pub fn detach_root_slot(&mut self, slot: usize) -> Option<FrameNumber> {
        assert!(!self.is_shared_root(slot), "cannot detach shared root slot");
        let entry = self.mem.table_mut(self.root)[slot];
        self.mem.table_mut(self.root)[slot] = Pte::invalid();
        self.clear_owned_root(slot);
        entry.is_branch().then(|| entry.next_frame())
    }

    /// 有界 drain 已摘除全部 owned root 槽后，交出 root 帧。
    pub fn finish_drain(mut self) -> FrameNumber {
        assert!(
            self.owned_root.iter().all(|word| *word == 0),
            "owned root slot remains at drain completion"
        );
        self.root_owned = false;
        self.root
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

    fn reserve_frames(&mut self, count: usize) -> Result<Vec<M::ReservedFrame>, MapError> {
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(count)
            .map_err(|_| MapError::AllocationFailed)?;
        for _ in 0..count {
            let reserved = self.mem.reserve_frame()?;
            let frame = reserved.number();
            assert!(
                frame.0 < MAX_PPN,
                "reserved table frame exceeds PTE encoding"
            );
            self.mem.table_mut(frame).fill(Pte::invalid());
            frames.push(reserved);
        }
        Ok(frames)
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
            let entry = self.mem.table_mut(frame)[vpn.index_at(level)];
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
        prepared: &mut PreparedTranslation<M::ReservedFrame>,
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
        prepared: &mut PreparedTranslation<M::ReservedFrame>,
    ) {
        let mut frame = self.root;
        let mut level = LEVELS - 1;
        loop {
            let idx = vpn.index_at(level);
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
            let new_frame = prepared.take_frame();
            self.mem.table_mut(frame)[idx] = Pte::branch(new_frame);
            frame = new_frame;
            level -= 1;
        }
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
        prepared: &mut PreparedTranslation<M::ReservedFrame>,
    ) -> FrameNumber {
        assert!(level >= 1, "level-0 leaf cannot split further");
        let entry = self.mem.table_mut(frame)[idx];
        assert!(entry.is_leaf(), "only a leaf can be split");
        let mega_pages = pages_at(level);
        let base_ppn = entry.ppn().0 - entry.ppn().0 % mega_pages;
        let flags = entry.flags();
        let new_frame = prepared.take_frame();
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

    fn unmap_range(
        &mut self,
        frame: FrameNumber,
        level: usize,
        table_base: usize,
        vpn_start: usize,
        vpn_end: usize,
        prepared: &mut PreparedTranslation<M::ReservedFrame>,
    ) {
        let slot_pages = pages_at(level);
        for index in 0..ENTRIES {
            let slot_base = table_base + index * slot_pages;
            let slot_end = slot_base + slot_pages;
            if slot_base >= vpn_end || slot_end <= vpn_start {
                continue;
            }
            let entry = self.mem.table_mut(frame)[index];
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
                    self.unmap_range(sub, level - 1, slot_base, vpn_start, vpn_end, prepared);
                }
            } else {
                self.unmap_range(
                    entry.next_frame(),
                    level - 1,
                    slot_base,
                    vpn_start,
                    vpn_end,
                    prepared,
                );
            }
        }
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
        prepared: &mut PreparedTranslation<M::ReservedFrame>,
    ) {
        let slot_pages = pages_at(level);
        for index in 0..ENTRIES {
            let slot_base = table_base + index * slot_pages;
            let slot_end = slot_base + slot_pages;
            if slot_base >= vpn_end || slot_end <= vpn_start {
                continue;
            }
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

    /// 递归释放 `frame` 子树中全部分支表帧（不含 `frame` 自身）。
    fn free_subtree(&mut self, frame: FrameNumber, level: usize) {
        if level == 0 {
            return;
        }
        // 按索引逐项读取：整表按值拷贝会在栈上生成 4KiB 数组，debug 构建
        // 下函数栈帧超过每 hart 栈预算，内核栈溢出踩踏相邻 hart 栈。
        for i in 0..ENTRIES {
            let entry = self.mem.table_mut(frame)[i];
            if entry.is_branch() {
                let sub = entry.next_frame();
                self.free_subtree(sub, level - 1);
                self.mem.free_frame(sub);
            }
        }
    }
}

impl<M: FrameMemory, const LEVELS: usize> Drop for TableTree<M, LEVELS> {
    fn drop(&mut self) {
        if !self.root_owned {
            return;
        }
        for slot in 0..ENTRIES {
            if !self.is_owned_root(slot) {
                continue;
            }
            let entry = self.mem.table_mut(self.root)[slot];
            if entry.is_branch() {
                let sub = entry.next_frame();
                self.free_subtree(sub, LEVELS - 2);
                self.mem.free_frame(sub);
            }
        }
        self.mem.free_frame(self.root);
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

fn slot_state(entry: Pte) -> SlotState {
    if entry.is_branch() {
        SlotState::Branch(entry.next_frame())
    } else if entry.is_leaf() {
        SlotState::Leaf
    } else {
        SlotState::Empty
    }
}

fn valid_leaf_flags(value: u64) -> bool {
    value & !0x3ff == 0
        && value & flags::V != 0
        && value & (flags::R | flags::W | flags::X) != 0
        && (value & flags::W == 0 || value & flags::R != 0)
}
