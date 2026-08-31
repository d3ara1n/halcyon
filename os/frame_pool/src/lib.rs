//! 物理帧库存：启动期外置元数据上的分级 order 树（见 notes/ideas/mm.md）。
//!
//! 每个 DT memory region 被分解为全局对齐的 power-of-two arena。arena 用一棵
//! 完全二叉树记录各子树可提供的最大 order；分配沿树下降，归还沿祖先合并，
//! 操作步数只由地址位宽与 arena 数上限决定，不随运行期碎片数增长。
//!
//! 元数据由调用方从启动 reservation 提供，不落在被管理帧内。库存只维护哪些帧
//! 可用或已经 claim，不读取、写入或初始化被管理帧的内容。

#![no_std]
#![forbid(unsafe_code)]

use page_table::FrameNumber;

/// 单个 FramePool 最多容纳的对齐 arena 数。
///
/// 一个任意半开区间的 canonical power-of-two 分解不超过地址位宽的两倍；
/// 2048 覆盖内核最多 16 个 DT memory region 的结构上界。
pub const MAX_ARENAS: usize = 2048;

/// 树节点没有可分配块。其余值直接编码该子树可提供的最大 order。
const UNAVAILABLE: u8 = u8::MAX;

/// 每个托管帧需要两个树节点字节（完全二叉树的精确上界）。
pub const fn metadata_bytes(frame_count: usize) -> Option<usize> {
    frame_count.checked_mul(2)
}

/// 帧 extent 的纯几何；不表达或复制帧所有权。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ExtentGeometry {
    base: FrameNumber,
    count: usize,
}

impl ExtentGeometry {
    pub fn new(base: FrameNumber, count: usize) -> Option<Self> {
        (count > 0 && base.0.checked_add(count).is_some()).then_some(Self { base, count })
    }

    pub const fn base(self) -> FrameNumber {
        self.base
    }

    pub const fn count(self) -> usize {
        self.count
    }

    pub fn end(self) -> FrameNumber {
        FrameNumber(self.base.0 + self.count)
    }

    pub fn split_at(self, offset: usize) -> Option<(Self, Self)> {
        if offset == 0 || offset >= self.count {
            return None;
        }
        Some((
            Self {
                base: self.base,
                count: offset,
            },
            Self {
                base: FrameNumber(self.base.0 + offset),
                count: self.count - offset,
            },
        ))
    }
}

/// 注册托管物理区间失败。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddRegionError {
    InvalidRange,
    Overlap,
    ArenaLimit,
    MetadataExhausted,
}

/// `alloc_at` 失败原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocAtError {
    /// 请求区间未完整落在托管范围，或其中至少一帧不可用。
    Unavailable,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct ArenaMetadata {
    base: usize,
    metadata_start: usize,
    order: u8,
}

impl ArenaMetadata {
    pub const EMPTY: Self = Self {
        base: 0,
        order: 0,
        metadata_start: 0,
    };

    fn frames(self) -> usize {
        1usize << self.order
    }

    fn end(self) -> usize {
        self.base + self.frames()
    }

    fn metadata_len(self) -> usize {
        self.frames() * 2
    }
}

/// 外置元数据的分级物理帧库存。
///
/// `metadata` 与 `arenas` 必须在库存存活期间保持独占，并且不能来自随后加入
/// 本库存的空闲帧。内核侧从 DT memory 中先保留两者，再构造本对象。
pub struct FramePool<'a> {
    metadata: &'a mut [u8],
    metadata_used: usize,
    arenas: &'a mut [ArenaMetadata],
    arena_count: usize,
    free_frames: usize,
}

impl<'a> FramePool<'a> {
    pub fn new(metadata: &'a mut [u8], arenas: &'a mut [ArenaMetadata]) -> Self {
        assert!(
            !arenas.is_empty() && arenas.len() <= MAX_ARENAS,
            "invalid arena metadata capacity"
        );
        metadata.fill(UNAVAILABLE);
        arenas.fill(ArenaMetadata::EMPTY);
        Self {
            metadata,
            metadata_used: 0,
            arenas,
            arena_count: 0,
            free_frames: 0,
        }
    }

    /// 空闲帧总数。
    pub fn free_frames(&self) -> usize {
        self.free_frames
    }

    /// 当前 canonical arena 数（诊断与结构上界测试用）。
    pub fn arena_count(&self) -> usize {
        self.arena_count
    }

