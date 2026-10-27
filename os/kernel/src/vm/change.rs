//! ChangeWork：一次已提交地址空间修改的维护记录（内存分案设计轮次 3）。
//!
//! 阶段（单调）：
//! 1. `Publishing`：每步在空间锁内经 [`AnySpace::step`] 成对推进 ledger 与 PTE。
//! 2. 最后一次 PTE store 之后：最后离开者摘边界表进固定批次，取新 translation
//!    epoch（与责任提交身份分开），在空间锁内确定需覆盖的 active 目标，
//!    `maintenance::arm` 取唤醒根后经内嵌 [`Transport`] 发布；返回 Blocked。
//!    零目标不伪造 ack，同一完成谓词直接成立。
//! 3. `Retiring`：fence 全部确认（含槽归还）后，空间锁内 synchronize/complete
//!    账本；锁外每步放一张表帧或一个退休片段/许可，储备表页锁外退款。
//!
//! 锁序：change 状态（LEAF）→ 空间（ADDRESS_SPACE）→ 收件链（REMOTE_CALL）
//! → 维护队列（MAINTENANCE）；arm/wake 不在空间锁内调用。

use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};

use erhino_shared::call::SystemCallError;
use funded_storage::Buffer;

use maintenance::record::Outcome;

use super::{
    mapping::{MappingRelease, MappingUse},
    remote::{Fence, Transport},
    space::{AnySpace, ChangeProgress, RetireBatch, STEP_UNITS, TableCleanup},
    views::{PreparedView, ViewBoundary, ViewRetirement, Views},
};
use crate::{
    frame::FundedTableFrame,
    maintenance::{self as mt, WakeToken, Work, WorkRecord},
    sync::{Spinlock, ranks},
    task::{memory_pool::MemoryPool, resources::MetadataCharge},
};

/// 地址空间稳定外壳：identity 与 epoch 不随空间锁借用移动；`active` 由执行
/// 进入协议（entry 握手）在空间锁内维护，表示此前进入、可能使用旧翻译的 hart。
pub(crate) struct VmShell {
    pub(crate) identity: usize,
    satp: usize,
    pub(crate) translation_epoch: AtomicU64,
    pub(crate) instruction_epoch: AtomicU64,
    pub(crate) space: Spinlock<VmState>,
    forced: Spinlock<Option<Arc<super::forced::ForcedRetirement>>>,
    _metadata: MetadataCharge,
}

static NEXT_VM_ID: monotonic_id::AtomicIdUsize = monotonic_id::AtomicIdUsize::new(1);

/// 空间锁保护的状态：账本/页表/账户与执行进入集合。
pub(crate) struct VmState {
    /// 整空间退休完成后取走（None 表示已退休，ChangeWork 不再可准入）。
    pub(crate) space: Option<AnySpace>,
    /// 执行与访问准入门，Frozen 后一并关闭。
    pub(crate) entry_open: bool,
    pub(crate) access_open: bool,
    /// 当前执行租约；Frozen 封口时的快照由退休记录稳定持有。
    pub(crate) active: u64,
    /// 已受理的 Builder 操作、显式访问与独立推进的修改责任。
    pub(crate) access_leases: usize,
    building_open: bool,
    start_waiter: Option<WakeToken>,
    /// 退休等待依赖；只有两种租约都离场时才摘取并在锁外唤醒。
    quiescence_waiter: Option<WakeToken>,
    /// 同一纯层预约的观察依赖；容量取唯一 ledger 限额，来源不持 Work。
    pub(crate) dependencies: Buffer<
        (
            memory_space::ChangeKey,
            Arc<dyn crate::task::wait_source::ObservationSource>,
        ),
        MetadataCharge,
    >,
}

impl VmState {
    pub(crate) fn remove_dependency(
        &mut self,
        key: memory_space::ChangeKey,
    ) -> Arc<dyn crate::task::wait_source::ObservationSource> {
        let index = self
            .dependencies
            .iter()
            .position(|(id, _)| *id == key)
            .expect("change reservation lost its completion dependency");
        self.dependencies.remove(index).1
    }
}

/// 访问与修改责任的不可复制持有；提交后由独立 Work 接管至真实完成。
pub(crate) struct VmAccessLease {
    shell: Option<Arc<VmShell>>,
}

