//! Sv39/48/57 页表纯逻辑（见 notes/impls/mm.md「页表纯逻辑」）。
//!
//! 本 crate 只管表结构：`TableTree` 经 `FrameMemory` 抽象访问表帧，
//! 不直接解引用物理地址，host 与内核 target 复用同一份代码。
//! 匿名内存整备（先取帧再映射）不属于本 crate，由内核 mm 层组合。

#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

use core::fmt;

/// 每张表的项数（RISC-V 标准 9 位索引）。
pub const ENTRIES: usize = 512;
/// 页偏移位数（4KiB 页）。
pub const PAGE_BITS: usize = 12;

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
            write!(f, "Pte(leaf ppn={:#x} flags={:#x})", self.ppn().0, self.flags())
        } else {
            write!(f, "Pte(branch frame={:#x})", self.next_frame().0)
        }
    }
}

// ---------------------------------------------------------------------------
// 帧访问抽象
// ---------------------------------------------------------------------------

/// 表帧的分配与访问抽象。
///
/// * [`FrameMemory::alloc_frame`] 返回的帧内容不作要求——`TableTree` 会在
///   使用前自行清零，host 实现只需提供存储。
/// * [`FrameMemory::table_mut`] 以帧号索引一张表。
pub trait FrameMemory {
    /// 分配一个表帧，失败表示内存耗尽。
    fn alloc_frame(&mut self) -> Result<FrameNumber, FrameExhausted>;
    /// 释放一个表帧（仅表帧；叶数据帧不经过本 crate）。
    fn free_frame(&mut self, frame: FrameNumber);
    /// 访问指定帧。
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

/// `map` 的错误。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MapError {
    /// 页号超出 `LEVELS` 级页表可表示范围。
    OutOfRange,
    /// 目标位置已有不兼容的有效映射（同 ppn 同 flags 才幂等）。
    Conflict { vpn: Vpn },
    /// 表帧耗尽。
    FrameExhausted,
}

impl From<FrameExhausted> for MapError {
    fn from(_: FrameExhausted) -> Self {
        Self::FrameExhausted
    }
}

// ---------------------------------------------------------------------------
// TableTree
// ---------------------------------------------------------------------------

/// 一棵页表树，const 泛型于级数：`TableTree<M, 3>` 即 sv39。
///
/// 树**拥有** root 与全部中间表帧；叶数据帧不属于树。
/// `unmap` 只解除映射，不回收空表（空表保留复用，整树销毁时统一释放）。
pub struct TableTree<M: FrameMemory, const LEVELS: usize> {
    mem: M,
    root: FrameNumber,
}

impl<M: FrameMemory, const LEVELS: usize> TableTree<M, LEVELS> {
    /// 建树：分配并清零 root。
    pub fn new(mut mem: M) -> Result<Self, FrameExhausted> {
        assert!(LEVELS >= 2 && LEVELS <= 5, "only sv39..sv57 supported (LEVELS 2..5)");
        let root = mem.alloc_frame()?;
        mem.table_mut(root).fill(Pte::invalid());
        Ok(Self { mem, root })
    }

    pub fn root_frame(&self) -> FrameNumber {
        self.root
    }

    /// 组装 satp 的 PPN 字段值（模式位由调用方拼）。
    pub fn satp_ppn(&self) -> usize {
        self.root.0
    }

