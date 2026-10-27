//! Backing 区间持有：按逻辑页 offset 分段的物理/资金 owner 与 View 的区间 use。
//!
//! 真值是段：段不跨物理连续范围与资金凭证边界，记录 `[start,end)`、真实 owner
//! 与覆盖 use 数。View 持不可复制的 [`RangeUse`]；释放时逐段推进，零 use 段
//! 原样摘出交给调用方退款，期间不分配。切段所需节点必须事先经
//! [`SegmentMap::reserve`] 预取（[`NodeReserve`]），不足时在改动前失败。
//!
//! 索引为 [`OrdMap`]（arena AVL，按段起点），定位 O(log 段数)；节点经
//! [`SegmentMap::reserve`] 预取，切段与摘段不分配。

use crate::ordmap::{OrdMap, OrdMapStorage, ReserveFailed};

/// 段 owner：真实物理 extent 与资金凭证。切分必须同时切物理与 charge，守恒。
pub trait SegmentOwner: Sized {
    /// 在 `pages` 处切成前后两半；`0 < pages < 当前页数` 由调用方保证。
    fn split_at(self, pages: u64) -> (Self, Self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackingError {
    /// 预取节点失败（OOM）。
    NodeLimit,
    /// 候选容量已被竞争越过；旧/未用存储须锁外重备，不接受新的 use。
    CapacityChanged,
    /// 预取的节点不足以完成本次切界。
    ReserveExhausted,
    /// 区间为空、越界或未对齐到现有段集合。
    InvalidRange,
    /// use 不属于本 map。
    ForeignUse,
    /// 已开始最终收束，不再接受新 use。
    Retiring,
    /// 覆盖段的 use 计数到达表示上限；失败不改变切界或已有引用。
    UseLimit,
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

/// 覆盖 `[lo,hi)` 逻辑页的业务 use；不可复制，由 map 拆分或开始释放。
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

/// 分步取得的已持前缀；调用方须保留未取得部分的来源责任直到完成/取消。
/// 每次只增加一个实际段的引用，取消把已取得前缀交普通释放路径。
#[derive(Debug)]
#[must_use = "an acquisition must be finished or its acquired prefix released"]
pub struct AcquireCursor {
    map: u64,
    lo: u64,
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

pub struct SegmentStorage<O, C> {
    index: OrdMapStorage<u64, Segment<O>, C>,
}
impl<O, C> SegmentStorage<O, C> {
    pub const fn new() -> Self {
        Self {
            index: OrdMapStorage::new(),
        }
    }
    pub fn try_empty<F: funded_storage::Funding<Charge = C>>(
        capacity: usize,
        payer: &F,
    ) -> Result<Self, funded_storage::StorageError<F::Error>> {
        Ok(Self {
            index: OrdMapStorage::try_empty(capacity, payer)?,
        })
    }
}
impl<O, C> Default for SegmentStorage<O, C> {
    fn default() -> Self {
        Self::new()
    }
}

/// 单个 Backing 的段集合，按段起点索引。
pub struct SegmentMap<O, C> {
    id: u64,
    segs: OrdMap<u64, Segment<O>, C>,
    pages: u64,
    retiring: bool,
}

impl<O: SegmentOwner, C> SegmentMap<O, C> {
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

    /// 来源同步域内借用包含逻辑页的物理/资金 owner；不交出其所有权。
    /// 调用者持 RangeUse 才能让解析出的地址跨同步域使用。
    pub fn segment_at(&self, page: u64) -> Option<(u64, u64, &O)> {
        let (start, segment) = self.segs.floor(&page)?;
        (page < segment.end).then_some((start, segment.end, &segment.owner))
    }

    /// 预取 `nodes` 个切段节点。只在准备阶段调用。
    pub fn growth_capacity(&self, nodes: usize) -> Result<usize, BackingError> {
        self.segs.growth_capacity(nodes).map_err(reserve_error)
    }
    pub fn storage_capacity(&self, nodes: usize) -> Result<usize, BackingError> {
        self.segs.storage_capacity(nodes).map_err(reserve_error)
    }
    pub fn take_empty_storage(&mut self) -> SegmentStorage<O, C> {
        SegmentStorage {
            index: self.segs.take_empty_storage(),
        }
    }
    pub fn storage_idle(&self) -> bool {
        self.segs.is_empty() && self.segs.reserved() == 0
    }
    pub fn is_retiring(&self) -> bool {
        self.retiring
    }
    pub fn reserve(
        &mut self,
        nodes: usize,
        prepared: &mut SegmentStorage<O, C>,
    ) -> Result<NodeReserve, BackingError> {
        self.segs
            .reserve(nodes, &mut prepared.index)
            .map_err(reserve_error)?;
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
    ) -> Result<(), (BackingError, O)> {
        if self.retiring {
            return Err((BackingError::Retiring, owner));
        }
        let Some(end) = self.pages.checked_add(pages).filter(|_| pages != 0) else {
            return Err((BackingError::InvalidRange, owner));
        };
        if let Err(error) = Self::spend(self.id, reserve) {
            return Err((error, owner));
        }
        let start = self.pages;
        self.segs.insert(
            start,
            Segment {
                end,
                uses: 0,
                owner,
            },
        );
        self.pages = end;
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
        if lo >= hi || hi > self.pages {
            return Err(BackingError::InvalidRange);
        }
        self.check_acquire(lo, hi)?;
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

    pub fn begin_acquire(
        &mut self,
        lo: u64,
        hi: u64,
        reserve: &mut NodeReserve,
    ) -> Result<AcquireCursor, BackingError> {
        if reserve.map != self.id {
            return Err(BackingError::ForeignUse);
        }
        if self.retiring {
            return Err(BackingError::Retiring);
        }
        if lo >= hi || hi > self.pages {
            return Err(BackingError::InvalidRange);
        }
        let need = usize::from(self.needs_cut(lo)) + usize::from(self.needs_cut(hi));
        if reserve.credits < need {
            return Err(BackingError::ReserveExhausted);
        }
        self.cut(lo, reserve);
        self.cut(hi, reserve);
        Ok(AcquireCursor {
            map: self.id,
            lo,
            next: lo,
            hi,
        })
    }

    /// 即使来源已封新准入，已接受游标仍可推进；来源 owner 由外层保持。
    pub fn acquire_step(&mut self, cursor: &mut AcquireCursor) -> Result<bool, BackingError> {
        if cursor.map != self.id {
            return Err(BackingError::ForeignUse);
        }
        if cursor.next == cursor.hi {
            return Ok(true);
        }
        let segment = self
            .segs
            .get_mut(&cursor.next)
            .ok_or(BackingError::InvalidRange)?;
        if segment.end > cursor.hi {
            return Err(BackingError::InvalidRange);
        }
        segment.uses = segment.uses.checked_add(1).ok_or(BackingError::UseLimit)?;
        cursor.next = segment.end;
        Ok(cursor.next == cursor.hi)
    }

    /// 返回真正已增加引用的前缀；完整成功时该前缀就是原请求全部区间。
    pub fn finish_acquire(&self, cursor: AcquireCursor) -> Option<RangeUse> {
        assert_eq!(cursor.map, self.id, "foreign backing acquisition cursor");
        (cursor.next != cursor.lo).then_some(RangeUse {
            map: self.id,
            lo: cursor.lo,
            hi: cursor.next,
        })
    }

    /// 一份既有 use 在内部点拆为两份，各段 use 不增不减；至多切一个物理段。
    /// 准备失败返还原 use 与未消费 reserve，终结后仍允许已受理责任拆分/释放。
    pub fn split_use(
        &mut self,
        range_use: RangeUse,
        at: u64,
        reserve: &mut NodeReserve,
    ) -> Result<(RangeUse, RangeUse), (BackingError, RangeUse)> {
        if range_use.map != self.id || reserve.map != self.id {
            return Err((BackingError::ForeignUse, range_use));
        }
        if at <= range_use.lo || at >= range_use.hi {
            return Err((BackingError::InvalidRange, range_use));
        }
        if self.needs_cut(at) && reserve.credits == 0 {
            return Err((BackingError::ReserveExhausted, range_use));
        }
        self.cut(at, reserve);
        Ok((
            RangeUse {
                map: self.id,
                lo: range_use.lo,
                hi: at,
            },
            RangeUse {
                map: self.id,
                lo: at,
                hi: range_use.hi,
            },
        ))
    }

    /// 开始释放一个 use；之后逐步 [`Self::release_step`]。
    pub fn begin_release(
        &mut self,
        range_use: RangeUse,
    ) -> Result<ReleaseCursor, (BackingError, RangeUse)> {
        if range_use.map != self.id {
            return Err((BackingError::ForeignUse, range_use));
        }
        Ok(ReleaseCursor {
            map: self.id,
            next: range_use.lo,
            hi: range_use.hi,
        })
    }

    /// 处理一个段；不分配，O(log 段数)。
    pub fn release_step(&mut self, cursor: &mut ReleaseCursor) -> ReleaseStep<O> {
        assert_eq!(cursor.map, self.id, "foreign backing release cursor");
        if cursor.next >= cursor.hi {
            return ReleaseStep::Done;
        }
        let start = cursor.next;
        let seg = self
            .segs
            .get_mut(&start)
            .expect("live use boundaries are materialized while the use exists");
        assert!(
            seg.end <= cursor.hi && seg.uses > 0,
            "invalid backing release ownership"
        );
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

    /// 准备检查完全覆盖和逐段计数上限，任何切界/计数变更之前完成。
    fn check_acquire(&self, lo: u64, hi: u64) -> Result<(), BackingError> {
        let Some((start, seg)) = self.segs.floor(&lo) else {
            return Err(BackingError::InvalidRange);
        };
        if seg.end <= lo {
            return Err(BackingError::InvalidRange);
        }
        if seg.uses == u32::MAX {
            return Err(BackingError::UseLimit);
        }
        let (mut key, mut end) = (start, seg.end);
        while end < hi {
            match self.segs.after(&key) {
                Some((next, seg)) if next == end => {
                    if seg.uses == u32::MAX {
                        return Err(BackingError::UseLimit);
                    }
                    key = next;
                    end = seg.end;
                }
                _ => return Err(BackingError::InvalidRange),
            }
        }
        Ok(())
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
        self.segs.reserve_recycled(1);
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

fn reserve_error(error: ReserveFailed) -> BackingError {
    match error {
        ReserveFailed::Overflow => BackingError::NodeLimit,
        ReserveFailed::CapacityChanged => BackingError::CapacityChanged,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct TestFunding;
    impl funded_storage::Funding for TestFunding {
        type Charge = ();
        type Error = ();
        fn charge(&self, _: core::alloc::Layout) -> Result<(), ()> {
            Ok(())
        }
    }

    struct Ext(u64);
    impl SegmentOwner for Ext {
        fn split_at(self, pages: u64) -> (Self, Self) {
            assert!(pages > 0 && pages < self.0);
            (Self(pages), Self(self.0 - pages))
        }
    }

    #[test]
    fn existing_credit_survives_rejected_candidate_without_allocating() {
        let mut map = SegmentMap::new(1);
        let mut storage = SegmentStorage::try_empty(2, &TestFunding).unwrap();
        let mut reserve = map.reserve(2, &mut storage).unwrap();
        assert!(map.push_extent(10, Ext(10), &mut reserve).is_ok());
        let whole = map.acquire(0, 10, &mut reserve).unwrap();
        let capacity = map.segs.capacity();
        assert!(matches!(
            map.reserve(8, &mut storage),
            Err(BackingError::CapacityChanged)
        ));
        assert!(matches!(
            map.reserve(usize::MAX, &mut storage),
            Err(BackingError::NodeLimit)
        ));
        let (left, right) = map.split_use(whole, 4, &mut reserve).unwrap();
        assert_eq!(map.segs.capacity(), capacity);
        for (part, expected) in [(left, 4), (right, 6)] {
            let mut cursor = map.begin_release(part).unwrap();
            match map.release_step(&mut cursor) {
                ReleaseStep::Detached {
                    owner: Ext(pages), ..
                } => assert_eq!(pages, expected),
                _ => panic!("split use did not refund its unique extent"),
            }
            assert!(matches!(map.release_step(&mut cursor), ReleaseStep::Done));
        }
        assert_eq!(map.segs.capacity(), capacity);
        assert!(map.is_empty());
    }
}