    /// 注册一段托管区间，初始全部为 unavailable。
    ///
    /// 用于先覆盖完整 DT memory，再按启动 reservation 的补集调用
    /// [`Self::release_range`] 发布空闲帧。区间必须页对齐后换算为帧号，且与既有
    /// 托管区间不重叠。
    pub fn add_managed_region(
        &mut self,
        start: FrameNumber,
        end: FrameNumber,
    ) -> Result<(), AddRegionError> {
        if start.0 >= end.0 {
            return Err(AddRegionError::InvalidRange);
        }
        if self.arenas[..self.arena_count]
            .iter()
            .any(|arena| start.0 < arena.end() && arena.base < end.0)
        {
            return Err(AddRegionError::Overlap);
        }

        let frames = end.0 - start.0;
        let arena_need = block_count(start.0, end.0);
        if self.arena_count + arena_need > self.arenas.len() {
            return Err(AddRegionError::ArenaLimit);
        }
        let metadata_need = metadata_bytes(frames).ok_or(AddRegionError::MetadataExhausted)?;
        if self.metadata_used + metadata_need > self.metadata.len() {
            return Err(AddRegionError::MetadataExhausted);
        }

        for_each_block(start.0, end.0, |base, order| {
            let arena = ArenaMetadata {
                base,
                order: order as u8,
                metadata_start: self.metadata_used,
            };
            let metadata_end = self.metadata_used + arena.metadata_len();
            self.metadata[self.metadata_used..metadata_end].fill(UNAVAILABLE);
            self.metadata_used = metadata_end;
            self.arenas[self.arena_count] = arena;
            self.arena_count += 1;
        });
        Ok(())
    }

    /// 注册一段立即可用的托管区间。主要供 host 测试与无 reservation 的平台使用。
    pub fn add_region(
        &mut self,
        start: FrameNumber,
        end: FrameNumber,
    ) -> Result<(), AddRegionError> {
        self.add_managed_region(start, end)?;
        self.release_range(start, end)
            .expect("newly managed frame range must be releasable");
        Ok(())
    }

    /// 分配 `2^order` 个物理连续且同阶对齐的帧。
    pub fn alloc_order(&mut self, order: usize) -> Option<FrameNumber> {
        let count = order_size(order)?;
        let arena_index = (0..self.arena_count).find(|&index| {
            let arena = self.arenas[index];
            arena.order as usize >= order && state_available(self.state(arena, 1), order)
        })?;
        let arena = self.arenas[arena_index];
        let base = self.take_any(arena, order);
        self.free_frames -= count;
        Some(FrameNumber(base))
    }

    /// 在不超过 `max_count` 的约束下分配当前可用的最大 power-of-two extent。
    ///
    /// 只扫描固定上限 arena 一次，再沿选中树下降；不会按 order 重扫库存。
    pub fn alloc_largest(&mut self, max_count: usize) -> Option<(FrameNumber, usize)> {
        if max_count == 0 {
            return None;
        }
        let requested_order = floor_order(max_count);
        let mut choice: Option<(usize, usize)> = None;
        for index in 0..self.arena_count {
            let arena = self.arenas[index];
            let state = self.state(arena, 1);
            if state == UNAVAILABLE {
                continue;
            }
            let order = (state as usize).min(requested_order);
            if choice.is_none_or(|(_, best)| order > best) {
                choice = Some((index, order));
            }
        }
        let (arena_index, order) = choice?;
        let arena = self.arenas[arena_index];
        let base = self.take_any(arena, order);
        let count = 1usize << order;
        self.free_frames -= count;
        Some((FrameNumber(base), count))
    }

    /// 精确取指定区间。请求可跨 arena，但必须完整托管且当前全部空闲。
    /// 预验证先于修改，失败不改变库存。
    pub fn alloc_at(&mut self, base: FrameNumber, count: usize) -> Result<(), AllocAtError> {
        if count == 0 {
            return Err(AllocAtError::Unavailable);
        }
        let end = base.0.checked_add(count).ok_or(AllocAtError::Unavailable)?;
        if !self.range_is_free(base.0, end) {
            return Err(AllocAtError::Unavailable);
        }
        self.take_range(base.0, end);
        self.free_frames -= count;
        Ok(())
    }