impl Drop for VmAccessLease {
    fn drop(&mut self) {
        let shell = self
            .shell
            .take()
            .expect("space access lease released twice");
        let pointer = Arc::as_ptr(&shell);
        let (wake, start) = {
            // SAFETY: VM 由 Body 与退休责任拥有；access_leases 非零时父回收
            // 不能移出 Body/结束退休。持 VM 门先交回本租约强根，必非最后引用；
            // 随后发布计数归零。父回收复检同门，因此解锁前 owner 仍稳定。
            let mut vm = unsafe { &*pointer }.space.lock();
            assert!(vm.access_leases > 0, "space access lease underflow");
            drop(shell);
            vm.access_leases -= 1;
            let wake = if !vm.entry_open && vm.active == 0 && vm.access_leases == 0 {
                vm.quiescence_waiter.take()
            } else {
                None
            };
            let start = (vm.access_leases == 0)
                .then(|| vm.start_waiter.take())
                .flatten();
            (wake, start)
        };
        for wake in [wake, start].into_iter().flatten() {
            drop(mt::wake(wake));
            mt::kick();
        }
    }
}

impl VmShell {
    /// 出生时即固定空间账户；Building 可修改，首次 Start 前不允许执行进入。
    pub(crate) fn try_new(
        space: AnySpace,
        pool: &Arc<MemoryPool>,
    ) -> Result<Arc<Self>, SystemCallError> {
        let metadata = MetadataCharge::arc::<Self>(pool)?;
        let identity = NEXT_VM_ID.allocate().ok_or(SystemCallError::ReachLimit)?;
        let satp = (8usize << 60) | space.satp_ppn();
        let views = space.views().clone();
        let dependencies = Buffer::try_empty(
            space.ledger().transaction_capacity(),
            &crate::task::resources::MetadataFunding(pool),
        )
        .map_err(crate::task::resources::map_storage_error)?;
        let shell = Arc::try_new(Self {
            identity,
            satp,
            translation_epoch: AtomicU64::new(1),
            instruction_epoch: AtomicU64::new(1),
            forced: Spinlock::new(ranks::LEAF, None),
            space: Spinlock::new(
                ranks::ADDRESS_SPACE,
                VmState {
                    space: Some(space),
                    entry_open: false,
                    access_open: true,
                    active: 0,
                    access_leases: 0,
                    building_open: true,
                    start_waiter: None,
                    quiescence_waiter: None,
                    dependencies,
                },
            ),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        let forced = super::forced::ForcedRetirement::new(pool, Arc::downgrade(&shell), views)?;
        *shell.forced.lock() = Some(forced);
        Ok(shell)
    }

    pub(crate) fn forced(&self) -> Option<Arc<super::forced::ForcedRetirement>> {
        self.forced.lock().clone()
    }

    pub(crate) const fn satp(&self) -> usize {
        self.satp
    }

    /// Shell 的启动提交后关闭 Building 修改；既有 lease 由原操作自主完成。
    pub(crate) fn begin_start(&self) {
        self.space.lock().building_open = false;
    }

    /// 启动记录 arm 后复检。成功或 Frozen 都返还 token；只有真实依赖才停驻。
    pub(crate) fn wait_start(&self, token: WakeToken) -> Option<WakeToken> {
        let mut vm = self.space.lock();
        if !vm.access_open {
            return Some(token);
        }
        assert!(
            !vm.building_open,
            "entry opened before Building admission closed"
        );
        if vm.access_leases != 0 {
            assert!(vm.start_waiter.is_none(), "duplicate start waiter");
            vm.start_waiter = Some(token);
            return None;
        }
        vm.entry_open = true;
        Some(token)
    }

    /// Shell 已授予运行准入后调用；空间门内同步本 hart 当前代次并登记执行租约。
    /// 若退休先关闭入口，调用方须在 Shell 门下归还运行准入。
    pub(crate) fn enter(&self, slot: usize) -> bool {
        let mut vm = self.space.lock();
        if !vm.entry_open {
            return false;
        }
        let bit = 1u64.checked_shl(slot as u32).expect("invalid hart slot");
        assert_eq!(vm.active & bit, 0, "hart entered one space twice");
        super::remote::synchronize_local(
            self.identity,
            self.translation_epoch.load(Ordering::Acquire),
            self.instruction_epoch.load(Ordering::Acquire),
        );
        vm.active |= bit;
        true
    }

    /// 已切回内核 root 并发布所有显式访问后注销执行租约。
    pub(crate) fn leave(&self, slot: usize) {
        let wake = {
            let mut vm = self.space.lock();
            let bit = 1u64.checked_shl(slot as u32).expect("invalid hart slot");
            assert_ne!(vm.active & bit, 0, "hart left a space without entry");
            vm.active &= !bit;
            if !vm.entry_open && vm.active == 0 && vm.access_leases == 0 {
                vm.quiescence_waiter.take()
            } else {
                None
            }
        };
        if let Some(wake) = wake {
            drop(mt::wake(wake));
            mt::kick();
        }
    }

    /// 进程 Frozen 后封闭执行和访问准入；返回原始 active 快照。
    pub(crate) fn close_entry(self: &Arc<Self>) -> u64 {
        let forced = self.forced.lock().take();
        let mut vm = self.space.lock();
        vm.entry_open = false;
        vm.access_open = false;
        vm.building_open = false;
        let active = vm.active;
        let start = vm.start_waiter.take();
        let lease = forced.as_ref().map(|_| {
            vm.access_leases = vm
                .access_leases
                .checked_add(1)
                .expect("VM exit lease overflow");
            VmAccessLease {
                shell: Some(self.clone()),
            }
        });
        drop(vm);
        if let (Some(forced), Some(lease)) = (forced, lease) {
            forced.stop(lease);
        }
        if let Some(start) = start {
            drop(mt::wake(start));
            mt::kick();
        }
        active
    }

    /// 受理一份空间操作；已提交的修改必须把该租约移交到自身 Work。
    pub(crate) fn admit_access(self: &Arc<Self>) -> Option<VmAccessLease> {
        self.admit(false)
    }

    pub(crate) fn admit_building(self: &Arc<Self>) -> Option<VmAccessLease> {
        self.admit(true)
    }

    fn admit(self: &Arc<Self>, building: bool) -> Option<VmAccessLease> {
        let mut vm = self.space.lock();
        if !vm.access_open || (building && !vm.building_open) {
            return None;
        }
        vm.access_leases = vm
            .access_leases
            .checked_add(1)
            .expect("space access lease count exhausted");
        Some(VmAccessLease {
            shell: Some(self.clone()),
        })
    }

    /// Process Work 已 arm；返回 None 才可 Blocked，否则凭证交调用方锁外释放。
    pub(crate) fn wait_quiescent(&self, token: WakeToken) -> Option<WakeToken> {
        let mut vm = self.space.lock();
        assert!(
            !vm.entry_open && !vm.access_open,
            "wait before space was frozen"
        );
        if vm.active == 0 && vm.access_leases == 0 {
            assert!(
                vm.dependencies.is_empty(),
                "quiescent VM retained change dependencies"
            );
            return Some(token);
        }
        assert!(
            vm.quiescence_waiter.is_none(),
            "duplicate space quiescence waiter"
        );
        vm.quiescence_waiter = Some(token);
        None
    }

    /// 空间锁内调用：最后一次 PTE store 之后发布新代次，返回本批 fence 内容。
    pub(crate) fn advance_translation(&self) -> Fence {
        let translation = self.translation_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let instruction = self.instruction_epoch.fetch_add(1, Ordering::AcqRel) + 1;
        Fence {
            space_identity: self.identity,
            translation_epoch: translation,
            instruction_epoch: instruction,
        }
    }
}

struct State {
    phase: Phase,
    tables: RetireBatch,
}

#[derive(Default)]
pub(crate) struct Preparation {
    pub(crate) region_storage: memory_space::RegionStorage<crate::task::resources::MetadataCharge>,
    pub(crate) backing_storage: super::backing::BackingStorage,
    pub(crate) plan: PreparationPlan,
    pub(crate) permits: Buffer<memory_space::WritePermit, MetadataCharge>,
    pub(crate) unindexed: Option<(Arc<crate::task::memory_object::MemoryObjectCore>, usize)>,
    pub(crate) usage: Option<MappingUse>,
    pub(crate) view: Option<PreparedView>,
    pub(crate) boundaries: Buffer<ViewBoundary, MetadataCharge>,
    pub(crate) frames: Buffer<FundedTableFrame, MetadataCharge>,
    pub(crate) cursors: Buffer<page_table::PublishCursor, MetadataCharge>,
    release: Option<MappingRelease>,
}

/// 同一计划 owner 的互斥阶段；空候选也由 Candidate 表示，不并排内联三份载荷。
pub(crate) enum PreparationPlan {
    Candidate(memory_space::ChangeStorage<MetadataCharge>),
    Validated(memory_space::ValidatedChange<MetadataCharge>),
    Reserved(memory_space::PreparedChange<MetadataCharge>),
}
impl Default for PreparationPlan {
    fn default() -> Self {
        Self::Candidate(memory_space::ChangeStorage::default())
    }
}
impl PreparationPlan {
    pub(crate) fn candidate_mut(&mut self) -> &mut memory_space::ChangeStorage<MetadataCharge> {
        match self {
            Self::Candidate(value) => value,
            _ => panic!("operation does not own candidate storage"),
        }
    }
    pub(crate) fn validated(&self) -> &memory_space::ValidatedChange<MetadataCharge> {
        match self {
            Self::Validated(value) => value,
            _ => panic!("operation does not own a validated plan"),
        }
    }
    pub(crate) fn validated_mut(&mut self) -> &mut memory_space::ValidatedChange<MetadataCharge> {
        match self {
            Self::Validated(value) => value,
            _ => panic!("operation does not own a validated plan"),
        }
    }
    pub(crate) fn reserved(&self) -> &memory_space::PreparedChange<MetadataCharge> {
        match self {
            Self::Reserved(value) => value,
            _ => panic!("operation does not own a reserved plan"),
        }
    }
    pub(crate) fn take_validated(&mut self) -> memory_space::ValidatedChange<MetadataCharge> {
        match core::mem::take(self) {
            Self::Validated(value) => value,
            _ => panic!("operation does not own a validated plan"),
        }
    }
    pub(crate) fn take_reserved(&mut self) -> memory_space::PreparedChange<MetadataCharge> {
        match core::mem::take(self) {
            Self::Reserved(value) => value,
            _ => panic!("operation does not own a reserved plan"),
        }
    }
}

/// 出生记录先于首份数据/许可。未提交离开作用域时，原记录接管全部失败责任。
pub(crate) struct Operation {
    work: Arc<ChangeWork>,
    pub(crate) prep: Preparation,
    submitted: bool,
}
impl Operation {
    fn attach_mmio_publication(&mut self, publication: crate::resource::mmio::MmioPublication) {
        let work = Arc::get_mut(&mut self.work).expect("unpublished operation has another owner");
        assert!(
            work.completion.publication.is_none(),
            "MMIO publication attached twice"
        );
        work.completion.publication = Some(publication);
    }

    pub(crate) fn prepare_mmio(
        &mut self,
        node: Arc<crate::resource::mmio::MmioNode>,
        admission: &resource_tree::Admission,
        offset: usize,
        bytes: usize,
    ) -> Result<memory_space::MappingSource, SystemCallError> {
        assert!(
            self.prep.usage.is_none(),
            "map operation already owns a resource"
        );
        let (usage, publication) = crate::resource::mmio::MmioMapping::prepare(
            node,
            admission,
            &self.work.shell,
            self.pool(),
            offset,
            bytes,
            self.completion_source(),
        )?;
        let mapping = usage.mapping().id();
        self.prep.usage = Some(usage.into());
        self.attach_mmio_publication(publication);
        Ok(memory_space::MappingSource::Mmio { mapping, bytes })
    }

    pub(crate) fn advance_publication(&mut self) -> Result<bool, SystemCallError> {
        let work = Arc::get_mut(&mut self.work).expect("unpublished operation has another owner");
        match work.completion.publication.as_mut() {
            Some(publication) => publication.advance(),
            None => Ok(true),
        }
    }

    pub(crate) fn grant_publication(&self) -> Result<(), SystemCallError> {
        if let Some(publication) = &self.work.completion.publication {
            publication.grant()?;
        }
        Ok(())
    }

    pub(crate) fn commit_publication(&self) {
        if let Some(publication) = &self.work.completion.publication {
            publication.commit();
        }
    }

    pub(crate) fn completion_source(&self) -> Arc<dyn crate::task::wait_source::ObservationSource> {
        self.work.completion.source()
    }

    pub(crate) fn pool(&self) -> &Arc<MemoryPool> {
        self.work.completion.payer()
    }
    pub(crate) fn new(
        pool: Arc<MemoryPool>,
        shell: Arc<VmShell>,
        views: Arc<Views>,
    ) -> Result<Self, SystemCallError> {
        let access = shell.admit_access().ok_or(SystemCallError::ObjectClosed)?;
        Self::with_access(pool, shell, views, access)
    }

    pub(crate) fn with_access(
        pool: Arc<MemoryPool>,
        shell: Arc<VmShell>,
        views: Arc<Views>,
        access: VmAccessLease,
    ) -> Result<Self, SystemCallError> {
        let work = ChangeWork::new(pool, shell, access, views)?;
        Ok(Self {
            work,
            prep: Preparation::default(),
            submitted: false,
        })
    }
    pub(crate) fn submit(mut self, progress: ChangeProgress) -> crate::task::wait::WaitPlan {
        assert!(
            matches!(self.prep.plan, PreparationPlan::Candidate(_))
                && self.prep.permits.is_empty()
                && self.prep.unindexed.is_none()
                && self.prep.usage.is_none()
                && self.prep.view.is_none()
                && self.prep.boundaries.is_empty()
                && self.prep.frames.is_empty()
                && self.prep.cursors.is_empty()
                && self.prep.release.is_none(),
            "committed VM operation retained preparation resources"
        );
        // 空 paid block 仍是责任；发布 Work 前锁外释放，DONE 不能领先这些退款。
        drop(core::mem::take(&mut self.prep));
        self.submitted = true;
        let wait = self.work.wait_plan();
        self.work.start(progress);
        wait
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        if self.submitted {
            return;
        }
        {
            let mut state = self.work.state.lock();
            assert!(
                matches!(state.phase, Phase::Idle),
                "VM preparation lost its exclusive owner"
            );
            state.phase = Phase::Rollback(core::mem::take(&mut self.prep));
        }
        let root: Arc<dyn Work> = self.work.clone();
        mt::publish(root);
        mt::kick();
    }
}

pub(super) enum Phase {
    Idle,
    /// 阶段被唯一 Work 执行 owner 移到锁外；外部观察仍不能认为完成。
    Advancing,
    Rollback(Preparation),
    RollbackFence(Preparation),
    Publishing(ChangeProgress),
    Fencing {
        committed: memory_space::CommittedChange<MetadataCharge>,
        reserve: Buffer<FundedTableFrame, MetadataCharge>,
        footprint: memory_space::PageRange,
    },
    Retiring {
        retiring: Option<memory_space::RetiringChange<MetadataCharge>>,
        reserve: Buffer<FundedTableFrame, MetadataCharge>,
        fragment: Option<ViewRetirement>,
        footprint: memory_space::PageRange,
    },
    Cleaning(Cleanup),
    TableFence(Cleanup),
    TableRefund(Cleanup),
    Done(Option<memory_space::ChangeStorage<MetadataCharge>>),
}

pub(super) struct Cleanup {
    retiring: Option<memory_space::RetiringChange<MetadataCharge>>,
    storage: Option<memory_space::ChangeStorage<MetadataCharge>>,
    cursor: TableCleanup,
    final_sweep: bool,
}

pub(crate) struct ChangeWork {
    record: WorkRecord,
    shell: Arc<VmShell>,
    views: Arc<Views>,
    /// 出生时准入的内嵌运输与固定表帧批次（批间复用，不做可失败准入）。
    transport: Transport,
    /// 阶段与固定表帧批次同受一把锁：step 已由记录状态串行，批次不另设锁，
    /// 也避免在空间锁（ADDRESS_SPACE）内逆序获取 LEAF 锁。
    state: Spinlock<State>,
    completion: super::completion::Completion,
}

impl ChangeWork {
    /// 提交前可失败出生；提交后只移交进度并发布，不再分配或获取容量。
    pub(crate) fn new(
        pool: Arc<MemoryPool>,
        shell: Arc<VmShell>,
        access: VmAccessLease,
        views: Arc<Views>,
    ) -> Result<Arc<Self>, SystemCallError> {
        let metadata = MetadataCharge::arc::<Self>(&pool)?;
        let source = crate::task::wait_source::SignalSource::try_new(pool.clone())?;
        let work = Arc::try_new(Self {
            record: WorkRecord::new(pool.maintenance_account().clone()),
            shell,
            views,
            transport: Transport::new(),
            state: Spinlock::new(
                ranks::LEAF,
                State {
                    phase: Phase::Idle,
                    tables: RetireBatch::new(),
                },
            ),
            completion: super::completion::Completion::new(metadata, Some(access), source, false),
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(work)
    }

    pub(crate) fn start(self: &Arc<Self>, progress: ChangeProgress) {
        {
            let mut state = self.state.lock();
            assert!(
                matches!(state.phase, Phase::Idle),
                "change work started twice"
            );
            state.phase = Phase::Publishing(progress);
        }
        let root: Arc<dyn Work> = self.clone();
        mt::publish(root);
        mt::kick();
    }

    pub(crate) fn wait_plan(&self) -> crate::task::wait::WaitPlan {
        crate::task::wait::WaitPlan::completion(self.completion.source())
    }
}

/// 普通修改与预付强制退休共用同一发布、fence、View 退休和空表收束协议。
pub(super) struct ChangeContext<'a> {
    pub(super) shell: &'a Arc<VmShell>,
    pub(super) views: &'a Arc<Views>,
    pub(super) transport: &'a Transport,
}

impl ChangeContext<'_> {
    pub(super) fn advance(
        &self,
        phase: &mut Phase,
        tables: &mut RetireBatch,
        root: &Arc<dyn Work>,
    ) -> Outcome {
        match phase {
            Phase::Idle | Phase::Advancing => unreachable!("change work has no executable phase"),
            Phase::Publishing(_) => self.step_publish(phase, root),
            Phase::Rollback(_) => self.step_rollback(phase, tables, root),
            Phase::RollbackFence(_) => {
                assert!(
                    self.transport.idle(),
                    "VM retirement advanced before fence completion"
                );
                let Phase::RollbackFence(prep) = core::mem::replace(phase, Phase::Advancing) else {
                    unreachable!()
                };
                *phase = Phase::Rollback(prep);
                Outcome::Ready
            }
            Phase::Fencing { .. } => self.step_synchronize(phase),
            Phase::Retiring { .. } => self.step_retire(phase),
            Phase::Cleaning(_) => self.step_cleanup(phase, tables, root),
            Phase::TableFence(_) => {
                assert!(
                    self.transport.idle(),
                    "VM retirement advanced before fence completion"
                );
                let Phase::TableFence(cleanup) = core::mem::replace(phase, Phase::Advancing) else {
                    unreachable!()
                };
                *phase = Phase::TableRefund(cleanup);
                Outcome::Ready
            }
            Phase::TableRefund(_) => self.step_table_refund(phase, tables),
            Phase::Done(_) => Outcome::Done,
        }
    }

    fn step_rollback(
        &self,
        phase: &mut Phase,
        tables: &mut RetireBatch,
        root: &Arc<dyn Work>,
    ) -> Outcome {
        let Phase::Rollback(prep) = phase else {
            unreachable!()
        };
        for _ in 0..STEP_UNITS {
            if matches!(prep.plan, PreparationPlan::Reserved(_)) {
                let prepared = prep.plan.take_reserved();
                assert!(prep.permits.is_empty(), "reserved permits have two owners");
                let mut vm = self.shell.space.lock();
                let space = vm.space.as_mut().expect("rollback lost live space");
                let range = space.ledger().prepared_footprint(&prepared);
                let key = prepared.key();
                space.detach_boundary_tables(prepared.key(), range, tables);
                let storage = space.ledger_mut().rollback(prepared);
                let dependency = vm.remove_dependency(key);
                let notice =
                    (!tables.is_empty()).then(|| (self.shell.advance_translation(), vm.active));
                drop(vm);
                drop(dependency);
                prep.plan = PreparationPlan::Candidate(storage);
                if let Some((fence, mask)) = notice {
                    let Phase::Rollback(prep) = core::mem::replace(phase, Phase::Advancing) else {
                        unreachable!()
                    };
                    *phase = Phase::RollbackFence(prep);
                    let wake = mt::arm(root);
                    if let Some(wake) = self.transport.publish(mask, fence, wake) {
                        drop(mt::wake(wake));
                    }
                    return Outcome::Blocked;
                }
            } else if matches!(prep.plan, PreparationPlan::Validated(_)) {
                prep.plan = PreparationPlan::Candidate(prep.plan.take_validated().into_storage());
            } else if let Some(frame) = tables.pop() {
                // 只有 RollbackFence 完全确认后才再次进入此分支。
                drop(frame);
            } else if let Some((object, suffix)) = prep.unindexed.as_ref() {
                if prep.permits.len() > *suffix {
                    object.cancel_write(prep.permits.pop().expect("unindexed suffix disappeared"));
                } else {
                    prep.unindexed.take();
                }
            } else if let Some(permit) = prep.permits.pop() {
                self.views.cancel_write(permit);
            } else if let step @ (memory_space::RollbackStep::Progress
            | memory_space::RollbackStep::Permit(_)) =
                prep.plan.candidate_mut().rollback_step()
            {
                if let memory_space::RollbackStep::Permit(permit) = step {
                    self.views.cancel_write(permit);
                }
            } else if let Some(boundary) = prep.boundaries.pop() {
                drop(boundary);
            } else if let Some(view) = prep.view.take() {
                assert!(prep.usage.is_none(), "prepared view has duplicate use");
                prep.usage = Some(view.cancel());
            } else if let Some(usage) = prep.usage.take() {
                prep.release = Some(usage.retire());
            } else if let Some(release) = prep.release.as_mut() {
                if release.step() {
                    prep.release.take();
                }
            } else if let Some(frame) = prep.frames.pop() {
                drop(frame);
            } else {
                *phase = Phase::Done(None);
                return Outcome::Done;
            }
        }
        Outcome::Ready
    }

    /// 发布阶段一步；完成后进入 fence。
    fn step_publish(&self, phase: &mut Phase, root: &Arc<dyn Work>) -> Outcome {
        let Phase::Publishing(progress) = phase else {
            unreachable!()
        };
        let mut vm = self.shell.space.lock();
        if !vm
            .space
            .as_mut()
            .expect("change on retired space")
            .step(progress)
        {
            return Outcome::Ready;
        }
        let footprint = progress.footprint();
        let fence = self.shell.advance_translation();
        let mask = vm.active;
        drop(vm);
        let Phase::Publishing(progress) = core::mem::replace(phase, Phase::Advancing) else {
            unreachable!()
        };
        let (committed, reserve) = progress.finish();
        *phase = Phase::Fencing {
            committed,
            reserve,
            footprint,
        };
        let wake = mt::arm(root);
        if let Some(wake) = self.transport.publish(mask, fence, wake) {
            drop(mt::wake(wake));
        }
        Outcome::Blocked
    }

    fn step_synchronize(&self, phase: &mut Phase) -> Outcome {
        assert!(
            self.transport.idle(),
            "VM retirement advanced before fence completion"
        );
        let Phase::Fencing {
            committed,
            reserve,
            footprint,
        } = core::mem::replace(phase, Phase::Advancing)
        else {
            unreachable!()
        };
        let retiring = self
            .shell
            .space
            .lock()
            .space
            .as_mut()
            .expect("change on retired space")
            .ledger_mut()
            .synchronize(committed);
        *phase = Phase::Retiring {
            retiring: Some(retiring),
            reserve,
            fragment: None,
            footprint,
        };
        Outcome::Ready
    }

    fn step_retire(&self, phase: &mut Phase) -> Outcome {
        let Phase::Retiring {
            retiring,
            reserve,
            fragment,
            ..
        } = phase
        else {
            unreachable!()
        };
        for _ in 0..STEP_UNITS {
            if let Some(current) = fragment.as_mut() {
                if self.views.release_fragment_step(current) {
                    fragment.take();
                }
            } else if let Some(next) = retiring.as_mut().expect("retire lost owner").pop_fragment()
            {
                *fragment = Some(ViewRetirement::new(next));
            } else if let Some(permit) = retiring.as_mut().expect("retire lost owner").pop_permit()
            {
                self.views.release_permit(permit);
            } else if let Some(frame) = reserve.pop() {
                drop(frame);
            } else {
                let Phase::Retiring {
                    retiring,
                    footprint,
                    ..
                } = core::mem::replace(phase, Phase::Advancing)
                else {
                    unreachable!()
                };
                *phase = Phase::Cleaning(Cleanup {
                    retiring,
                    storage: None,
                    cursor: TableCleanup::new(footprint),
                    final_sweep: false,
                });
                return Outcome::Ready;
            }
        }
        Outcome::Ready
    }

    /// 内部表按固定批次摘出；边界复查与退出预约同锁，最后离开者不能互相漏扫。
    fn step_cleanup(
        &self,
        phase: &mut Phase,
        tables: &mut RetireBatch,
        root: &Arc<dyn Work>,
    ) -> Outcome {
        let Phase::Cleaning(cleanup) = phase else {
            unreachable!()
        };
        let mut vm = self.shell.space.lock();
        let mut dependency = None;
        let space = vm.space.as_mut().expect("table cleanup on retired space");
        let owner = cleanup
            .retiring
            .as_ref()
            .expect("cleanup lost reservation")
            .key();
        if tables.is_empty() && !cleanup.final_sweep {
            if !space.detach_interior_tables(owner, &mut cleanup.cursor, tables) {
                if tables.is_empty() {
                    return Outcome::Ready;
                }
            } else if tables.is_empty() {
                // 一个批次容纳两个端点的所有层级；此处不得中途释放 VM 锁。
                space.detach_boundary_tables(owner, cleanup.cursor.range(), tables);
                cleanup.final_sweep = true;
            }
        }
        if cleanup.final_sweep {
            cleanup.storage = Some(
                space
                    .ledger_mut()
                    .complete(cleanup.retiring.take().expect("cleanup completed twice")),
            );
            dependency = Some(vm.remove_dependency(owner));
            if tables.is_empty() {
                drop(vm);
                drop(dependency);
                *phase = Phase::Done(cleanup.storage.take());
                return Outcome::Done;
            }
        }
        if tables.is_empty() {
            return Outcome::Ready;
        }
        // 被摘表帧在下一轮 fence 全 ack（含 slot 归还）前始终留在批次内。
        let fence = self.shell.advance_translation();
        let mask = vm.active;
        drop(vm);
        drop(dependency);
        let Phase::Cleaning(cleanup) = core::mem::replace(phase, Phase::Advancing) else {
            unreachable!()
        };
        *phase = Phase::TableFence(cleanup);
        let wake = mt::arm(root);
        if let Some(wake) = self.transport.publish(mask, fence, wake) {
            drop(mt::wake(wake));
        }
        Outcome::Blocked
    }

    fn step_table_refund(&self, phase: &mut Phase, tables: &mut RetireBatch) -> Outcome {
        for _ in 0..STEP_UNITS {
            if let Some(frame) = tables.pop() {
                drop(frame);
            } else {
                let Phase::TableRefund(cleanup) = core::mem::replace(phase, Phase::Advancing)
                else {
                    unreachable!()
                };
                if cleanup.final_sweep {
                    assert!(
                        cleanup.retiring.is_none(),
                        "final table refund retained reservation"
                    );
                    *phase = Phase::Done(cleanup.storage);
                    return Outcome::Done;
                }
                *phase = Phase::Cleaning(cleanup);
                return Outcome::Ready;
            }
        }
        Outcome::Ready
    }
}

impl Work for ChangeWork {
    fn record(&self) -> &WorkRecord {
        &self.record
    }

    fn step(self: Arc<Self>) -> Outcome {
        let root: Arc<dyn Work> = self.clone();
        let (mut phase, mut tables) = {
            let mut state = self.state.lock();
            (
                core::mem::replace(&mut state.phase, Phase::Advancing),
                core::mem::replace(&mut state.tables, RetireBatch::new()),
            )
        };
        let context = ChangeContext {
            shell: &self.shell,
            views: &self.views,
            transport: &self.transport,
        };
        let outcome = context.advance(&mut phase, &mut tables, &root);
        let remote = (outcome == Outcome::Blocked
            && matches!(
                phase,
                Phase::Fencing { .. } | Phase::TableFence(_) | Phase::RollbackFence(_)
            ))
        .then(|| self.transport.pending());
        {
            let mut state = self.state.lock();
            assert!(
                matches!(state.phase, Phase::Advancing),
                "change phase moved twice"
            );
            state.phase = phase;
            state.tables = tables;
        }
        if let Some(mask) = remote {
            super::remote::kick(mask);
        }
        outcome
    }
}
