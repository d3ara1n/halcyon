//! MemoryPool capability 对象：额度状态、派生事务与自然父级退款。
//!
//! parent 不登记 child；child core 以强引用持 parent，state 内嵌不可拆分的
//! delegated credit。child/charge/Handle 引用消散后逐锁退款，不同时持有两把 Pool 锁。

use alloc::{sync::Arc, vec::Vec};
use core::any::Any;

use erhino_shared::{
    call::SystemCallError,
    memory_pool::MemoryPoolSnapshot,
    object::{Handle, Rights},
};
use memory_pool::{
    AllocatedCredit, ChargeReservation, DelegationReservation, PoolError, PoolId, PoolState,
    PreparedChild,
};

use super::{
    Thread,
    object::{HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef},
    resources::{MetadataSponsor, PoolCorePermit},
};

static ROOT: crate::sync::Spinlock<Option<Arc<MemoryPool>>> =
    crate::sync::Spinlock::new(crate::sync::ranks::LEAF, None);

pub struct MemoryPool {
    header: ObjectHeader,
    inner: crate::sync::Spinlock<PoolInner>,
    _permit: PoolCorePermit,
}

enum PoolInner {
    Active {
        state: Option<PoolState>,
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
        let permit = PoolCorePermit::primordial().expect("root Pool metadata admission failed");
        let header = ObjectHeader::try_new().expect("root Pool identity exhausted");
        let identity = PoolId::new(header.koid()).expect("kernel object identity must be nonzero");
        let state = PoolState::root(identity, seed.into_pages())
            .expect("platform user supply cannot create the root Pool");
        let total = state.snapshot().total;
        let pool = Arc::try_new(Self {
            header,
            inner: crate::sync::Spinlock::new(
                crate::sync::ranks::MEMORY_POOL,
                PoolInner::Active {
                    state: Some(state),
                    funding: Funding::Root,
                },
            ),
            _permit: permit,
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
        let permit = PoolCorePermit::primordial().expect("Pool self-test admission failed");
        let header = ObjectHeader::try_new().expect("Pool self-test identity exhausted");
        let reservation =
            Self::reserve_delegation(root, PAGES).expect("Pool self-test reservation failed");
        let prepared = Self::prepared_child(Arc::clone(root), reservation, permit, header)
            .expect("Pool self-test child preparation failed");
        drop(prepared);
        assert_eq!(
            root.snapshot(),
            baseline,
            "prepared Pool rollback did not restore its parent"
        );

        let permit = PoolCorePermit::primordial().expect("Pool self-test admission failed");
        let header = ObjectHeader::try_new().expect("Pool self-test identity exhausted");
        let reservation =
            Self::reserve_delegation(root, PAGES).expect("Pool self-test reservation failed");
        let child = Self::prepared_child(Arc::clone(root), reservation, permit, header)
            .expect("Pool self-test child preparation failed");
        child.commit_prepared();
        let child_snapshot = child.snapshot();
        assert_eq!(child_snapshot.parent_identity, baseline.identity);
        assert_eq!(child_snapshot.total, PAGES);
        let parent_committed = root.snapshot();
        assert_eq!(parent_committed.delegated, baseline.delegated + PAGES);
        assert_eq!(parent_committed.available, baseline.available - PAGES);

        let peer = Arc::clone(&child);
        drop(child);
        assert_eq!(
            root.snapshot(),
            parent_committed,
            "Pool credit returned before the last core reference disappeared"
        );
        drop(peer);
        assert_eq!(
            root.snapshot(),
            baseline,
            "committed Pool credit did not return to its parent"
        );
        log!(
            Memory,
            "Pool self-test passed: rollback, commit, and last-reference refund ok"
        );
    }

    pub(crate) fn syscall_self_test(thread: &Thread, root: &Arc<Self>, wrong_object: ObjectRef) {
        let pool_rights =
            Rights::CREATE | Rights::READ | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT;

        assert!(
            super::handle::entry(Self::object_ref(root), HandleRole::JobControl, Rights::READ,)
                .is_err(),
            "Pool accepted a non-Pool Handle role"
        );

        let wrong_entry = super::handle::entry(wrong_object, HandleRole::JobControl, Rights::READ)
            .expect("Pool self-test wrong-kind entry is invalid");
        let wrong_handle = thread
            .process
            .handles
            .lock()
            .insert(wrong_entry)
            .expect("Pool self-test cannot install wrong-kind entry");
        assert_eq!(
            query(thread, wrong_handle, 0),
            Err(SystemCallError::WrongObjectType)
        );
        let wrong_entry = thread
            .process
            .handles
            .lock()
            .remove(wrong_handle)
            .expect("Pool self-test wrong-kind Handle disappeared");
        super::handle::close_entry_infallible(wrong_entry, &thread.process, false);

        let create_only = super::handle::entry(
            Self::object_ref(root),
            HandleRole::MemoryPool,
            Rights::CREATE,
        )
        .expect("Pool self-test CREATE-only entry is invalid");
        let create_only_handle = thread
            .process
            .handles
            .lock()
            .insert(create_only)
            .expect("Pool self-test cannot install CREATE-only entry");
        assert_eq!(
            query(thread, create_only_handle, 0),
            Err(SystemCallError::RightsDenied)
        );
        let create_only = thread
            .process
            .handles
            .lock()
            .remove(create_only_handle)
            .expect("Pool self-test CREATE-only Handle disappeared");
        super::handle::close_entry_infallible(create_only, &thread.process, false);

        let root_entry =
            super::handle::entry(Self::object_ref(root), HandleRole::MemoryPool, pool_rights)
                .expect("Pool self-test root entry is invalid");
        let root_handle = thread
            .process
            .handles
            .lock()
            .insert(root_entry)
            .expect("Pool self-test cannot install root entry");
        assert_eq!(
            query(thread, root_handle, 0),
            Err(SystemCallError::MemoryNotAccessible)
        );
        assert_eq!(
            derive(thread, root_handle, 1, Rights::WRITE, 0),
            Err(SystemCallError::RightsDenied)
        );
        assert_eq!(
            derive(thread, root_handle, 1, Rights::from_raw(1_u64 << 63), 0,),
            Err(SystemCallError::RightsDenied)
        );

        let baseline = root.snapshot();
        let mut permits = Vec::new();
        permits
            .try_reserve_exact(super::resources::POOL_CORES_PER_SPONSOR)
            .expect("Pool self-test permit vector allocation failed");
        for _ in 0..super::resources::POOL_CORES_PER_SPONSOR {
            permits.push(
                MetadataSponsor::reserve_pool_core(thread.process.resources.metadata())
                    .expect("Pool self-test sponsor exhausted early"),
            );
        }
        assert_eq!(
            derive(thread, root_handle, 1, Rights::READ, 0),
            Err(SystemCallError::ReachLimit)
        );
        assert_eq!(
            root.snapshot(),
            baseline,
            "metadata exhaustion changed parent Pool credit"
        );
        drop(permits);

        assert_eq!(
            derive(thread, root_handle, 1, Rights::READ, 0),
            Err(SystemCallError::MemoryNotAccessible)
        );
        assert_eq!(
            root.snapshot(),
            baseline,
            "failed Handle publication changed parent Pool credit"
        );
        let root_entry = thread
            .process
            .handles
            .lock()
            .remove(root_handle)
            .expect("Pool self-test root Handle disappeared");
        super::handle::close_entry_infallible(root_entry, &thread.process, false);
        log!(
            Memory,
            "Pool syscall self-test passed: policy, admission, and publication rollback ok"
        );
    }

    fn prepared_child(
        parent: Arc<Self>,
        reservation: DelegationReservation,
        permit: PoolCorePermit,
        header: ObjectHeader,
    ) -> Result<Arc<Self>, SystemCallError> {
        let identity = PoolId::new(header.koid()).expect("kernel object identity must be nonzero");
        let prepared = match PoolState::prepare_child(identity, reservation) {
            Ok(prepared) => prepared,
            Err(error) => {
                let kind = error.error();
                let reservation = error.into_token();
                Self::active_state_mut(&mut parent.inner.lock())
                    .rollback_delegation(reservation)
                    .unwrap_or_else(|rollback| {
                        panic!("invalid child Pool rollback failed: {:?}", rollback.error())
                    });
                return Err(map_pool_error(kind));
            }
        };
        Arc::try_new(Self {
            header,
            inner: crate::sync::Spinlock::new(
                crate::sync::ranks::MEMORY_POOL,
                PoolInner::Prepared(PreparedChildOwner {
                    parent,
                    prepared: Some(prepared),
                }),
            ),
            _permit: permit,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub(crate) fn object_ref(pool: &Arc<Self>) -> ObjectRef {
        Arc::clone(pool) as ObjectRef
    }

    pub(crate) fn concrete(object: &ObjectRef) -> Result<Arc<Self>, SystemCallError> {
        let any: Arc<dyn Any + Send + Sync> = object.clone();
        any.downcast::<Self>()
            .map_err(|_| SystemCallError::WrongObjectType)
    }

    fn active_state(inner: &PoolInner) -> &PoolState {
        match inner {
            PoolInner::Active { state, .. } => state
                .as_ref()
                .expect("active MemoryPool state already consumed"),
            PoolInner::Prepared(_) | PoolInner::Committing => {
                panic!("unpublished MemoryPool state became observable")
            }
        }
    }

    fn active_state_mut(inner: &mut PoolInner) -> &mut PoolState {
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
        let snapshot = Self::active_state(&inner).snapshot();
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
        }
    }

    fn reserve_delegation(
        parent: &Arc<Self>,
        pages: u64,
    ) -> Result<DelegationReservation, SystemCallError> {
        Self::active_state_mut(&mut parent.inner.lock())
            .reserve_delegation(pages)
            .map_err(map_pool_error)
    }

    pub(crate) fn reserve_charge(
        pool: &Arc<Self>,
        pages: usize,
    ) -> Result<PreparedMemoryCharge, PoolError> {
        let pages = u64::try_from(pages).map_err(|_| PoolError::ArithmeticOverflow)?;
        let reservation = Self::active_state_mut(&mut pool.inner.lock()).reserve_charge(pages)?;
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
                    state.is_fully_available(),
                    "MemoryPool core dropped with outstanding credit"
                );
                match funding {
                    Funding::Root => {
                        assert!(
                            state
                                .into_parent_credit()
                                .expect("root Pool state is quiescent")
                                .is_none(),
                            "root Pool unexpectedly carried parent credit"
                        );
                    }
                    Funding::Child(parent) => {
                        let expected_parent = PoolId::new(parent.header.koid())
                            .expect("kernel object identity must be nonzero");
                        assert_eq!(
                            state.snapshot().parent_identity,
                            Some(expected_parent),
                            "MemoryPool parent identity changed"
                        );
                        let credit = state
                            .into_parent_credit()
                            .expect("child Pool state is quiescent")
                            .expect("child Pool lost parent credit");
                        Self::active_state_mut(&mut parent.inner.lock())
                            .return_delegation(credit)
                            .unwrap_or_else(|error| {
                                panic!(
                                    "MemoryPool parent credit return failed: {:?}",
                                    error.error()
                                )
                            });
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
            .rollback_charge(reservation)
            .unwrap_or_else(|error| {
                panic!("MemoryPool charge rollback failed: {:?}", error.error())
            });
    }
}

impl MemoryCharge {
    pub(crate) fn pages(&self) -> usize {
        usize::try_from(
            self.credit
                .as_ref()
                .expect("MemoryPool charge ownership already transferred")
                .pages(),
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

impl Drop for MemoryCharge {
    fn drop(&mut self) {
        let Some(credit) = self.credit.take() else {
            return;
        };
        MemoryPool::active_state_mut(&mut self.pool.inner.lock())
            .return_charge(credit)
            .unwrap_or_else(|error| panic!("MemoryPool charge return failed: {:?}", error.error()));
    }
}

impl KernelObject for MemoryPool {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::MemoryPool
    }

    fn allowed_rights(&self, role: HandleRole) -> Option<Rights> {
        (role == HandleRole::MemoryPool).then_some(
            Rights::CREATE | Rights::READ | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT,
        )
    }

    fn allowed_signals(&self, _role: HandleRole) -> Option<erhino_shared::object::ObjectSignals> {
        None
    }

    fn close_handle(&self, role: HandleRole, _owner: &super::proc::Process, _exiting: bool) {
        debug_assert!(role == HandleRole::MemoryPool);
    }

    fn close_transit(&self, role: HandleRole) {
        debug_assert!(role == HandleRole::MemoryPool);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct PreparedChildOwner {
    parent: Arc<MemoryPool>,
    prepared: Option<PreparedChild>,
}

impl PreparedChildOwner {
    fn commit(mut self) -> (PoolState, Arc<MemoryPool>) {
        let prepared = self
            .prepared
            .take()
            .expect("prepared child Pool already completed");
        let state = MemoryPool::active_state_mut(&mut self.parent.inner.lock())
            .commit_child(prepared)
            .unwrap_or_else(|error| {
                panic!("prepared child Pool commit failed: {:?}", error.error())
            });
        let parent_identity =
            PoolId::new(self.parent.header.koid()).expect("kernel object identity must be nonzero");
        assert_eq!(
            state.snapshot().parent_identity,
            Some(parent_identity),
            "committed child Pool parent identity changed"
        );
        (state, Arc::clone(&self.parent))
    }
}

impl Drop for PreparedChildOwner {
    fn drop(&mut self) {
        let Some(prepared) = self.prepared.take() else {
            return;
        };
        MemoryPool::active_state_mut(&mut self.parent.inner.lock())
            .rollback_child(prepared)
            .unwrap_or_else(|error| {
                panic!("prepared child Pool rollback failed: {:?}", error.error())
            });
    }
}

fn resolve(
    thread: &Thread,
    handle: Handle,
    required: Rights,
) -> Result<(Arc<MemoryPool>, Rights), SystemCallError> {
    let table = thread.process.handles.lock();
    let entry = table
        .get(handle, required)
        .map_err(super::handle::map_error)?;
    if *entry.role() != HandleRole::MemoryPool || entry.object().kind() != ObjectKind::MemoryPool {
        return Err(SystemCallError::WrongObjectType);
    }
    let rights = entry.rights();
    Ok((MemoryPool::concrete(entry.object())?, rights))
}

pub fn query(thread: &Thread, handle: Handle, output: usize) -> Result<(), SystemCallError> {
    let (pool, _) = resolve(thread, handle, Rights::READ)?;
    let snapshot = pool.snapshot();
    let mut space = thread.process.space.lock();
    space.check_range(output, core::mem::size_of::<MemoryPoolSnapshot>(), true)?;
    // SAFETY: MemoryPoolSnapshot 无 padding；check_range 已验证当前可写范围。
    unsafe { crate::uaccess::deliver_output(thread, &mut space, output, &snapshot) }
}

pub fn derive(
    thread: &Thread,
    handle: Handle,
    pages: u64,
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    if !rights.is_known() {
        return Err(SystemCallError::RightsDenied);
    }
    let (parent, source_rights) = resolve(thread, handle, Rights::CREATE)?;
    if !rights.is_subset_of(source_rights) {
        return Err(SystemCallError::RightsDenied);
    }

    let permit = MetadataSponsor::reserve_pool_core(thread.process.resources.metadata())?;
    let header = ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?;
    let reservation = MemoryPool::reserve_delegation(&parent, pages)?;
    let child = MemoryPool::prepared_child(Arc::clone(&parent), reservation, permit, header)?;
    let entry = super::handle::entry(
        MemoryPool::object_ref(&child),
        HandleRole::MemoryPool,
        rights,
    )
    .map_err(super::handle::map_error)?;

    super::handle::install_one(thread, entry, output, move || {
        child.commit_prepared();
    })
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
