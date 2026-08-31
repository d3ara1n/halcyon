//! 进程资源绑定的长期结构 seam 与过渡期 metadata admission。
//!
//! Process core/Builder/Control、Pool core 与 AddressSpace 按真实寿命持 permit；
//! AddressSpace 预付 planner 固定容量，资金化 backing 与内存事务使用独立类型化 slots。
//! 显式 KernelMemoryBudget 继续在同一 ProcessResources 中增量接入。

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
const MAX_ADMITTED_BOUND_ADDRESS_SPACES: usize = 32;
pub(crate) const REGION_SLOTS_PER_ADDRESS_SPACE: usize = 4_096;
pub(crate) const MEMORY_CHANGES_PER_ADDRESS_SPACE: usize = 4;
const REGION_SLOT_GLOBAL_LIMIT: usize =
    REGION_SLOTS_PER_ADDRESS_SPACE * MAX_ADMITTED_BOUND_ADDRESS_SPACES;
const PLANNER_TRANSACTION_GLOBAL_LIMIT: usize =
    MEMORY_CHANGES_PER_ADDRESS_SPACE * MAX_ADMITTED_BOUND_ADDRESS_SPACES;
const BACKING_SLICE_GLOBAL_LIMIT: usize = 32_768;
pub(crate) const MEMORY_CHANGE_GLOBAL_LIMIT: usize = 128;
const MEMORY_WAIT_GLOBAL_LIMIT: usize = 128;
const REMOTE_COMPLETION_GLOBAL_LIMIT: usize = 128;
pub(crate) const POOL_CORES_PER_SPONSOR: usize = 1_024;
const ADDRESS_SPACES_PER_SPONSOR: usize = 1;
const BUILDERS_PER_SPONSOR: usize = 1;
const CONTROLS_PER_SPONSOR: usize = 1;
const BACKING_SLICES_PER_SPONSOR: usize = REGION_SLOTS_PER_ADDRESS_SPACE;
const MEMORY_WAITS_PER_SPONSOR: usize = MEMORY_CHANGES_PER_ADDRESS_SPACE;
const REMOTE_COMPLETIONS_PER_SPONSOR: usize = MEMORY_CHANGES_PER_ADDRESS_SPACE;

#[derive(Clone)]
struct MetadataAdmission {
    sponsors: Arc<Counter>,
    pool_cores: Arc<Counter>,
    address_spaces: Arc<Counter>,
    builders: Arc<Counter>,
    controls: Arc<Counter>,
    region_slots: Arc<Counter>,
    planner_transactions: Arc<Counter>,
    backing_slices: Arc<Counter>,
    memory_changes: Arc<Counter>,
    memory_waits: Arc<Counter>,
    remote_completions: Arc<Counter>,
}

static ADMISSION: crate::sync::Spinlock<Option<MetadataAdmission>> =
    crate::sync::Spinlock::new(crate::sync::ranks::LEAF, None);

