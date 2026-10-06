//! MemoryPool capability 对象：额度状态、派生事务与自然父级退款。
//!
//! parent 不登记 child；child core 以强引用持 parent，state 内嵌不可拆分的
//! delegated credit。child/charge/Handle 引用消散后逐锁退款，不同时持有两把 Pool 锁。

use alloc::sync::Arc;
use core::any::Any;

use erhino_shared::{
    call::SystemCallError,
    memory_pool::{
        MemoryPoolBudget, MemoryPoolDeriveRequest, MemoryPoolSnapshot, MetadataSnapshot,
    },
    object::{Handle, Rights},
};
use memory_pool::{
    AllocatedCredit, ChargeReservation, DelegationReservation, LedgerState, PoolError, PoolId,
    PreparedChild,
};

use super::{
    Thread,
    object::{KernelObject, ObjectHeader, ObjectKind, ObjectRef},
    resources::MetadataCharge,
};

static ROOT: crate::sync::Spinlock<Option<Arc<MemoryPool>>> =
    crate::sync::Spinlock::new(crate::sync::ranks::LEAF, None);

pub struct MemoryPool {
    /// capability 持有计数（ZeroPolicy 按类型固定）。
    holders: capability::Holders,
    header: ObjectHeader,
    inner: crate::sync::Spinlock<PoolInner>,
    /// 维护公平域：资金账户即 Pool（预算分案账户形状）。Work 记录强持该账户。
    maintenance: Arc<crate::maintenance::Account>,
    /// 本体独立寿命由父账户付款；root 计入 boot 固定成本。
    _metadata: Option<MetadataCharge>,
}

struct PoolLedgers {
    pages: LedgerState,
    metadata: LedgerState,
}

enum PoolInner {
    Active {
        state: Option<PoolLedgers>,
        funding: Funding,
    },
    /// Handle 发布前的不可用 child；PreparedChildOwner 析构即回滚 parent。
    Prepared(PreparedChildOwner),
    /// 只存在于 Commit 的逐锁交接窗口；不执行可失败工作。
    Committing,
}

enum Funding {
    Root,
    Child(Arc<MemoryPool>),
}

/// Pool 内已预留但尚未与物理 backing 配对的额度；析构自动回滚。
pub(crate) struct PreparedMemoryCharge {
    pool: Arc<MemoryPool>,
    reservation: Option<ChargeReservation>,
}

/// 与 page-backed storage 同寿命的额度 owner；析构把 allocated 额度退回来源 Pool。
pub(crate) struct MemoryCharge {
    pool: Arc<MemoryPool>,
    credit: Option<AllocatedCredit>,
}

impl MemoryPool {
    pub(crate) fn initialize_root(seed: crate::frame::RootPoolSeed) -> Arc<Self> {
        let header = ObjectHeader::try_new().expect("root Pool identity exhausted");
        let identity = PoolId::new(header.koid()).expect("kernel object identity must be nonzero");
        let pages = LedgerState::root(identity, seed.into_pages())
            .expect("platform user supply cannot create the root Pool");
        let fixed = super::resources::arc_slots::<Self>()
            + super::resources::arc_slots::<crate::maintenance::Account>();
        let metadata = LedgerState::root(identity, super::resources::initial_slots(fixed))
            .expect("system supply cannot create the root metadata ledger");
        let total = pages.snapshot().total;
        let pool = Arc::try_new(Self {
            holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
            header,
            inner: crate::sync::Spinlock::new(
                crate::sync::ranks::MEMORY_POOL,
                PoolInner::Active {
                    state: Some(PoolLedgers { pages, metadata }),
                    funding: Funding::Root,
                },
            ),
            maintenance: Arc::try_new(crate::maintenance::Account::root())
                .expect("root Pool maintenance account allocation failed"),
            _metadata: None,
        })
        .expect("root Pool allocation failed");

        let mut root = ROOT.lock();
        assert!(root.is_none(), "root Pool initialized twice");
        *root = Some(Arc::clone(&pool));
        drop(root);
        log!(Memory, "root Pool minted with {} page(s)", total);
        pool
    }

