//! 进程资源绑定的长期结构 seam 与过渡期 metadata admission。
//!
//! 当前只开放 MetadataSponsor；后续 MemoryPool binding 与显式
//! KernelMemoryBudget 在同一 ProcessResources 中增量接入。

use alloc::sync::Arc;
use erhino_shared::call::SystemCallError;
use metadata_admission::{Counter, Permit, SponsoredPermit};

const SPONSOR_GLOBAL_LIMIT: usize = 4_096;
const POOL_CORE_GLOBAL_LIMIT: usize = 4_096;
pub(crate) const POOL_CORES_PER_SPONSOR: usize = 1_024;

struct MetadataAdmission {
    sponsors: Arc<Counter>,
    pool_cores: Arc<Counter>,
}

static ADMISSION: crate::sync::Spinlock<Option<MetadataAdmission>> =
    crate::sync::Spinlock::new(crate::sync::ranks::LEAF, None);

/// heap 就绪后、首个 Process/Pool core 构造前初始化固定全局 slots。
pub(crate) fn init() {
    let sponsors = Arc::try_new(Counter::new(SPONSOR_GLOBAL_LIMIT))
        .expect("metadata sponsor admission allocation failed");
    let pool_cores = Arc::try_new(Counter::new(POOL_CORE_GLOBAL_LIMIT))
        .expect("Pool core admission allocation failed");
    let mut admission = ADMISSION.lock();
    assert!(admission.is_none(), "metadata admission initialized twice");
    *admission = Some(MetadataAdmission {
        sponsors,
        pool_cores,
    });
}

fn counters() -> (Arc<Counter>, Arc<Counter>) {
    let admission = ADMISSION.lock();
    let admission = admission
        .as_ref()
        .expect("metadata admission not initialized");
    (
        Arc::clone(&admission.sponsors),
        Arc::clone(&admission.pool_cores),
    )
}

pub(crate) struct MetadataSponsor {
    _global_slot: Permit,
    pool_global: Arc<Counter>,
    pool_local: Arc<Counter>,
}

impl MetadataSponsor {
    fn try_new() -> Result<Arc<Self>, SystemCallError> {
        let (sponsors, pool_global) = counters();
        let global_slot =
            Counter::try_acquire(&sponsors).map_err(|_| SystemCallError::ReachLimit)?;
        let pool_local = Arc::try_new(Counter::new(POOL_CORES_PER_SPONSOR))
            .map_err(|_| SystemCallError::OutOfMemory)?;
        Arc::try_new(Self {
            _global_slot: global_slot,
            pool_global,
            pool_local,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub(crate) fn reserve_pool_core(
        sponsor: &Arc<Self>,
    ) -> Result<PoolCorePermit, SystemCallError> {
        let permit =
            SponsoredPermit::try_acquire(sponsor, &sponsor.pool_global, &sponsor.pool_local)
                .map_err(|_| SystemCallError::ReachLimit)?;
        Ok(PoolCorePermit {
            _owner: PoolCorePermitOwner::Sponsored { _permit: permit },
        })
    }
}

/// Pool core 的唯一 metadata owner。对象跨进程移动或 creator Dead 时继续强持
/// sponsor，直到 core 真实析构才同时归还本地和全局 slots。
pub(crate) struct PoolCorePermit {
    _owner: PoolCorePermitOwner,
}

enum PoolCorePermitOwner {
    Primordial {
        _permit: Permit,
    },
    Sponsored {
        _permit: SponsoredPermit<MetadataSponsor>,
    },
}

impl PoolCorePermit {
    pub(crate) fn primordial() -> Result<Self, SystemCallError> {
        let (_, pool_global) = counters();
        let global = Counter::try_acquire(&pool_global).map_err(|_| SystemCallError::ReachLimit)?;
        Ok(Self {
            _owner: PoolCorePermitOwner::Primordial { _permit: global },
        })
    }
}

/// 进程正交资源绑定容器。当前仅有过渡期 metadata sponsor；切片 4 在此加入
/// page PoolBinding，不改变调用方结构。
pub(crate) struct ProcessResources {
    metadata: Arc<MetadataSponsor>,
}

impl ProcessResources {
    pub(crate) fn try_new() -> Result<Self, SystemCallError> {
        Ok(Self {
            metadata: MetadataSponsor::try_new()?,
        })
    }

    pub(crate) fn bootstrap() -> Self {
        Self::try_new().expect("initial process metadata sponsor allocation failed")
    }

    pub(crate) fn metadata(&self) -> &Arc<MetadataSponsor> {
        &self.metadata
    }
}
