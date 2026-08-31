//! 进程资源绑定的长期结构 seam 与过渡期 metadata admission。
//!
//! 当前开放 Process core/Builder/Control、Pool core 与 AddressSpace 的类型化
//! admission；显式 KernelMemoryBudget 继续在同一 ProcessResources 中增量接入。

use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, Ordering};
use erhino_shared::call::SystemCallError;
use metadata_admission::{Counter, Permit, SponsoredPermit};
use page_table::FrameNumber;

use super::memory_pool::{MemoryPool, map_pool_error};

const SPONSOR_GLOBAL_LIMIT: usize = 4_096;
const POOL_CORE_GLOBAL_LIMIT: usize = 4_096;
const ADDRESS_SPACE_GLOBAL_LIMIT: usize = 4_096;
const BUILDER_GLOBAL_LIMIT: usize = 4_096;
const CONTROL_GLOBAL_LIMIT: usize = 4_096;
pub(crate) const POOL_CORES_PER_SPONSOR: usize = 1_024;
const ADDRESS_SPACES_PER_SPONSOR: usize = 1;
const BUILDERS_PER_SPONSOR: usize = 1;
const CONTROLS_PER_SPONSOR: usize = 1;

struct MetadataAdmission {
    sponsors: Arc<Counter>,
    pool_cores: Arc<Counter>,
    address_spaces: Arc<Counter>,
    builders: Arc<Counter>,
    controls: Arc<Counter>,
}

static ADMISSION: crate::sync::Spinlock<Option<MetadataAdmission>> =
    crate::sync::Spinlock::new(crate::sync::ranks::LEAF, None);

/// heap 就绪后、首个 Process/Pool core 构造前初始化固定全局 slots。
pub(crate) fn init() {
    let sponsors = Arc::try_new(Counter::new(SPONSOR_GLOBAL_LIMIT))
        .expect("metadata sponsor admission allocation failed");
    let pool_cores = Arc::try_new(Counter::new(POOL_CORE_GLOBAL_LIMIT))
        .expect("Pool core admission allocation failed");
    let address_spaces = Arc::try_new(Counter::new(ADDRESS_SPACE_GLOBAL_LIMIT))
        .expect("AddressSpace admission allocation failed");
    let builders = Arc::try_new(Counter::new(BUILDER_GLOBAL_LIMIT))
        .expect("ProcessBuilder admission allocation failed");
    let controls = Arc::try_new(Counter::new(CONTROL_GLOBAL_LIMIT))
        .expect("ProcessControl admission allocation failed");
    let mut admission = ADMISSION.lock();
    assert!(admission.is_none(), "metadata admission initialized twice");
    *admission = Some(MetadataAdmission {
        sponsors,
        pool_cores,
        address_spaces,
        builders,
        controls,
    });
}

/// 启动期真实穿过 Process shell 类型化子额度：本地耗尽、最后 owner 退款与重取。
pub(crate) fn self_test() {
    let resources = ProcessResources::try_new().expect("process metadata self-test sponsor failed");
    let builder = MetadataSponsor::reserve_builder(resources.metadata())
        .expect("ProcessBuilder metadata self-test acquire failed");
    assert!(matches!(
        MetadataSponsor::reserve_builder(resources.metadata()),
        Err(SystemCallError::ReachLimit)
    ));
    drop(builder);
    drop(
        MetadataSponsor::reserve_builder(resources.metadata())
            .expect("ProcessBuilder metadata self-test refund failed"),
    );

    let control = MetadataSponsor::reserve_control(resources.metadata())
        .expect("ProcessControl metadata self-test acquire failed");
    assert!(matches!(
        MetadataSponsor::reserve_control(resources.metadata()),
        Err(SystemCallError::ReachLimit)
    ));
    drop(control);
    drop(
        MetadataSponsor::reserve_control(resources.metadata())
            .expect("ProcessControl metadata self-test refund failed"),
    );
}

fn counters() -> (
    Arc<Counter>,
    Arc<Counter>,
    Arc<Counter>,
    Arc<Counter>,
    Arc<Counter>,
) {
    let admission = ADMISSION.lock();
    let admission = admission
        .as_ref()
        .expect("metadata admission not initialized");
    (
        Arc::clone(&admission.sponsors),
        Arc::clone(&admission.pool_cores),
        Arc::clone(&admission.address_spaces),
        Arc::clone(&admission.builders),
        Arc::clone(&admission.controls),
    )
}

pub(crate) struct MetadataSponsor {
    // Process core permit：ProcessResources 与 core 同寿命，Dead 后真实析构才退款。
    _global_slot: Permit,
    pool_global: Arc<Counter>,
    pool_local: Arc<Counter>,
    address_space_global: Arc<Counter>,
    address_space_local: Arc<Counter>,
    builder_global: Arc<Counter>,
    builder_local: Arc<Counter>,
    control_global: Arc<Counter>,
    control_local: Arc<Counter>,
}