    /// 归还一段已分配或启动期保留的物理区间。
    ///
    /// 任意长度区间先做 canonical power-of-two 分解，每块归还沿树至多更新
    /// `usize::BITS` 层。debug 构建拒绝与现有空闲块重叠。
    pub fn dealloc(&mut self, base: FrameNumber, count: usize) {
        assert!(count > 0, "zero-frame deallocation");
        let end = base.0.checked_add(count).expect("frame range overflow");
        assert!(self.range_is_managed(base.0, end), "unmanaged frame range");
        assert!(
            self.range_is_unavailable(base.0, end),
            "frame range [{:#x}, {:#x}) overlaps free inventory: double free or accounting bug",
            base.addr(),
            FrameNumber(end).addr()
        );
        self.release_blocks(base.0, end);
        self.free_frames += count;
    }

    /// 发布一段启动期 reservation。语义与 dealloc 相同，但名称显式区分来源。
    pub fn release_range(
        &mut self,
        start: FrameNumber,
        end: FrameNumber,
    ) -> Result<(), AllocAtError> {
        if start.0 >= end.0 || !self.range_is_managed(start.0, end.0) {
            return Err(AllocAtError::Unavailable);
        }
        if !self.range_is_unavailable(start.0, end.0) {
            return Err(AllocAtError::Unavailable);
        }
        self.release_blocks(start.0, end.0);
        self.free_frames += end.0 - start.0;
        Ok(())
    }

    fn state(&self, arena: ArenaMetadata, node: usize) -> u8 {
        self.metadata[arena.metadata_start + node]
    }

    fn set_state(&mut self, arena: ArenaMetadata, node: usize, state: u8) {
        self.metadata[arena.metadata_start + node] = state;
    }

    fn order_at(arena: ArenaMetadata, node: usize) -> usize {
        arena.order as usize - floor_order(node)
    }

    fn update_ancestors(&mut self, arena: ArenaMetadata, mut node: usize) {
        while node > 1 {
            node /= 2;
            let child_order = Self::order_at(arena, node) - 1;
            let left = self.state(arena, node * 2);
            let right = self.state(arena, node * 2 + 1);
            let state = if left as usize == child_order && right as usize == child_order {
                (child_order + 1) as u8
            } else {
                max_available(left, right)
            };
            self.set_state(arena, node, state);
        }
    }

    fn split_whole(&mut self, arena: ArenaMetadata, node: usize, order: usize) {
        debug_assert!(order > 0 && self.state(arena, node) as usize == order);
        self.set_state(arena, node * 2, (order - 1) as u8);
        self.set_state(arena, node * 2 + 1, (order - 1) as u8);
    }

    fn take_any(&mut self, arena: ArenaMetadata, target_order: usize) -> usize {
        let mut node = 1usize;
        let mut order = arena.order as usize;
        let mut base = arena.base;
        while order > target_order {
            if self.state(arena, node) as usize == order {
                self.split_whole(arena, node, order);
            }
            let child_order = order - 1;
            let left = node * 2;
            if state_available(self.state(arena, left), target_order) {
                node = left;
            } else {
                node = left + 1;
                base += 1usize << child_order;
            }
            order = child_order;
        }
        debug_assert_eq!(self.state(arena, node) as usize, target_order);
        self.set_state(arena, node, UNAVAILABLE);
        self.update_ancestors(arena, node);
        base
    }

    fn take_at_block(&mut self, arena: ArenaMetadata, base: usize, target_order: usize) {
        let mut node = 1usize;
        let mut order = arena.order as usize;
        while order > target_order {
            if self.state(arena, node) as usize == order {
                self.split_whole(arena, node, order);
            }
            order -= 1;
            let right = ((base - arena.base) & (1usize << order)) != 0;
            node = node * 2 + usize::from(right);
        }
        debug_assert_eq!(self.state(arena, node) as usize, target_order);
        self.set_state(arena, node, UNAVAILABLE);
        self.update_ancestors(arena, node);
    }

    fn block_is_free(&self, arena: ArenaMetadata, base: usize, target_order: usize) -> bool {
        let mut node = 1usize;
        let mut order = arena.order as usize;
        loop {
            let state = self.state(arena, node);
            if state as usize == order {
                return true;
            }
            if state == UNAVAILABLE || order == target_order {
                return false;
            }
            order -= 1;
            let right = ((base - arena.base) & (1usize << order)) != 0;
            node = node * 2 + usize::from(right);
        }
    }