    /// 映射 `[vpn, vpn+count)` 到连续物理页 `[ppn, ppn+count)`。
    ///
    /// 区域切段：每段取最大可行 mega 级（段首与 ppn 均对齐 512^l 页且
    /// 整段覆盖），段内单一路径；已存在映射同 ppn 同 flags 幂等成功，
    /// 否则 `Conflict`。
    pub fn map(&mut self, vpn: Vpn, count: usize, ppn: Ppn, flags: u64) -> Result<(), MapError> {
        let Some(end) = vpn.0.checked_add(count) else {
            return Err(MapError::OutOfRange);
        };
        if end > max_vpn(LEVELS) {
            return Err(MapError::OutOfRange);
        }

        let mut vpn_cur = vpn.0;
        let mut ppn_cur = ppn.0;
        while vpn_cur < end {
            // 选最大可行 mega 级
            let mut seg_level = 0;
            for level in (1..LEVELS).rev() {
                let pages = pages_at(level);
                if vpn_cur % pages == 0 && ppn_cur % pages == 0 && end - vpn_cur >= pages {
                    seg_level = level;
                    break;
                }
            }
            let seg_pages = pages_at(seg_level);
            self.map_segment(Vpn(vpn_cur), seg_level, Ppn(ppn_cur), flags)?;
            vpn_cur += seg_pages;
            ppn_cur += seg_pages;
        }
        Ok(())
    }

    /// 解除 `[vpn, vpn+count)` 的映射。
    ///
    /// 语义宽松：区间内未映射的部分跳过；跨 mega 部分覆盖的先分裂再解除。
    pub fn unmap(&mut self, vpn: Vpn, count: usize) -> Result<(), MapError> {
        let end = vpn.0.checked_add(count).ok_or(MapError::OutOfRange)?;
        if end > max_vpn(LEVELS) {
            return Err(MapError::OutOfRange);
        }
        self.unmap_range(self.root, LEVELS - 1, 0, vpn.0, end)
    }

    /// 查询 `vpn` 的映射。
    /// 底层表存储访问（内核建表路径与 host 测试构造外部子树用）。
    pub fn mem_mut(&mut self) -> &mut M {
        &mut self.mem
    }

