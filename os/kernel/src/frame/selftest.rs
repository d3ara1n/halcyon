//! 启动静止点上的真实 funding 接线验证；所有页内容访问均由独占 owner 保活。

use super::*;
use erhino_shared::memory_pool::MemoryPoolSnapshot;

#[derive(Clone, Copy)]
struct Inventory {
    pool: MemoryPoolSnapshot,
    frames: usize,
}

impl Inventory {
    fn read(pool: &MemoryPool) -> Self {
        Self {
            pool: pool.snapshot(),
            frames: free_frames(),
        }
    }

    fn assert_held(self, pool: &MemoryPool, pages: usize) {
        let mut expected = self.pool;
        expected.available -= pages as u64;
        expected.allocated += pages as u64;
        assert_eq!(
            pool.snapshot(),
            expected,
            "funded source accounting mismatch"
        );
        assert_eq!(
            free_frames(),
            self.frames - pages,
            "funded inventory mismatch"
        );
    }
}

fn assert_contents(base: FrameNumber, pages: usize, expected: u8) {
    let address = mm::phys_to_virt(base.addr()) as *const u8;
    for offset in 0..pages * PAGE_SIZE {
        // SAFETY: 私有调用点持有覆盖整个范围的独占物理 owner，且 offset 不越界。
        assert_eq!(
            unsafe { address.add(offset).read_volatile() },
            expected,
            "funded extent content mismatch"
        );
    }
}

fn dirty_contents(base: FrameNumber, pages: usize) {
    let address = mm::phys_to_virt(base.addr()) as *mut u8;
    for offset in 0..pages * PAGE_SIZE {
        // SAFETY: 私有调用点持有独占 claim 或 funded extent；无库存锁跨越此访问。
        unsafe {
            address.add(offset).write_volatile(0xa5);
        }
    }
    assert_contents(base, pages, 0xa5);
}

/// 写脏同一个真实 claim，再交给 broker 的正式 clear；不依赖重取相同物理页。
struct DirtyInventory;

impl PhysicalSource for DirtyInventory {
    type Claim = ClaimedUserExtent;
    type Error = UserClaimError;

    fn claim_largest(&self, max_pages: usize) -> Result<Self::Claim, Self::Error> {
        let claim = UserInventory.claim_largest(max_pages)?;
        let geometry = claim.geometry();
        dirty_contents(geometry.base(), geometry.count());
        Ok(claim)
    }
}

fn check_funding(root: &Arc<MemoryPool>) {
    let baseline = Inventory::read(root);
    let limits = FundingLimits {
        max_pages: 3,
        max_extents: 3,
    };
    let funded = fund_user_frames(root, 3, limits).expect("funded frame self-test failed");
    assert_eq!(funded.pages(), 3);
    assert!((1..=3).contains(&funded.extent_count()));
    assert_eq!(funded.extents().map(|(_, count)| count).sum::<usize>(), 3);
    for (base, pages) in funded.extents() {
        assert_contents(base, pages, 0);
    }
    baseline.assert_held(root, 3);
    drop(funded);
    baseline.assert_held(root, 0);

    let dirty = funded_frame::fund::<_, _, 3>(&PoolQuota(root), &DirtyInventory, 3, limits)
        .expect("dirty claim funding failed");
    assert_eq!(dirty.pages(), 3);
    for claim in dirty.claims() {
        let geometry = claim.geometry();
        assert_contents(geometry.base(), geometry.count(), 0);
    }
    baseline.assert_held(root, 3);
    drop(dirty);
    baseline.assert_held(root, 0);

    let table = fund_user_table_frame(root).expect("funded table self-test failed");
    assert_contents(table.frame(), 1, 0);
    baseline.assert_held(root, 1);
    drop(table);
    baseline.assert_held(root, 0);

    let limited = fund_user_frames(
        root,
        3,
        FundingLimits {
            max_pages: 3,
            max_extents: 1,
        },
    );
    // 库存 claim 为 power-of-two，三页不可能在一个 extent 中完成。
    assert!(matches!(limited, Err(funded_frame::FundError::ExtentLimit)));
    baseline.assert_held(root, 0);
}

fn check_split(root: &Arc<MemoryPool>, right_first: bool) {
    let baseline = Inventory::read(root);
    // 当前启动供给在两平台均保留连续四页；这是 fixture 前置，不是分配 ABI 承诺。
    let funded = fund_user_frames(
        root,
        4,
        FundingLimits {
            max_pages: 4,
            max_extents: 1,
        },
    )
    .expect("four-page split fixture requires a contiguous extent");
    let mut extents = Vec::new();
    funded
        .into_extents(&mut extents)
        .expect("split fixture storage failed");
    assert_eq!(extents.len(), 1);
    let extent = extents.pop().expect("split fixture lost its extent");
    let (left, right) = extent.split_at(1);
    assert_eq!(left.pages(), 1);
    assert_eq!(right.pages(), 3);
    assert_eq!(right.base(), left.base() + left.pages());
    baseline.assert_held(root, 4);
    if right_first {
        drop(right);
        baseline.assert_held(root, 1);
        dirty_contents(left.base(), left.pages());
        drop(left);
    } else {
        drop(left);
        baseline.assert_held(root, 3);
        dirty_contents(right.base(), right.pages());
        drop(right);
    }
    baseline.assert_held(root, 0);
}

pub(crate) fn run(root: &Arc<MemoryPool>) {
    check_funding(root);
    check_split(root, false);
    check_split(root, true);
    log!(
        Frame,
        "funded frame self-test passed: full-range zeroing, split, rollback, and source refund"
    );
}