/// heap 就绪后、首个 Process/Pool core 构造前初始化固定全局 slots。
pub(crate) fn init() {
    let new_counter = |limit, message| Arc::try_new(Counter::new(limit)).expect(message);
    let counters = MetadataAdmission {
        sponsors: new_counter(
            SPONSOR_GLOBAL_LIMIT,
            "metadata sponsor admission allocation failed",
        ),
        pool_cores: new_counter(
            POOL_CORE_GLOBAL_LIMIT,
            "Pool core admission allocation failed",
        ),
        address_spaces: new_counter(
            ADDRESS_SPACE_GLOBAL_LIMIT,
            "AddressSpace admission allocation failed",
        ),
        builders: new_counter(
            BUILDER_GLOBAL_LIMIT,
            "ProcessBuilder admission allocation failed",
        ),
        controls: new_counter(
            CONTROL_GLOBAL_LIMIT,
            "ProcessControl admission allocation failed",
        ),
        region_slots: new_counter(
            REGION_SLOT_GLOBAL_LIMIT,
            "region slot admission allocation failed",
        ),
        planner_transactions: new_counter(
            PLANNER_TRANSACTION_GLOBAL_LIMIT,
            "planner transaction admission allocation failed",
        ),
        backing_slices: new_counter(
            BACKING_SLICE_GLOBAL_LIMIT,
            "backing slice admission allocation failed",
        ),
        memory_changes: new_counter(
            MEMORY_CHANGE_GLOBAL_LIMIT,
            "MemoryChange admission allocation failed",
        ),
        memory_waits: new_counter(
            MEMORY_WAIT_GLOBAL_LIMIT,
            "memory WaitContext admission allocation failed",
        ),
        remote_completions: new_counter(
            REMOTE_COMPLETION_GLOBAL_LIMIT,
            "remote completion admission allocation failed",
        ),
    };
    let mut admission = ADMISSION.lock();
    assert!(admission.is_none(), "metadata admission initialized twice");
    *admission = Some(counters);
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

    let operation_1 = MetadataSponsor::reserve_memory_operation(resources.metadata())
        .expect("memory operation metadata self-test acquire failed");
    let operation_2 = MetadataSponsor::reserve_memory_operation(resources.metadata())
        .expect("memory operation metadata self-test acquire failed");
    let operation_3 = MetadataSponsor::reserve_memory_operation(resources.metadata())
        .expect("memory operation metadata self-test acquire failed");
    let operation_4 = MetadataSponsor::reserve_memory_operation(resources.metadata())
        .expect("memory operation metadata self-test acquire failed");
    assert!(matches!(
        MetadataSponsor::reserve_memory_operation(resources.metadata()),
        Err(SystemCallError::ReachLimit)
    ));
    drop((operation_1, operation_2, operation_3, operation_4));
    let replacement = MetadataSponsor::reserve_memory_operation(resources.metadata())
        .expect("memory operation metadata self-test refund failed");
    let (change, wait, remote) = replacement.into_parts();
    drop((change, wait, remote));

    let backing = MetadataSponsor::reserve_backing_slice(resources.metadata())
        .expect("backing slice metadata self-test acquire failed");
    drop(backing);
    drop(
        MetadataSponsor::reserve_backing_slice(resources.metadata())
            .expect("backing slice metadata self-test refund failed"),
    );
}

fn counters() -> MetadataAdmission {
    let admission = ADMISSION.lock();
    admission
        .as_ref()
        .expect("metadata admission not initialized")
        .clone()
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
    region_global: Arc<Counter>,
    region_local: Arc<Counter>,
    planner_transaction_global: Arc<Counter>,
    planner_transaction_local: Arc<Counter>,
    backing_slice_global: Arc<Counter>,
    backing_slice_local: Arc<Counter>,
    memory_change_global: Arc<Counter>,
    memory_change_local: Arc<Counter>,
    memory_wait_global: Arc<Counter>,
    memory_wait_local: Arc<Counter>,
    remote_completion_global: Arc<Counter>,
    remote_completion_local: Arc<Counter>,
}