    fn block_has_free(&self, arena: ArenaMetadata, base: usize, target_order: usize) -> bool {
        let mut node = 1usize;
        let mut order = arena.order as usize;
        loop {
            let state = self.state(arena, node);
            if state as usize == order {
                return true;
            }
            if state == UNAVAILABLE {
                return false;
            }
            if order == target_order {
                return true;
            }
            order -= 1;
            let right = ((base - arena.base) & (1usize << order)) != 0;
            node = node * 2 + usize::from(right);
        }
    }

    fn release_block(&mut self, arena: ArenaMetadata, base: usize, order: usize) {
        debug_assert!(!self.block_has_free(arena, base, order));
        let mut node = 1usize;
        let mut current_order = arena.order as usize;
        while current_order > order {
            let left = node * 2;
            if self.state(arena, node) == UNAVAILABLE {
                // 整块分配只标记父节点；后代可能保留更早碎片化留下的状态。
                // 部分归还前沿目标路径把占用事实向下物化，不能把陈旧兄弟当空闲。
                self.set_state(arena, left, UNAVAILABLE);
                self.set_state(arena, left + 1, UNAVAILABLE);
            }
            current_order -= 1;
            let right = ((base - arena.base) & (1usize << current_order)) != 0;
            node = left + usize::from(right);
        }
        debug_assert_eq!(self.state(arena, node), UNAVAILABLE);
        self.set_state(arena, node, order as u8);
        self.update_ancestors(arena, node);
    }

    fn find_arena(&self, frame: usize) -> Option<ArenaMetadata> {
        self.arenas[..self.arena_count]
            .iter()
            .copied()
            .find(|arena| frame >= arena.base && frame < arena.end())
    }

    fn range_is_managed(&self, mut start: usize, end: usize) -> bool {
        while start < end {
            let Some(arena) = self.find_arena(start) else {
                return false;
            };
            start = end.min(arena.end());
        }
        true
    }

    fn range_is_free(&self, mut start: usize, end: usize) -> bool {
        while start < end {
            let Some(arena) = self.find_arena(start) else {
                return false;
            };
            let segment_end = end.min(arena.end());
            let mut available = true;
            for_each_block(start, segment_end, |base, order| {
                available &= self.block_is_free(arena, base, order);
            });
            if !available {
                return false;
            }
            start = segment_end;
        }
        true
    }

    fn range_is_unavailable(&self, mut start: usize, end: usize) -> bool {
        while start < end {
            let Some(arena) = self.find_arena(start) else {
                return false;
            };
            let segment_end = end.min(arena.end());
            let mut unavailable = true;
            for_each_block(start, segment_end, |base, order| {
                unavailable &= !self.block_has_free(arena, base, order);
            });
            if !unavailable {
                return false;
            }
            start = segment_end;
        }
        true
    }

    fn take_range(&mut self, mut start: usize, end: usize) {
        while start < end {
            let arena = self.find_arena(start).expect("validated managed range");
            let segment_end = end.min(arena.end());
            for_each_block(start, segment_end, |base, order| {
                self.take_at_block(arena, base, order);
            });
            start = segment_end;
        }
    }

    fn release_blocks(&mut self, mut start: usize, end: usize) {
        while start < end {
            let arena = self.find_arena(start).expect("validated managed range");
            let segment_end = end.min(arena.end());
            for_each_block(start, segment_end, |base, order| {
                self.release_block(arena, base, order);
            });
            start = segment_end;
        }
    }
}

fn state_available(state: u8, order: usize) -> bool {
    state != UNAVAILABLE && state as usize >= order
}

fn max_available(left: u8, right: u8) -> u8 {
    match (left, right) {
        (UNAVAILABLE, value) | (value, UNAVAILABLE) => value,
        (a, b) => a.max(b),
    }
}

fn order_size(order: usize) -> Option<usize> {
    1usize.checked_shl(order as u32)
}

fn floor_order(value: usize) -> usize {
    debug_assert!(value > 0);
    (usize::BITS - 1 - value.leading_zeros()) as usize
}

fn block_count(start: usize, end: usize) -> usize {
    let mut count = 0;
    for_each_block(start, end, |_, _| count += 1);
    count
}

fn for_each_block(mut start: usize, end: usize, mut emit: impl FnMut(usize, usize)) {
    debug_assert!(start < end);
    while start < end {
        let align_order = if start == 0 {
            usize::BITS as usize - 1
        } else {
            start.trailing_zeros() as usize
        };
        let length_order = floor_order(end - start);
        let order = align_order.min(length_order);
        emit(start, order);
        start += 1usize << order;
    }
}