    pub(crate) fn self_test(root: &Arc<Self>) {
        const PAGES: u64 = 2;

        let baseline = root.snapshot();
        let header = ObjectHeader::try_new().expect("Pool self-test identity exhausted");
        let reservation = Self::reserve_delegation(
            root,
            MemoryPoolBudget {
                pages: PAGES,
                metadata: 0,
            },
        )
        .expect("Pool self-test reservation failed");
        let prepared = Self::prepared_child(Arc::clone(root), reservation, header)
            .expect("Pool self-test child preparation failed");
        drop(prepared);
        assert_eq!(
            root.snapshot(),
            baseline,
            "prepared Pool rollback did not restore its parent"
        );

        let header = ObjectHeader::try_new().expect("Pool self-test identity exhausted");
        let reservation = Self::reserve_delegation(
            root,
            MemoryPoolBudget {
                pages: PAGES,
                metadata: 0,
            },
        )
        .expect("Pool self-test reservation failed");
        let child = Self::prepared_child(Arc::clone(root), reservation, header)
            .expect("Pool self-test child preparation failed");
        child.commit_prepared();
        let child_snapshot = child.snapshot();
        assert_eq!(child_snapshot.parent_identity, baseline.identity);
        assert_eq!(child_snapshot.total, PAGES);
        let parent_committed = root.snapshot();
        assert_eq!(parent_committed.delegated, baseline.delegated + PAGES);
        assert_eq!(parent_committed.available, baseline.available - PAGES);

        let frames_before_funding = crate::frame::free_frames();
        let funded = crate::frame::fund_user_table_frame(&child)
            .expect("child-funded frame self-test failed");
        let mut child_funded = child_snapshot;
        child_funded.available -= 1;
        child_funded.allocated += 1;
        assert_eq!(child.snapshot(), child_funded);
        assert_eq!(root.snapshot(), parent_committed);
        assert_eq!(crate::frame::free_frames(), frames_before_funding - 1);
        let child_identity = Arc::downgrade(&child);
        let peer = Arc::clone(&child);
        drop(child);
        assert_eq!(
            root.snapshot(),
            parent_committed,
            "Pool credit returned before the last core reference disappeared"
        );
        drop(peer);
        assert!(
            child_identity.upgrade().is_some(),
            "funded owner lost its source Pool"
        );
        assert_eq!(
            root.snapshot(),
            parent_committed,
            "delegated credit returned while a child-funded owner remained alive"
        );
        drop(funded);
        assert!(
            child_identity.upgrade().is_none(),
            "source Pool survived its last funded owner"
        );
        assert_eq!(crate::frame::free_frames(), frames_before_funding);
        assert_eq!(
            root.snapshot(),
            baseline,
            "committed Pool credit did not return to its parent"
        );

        let frames_baseline = crate::frame::free_frames();
        let header = ObjectHeader::try_new().expect("Pool self-test identity exhausted");
        let reservation = Self::reserve_delegation(
            root,
            MemoryPoolBudget {
                pages: 1,
                metadata: 0,
            },
        )
        .expect("Pool self-test reservation failed");
        let child = Self::prepared_child(Arc::clone(root), reservation, header)
            .expect("Pool self-test child preparation failed");
        child.commit_prepared();
        let child_baseline = child.snapshot();
        let first =
            crate::frame::fund_user_table_frame(&child).expect("first table funding failed");
        let error = match crate::frame::fund_user_table_frame(&child) {
            Ok(_) => panic!("quota-limited table funding unexpectedly succeeded"),
            Err(error) => error,
        };
        assert!(
            matches!(
                error,
                funded_frame::FundError::Quota(PoolError::QuotaExceeded)
            ),
            "quota-limited table funding lost its public error class: {error:?}"
        );
        drop(first);
        assert_eq!(
            child.snapshot(),
            child_baseline,
            "failed table funding changed child Pool accounting"
        );
        assert_eq!(
            crate::frame::free_frames(),
            frames_baseline,
            "failed table funding leaked user inventory"
        );
        drop(child);
        assert_eq!(
            root.snapshot(),
            baseline,
            "quota failure child did not return delegated credit"
        );
        log!(
            Memory,
            "Pool self-test passed: rollback, commit, and funded-source last-reference refund ok"
        );
    }

