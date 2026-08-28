//! 物理帧池：in-band 有序空闲链（见 notes/impls/mm.md「帧池」）。
//!
//! 空闲区间按地址排序成单链，节点内嵌于区间首帧——空闲内存自身承载
//! 元数据，零堆依赖。本 crate 是纯逻辑，所有帧访问经 [`PoolMemory`]
//! 抽象，host 与内核 target 复用同一份代码。
//!
//! 节点内容是帧号而非地址，与地址转换解耦：内核从 bare 切到高半区
//! 直映射时，仅 `PoolMemory` 实现跟随转换函数，链结构零迁移。

#![no_std]
#![forbid(unsafe_code)]

use page_table::FrameNumber;

/// 链尾哨兵（帧号不可能达到 `usize::MAX`）。
const NONE: usize = usize::MAX;

/// 空闲区间节点，内嵌于区间首帧的前两个 usize。
///
/// 这是写进物理内存的数据布局：`next` 用 [`NONE`] 表链尾，不依赖
/// Rust 对 `Option` 的内部表示。区间为 `[首帧, 首帧 + len)`。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RegionNode {
    /// 区间帧数（含首帧自身）。
    pub len: usize,
    /// 下一个空闲区间的首帧号，[`NONE`] 为链尾。
    pub next: usize,
}

impl RegionNode {
    fn next_frame(&self) -> Option<FrameNumber> {
        if self.next == NONE {
            None
        } else {
            Some(FrameNumber(self.next))
        }
    }
}

/// 帧内存访问抽象：读写区间首帧的元数据槽、清零帧块。
///
/// 实现方保证：元数据槽（首帧前两个 usize）可读写；`clear_frames`
/// 把整块帧写零（覆盖其中一切旧元数据）。host 实现为模拟内存，
/// 内核实现为 `phys_to_virt` 后的裸访问。
pub trait PoolMemory {
    fn read_meta(&mut self, frame: FrameNumber) -> RegionNode;
    fn write_meta(&mut self, frame: FrameNumber, node: RegionNode);
    fn clear_frames(&mut self, base: FrameNumber, count: usize);
}

/// `alloc_at` 失败原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocAtError {
    /// 请求区间未整体落在任何空闲区间内（未注册或已分配）。
    Unavailable,
}

/// in-band 有序空闲链帧池。
///
/// `head` 与 [`RegionNode::next`] 构成地址严格递增的链；`free_frames`
/// 记账总空闲帧数。锁与全局容器由内核侧组合（`OnceLock<Spinlock<_>>`）。
pub struct FramePool<M: PoolMemory> {
    mem: M,
    head: Option<FrameNumber>,
    free_frames: usize,
}

impl<M: PoolMemory> FramePool<M> {
    pub fn new(mem: M) -> Self {
        Self {
            mem,
            head: None,
            free_frames: 0,
        }
    }

    /// 空闲帧总数。
    pub fn free_frames(&self) -> usize {
        self.free_frames
    }

    /// 消耗 pool 取回内存后端。
    ///
    /// 链结构全部落在物理帧内，后端无状态——地址转换函数变更
    /// （如内核 bare → 高半区切换）时用 `into_mem` + [`FramePool::new`]
    /// 重建即可，链零迁移。
    pub fn into_mem(self) -> M {
        self.mem
    }

    /// 空闲区间数（诊断用；遍历整链）。
    pub fn region_count(&mut self) -> usize {
        let mut n = 0;
        let mut cur = self.head;
        while let Some(f) = cur {
            n += 1;
            cur = self.mem.read_meta(f).next_frame();
        }
        n
    }

    /// 注册一段空闲区间 `[start, end)`（帧号，半开）。
    ///
    /// 启动期按 DTB 剔除后逐段调用；调用方保证区间互斥且不与已注册
    /// 区间重叠，重叠属板级配置错误，不在此校验。
    pub fn add_region(&mut self, start: FrameNumber, end: FrameNumber) {
        debug_assert!(start.0 < end.0, "empty region");
        self.insert_free(start, end.0 - start.0);
        self.free_frames += end.0 - start.0;
    }

    /// 分配 `count` 个物理连续帧，返回首帧；无足够连续区间时返回
    /// `None`。返回前整块清零。
    ///
    /// first-fit + 尾端切：取链上首个足够区间，从其尾部切出——
    /// 低地址区间先消耗，大区间主体保持完整。
    ///
    /// 当前消费者（堆 arena、页表帧）均无对齐需求，接口不设对齐参数；
    /// 如需对齐分配再在接口上扩展，勿在调用侧硬凑。
    pub fn alloc_contiguous(&mut self, count: usize) -> Option<FrameNumber> {
        debug_assert!(count > 0, "zero-frame allocation");
        let mut prev: Option<FrameNumber> = None;
        let mut cur = self.head?;
        loop {
            let node = self.mem.read_meta(cur);
            if node.len >= count {
                let base = FrameNumber(cur.0 + node.len - count);
                self.carve(prev, cur, base, count);
                self.mem.clear_frames(base, count);
                self.free_frames -= count;
                return Some(base);
            }
            match node.next_frame() {
                Some(next) => {
                    prev = Some(cur);
                    cur = next;
                }
                None => return None,
            }
        }
    }

