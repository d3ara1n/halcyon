//! Backing 区间持有：按逻辑页 offset 分段的物理/资金 owner 与 View 的区间 use。
//!
//! 真值是段：段不跨物理连续范围与资金凭证边界，记录 `[start,end)`、真实 owner
//! 与覆盖 use 数。View 持不可复制的 [`RangeUse`]；释放时逐段推进，零 use 段
//! 原样摘出交给调用方退款，期间不分配。切段所需节点必须事先经
//! [`SegmentMap::reserve`] 预取（[`NodeReserve`]），不足时在改动前失败。
//!
//! 索引为 [`OrdMap`]（arena AVL，按段起点），定位 O(log 段数)；节点经
//! [`SegmentMap::reserve`] 预取，切段与摘段不分配。

use crate::ordmap::OrdMap;

/// 段 owner：真实物理 extent 与资金凭证。切分必须同时切物理与 charge，守恒。
pub trait SegmentOwner: Sized {
    /// 在 `pages` 处切成前后两半；`0 < pages < 当前页数` 由调用方保证。
    fn split_at(self, pages: u64) -> (Self, Self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackingError {
    /// 预取节点失败（OOM）。
    NoMemory,
    /// 预取的节点不足以完成本次切界。
    ReserveExhausted,
    /// 区间为空、越界或未对齐到现有段集合。
    InvalidRange,
    /// use 不属于本 map。
    ForeignUse,
    /// 已开始最终收束，不再接受新 use。
    Retiring,
}

/// 切段节点的预取额度；只能由同一 map 消费。
#[derive(Debug)]
pub struct NodeReserve {
    map: u64,
    credits: usize,
}

impl NodeReserve {
    pub fn credits(&self) -> usize {
        self.credits
    }
}

/// 覆盖 `[lo,hi)` 逻辑页的业务 use；不可复制，只能经 [`SegmentMap::begin_release`] 消费。
#[derive(Debug, PartialEq, Eq)]
#[must_use = "a range use must be released through its segment map"]
pub struct RangeUse {
    map: u64,
    lo: u64,
    hi: u64,
}

impl RangeUse {
    pub fn range(&self) -> (u64, u64) {
        (self.lo, self.hi)
    }
}

/// 释放游标：每次 [`SegmentMap::release_step`] 处理一个段。
#[derive(Debug)]
#[must_use = "release must be driven to Done"]
pub struct ReleaseCursor {
    map: u64,
    next: u64,
    hi: u64,
}

/// 单步释放结果。
#[derive(Debug, PartialEq, Eq)]
pub enum ReleaseStep<O> {
    /// 该段 use 归零，已从 map 摘出；调用方先还物理再退份额。
    Detached { start: u64, end: u64, owner: O },
    /// 该段仍被其他 use 覆盖，仅递减。
    Retained,
    /// 区间已全部处理。
    Done,
}

struct Segment<O> {
    end: u64,
    uses: u32,
    owner: O,
}

/// 单个 Backing 的段集合，按段起点索引。
pub struct SegmentMap<O> {
    id: u64,
    segs: OrdMap<u64, Segment<O>>,
    pages: u64,
    retiring: bool,
}

impl<O: SegmentOwner> SegmentMap<O> {
    /// `id` 由调用方保证在 use 可能交叉的范围内唯一。
    pub fn new(id: u64) -> Self {
        Self {
            id,
            segs: OrdMap::new(),
            pages: 0,
            retiring: false,
        }
    }

    pub fn pages(&self) -> u64 {
        self.pages
    }

    pub fn segment_count(&self) -> usize {
        self.segs.len()
    }

    /// 预取 `nodes` 个切段节点。只在准备阶段调用。
    pub fn reserve(&mut self, nodes: usize) -> Result<NodeReserve, BackingError> {
        self.segs
            .reserve(nodes)
            .map_err(|_| BackingError::NoMemory)?;
        Ok(NodeReserve {
            map: self.id,
            credits: nodes,
        })
    }

    /// 退还未用完的预取额度。
    pub fn unreserve(&mut self, reserve: NodeReserve) {
        debug_assert_eq!(reserve.map, self.id);
        self.segs.unreserve(reserve.credits);
    }

    /// 构造期在尾部追加一个物理 extent，`pages > 0`；初始 use 数为 0。
    pub fn push_extent(
        &mut self,
        pages: u64,
        owner: O,
        reserve: &mut NodeReserve,
    ) -> Result<(), BackingError> {
        if pages == 0 {
            return Err(BackingError::InvalidRange);
        }
        Self::spend(self.id, reserve)?;
        let start = self.pages;
        self.segs.insert(
            start,
            Segment {
                end: start + pages,
                uses: 0,
                owner,
            },
        );
        self.pages += pages;
        Ok(())
    }

    /// 取得覆盖 `[lo,hi)` 的 use：在两端切界（各至多消耗一个节点），覆盖段 use+1。
    /// 失败时 map 不变。工作量 O(覆盖段数 · log 段数)。
    pub fn acquire(
        &mut self,
        lo: u64,
        hi: u64,
        reserve: &mut NodeReserve,
    ) -> Result<RangeUse, BackingError> {
        if reserve.map != self.id {
            return Err(BackingError::ForeignUse);
        }
        if self.retiring {
            return Err(BackingError::Retiring);
        }
        if lo >= hi || hi > self.pages || !self.covered(lo, hi) {
            return Err(BackingError::InvalidRange);
        }
        let need = usize::from(self.needs_cut(lo)) + usize::from(self.needs_cut(hi));
        if reserve.credits < need {
            return Err(BackingError::ReserveExhausted);
        }
        self.cut(lo, reserve);
        self.cut(hi, reserve);
        let mut at = lo;
        while at < hi {
            let seg = self
                .segs
                .get_mut(&at)
                .expect("cut boundaries are segment starts");
            seg.uses += 1;
            at = seg.end;
        }
        Ok(RangeUse {
            map: self.id,
            lo,
            hi,
        })
    }

    /// 开始释放一个 use；之后逐步 [`Self::release_step`]。
    pub fn begin_release(&mut self, range_use: RangeUse) -> Result<ReleaseCursor, BackingError> {
        if range_use.map != self.id {
            return Err(BackingError::ForeignUse);
        }
        Ok(ReleaseCursor {
            map: self.id,
            next: range_use.lo,
            hi: range_use.hi,
        })
    }

    /// 处理一个段；不分配，O(log 段数)。
    pub fn release_step(&mut self, cursor: &mut ReleaseCursor) -> ReleaseStep<O> {
        debug_assert_eq!(cursor.map, self.id);
        if cursor.next >= cursor.hi {
            return ReleaseStep::Done;
        }
        let start = cursor.next;
        let seg = self
            .segs
            .get_mut(&start)
            .expect("live use boundaries are materialized while the use exists");
        debug_assert!(seg.end <= cursor.hi && seg.uses > 0);
        cursor.next = seg.end;
        seg.uses -= 1;
        if seg.uses > 0 {
            return ReleaseStep::Retained;
        }
        let seg = self.segs.remove(&start).expect("segment present");
        ReleaseStep::Detached {
            start,
            end: seg.end,
            owner: seg.owner,
        }
    }

    /// 进入最终收束：拒绝新 use。现有 use 照常释放。
    pub fn begin_retire(&mut self) {
        self.retiring = true;
    }

    /// 全部段已摘出。
    pub fn is_empty(&self) -> bool {
        self.segs.is_empty()
    }

    /// `[lo,hi)` 完全由连续的存活段覆盖（不含已摘出空洞）。
    fn covered(&self, lo: u64, hi: u64) -> bool {
        let Some((start, seg)) = self.segs.floor(&lo) else {
            return false;
        };
        if seg.end <= lo {
            return false;
        }
        let (mut key, mut end) = (start, seg.end);
        while end < hi {
            match self.segs.after(&key) {
                Some((next, seg)) if next == end => {
                    key = next;
                    end = seg.end;
                }
                _ => return false,
            }
        }
        true
    }

    fn needs_cut(&self, at: u64) -> bool {
        if at == 0 || at >= self.pages {
            return false;
        }
        matches!(self.segs.floor(&at), Some((start, seg)) if start != at && seg.end > at)
    }

    fn cut(&mut self, at: u64, reserve: &mut NodeReserve) {
        if !self.needs_cut(at) {
            return;
        }
        let (start, _) = self
            .segs
            .floor(&at)
            .expect("cut point lies in a live segment");
        Self::spend(self.id, reserve).expect("credits were checked before cutting");
        let seg = self.segs.remove(&start).expect("segment present");
        // remove 归还的槽即刻供左半复用：free 已有该槽，re-reserve 不分配。
        self.segs
            .reserve(1)
            .expect("slot just returned to free list");
        let (left, right) = seg.owner.split_at(at - start);
        self.segs.insert(
            start,
            Segment {
                end: at,
                uses: seg.uses,
                owner: left,
            },
        );
        self.segs.insert(
            at,
            Segment {
                end: seg.end,
                uses: seg.uses,
                owner: right,
            },
        );
    }

    /// 消费一个预取额度；对应的 OrdMap 槽已在 reserve 时承诺。
    fn spend(id: u64, reserve: &mut NodeReserve) -> Result<(), BackingError> {
        if reserve.map != id {
            return Err(BackingError::ForeignUse);
        }
        if reserve.credits == 0 {
            return Err(BackingError::ReserveExhausted);
        }
        reserve.credits -= 1;
        Ok(())
    }
}