    fn prepared_child(
        parent: Arc<Self>,
        reservation: (DelegationReservation, DelegationReservation),
        header: ObjectHeader,
    ) -> Result<Arc<Self>, SystemCallError> {
        let identity = PoolId::new(header.koid()).expect("kernel object identity must be nonzero");
        let (pages, metadata) = reservation;
        let pages = match LedgerState::prepare_child(identity, pages) {
            Ok(prepared) => prepared,
            Err(error) => {
                let kind = error.error();
                let mut inner = parent.inner.lock();
                let state = Self::active_state_mut(&mut inner);
                state
                    .pages
                    .rollback_delegation(error.into_token())
                    .expect("page reservation belongs to parent");
                state
                    .metadata
                    .rollback_delegation(metadata)
                    .expect("metadata reservation belongs to parent");
                return Err(map_pool_error(kind));
            }
        };
        let metadata = match LedgerState::prepare_child(identity, metadata) {
            Ok(prepared) => prepared,
            Err(error) => {
                let kind = error.error();
                let mut inner = parent.inner.lock();
                let state = Self::active_state_mut(&mut inner);
                state
                    .pages
                    .rollback_child(pages)
                    .expect("prepared page budget belongs to parent");
                state
                    .metadata
                    .rollback_delegation(error.into_token())
                    .expect("metadata reservation belongs to parent");
                return Err(map_pool_error(kind));
            }
        };
        let owner = PreparedChildOwner {
            parent,
            prepared: Some((pages, metadata)),
        };
        let metadata = MetadataCharge::arc::<Self>(&owner.parent)?;
        let account = MetadataCharge::arc::<crate::maintenance::Account>(&owner.parent)?;
        let maintenance = Arc::try_new(crate::maintenance::Account::funded(account))
            .map_err(|_| SystemCallError::OutOfMemory)?;
        Arc::try_new(Self {
            holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
            header,
            inner: crate::sync::Spinlock::new(
                crate::sync::ranks::MEMORY_POOL,
                PoolInner::Prepared(owner),
            ),
            maintenance,
            _metadata: Some(metadata),
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    /// 本 Pool 的维护账户（Work 记录准入用）。
    pub(crate) fn maintenance_account(&self) -> &Arc<crate::maintenance::Account> {
        &self.maintenance
    }

    pub(crate) fn object_ref(pool: &Arc<Self>) -> ObjectRef {
        Arc::clone(pool) as ObjectRef
    }

    pub(crate) fn concrete(object: &ObjectRef) -> Result<Arc<Self>, SystemCallError> {
        let any: Arc<dyn Any + Send + Sync> = object.clone();
        any.downcast::<Self>()
            .map_err(|_| SystemCallError::WrongObjectType)
    }

    fn active_state(inner: &PoolInner) -> &PoolLedgers {
        match inner {
            PoolInner::Active { state, .. } => state
                .as_ref()
                .expect("active MemoryPool state already consumed"),
            PoolInner::Prepared(_) | PoolInner::Committing => {
                panic!("unpublished MemoryPool state became observable")
            }
        }
    }

    fn active_state_mut(inner: &mut PoolInner) -> &mut PoolLedgers {
        match inner {
            PoolInner::Active { state, .. } => state
                .as_mut()
                .expect("active MemoryPool state already consumed"),
            PoolInner::Prepared(_) | PoolInner::Committing => {
                panic!("unpublished MemoryPool state entered an active operation")
            }
        }
    }

    pub(crate) fn snapshot(&self) -> MemoryPoolSnapshot {
        let inner = self.inner.lock();
        let state = Self::active_state(&inner);
        let snapshot = state.pages.snapshot();
        let metadata = state.metadata.snapshot();
        MemoryPoolSnapshot {
            identity: snapshot.identity.get(),
            parent_identity: snapshot.parent_identity.map_or(0, PoolId::get),
            total: snapshot.total,
            available: snapshot.available,
            reserved: snapshot.reserved,
            allocated: snapshot.allocated,
            delegated: snapshot.delegated,
            depth: snapshot.depth,
            reserved0: 0,
            metadata: MetadataSnapshot {
                total: metadata.total,
                available: metadata.available,
                reserved: metadata.reserved,
                allocated: metadata.allocated,
                delegated: metadata.delegated,
            },
        }
    }

    fn reserve_delegation(
        parent: &Arc<Self>,
        budget: MemoryPoolBudget,
    ) -> Result<(DelegationReservation, DelegationReservation), SystemCallError> {
        if budget.is_empty() {
            return Err(SystemCallError::IllegalArgument);
        }
        let mut inner = parent.inner.lock();
        let state = Self::active_state_mut(&mut inner);
        let pages = state
            .pages
            .reserve_delegation(budget.pages)
            .map_err(map_pool_error)?;
        let metadata = match state.metadata.reserve_delegation(budget.metadata) {
            Ok(metadata) => metadata,
            Err(error) => {
                state
                    .pages
                    .rollback_delegation(pages)
                    .expect("page reservation belongs to parent");
                return Err(map_pool_error(error));
            }
        };
        Ok((pages, metadata))
    }

    pub(crate) fn charge_metadata(
        pool: &Arc<Self>,
        slots: u64,
    ) -> Result<AllocatedCredit, SystemCallError> {
        let mut inner = pool.inner.lock();
        let ledger = &mut Self::active_state_mut(&mut inner).metadata;
        let reservation = ledger.reserve_charge(slots).map_err(map_pool_error)?;
        Ok(ledger
            .commit_charge(reservation)
            .expect("accepted metadata reservation belongs to Pool"))
    }

    pub(crate) fn return_metadata(&self, credit: AllocatedCredit) {
        Self::active_state_mut(&mut self.inner.lock())
            .metadata
            .return_charge(credit)
            .expect("metadata credit belongs to Pool");
    }

    pub(crate) fn root_metadata_snapshot() -> MetadataSnapshot {
        let root = Self::system_pool();
        root.snapshot().metadata
    }

    /// boot 冻结的系统调度域为长期共享存储选择的付款源。
    pub(crate) fn system_pool() -> Arc<Self> {
        ROOT.lock()
            .as_ref()
            .expect("root Pool not initialized")
            .clone()
    }

    pub(crate) fn reserve_charge(
        pool: &Arc<Self>,
        pages: usize,
    ) -> Result<PreparedMemoryCharge, PoolError> {
        let pages = u64::try_from(pages).map_err(|_| PoolError::ArithmeticOverflow)?;
        let reservation = Self::active_state_mut(&mut pool.inner.lock())
            .pages
            .reserve_charge(pages)?;
        Ok(PreparedMemoryCharge {
            pool: Arc::clone(pool),
            reservation: Some(reservation),
        })
    }

    /// parent reserved→delegated 后把 child 从不可见 Prepared 转为 Active。
    /// 两把 Pool 锁逐把获取；此路径无分配、无可恢复失败。
    fn commit_prepared(&self) {
        let owner = {
            let mut inner = self.inner.lock();
            match core::mem::replace(&mut *inner, PoolInner::Committing) {
                PoolInner::Prepared(owner) => owner,
                PoolInner::Active { .. } | PoolInner::Committing => {
                    panic!("child Pool committed outside its prepared state")
                }
            }
        };
        let (state, parent) = owner.commit();
        let mut inner = self.inner.lock();
        assert!(
            matches!(*inner, PoolInner::Committing),
            "child Pool commit marker changed"
        );
        *inner = PoolInner::Active {
            state: Some(state),
            funding: Funding::Child(parent),
        };
    }
}

impl Drop for MemoryPool {
    fn drop(&mut self) {
        match self.inner.get_mut() {
            PoolInner::Active { state, funding } => {
                let state = state
                    .take()
                    .expect("active MemoryPool state already consumed");
                assert!(
                    state.pages.is_fully_available() && state.metadata.is_fully_available(),
                    "MemoryPool core dropped with outstanding credit"
                );
                match funding {
                    Funding::Root => {
                        assert!(
                            state
                                .pages
                                .into_parent_credit()
                                .expect("root Pool state is quiescent")
                                .is_none(),
                            "root Pool unexpectedly carried parent credit"
                        );
                        assert!(
                            state
                                .metadata
                                .into_parent_credit()
                                .expect("root metadata is quiescent")
                                .is_none()
                        );
                    }
                    Funding::Child(parent) => {
                        let expected_parent = PoolId::new(parent.header.koid())
                            .expect("kernel object identity must be nonzero");
                        assert_eq!(
                            state.pages.snapshot().parent_identity,
                            Some(expected_parent),
                            "MemoryPool parent identity changed"
                        );
                        let credit = state
                            .pages
                            .into_parent_credit()
                            .expect("child Pool state is quiescent")
                            .expect("child Pool lost parent credit");
                        let metadata = state
                            .metadata
                            .into_parent_credit()
                            .expect("child metadata is quiescent")
                            .expect("child metadata lost parent credit");
                        let mut inner = parent.inner.lock();
                        let parent = Self::active_state_mut(&mut inner);
                        parent
                            .pages
                            .return_delegation(credit)
                            .unwrap_or_else(|error| {
                                panic!(
                                    "MemoryPool parent credit return failed: {:?}",
                                    error.error()
                                )
                            });
                        parent
                            .metadata
                            .return_delegation(metadata)
                            .expect("child metadata credit belongs to parent");
                    }
                }
            }
            PoolInner::Prepared(_) => {}
            PoolInner::Committing => panic!("MemoryPool dropped during its commit handoff"),
        }
    }
}

impl funded_frame::QuotaReservation for PreparedMemoryCharge {
    type Credit = MemoryCharge;

    fn commit(mut self) -> Self::Credit {
        let reservation = self
            .reservation
            .take()
            .expect("MemoryPool charge reservation completed twice");
        let credit = MemoryPool::active_state_mut(&mut self.pool.inner.lock())
            .pages
            .commit_charge(reservation)
            .unwrap_or_else(|error| {
                panic!(
                    "reserved MemoryPool charge commit failed: {:?}",
                    error.error()
                )
            });
        MemoryCharge {
            pool: Arc::clone(&self.pool),
            credit: Some(credit),
        }
    }
}

impl Drop for PreparedMemoryCharge {
    fn drop(&mut self) {
        let Some(reservation) = self.reservation.take() else {
            return;
        };
        MemoryPool::active_state_mut(&mut self.pool.inner.lock())
            .pages
            .rollback_charge(reservation)
            .unwrap_or_else(|error| {
                panic!("MemoryPool charge rollback failed: {:?}", error.error())
            });
    }
}

impl MemoryCharge {
    /// 该 charge 是否来自 `pool`（同源校验，按对象身份）。
    pub(crate) fn is_from(&self, pool: &MemoryPool) -> bool {
        core::ptr::eq(Arc::as_ptr(&self.pool), pool)
    }

    pub(crate) fn pages(&self) -> usize {
        usize::try_from(
            self.credit
                .as_ref()
                .expect("MemoryPool charge ownership already transferred")
                .units(),
        )
        .expect("MemoryPool charge does not fit the architecture")
    }

    pub(crate) fn split(&mut self, pages: usize) -> Result<Self, PoolError> {
        let pages = u64::try_from(pages).map_err(|_| PoolError::ArithmeticOverflow)?;
        let credit = self
            .credit
            .as_mut()
            .expect("MemoryPool charge ownership already transferred")
            .split(pages)?;
        Ok(Self {
            pool: Arc::clone(&self.pool),
            credit: Some(credit),
        })
    }
}

impl funded_frame::QuotaCredit for MemoryCharge {
    type Error = PoolError;

    fn split(&mut self, pages: usize) -> Result<Self, Self::Error> {
        MemoryCharge::split(self, pages)
    }

    fn merge(
        &mut self,
        mut other: Self,
    ) -> Result<(), funded_frame::MergeFailure<Self, Self::Error>> {
        let credit = other
            .credit
            .take()
            .expect("MemoryPool charge ownership already transferred");
        match self
            .credit
            .as_mut()
            .expect("MemoryPool charge ownership already transferred")
            .merge(credit)
        {
            Ok(()) => Ok(()),
            Err(failure) => {
                let error = failure.error();
                other.credit = Some(failure.into_token());
                Err(funded_frame::MergeFailure::new(error, other))
            }
        }
    }
}

impl Drop for MemoryCharge {
    fn drop(&mut self) {
        let Some(credit) = self.credit.take() else {
            return;
        };
        MemoryPool::active_state_mut(&mut self.pool.inner.lock())
            .pages
            .return_charge(credit)
            .unwrap_or_else(|error| panic!("MemoryPool charge return failed: {:?}", error.error()));
    }
}

impl KernelObject for MemoryPool {
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }

    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::MemoryPool
    }

    fn allowed_rights(&self) -> Option<Rights> {
        Some(Rights::CREATE | Rights::READ | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT)
    }

    fn allowed_signals(&self) -> Option<erhino_shared::object::ObjectSignals> {
        None
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct PreparedChildOwner {
    parent: Arc<MemoryPool>,
    prepared: Option<(PreparedChild, PreparedChild)>,
}

impl PreparedChildOwner {
    fn commit(mut self) -> (PoolLedgers, Arc<MemoryPool>) {
        let (pages, metadata) = self
            .prepared
            .take()
            .expect("prepared child Pool already completed");
        let state = {
            let mut inner = self.parent.inner.lock();
            let parent = MemoryPool::active_state_mut(&mut inner);
            PoolLedgers {
                pages: parent
                    .pages
                    .commit_child(pages)
                    .expect("prepared page budget belongs to parent"),
                metadata: parent
                    .metadata
                    .commit_child(metadata)
                    .expect("prepared metadata budget belongs to parent"),
            }
        };
        (state, Arc::clone(&self.parent))
    }
}

impl Drop for PreparedChildOwner {
    fn drop(&mut self) {
        let Some((pages, metadata)) = self.prepared.take() else {
            return;
        };
        let mut inner = self.parent.inner.lock();
        let parent = MemoryPool::active_state_mut(&mut inner);
        parent
            .pages
            .rollback_child(pages)
            .expect("prepared page budget belongs to parent");
        parent
            .metadata
            .rollback_child(metadata)
            .expect("prepared metadata budget belongs to parent");
    }
}

fn resolve(
    thread: &Thread,
    handle: Handle,
    required: Rights,
) -> Result<(super::handle::Authorization, Arc<MemoryPool>), SystemCallError> {
    let authority = super::handle::Authorization::accept(thread, handle, required)?;
    let pool = MemoryPool::concrete(authority.object())?;
    Ok((authority, pool))
}

pub fn query(thread: &Thread, handle: Handle, output: usize) -> Result<(), SystemCallError> {
    let (_authority, pool) = resolve(thread, handle, Rights::READ)?;
    crate::uaccess::write_user(thread, output, &pool.snapshot())
}

pub fn derive(
    thread: &Thread,
    handle: Handle,
    request: usize,
    output: usize,
) -> Result<(), SystemCallError> {
    let request: MemoryPoolDeriveRequest = {
        let body = thread
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let mut vm = body.vm.space.lock();
        // SAFETY: 固定宽整数允许任意位型；借存储期间持 VM 锁，随后检查保留位/额度/rights。
        unsafe {
            crate::uaccess::read_user_value(
                vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?,
                request,
            )
        }?
    };
    if request.reserved != 0 || request.budget.is_empty() {
        return Err(SystemCallError::IllegalArgument);
    }
    let rights = request.rights;
    if !rights.is_known() {
        return Err(SystemCallError::RightsDenied);
    }
    let (authority, parent) = resolve(thread, handle, Rights::CREATE)?;
    if !rights.is_subset_of(authority.rights()) {
        return Err(SystemCallError::RightsDenied);
    }
    let header = ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?;
    let reservation = MemoryPool::reserve_delegation(&parent, request.budget)?;
    let child = MemoryPool::prepared_child(Arc::clone(&parent), reservation, header)?;
    let caps = [super::handle::mint(MemoryPool::object_ref(&child), rights)?];
    super::handle::install(
        thread,
        caps,
        |handles| crate::uaccess::write_user(thread, output, &handles[0]),
        || child.commit_prepared(),
    )
    .map(|_| ())
}

pub(crate) fn map_pool_error(error: PoolError) -> SystemCallError {
    match error {
        PoolError::ZeroAmount | PoolError::InvalidSplit | PoolError::InvalidTopology => {
            SystemCallError::IllegalArgument
        }
        PoolError::QuotaExceeded => SystemCallError::QuotaExceeded,
        PoolError::DepthLimit | PoolError::IdentityExhausted => SystemCallError::ReachLimit,
        PoolError::ArithmeticOverflow | PoolError::WrongOwner | PoolError::InvariantViolation => {
            SystemCallError::InternalError
        }
    }
}