    /// 取指定区间 `[base, base + count)`；区间不可用（未注册或部分
    /// 已分配）返回 [`AllocAtError::Unavailable`]。返回前整块清零。
    ///
    /// 供启动协议三件套与页表解映射回投等已知地址的分配。
    pub fn alloc_at(&mut self, base: FrameNumber, count: usize) -> Result<(), AllocAtError> {
        debug_assert!(count > 0, "zero-frame allocation");
        let end = base.0 + count;
        let mut prev: Option<FrameNumber> = None;
        let mut cur = self.head.ok_or(AllocAtError::Unavailable)?;
        loop {
            let node = self.mem.read_meta(cur);
            // 地址序保证：cur 起点已达或越过请求区间尾部 → 不可用
            if cur.0 >= end {
                return Err(AllocAtError::Unavailable);
            }
            if cur.0 <= base.0 && end <= cur.0 + node.len {
                self.carve(prev, cur, base, count);
                self.mem.clear_frames(base, count);
                self.free_frames -= count;
                return Ok(());
            }
            match node.next_frame() {
                Some(next) => {
                    prev = Some(cur);
                    cur = next;
                }
                None => return Err(AllocAtError::Unavailable),
            }
        }
    }

    /// 归还 `[base, base + count)`，按地址序插入并与相邻空闲区间合并。
    ///
    /// debug 断言拒绝与现有空闲区间重叠——归还区域必须是已分配
    /// 状态，重叠意味着双重释放或记账错误。
    pub fn dealloc(&mut self, base: FrameNumber, count: usize) {
        debug_assert!(count > 0, "zero-frame deallocation");
        self.insert_free(base, count);
        self.free_frames += count;
    }

    /// 有界归还：每调用至多消耗 `budget` 步链扫描定位插入位，找到后执行
    /// O(1) 插入并返回 `(消耗步数, true)`；预算耗尽则持久化游标并返回
    /// `(budget, false)`，下次续扫。游标恢复前 O(1) 校验邻接
    /// （prev.next == cur / head == cur）；他方归还使其失效时从链头重启，
    /// 本次仍受 budget 约束。完成插入计 1 步（保证每调用进展 ≥1）。
    pub fn dealloc_bounded(
        &mut self,
        base: FrameNumber,
        count: usize,
        scan: &mut FreeScan,
        budget: usize,
    ) -> (usize, bool) {
        debug_assert!(count > 0 && budget > 0, "zero-budget bounded dealloc");
        let (mut prev, mut cur) = if scan.started {
            let valid = match (scan.prev, scan.cur) {
                (Some(p), Some(c)) => self.mem.read_meta(p).next_frame() == Some(c),
                (Some(p), None) => self.mem.read_meta(p).next_frame().is_none(),
                (None, Some(c)) => self.head == Some(c),
                (None, None) => self.head.is_none(),
            };
            if valid { (scan.prev, scan.cur) } else { (None, self.head) }
        } else {
            (None, self.head)
        };
        scan.started = true;
        let end = base.0 + count;
        let mut steps = 0;
        while let Some(c) = cur {
            if steps >= budget {
                scan.prev = prev;
                scan.cur = cur;
                return (steps, false);
            }
            let node = self.mem.read_meta(c);
            debug_assert!(
                c.0 >= end || base.0 >= c.0 + node.len,
                "region [{:#x}, {:#x}) overlaps free region [{:#x}, {:#x}): double free or accounting bug",
                base.0,
                end,
                c.0,
                c.0 + node.len,
            );
            if c.0 > base.0 {
                break;
            }
            prev = Some(c);
            cur = node.next_frame();
            steps += 1;
        }
        // 预算恰好用在最后一跳：完成插入步无预算，持久化游标（已位于
        // 插入前位置）重入——否则完成返回 steps+1 超预算（work_done 违约
        // 超 max，用户侧校验拒绝）。
        if steps >= budget {
            scan.prev = prev;
            scan.cur = cur;
            return (steps, false);
        }
        self.insert_at(base, count, prev, cur);
        self.free_frames += count;
        (steps + 1, true)
    }