impl MetadataSponsor {
    fn try_new() -> Result<Arc<Self>, SystemCallError> {
        let (sponsors, pool_global, address_space_global, builder_global, control_global) =
            counters();
        let global_slot =
            Counter::try_acquire(&sponsors).map_err(|_| SystemCallError::ReachLimit)?;
        let pool_local = Arc::try_new(Counter::new(POOL_CORES_PER_SPONSOR))
            .map_err(|_| SystemCallError::OutOfMemory)?;
        let address_space_local = Arc::try_new(Counter::new(ADDRESS_SPACES_PER_SPONSOR))
            .map_err(|_| SystemCallError::OutOfMemory)?;
        let builder_local = Arc::try_new(Counter::new(BUILDERS_PER_SPONSOR))
            .map_err(|_| SystemCallError::OutOfMemory)?;
        let control_local = Arc::try_new(Counter::new(CONTROLS_PER_SPONSOR))
            .map_err(|_| SystemCallError::OutOfMemory)?;
        Arc::try_new(Self {
            _global_slot: global_slot,
            pool_global,
            pool_local,
            address_space_global,
            address_space_local,
            builder_global,
            builder_local,
            control_global,
            control_local,
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

    pub(crate) fn reserve_address_space(
        sponsor: &Arc<Self>,
    ) -> Result<AddressSpacePermit, SystemCallError> {
        let permit = SponsoredPermit::try_acquire(
            sponsor,
            &sponsor.address_space_global,
            &sponsor.address_space_local,
        )
        .map_err(|_| SystemCallError::ReachLimit)?;
        Ok(AddressSpacePermit { _permit: permit })
    }

    pub(crate) fn reserve_builder(sponsor: &Arc<Self>) -> Result<BuilderPermit, SystemCallError> {
        let permit =
            SponsoredPermit::try_acquire(sponsor, &sponsor.builder_global, &sponsor.builder_local)
                .map_err(|_| SystemCallError::ReachLimit)?;
        Ok(BuilderPermit { _permit: permit })
    }

    pub(crate) fn reserve_control(sponsor: &Arc<Self>) -> Result<ControlPermit, SystemCallError> {
        let permit =
            SponsoredPermit::try_acquire(sponsor, &sponsor.control_global, &sponsor.control_local)
                .map_err(|_| SystemCallError::ReachLimit)?;
        Ok(ControlPermit { _permit: permit })
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
        let (_, pool_global, _, _, _) = counters();
        let global = Counter::try_acquire(&pool_global).map_err(|_| SystemCallError::ReachLimit)?;
        Ok(Self {
            _owner: PoolCorePermitOwner::Primordial { _permit: global },
        })
    }
}

/// ProcessBuilder 壳的唯一 metadata owner；最后 capability 消散才退款。
pub(crate) struct BuilderPermit {
    _permit: SponsoredPermit<MetadataSponsor>,
}

/// ProcessControl 终态观察壳的唯一 metadata owner；最后 capability 消散才退款。
pub(crate) struct ControlPermit {
    _permit: SponsoredPermit<MetadataSponsor>,
}

/// Bound AddressSpace 的 metadata admission owner。permit 强持 sponsor 到地址空间
/// 完成有界收束并真实析构，Process core 提前 Dead 不会退款。
pub(crate) struct AddressSpacePermit {
    _permit: SponsoredPermit<MetadataSponsor>,
}

/// 进程 page-backed storage 的不可转移内部 binding。root 页表物理 owner 与
/// MemoryPool charge 保持同寿命；后续页表/backing 只从同一 pool getter 取得额度。
pub(crate) struct PoolBinding {
    root: crate::frame::FundedRootFrame,
    pool: Arc<MemoryPool>,
    _metadata: AddressSpacePermit,
}

impl PoolBinding {
    pub(crate) fn prepare(
        pool: Arc<MemoryPool>,
        sponsor: &Arc<MetadataSponsor>,
    ) -> Result<Self, SystemCallError> {
        let metadata = MetadataSponsor::reserve_address_space(sponsor)?;
        let root = crate::frame::fund_user_root(&pool).map_err(|error| match error {
            funded_frame::FundError::Quota(error) => map_pool_error(error),
            funded_frame::FundError::Physical(_) => SystemCallError::OutOfMemory,
            funded_frame::FundError::ZeroPages
            | funded_frame::FundError::PageLimit
            | funded_frame::FundError::ExtentLimit
            | funded_frame::FundError::InvalidClaim => SystemCallError::InternalError,
        })?;
        Ok(Self {
            root,
            pool,
            _metadata: metadata,
        })
    }

    pub(crate) fn root_frame(&self) -> FrameNumber {
        self.root.frame()
    }

    pub(crate) fn pool(&self) -> &Arc<MemoryPool> {
        &self.pool
    }
}

/// 进程正交资源绑定容器。MetadataSponsor 是创建期内部预算；页池绑定由
/// AddressSpace 的 Bound 状态唯一拥有，避免资源容器与地址空间形成双重真值。
pub(crate) struct ProcessResources {
    metadata: Arc<MetadataSponsor>,
    bind_in_progress: AtomicBool,
}

impl ProcessResources {
    pub(crate) fn try_new() -> Result<Self, SystemCallError> {
        Ok(Self {
            metadata: MetadataSponsor::try_new()?,
            bind_in_progress: AtomicBool::new(false),
        })
    }

    pub(crate) fn bootstrap() -> Self {
        Self::try_new().expect("initial process metadata sponsor allocation failed")
    }

    pub(crate) fn metadata(&self) -> &Arc<MetadataSponsor> {
        &self.metadata
    }

    pub(crate) fn try_reserve_binding(&self) -> Result<BindReservation<'_>, SystemCallError> {
        self.bind_in_progress
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_| SystemCallError::ObjectBusy)?;
        Ok(BindReservation { resources: self })
    }
}

pub(crate) struct BindReservation<'a> {
    resources: &'a ProcessResources,
}

impl Drop for BindReservation<'_> {
    fn drop(&mut self) {
        self.resources
            .bind_in_progress
            .store(false, Ordering::Release);
    }
}