impl MetadataSponsor {
    fn try_new() -> Result<Arc<Self>, SystemCallError> {
        let counters = counters();
        let global_slot =
            Counter::try_acquire(&counters.sponsors).map_err(|_| SystemCallError::ReachLimit)?;
        let new_local =
            |limit| Arc::try_new(Counter::new(limit)).map_err(|_| SystemCallError::OutOfMemory);
        let pool_local = new_local(POOL_CORES_PER_SPONSOR)?;
        let address_space_local = new_local(ADDRESS_SPACES_PER_SPONSOR)?;
        let builder_local = new_local(BUILDERS_PER_SPONSOR)?;
        let control_local = new_local(CONTROLS_PER_SPONSOR)?;
        let region_local = new_local(REGION_SLOTS_PER_ADDRESS_SPACE)?;
        let planner_transaction_local = new_local(MEMORY_CHANGES_PER_ADDRESS_SPACE)?;
        let backing_slice_local = new_local(BACKING_SLICES_PER_SPONSOR)?;
        let memory_change_local = new_local(MEMORY_CHANGES_PER_ADDRESS_SPACE)?;
        let memory_wait_local = new_local(MEMORY_WAITS_PER_SPONSOR)?;
        let remote_completion_local = new_local(REMOTE_COMPLETIONS_PER_SPONSOR)?;
        Arc::try_new(Self {
            _global_slot: global_slot,
            pool_global: counters.pool_cores,
            pool_local,
            address_space_global: counters.address_spaces,
            address_space_local,
            builder_global: counters.builders,
            builder_local,
            control_global: counters.controls,
            control_local,
            region_global: counters.region_slots,
            region_local,
            planner_transaction_global: counters.planner_transactions,
            planner_transaction_local,
            backing_slice_global: counters.backing_slices,
            backing_slice_local,
            memory_change_global: counters.memory_changes,
            memory_change_local,
            memory_wait_global: counters.memory_waits,
            memory_wait_local,
            remote_completion_global: counters.remote_completions,
            remote_completion_local,
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
        let shell = SponsoredPermit::try_acquire(
            sponsor,
            &sponsor.address_space_global,
            &sponsor.address_space_local,
        )
        .map_err(|_| SystemCallError::ReachLimit)?;
        let regions = SponsoredPermit::try_acquire_many(
            sponsor,
            &sponsor.region_global,
            &sponsor.region_local,
            REGION_SLOTS_PER_ADDRESS_SPACE,
        )
        .map_err(|_| SystemCallError::ReachLimit)?;
        let transactions = SponsoredPermit::try_acquire_many(
            sponsor,
            &sponsor.planner_transaction_global,
            &sponsor.planner_transaction_local,
            MEMORY_CHANGES_PER_ADDRESS_SPACE,
        )
        .map_err(|_| SystemCallError::ReachLimit)?;
        Ok(AddressSpacePermit {
            _shell: shell,
            _regions: regions,
            _transactions: transactions,
        })
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

    pub(crate) fn reserve_backing_slice(
        sponsor: &Arc<Self>,
    ) -> Result<BackingSlicePermit, SystemCallError> {
        let permit = SponsoredPermit::try_acquire(
            sponsor,
            &sponsor.backing_slice_global,
            &sponsor.backing_slice_local,
        )
        .map_err(|_| SystemCallError::ReachLimit)?;
        Ok(BackingSlicePermit { _permit: permit })
    }

    pub(crate) fn reserve_memory_operation(
        sponsor: &Arc<Self>,
    ) -> Result<MemoryOperationPermits, SystemCallError> {
        let change = SponsoredPermit::try_acquire(
            sponsor,
            &sponsor.memory_change_global,
            &sponsor.memory_change_local,
        )
        .map_err(|_| SystemCallError::ReachLimit)?;
        let wait = SponsoredPermit::try_acquire(
            sponsor,
            &sponsor.memory_wait_global,
            &sponsor.memory_wait_local,
        )
        .map_err(|_| SystemCallError::ReachLimit)?;
        let remote = SponsoredPermit::try_acquire(
            sponsor,
            &sponsor.remote_completion_global,
            &sponsor.remote_completion_local,
        )
        .map_err(|_| SystemCallError::ReachLimit)?;
        Ok(MemoryOperationPermits {
            change: MemoryChangePermit { _permit: change },
            wait: MemoryWaitPermit { _permit: wait },
            remote: RemoteCompletionPermit { _permit: remote },
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
        let counters = counters();
        let global =
            Counter::try_acquire(&counters.pool_cores).map_err(|_| SystemCallError::ReachLimit)?;
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

/// Bound AddressSpace 的 metadata admission owner。除壳身份外，它预付 planner
/// 的完整 Region/Transaction 固定容量，并强持 sponsor 到地址空间真实析构。
pub(crate) struct AddressSpacePermit {
    _shell: SponsoredPermit<MetadataSponsor>,
    _regions: SponsoredPermit<MetadataSponsor>,
    _transactions: SponsoredPermit<MetadataSponsor>,
}

pub(crate) struct BackingSlicePermit {
    _permit: SponsoredPermit<MetadataSponsor>,
}

pub(crate) struct MemoryChangePermit {
    _permit: SponsoredPermit<MetadataSponsor>,
}

pub(crate) struct MemoryWaitPermit {
    _permit: SponsoredPermit<MetadataSponsor>,
}

pub(crate) struct RemoteCompletionPermit {
    _permit: SponsoredPermit<MetadataSponsor>,
}

pub(crate) struct MemoryOperationPermits {
    change: MemoryChangePermit,
    wait: MemoryWaitPermit,
    remote: RemoteCompletionPermit,
}

impl MemoryOperationPermits {
    pub(crate) fn into_parts(
        self,
    ) -> (MemoryChangePermit, MemoryWaitPermit, RemoteCompletionPermit) {
        (self.change, self.wait, self.remote)
    }
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