    /// 从区间 `cur`（前驱 `prev`）中切出 `[base, base + count)`，
    /// 左右残段各自成节点。
    ///
    /// `s = cur.0`，`e = s + node.len`，四种情形：
    /// - 中切（左残 + 右残）：cur 缩为左残，右残新建节点接在其后；
    /// - 仅左残：cur 原地缩短，链不变；
    /// - 仅右残：右残顶替 cur 的链位；
    /// - 整取：前驱跨过 cur。
    fn carve(
        &mut self,
        prev: Option<FrameNumber>,
        cur: FrameNumber,
        base: FrameNumber,
        count: usize,
    ) {
        let node = self.mem.read_meta(cur);
        let (s, e) = (cur.0, cur.0 + node.len);
        let has_left = base.0 > s;
        let has_right = base.0 + count < e;

        match (has_left, has_right) {
            (true, true) => {
                let right = base.0 + count;
                self.mem.write_meta(
                    cur,
                    RegionNode {
                        len: base.0 - s,
                        next: right,
                    },
                );
                self.mem.write_meta(
                    FrameNumber(right),
                    RegionNode {
                        len: e - right,
                        next: node.next,
                    },
                );
            }
            (true, false) => {
                self.mem.write_meta(
                    cur,
                    RegionNode {
                        len: base.0 - s,
                        next: node.next,
                    },
                );
            }
            (false, true) => {
                let right = base.0 + count;
                self.link(prev, Some(FrameNumber(right)));
                self.mem.write_meta(
                    FrameNumber(right),
                    RegionNode {
                        len: e - right,
                        next: node.next,
                    },
                );
            }
            (false, false) => {
                self.link(prev, node.next_frame());
            }
        }
    }

    /// 把 `frame` 接到 `prev` 之后（`prev` 为 `None` 时设为链头）。
    fn link(&mut self, prev: Option<FrameNumber>, frame: Option<FrameNumber>) {
        match (prev, frame) {
            (Some(p), Some(f)) => {
                let mut pn = self.mem.read_meta(p);
                pn.next = f.0;
                self.mem.write_meta(p, pn);
            }
            (Some(p), None) => {
                let mut pn = self.mem.read_meta(p);
                pn.next = NONE;
                self.mem.write_meta(p, pn);
            }
            (None, Some(f)) => self.head = Some(f),
            (None, None) => self.head = None,
        }
    }

    /// 把 `[base, base + count)` 以空闲区间插入链（地址序），并与
    /// 前后相邻区间合并。
    fn insert_free(&mut self, base: FrameNumber, count: usize) {
        let end = base.0 + count;
        let mut prev: Option<FrameNumber> = None;
        let mut cur = self.head;

        // 找插入位：链上首个 start > base 的区间（cur）及其前驱（prev）
        while let Some(c) = cur {
            let node = self.mem.read_meta(c);
            debug_assert!(
                c.0 >= end || base.0 >= c.0 + node.len,
                "region [{:#x}, {:#x}) overlaps free region [{:#x}, {:#x}): double free or accounting bug",
                base.0,
                end,
                c.0,
                c.0 + node.len,
            );
            if c.0 > base.0 {
                break;
            }
            prev = Some(c);
            cur = node.next_frame();
        }
        self.insert_at(base, count, prev, cur);
    }

    /// 在已定位的插入位（prev 前驱 / cur 后继）执行 O(1) 插入与合并。
    fn insert_at(
        &mut self,
        base: FrameNumber,
        count: usize,
        prev: Option<FrameNumber>,
        cur: Option<FrameNumber>,
    ) {
        let end = base.0 + count;
        // 前合并条件：prev 区间尾部紧贴 base 起点
        let merge_prev = match prev {
            Some(p) => {
                let pn = self.mem.read_meta(p);
                p.0 + pn.len == base.0
            }
            None => false,
        };
        // 后合并条件：cur 区间起点紧贴 base 尾部
        let cur_node = cur.map(|c| self.mem.read_meta(c));
        let merge_next = matches!(cur, Some(c) if c.0 == end);

        match (merge_prev, merge_next) {
            (true, true) => {
                // 三向：prev 吞并 base 与 cur
                let p = prev.unwrap();
                let mut pn = self.mem.read_meta(p);
                pn.len += count + cur_node.unwrap().len;
                pn.next = cur_node.unwrap().next;
                self.mem.write_meta(p, pn);
            }
            (true, false) => {
                let p = prev.unwrap();
                let mut pn = self.mem.read_meta(p);
                pn.len += count;
                self.mem.write_meta(p, pn);
            }
            (false, true) => {
                // base 吞并 cur，顶替其链位
                let node = RegionNode {
                    len: count + cur_node.unwrap().len,
                    next: cur_node.unwrap().next,
                };
                self.link(prev, Some(base));
                self.mem.write_meta(base, node);
            }
            (false, false) => {
                let node = RegionNode {
                    len: count,
                    next: match cur {
                        Some(c) => c.0,
                        None => NONE,
                    },
                };
                self.link(prev, Some(base));
                self.mem.write_meta(base, node);
            }
        }
    }
}

/// 有界归还（`dealloc_bounded`）的扫描游标：记录插入位定位的
/// (prev, cur)；`started == false` 表示尚未起步（首调用从链头开始）。
#[derive(Clone, Copy, Default)]
pub struct FreeScan {
    prev: Option<FrameNumber>,
    cur: Option<FrameNumber>,
    started: bool,
}