    /// 清空 `frame` 表内 [start, end) 槽位：不递归、不归还任何子树帧。
    ///
    /// 用于剥离启动期拷贝进用户 root 的内核共享顶层项——这些子树归
    /// 内核所有，teardown（Drop 的 free_subtree）不得回收；先剥离，
    /// 递归释放就只触及用户部分。
    pub fn clear_slots(&mut self, frame: FrameNumber, start: usize, end: usize) {
        for i in start..end {
            self.mem.table_mut(frame)[i] = Pte::invalid();
        }
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

    /// 把 `seg_level` 粒度的一个段（段首 `vpn`，物理页 `ppn`）落下。
    /// 段的长度恰为 `pages_at(seg_level)`（由 `map` 的切段保证）。
    fn map_segment(
        &mut self,
        vpn: Vpn,
        seg_level: usize,
        ppn: Ppn,
        flags: u64,
    ) -> Result<(), MapError> {
        let mut frame = self.root;
        let mut level = LEVELS - 1;
        loop {
            let idx = vpn.index_at(level);
            let entry = self.mem.table_mut(frame)[idx];
            if level == seg_level {
                // 目标层级：写叶（或幂等校验）
                if !entry.is_valid() || (entry.is_leaf() && self.leaf_matches(entry, ppn, flags)) {
                    self.mem.table_mut(frame)[idx] = Pte::leaf(ppn, flags);
                    return Ok(());
                }
                // 分支（已有更细子树）或异 ppn/异 flags 的叶：冲突
                return Err(MapError::Conflict { vpn });
            }
            // 还未到目标层级
            if entry.is_branch() {
                frame = entry.next_frame();
                level -= 1;
                continue;
            }
            if entry.is_leaf() {
                if self.leaf_matches_at(entry, level, vpn, ppn, flags) {
                    // 已有同粒度同映射覆盖本段起点——幂等，整段视为已映射
                    return Ok(());
                }
                // 需要比现有叶更细的粒度：分裂后下钻
                frame = self.split_mega(frame, idx, level)?;
                level -= 1;
                continue;
            }
            // 无效：建分支下钻
            let new_frame = self.mem.alloc_frame()?;
            self.mem.table_mut(new_frame).fill(Pte::invalid());
            self.mem.table_mut(frame)[idx] = Pte::branch(new_frame);
            frame = new_frame;
            level -= 1;
        }
    }

    /// 叶项是否与目标（同层级同覆盖）完全一致（幂等判定）。
    fn leaf_matches(&self, entry: Pte, ppn: Ppn, flags: u64) -> bool {
        entry.is_leaf() && entry.ppn() == ppn && entry.flags() == flags
    }

    /// 高层级叶与细粒度目标的兼容判定：现有 mega 的覆盖范围在物理上
    /// 与目标连续映射一致且 flags 相同，才允许分裂为细粒度继续。
    fn leaf_matches_at(
        &self,
        entry: Pte,
        level: usize,
        vpn: Vpn,
        ppn: Ppn,
        flags: u64,
    ) -> bool {
        if !entry.is_leaf() || entry.flags() != flags {
            return false;
        }
        // mega 覆盖的物理基址
        let mega_pages = pages_at(level);
        let mega_ppn_base = entry.ppn().0 - entry.ppn().0 % mega_pages;
        // 目标页在该 mega 内的期望物理页
        let expect = ppn.0 - ppn.0 % mega_pages + vpn.0 % mega_pages;
        mega_ppn_base == expect
    }

    /// 分裂 `frame[idx]` 处的 level 级 mega 叶为 level-1 级表，
    /// 返回新表帧号。原 mega 的 flags 与物理连续性在 512 个子项中保持。
    fn split_mega(
        &mut self,
        frame: FrameNumber,
        idx: usize,
        level: usize,
    ) -> Result<FrameNumber, MapError> {
        debug_assert!(level >= 1, "level-0 leaf cannot split further");
        let entry = self.mem.table_mut(frame)[idx];
        debug_assert!(entry.is_leaf());

        let mega_pages = pages_at(level);
        let base_ppn = entry.ppn().0 - entry.ppn().0 % mega_pages;
        let flags = entry.flags();

        let new_frame = self.mem.alloc_frame()?;
        let table = self.mem.table_mut(new_frame);
        table.fill(Pte::invalid());
        let sub_pages = pages_at(level - 1);
        for i in 0..ENTRIES {
            table[i] = Pte::leaf(Ppn(base_ppn + i * sub_pages), flags);
        }
        self.mem.table_mut(frame)[idx] = Pte::branch(new_frame);
        Ok(new_frame)
    }

    /// 递归解除 `[vpn_start, vpn_end)` 在 `frame`（`level` 级表）内的映射。
    /// `table_base` 是该 frame 实际覆盖的首 VPN，递归时随子表推进，不能
    /// 从初始请求起点重新推导。
    fn unmap_range(
        &mut self,
        frame: FrameNumber,
        level: usize,
        table_base: usize,
        vpn_start: usize,
        vpn_end: usize,
    ) -> Result<(), MapError> {
        for i in 0..ENTRIES {
            let slot_base = table_base + i * pages_at(level);
            let slot_pages = pages_at(level);
            // 与目标区间相交？
            if slot_base >= vpn_end || slot_base + slot_pages <= vpn_start {
                continue;
            }
            let entry = self.mem.table_mut(frame)[i];
            if !entry.is_valid() {
                continue;
            }
            if entry.is_leaf() {
                if slot_base >= vpn_start && slot_base + slot_pages <= vpn_end {
                    // 整个 mega 被覆盖：直接解除
                    self.mem.table_mut(frame)[i] = Pte::invalid();
                } else {
                    // 部分覆盖：分裂后下钻；表帧耗尽必须返回错误，调用方
                    // 不得在 PTE 尚存时归还数据帧。
                    let sub = self.split_mega(frame, i, level)?;
                    self.unmap_range(sub, level - 1, slot_base, vpn_start, vpn_end)?;
                }
            } else {
                let sub = entry.next_frame();
                self.unmap_range(sub, level - 1, slot_base, vpn_start, vpn_end)?;
            }
        }
        Ok(())
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
        self.free_subtree(self.root, LEVELS - 1);
        self.mem.free_frame(self.root);
    }
}
