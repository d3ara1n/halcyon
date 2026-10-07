//! Job：进程创建域与资源预算的结构根（notes/ideas/task.md「Job」）。
//!
//! 所有权图（单一生命周期根，无双向强持）：
//! 内核 static anchor ─strong→ root Job；parent Job ─strong→ child Jobs；
//! child ─weak→ parent；Job 直接成员表 ─strong→ 未 Dead 的 Process cores；
//! Process ─weak→ Job。JobControl Handle 只强持 authority 视图——关闭最后
//! 一个 JobControl 仅消散 authority，不影响层级根与成员。成员表同时承载
//! ProcessCreate 的占位事务；jid/parent_jid 在创建时冻结为不可变字段
//! （Dead 后父对象可先释放，快照仍可应答）。
//!
//! 锁序规范（顶级）：**Job 链锁（先父后子，≤JOB_DEPTH_MAX 把）→ lifecycle
//! 锁 → 其他对象锁**。创建/启动提交点在链锁内线性化「上行检查祖先 seal
//! 以及锁内分配 ID、占位插入」，与 JobSeal（持单锁）在 owner 锁上互斥，
//! 先到者定胜负；ProcessStart 的提交闸门在同一链锁内嵌套调用
//! lifecycle 线性化（锁序允许方向）。JobInner 锁内只改成员/子表与
//! sealed/dead 位，不出游取锁、不做 uaccess；CLOSED 发布与完成传播在
//! 锁外执行（对象 wait 锁不与 JobInner 锁嵌套），传播沿父链逐级
//! 「放子锁、取父锁」，每层由该 Job 的出生 Work 接管，单步有界
//! （延迟触发安全：sealed ⇒ 无新成员，判定幂等）。
//!
//! 成员/子表是按 ID 有序的 fallible AVL：ID 在 owner 锁内分配并与占位
//! 插入同临界区，查找、提交、回滚和完成摘除均不分配且受树高硬界约束；
//! 枚举按 ID 单调分页，遇未决占位即终止本批（屏障窗口只存在于创建方的
//! 单个 syscall 内，枚举方重试不活锁）。

use alloc::sync::{Arc, Weak};
use core::any::Any;

use super::resources::{MetadataCharge, MetadataFunding, map_storage_error};
use funded_storage::Buffer;
use maintenance::record::Outcome;
use ordered_table::{InsertError, OrderedTable, PreparedEntry};

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    proc::{
        JOB_ENUMERATE_MAX, JobEnumerateResult, JobId, JobMemberKind, JobSnapshot, JobState, Pid,
    },
};
/// Job 的完整业务授权，由 Job 域拥有；不可借用 ProcessControl 的掩码。
pub(crate) const JOB_RIGHTS: Rights = Rights::from_raw(
    Rights::CREATE.raw()
        | Rights::MANAGE.raw()
        | Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);

use super::{
    Thread,
    memory_pool::MemoryPool,
    object::{KernelObject, ObjectHeader, ObjectKind, ObjectRef},
    process::shell::ProcessShell,
    wait_source::{ObservationSource, SignalSource},
};
use crate::maintenance::{self as mt, Work, WorkRecord};

/// Job 层级深度硬上限（含 root）。
pub(crate) const JOB_DEPTH_MAX: usize = 32;
/// 单个 Job 的直接成员或直接 child 数上限。它同时把 AVL 高度与完成路径
/// 的比较次数封成常量；容量耗尽在创建口返回 ReachLimit。
const JOB_DIRECT_ENTRY_LIMIT: usize = 65_536;

fn table_insert_error<V>(error: InsertError<V, SystemCallError>) -> SystemCallError {
    match error {
        InsertError::Limit(_) => SystemCallError::ReachLimit,
        InsertError::Storage(error, _) => map_storage_error(error),
    }
}

/// 内核侧成员表条目：事务 marker 或强持的稳定 ProcessShell。
#[derive(Clone)]
pub(crate) enum MemberEntry {
    /// 事务预留：Create 提交前占位，对枚举与派生查找不可见（屏障条目）。
    Reserved { token: u64 },
    /// 未 Dead 的 Process core（Building 起，Dead 发布时移除）。
    Process(Arc<ProcessShell>),
}

/// 内核侧 Job 层级条目：事务 marker 或强持的 child Job。
#[derive(Clone)]
enum ChildEntry {
    /// JobCreate 事务预留：对枚举与派生查找不可见（屏障条目）。
    Reserved {
        token: u64,
    },
    Job(Arc<Job>),
}

enum JobPhase {
    Open,
    Sealing { cursor: JobId },
    Waiting { token: mt::WakeToken },
    Dead,
}

pub(crate) struct JobInner {
    /// 直接 child Jobs（按 JobId 升序；含事务占位）。
    children: OrderedTable<ChildEntry, MetadataCharge>,
    /// 直接成员（按 Pid 升序；含事务占位）。
    members: OrderedTable<MemberEntry, MetadataCharge>,
    phase: JobPhase,
    /// 父表已摘除、但 child CLOSED 尚未发布的步内责任。
    closing_children: usize,
}

impl JobInner {
    fn is_sealed(&self) -> bool {
        !matches!(self.phase, JobPhase::Open)
    }

    fn is_dead(&self) -> bool {
        matches!(self.phase, JobPhase::Dead)
    }

    /// 完成事实同锁发布；只有 Waiting 需要取出凭证交门外唤醒。
    /// Sealing 已有稳定 Ready/Running 根，最后依赖无需重复 publish。
    fn complete_if_ready(&mut self) -> Option<mt::WakeToken> {
        if self.is_sealed()
            && !self.is_dead()
            && self.members.is_empty()
            && self.children.is_empty()
            && self.closing_children == 0
        {
            return match core::mem::replace(&mut self.phase, JobPhase::Dead) {
                JobPhase::Waiting { token } => Some(token),
                _ => None,
            };
        }
        None
    }
}

pub struct Job {
    /// capability 持有计数（ZeroPolicy 按类型固定）。
    holders: capability::Holders,
    header: ObjectHeader,
    /// 全局单调不复用；root 是首个分配者，恒为 1。
    jid: JobId,
    /// 创建时冻结的父 JobId（root 为 0）；不依赖 weak 父的存活。
    parent_jid: JobId,
    parent: Option<Weak<Job>>,
    _pool: Arc<MemoryPool>,
    record: WorkRecord,
    state: crate::sync::Spinlock<JobInner>,
    source: Arc<SignalSource>,
    _metadata: super::resources::MetadataCharge,
}

/// root Job 的内核 static anchor（强持至本次启动结束）。
static ROOT: crate::sync::Spinlock<Option<Arc<Job>>> =
    crate::sync::Spinlock::new(crate::sync::ranks::LEAF, None);

/// 下一个 JobId（单调不复用；root 恒 1 = 首次分配）。
static NEXT_JID: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

/// 持久 PID 分配器（单调不复用；生命周期根是 Job 直接成员表）。
static NEXT_PID: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

/// 下一个成员/子表事务 token。
static NEXT_MEMBER_TOKEN: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

fn alloc_jid() -> Option<JobId> {
    NEXT_JID.allocate()
}

pub fn alloc_pid() -> Option<Pid> {
    NEXT_PID.allocate()
}

fn next_member_token() -> Result<u64, SystemCallError> {
    NEXT_MEMBER_TOKEN
        .allocate()
        .ok_or(SystemCallError::ReachLimit)
}

/// 成员表事务预留凭据。
#[derive(Clone, Copy)]
pub(crate) struct MemberReservation {
    pid: Pid,
    token: u64,
}

/// 子表事务预留凭据。
#[derive(Clone, Copy)]
pub(crate) struct ChildReservation {
    jid: JobId,
    token: u64,
}

/// 自 owner 向上收集祖先链并反转为 root-first（锁按此序获取）。
/// 祖先 weak 升级失败意味着该祖先已完成释放——完成的 Job 必然
/// sealed，创建/启动按 ObjectClosed 拒绝（永不错指新对象）。
fn collect_chain(owner: &Arc<Job>) -> Result<Buffer<Arc<Job>, MetadataCharge>, SystemCallError> {
    let mut upward = Buffer::try_empty(JOB_DEPTH_MAX, &MetadataFunding(&owner._pool))
        .map_err(map_storage_error)?;
    let mut current = owner.clone();
    loop {
        if upward.len() == JOB_DEPTH_MAX {
            return Err(SystemCallError::IllegalArgument);
        }
        upward.push(current.clone());
        let Some(parent) = current.parent.as_ref() else {
            break;
        };
        match parent.upgrade() {
            Some(strong) => current = strong,
            None => return Err(SystemCallError::ObjectClosed),
        }
    }
    upward.reverse();
    Ok(upward)
}

/// 先父后子锁住整条链（守卫按 root→owner 顺序持有）。
fn lock_chain(
    chain: &[Arc<Job>],
) -> Result<Buffer<crate::sync::SpinlockGuard<'_, JobInner>, MetadataCharge>, SystemCallError> {
    let pool = &chain.last().expect("chain always contains the owner")._pool;
    let mut guards =
        Buffer::try_empty(chain.len(), &MetadataFunding(pool)).map_err(map_storage_error)?;
    for job in chain {
        guards.push(job.state.lock());
    }
    Ok(guards)
}

impl Job {
    /// root Job：内核 static anchor 强持至本次启动结束；boot 交给 init 的
    /// Handle 只是 authority，不是生命周期根。完成发 CLOSED 但不移除
    /// 不释放（boot 生命周期）。
    pub fn root(pool: &Arc<MemoryPool>) -> Arc<Self> {
        let mut root = ROOT.lock();
        if root.is_none() {
            let metadata = super::resources::MetadataCharge::arc::<Self>(pool)
                .expect("root Job metadata funding failed");
            let jid = alloc_jid().expect("root Job identity exhausted");
            *root = Some(
                Arc::try_new(Self {
                    holders: capability::Holders::new(capability::ZeroPolicy::Remintable),
                    header: ObjectHeader::new(),
                    jid,
                    parent_jid: 0,
                    parent: None,
                    _pool: pool.clone(),
                    record: WorkRecord::new(pool.maintenance_account().clone()),
                    state: crate::sync::Spinlock::chained(
                        crate::sync::ranks::JOB_INNER,
                        jid,
                        JobInner {
                            children: OrderedTable::new(JOB_DIRECT_ENTRY_LIMIT),
                            members: OrderedTable::new(JOB_DIRECT_ENTRY_LIMIT),
                            phase: JobPhase::Open,
                            closing_children: 0,
                        },
                    ),
                    source: SignalSource::try_new(pool.clone())
                        .expect("root Job source allocation failed"),
                    _metadata: metadata,
                })
                .expect("root Job allocation failed"),
            );
        }
        root.clone().expect("root Job anchor is initialized")
    }

    fn child(
        jid: JobId,
        parent_jid: JobId,
        parent: &Arc<Self>,
        pool: Arc<MemoryPool>,
    ) -> Result<Arc<Self>, SystemCallError> {
        let account = pool.maintenance_account().clone();
        let metadata = super::resources::MetadataCharge::arc::<Self>(&pool)?;
        Arc::try_new(Self {
            holders: capability::Holders::new(capability::ZeroPolicy::Remintable),
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            jid,
            parent_jid,
            parent: Some(Arc::downgrade(parent)),
            _pool: pool.clone(),
            record: WorkRecord::new(account),
            state: crate::sync::Spinlock::chained(
                crate::sync::ranks::JOB_INNER,
                jid,
                JobInner {
                    children: OrderedTable::new(JOB_DIRECT_ENTRY_LIMIT),
                    members: OrderedTable::new(JOB_DIRECT_ENTRY_LIMIT),
                    phase: JobPhase::Open,
                    closing_children: 0,
                },
            ),
            source: SignalSource::try_new(pool)?,
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub fn object_ref(job: &Arc<Self>) -> ObjectRef {
        job.clone()
    }

    /// 从 Handle 表条目取具体 Job（trait upcasting 后 downcast）。
    pub(crate) fn concrete(object: &ObjectRef) -> Result<Arc<Self>, SystemCallError> {
        let any: Arc<dyn Any + Send + Sync> = object.clone();
        any.downcast::<Self>()
            .map_err(|_| SystemCallError::WrongObjectType)
    }

    /// ProcessCreate 创建口闸门：链锁内上行检查祖先 seal、锁内分配
    /// Pid 并把占位插入成员表（同一临界区，表内 ID 序 = 分配序）。
    /// 失败在一切副作用之前返回。
    pub(crate) fn gate_reserve_member(
        self: &Arc<Self>,
    ) -> Result<(Pid, MemberReservation), SystemCallError> {
        let mut prepared = PreparedEntry::try_new(
            0,
            MemberEntry::Reserved { token: 0 },
            &MetadataFunding(&self._pool),
        )
        .map_err(table_insert_error)?;
        let chain = collect_chain(self)?;
        let mut guards = lock_chain(&chain)?;
        if guards.iter().any(|state| state.is_sealed()) {
            return Err(SystemCallError::ObjectClosed);
        }
        let token = next_member_token()?;
        let pid = alloc_pid().ok_or(SystemCallError::ReachLimit)?;
        let owner = guards.last_mut().expect("chain always contains the owner");
        if !owner.members.has_room() {
            return Err(SystemCallError::ReachLimit);
        }
        *prepared.value_mut() = MemberEntry::Reserved { token };
        owner.members.insert_prepared(prepared.with_key(pid));
        Ok((pid, MemberReservation { pid, token }))
    }

    /// JobCreate 创建口闸门：链锁内上行检查祖先 seal 与深度上限、锁内
    /// 分配 JobId 并把占位插入子表。child 深度 = 链长 + 1 ≤ 上限。
    pub(crate) fn gate_reserve_child(
        self: &Arc<Self>,
    ) -> Result<(JobId, ChildReservation), SystemCallError> {
        let mut prepared = PreparedEntry::try_new(
            0,
            ChildEntry::Reserved { token: 0 },
            &MetadataFunding(&self._pool),
        )
        .map_err(table_insert_error)?;
        let chain = collect_chain(self)?;
        if chain.len() >= JOB_DEPTH_MAX {
            return Err(SystemCallError::IllegalArgument);
        }
        let mut guards = lock_chain(&chain)?;
        if guards.iter().any(|state| state.is_sealed()) {
            return Err(SystemCallError::ObjectClosed);
        }
        let token = next_member_token()?;
        let jid = alloc_jid().ok_or(SystemCallError::ReachLimit)?;
        let owner = guards.last_mut().expect("chain always contains the owner");
        if !owner.children.has_room() {
            return Err(SystemCallError::ReachLimit);
        }
        *prepared.value_mut() = ChildEntry::Reserved { token };
        owner.children.insert_prepared(prepared.with_key(jid));
        Ok((jid, ChildReservation { jid, token }))
    }

    /// 骨架与构造 lease 已出生，Job 接管生命周期根；capability 表锁不参与提交。
    pub(crate) fn commit_member(&self, reservation: MemberReservation, process: Arc<ProcessShell>) {
        let mut state = self.state.lock();
        let entry = state
            .members
            .get_mut(reservation.pid)
            .expect("job member reservation disappeared");
        assert!(
            matches!(entry, MemberEntry::Reserved { token } if *token == reservation.token),
            "job member reservation token changed"
        );
        *entry = MemberEntry::Process(process);
    }

    pub(crate) fn rollback_member(self: &Arc<Self>, reservation: MemberReservation) {
        let (removed, completed) = {
            let mut state = self.state.lock();
            let removed = state
                .members
                .remove(reservation.pid)
                .expect("job member reservation disappeared");
            (removed, state.complete_if_ready())
        };
        assert!(
            matches!(removed.value(), MemberEntry::Reserved { token } if *token == reservation.token),
            "job member reservation token changed"
        );
        drop(removed);
        Self::wake_completion(completed);
    }

    fn commit_child(&self, reservation: ChildReservation, job: Arc<Job>) {
        let sealed = {
            let mut state = self.state.lock();
            let entry = state
                .children
                .get_mut(reservation.jid)
                .expect("job child reservation disappeared");
            assert!(
                matches!(entry, ChildEntry::Reserved { token } if *token == reservation.token),
                "job child reservation token changed"
            );
            *entry = ChildEntry::Job(job.clone());
            state.is_sealed()
        };
        // 父游标可能已跨过此预约；封口后提交的子体独立接管传播责任。
        if sealed {
            job.seal();
        }
    }

    fn rollback_child(self: &Arc<Self>, reservation: ChildReservation) {
        let (removed, completed) = {
            let mut state = self.state.lock();
            let removed = state
                .children
                .remove(reservation.jid)
                .expect("job child reservation disappeared");
            (removed, state.complete_if_ready())
        };
        assert!(
            matches!(removed.value(), ChildEntry::Reserved { token } if *token == reservation.token),
            "job child reservation token changed"
        );
        drop(removed);
        Self::wake_completion(completed);
    }

    /// JobSeal：O(1) 接管有界后代封口责任；重复封口不重复发布。
    fn seal(self: &Arc<Self>) {
        {
            let mut state = self.state.lock();
            if state.is_sealed() {
                return;
            }
            state.phase = JobPhase::Sealing { cursor: 0 };
            let completed = state.complete_if_ready();
            debug_assert!(completed.is_none());
            // 初次封口与稳定根交接同锁；成员离场不能在 publish 前留下悬空责任。
            let root: Arc<dyn Work> = self.clone();
            mt::publish(root);
        }
        mt::kick();
    }

    /// Dead 发布后摘除成员；满足完成条件时该 Job 自身接管后续传播。
    pub(crate) fn remove_member(self: &Arc<Self>, pid: Pid) {
        let (removed, completed) = {
            let mut state = self.state.lock();
            let removed = state
                .members
                .remove(pid)
                .expect("job process member disappeared before removal");
            let completed = state.complete_if_ready();
            (removed, completed)
        };
        assert!(
            matches!(removed.value(), MemberEntry::Process(_)),
            "job process entry kind changed"
        );
        drop(removed);
        Self::wake_completion(completed);
    }

    /// 按 Pid 查直接成员（仅可见 Process 条目；占位对派生不可见）。
    pub(crate) fn member_process(&self, pid: Pid) -> Option<Arc<ProcessShell>> {
        let state = self.state.lock();
        state.members.get(pid).and_then(|entry| match entry {
            MemberEntry::Process(process) => Some(process.clone()),
            MemberEntry::Reserved { .. } => None,
        })
    }

    /// 按 JobId 查直接 child（仅可见 Job 条目；占位对派生不可见）。
    pub(crate) fn child_job(&self, jid: JobId) -> Option<Arc<Job>> {
        let state = self.state.lock();
        state.children.get(jid).and_then(|entry| match entry {
            ChildEntry::Job(job) => Some(job.clone()),
            ChildEntry::Reserved { .. } => None,
        })
    }

    /// 游标分页枚举（锁内收集；ids 长度即单批容量上限，非零）。
    fn enumerate(&self, kind: JobMemberKind, cursor: u64, ids: &mut [u64]) -> (usize, bool) {
        let state = self.state.lock();
        match kind {
            JobMemberKind::ChildJobs => state.children.scan_visible(
                |entry| matches!(entry, ChildEntry::Job(_)),
                cursor,
                ids,
            ),
            JobMemberKind::MemberProcesses => state.members.scan_visible(
                |entry| matches!(entry, MemberEntry::Process(_)),
                cursor,
                ids,
            ),
        }
    }

    /// 固定宽快照（JobQuery）。Dead 后两表必空（完成不变量），计数
    /// 自然为零——完成不变量即冻结，无需独立快照结构。
    pub(crate) fn snapshot(&self) -> JobSnapshot {
        let state = self.state.lock();
        let state_value = if state.is_dead() {
            JobState::Dead
        } else if state.is_sealed() {
            JobState::Sealed
        } else {
            JobState::Open
        };
        let live_processes = state
            .members
            .count_matching(|entry| matches!(entry, MemberEntry::Process(_)));
        let live_children = state
            .children
            .count_matching(|entry| matches!(entry, ChildEntry::Job(_)));
        JobSnapshot {
            jid: self.jid,
            parent_jid: self.parent_jid,
            state: state_value as u32,
            live_processes: live_processes as u32,
            live_children: live_children as u32,
            reserved: 0,
            reserved2: 0,
        }
    }

    /// CLOSED 发布（对象 wait 锁，不与 JobInner 锁嵌套；零分配完成交付）。
    fn publish_closed(&self) {
        self.source.publish(ObjectSignals::CLOSED, true);
    }

    fn wake_completion(token: Option<mt::WakeToken>) {
        if let Some(token) = token {
            drop(mt::wake(token));
            mt::kick();
        }
    }

    /// ProcessStart：Job 链 Seal 校验 → Shell gate 成员计数复核/Running
    /// 提交；返回短 ingress 准入供锁外逐槽发布 Ready。
    pub(crate) fn start_commit_gate(
        process: &Arc<ProcessShell>,
        expected: usize,
    ) -> Result<super::process::shell::BodyLease, SystemCallError> {
        let job = process.job.upgrade().expect("process Job lost its member");
        let chain = collect_chain(&job)?;
        let guards = lock_chain(&chain)?;
        if guards.iter().any(|state| state.is_sealed()) {
            return Err(SystemCallError::ObjectClosed);
        }
        process.begin_running(expected)
    }

    /// ThreadCreate 在准备期接受创建责任；后续 Seal/Frozen 不撤销该仿射预约。
    pub(crate) fn reserve_thread<'a>(
        process: &'a Arc<ProcessShell>,
        body: &'a super::process::shell::BodyLease,
    ) -> Result<super::process::shell::MemberReservation<'a>, SystemCallError> {
        let job = process.job.upgrade().expect("process Job lost its member");
        let chain = collect_chain(&job)?;
        let mut prepared = super::process::members::MemberStorage::new();
        loop {
            let result = {
                let guards = lock_chain(&chain)?;
                if guards.iter().any(|state| state.is_sealed()) {
                    Err(SystemCallError::ObjectClosed)
                } else {
                    process.reserve_member(body, &mut prepared)
                }
            };
            if !matches!(result, Err(SystemCallError::ObjectBusy)) {
                return result;
            }
            // 此处全部 Job/Shell/Members 锁已退出。初次失败要求真正准备容量；
            // 后续候选严格大于快照容量，竞争失败证明其他方已装更大块。
            // 容量不缩小，次数由自然字宽限制，不按抢占时序自旋。
            drop(prepared);
            prepared = body.members.prepare_storage(process.pool())?;
        }
    }
}

impl Work for Job {
    fn record(&self) -> &WorkRecord {
        &self.record
    }

    fn step(self: Arc<Self>) -> Outcome {
        enum Step {
            Child(Option<Arc<Job>>),
            Blocked,
            Complete,
        }
        let step = {
            let mut state = self.state.lock();
            match state.phase {
                JobPhase::Open => unreachable!("unsealed job work was scheduled"),
                JobPhase::Sealing { cursor } => {
                    let next = state
                        .children
                        .next_after(Some(&cursor))
                        .map(|(jid, entry)| {
                            (
                                *jid,
                                match entry {
                                    ChildEntry::Job(job) => Some(job.clone()),
                                    ChildEntry::Reserved { .. } => None,
                                },
                            )
                        });
                    if let Some((cursor, child)) = next {
                        state.phase = JobPhase::Sealing { cursor };
                        Step::Child(child)
                    } else {
                        let completed = state.complete_if_ready();
                        debug_assert!(completed.is_none());
                        if state.is_dead() {
                            Step::Complete
                        } else {
                            let root: Arc<dyn Work> = self.clone();
                            let token = mt::arm(&root);
                            state.phase = JobPhase::Waiting { token };
                            Step::Blocked
                        }
                    }
                }
                JobPhase::Waiting { .. } => {
                    unreachable!("job work woke without completion")
                }
                JobPhase::Dead => Step::Complete,
            }
        };
        match step {
            Step::Child(child) => {
                if let Some(child) = child {
                    child.seal();
                }
                return Outcome::Ready;
            }
            Step::Blocked => return Outcome::Blocked,
            Step::Complete => {}
        }

        let parent = self.parent.as_ref().map(|parent| {
            let parent = parent.upgrade().expect("ancestor job lost its child");
            let removed = {
                let mut state = parent.state.lock();
                let removed = state
                    .children
                    .remove(self.jid)
                    .expect("job child disappeared before ancestor removal");
                state.closing_children += 1;
                debug_assert!(state.closing_children <= crate::hart::HART_NUM_LIMIT);
                removed
            };
            match removed.value() {
                ChildEntry::Job(child) => {
                    assert!(Arc::ptr_eq(child, &self), "job child entry changed")
                }
                ChildEntry::Reserved { .. } => panic!("job child remained reserved"),
            }
            // WorkRecord 仍强持本 Job，完整节点及付款只在父门外释放。
            drop(removed);
            parent
        });
        // CLOSED 观察者可依赖父枚举已移除自己；父责任不先于本层完成信号。
        self.publish_closed();
        if let Some(parent) = parent {
            let completed = {
                let mut state = parent.state.lock();
                state.closing_children -= 1;
                state.complete_if_ready()
            };
            Self::wake_completion(completed);
        }
        Outcome::Done
    }
}

impl KernelObject for Job {
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }

    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::Job
    }

    fn allowed_rights(&self) -> Option<Rights> {
        Some(JOB_RIGHTS)
    }

    fn allowed_signals(&self) -> Option<ObjectSignals> {
        Some(ObjectSignals::CLOSED)
    }

    fn wait_source(&self) -> Option<Arc<dyn ObservationSource>> {
        Some(self.source.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 解析 JobControl capability（rights 前置校验 + 对象类型核对）；
/// 返回 (Job, 该 Handle 的 rights)——派生权的「源 rights」基准。
fn resolve_job_control(
    thread: &Thread,
    handle: Handle,
    rights: Rights,
) -> Result<(Arc<Job>, Rights), SystemCallError> {
    let body = thread
        .process
        .lease_body()
        .ok_or(SystemCallError::ObjectClosed)?;
    let table = body.handles.lock();
    let entry = table
        .get(handle, rights)
        .map_err(super::handle::map_error)?;
    if entry.object().0.kind() != ObjectKind::Job {
        return Err(SystemCallError::WrongObjectType);
    }
    let source_rights = entry.rights();
    Ok((Job::concrete(&entry.object().0)?, source_rights))
}

fn parse_member_kind(kind_raw: u64) -> Result<JobMemberKind, SystemCallError> {
    match kind_raw {
        0 => Ok(JobMemberKind::ChildJobs),
        1 => Ok(JobMemberKind::MemberProcesses),
        _ => Err(SystemCallError::IllegalArgument),
    }
}

pub fn create(
    thread: &Thread,
    parent: Handle,
    pool: Handle,
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let (parent, pool) = {
        let body = thread
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let table = body.handles.lock();
        let parent_cap = table
            .get(parent, Rights::CREATE)
            .map_err(super::handle::map_error)?;
        if !rights.is_subset_of(parent_cap.rights()) {
            return Err(SystemCallError::RightsDenied);
        }
        let parent = super::handle::resolve(&table, parent, Rights::CREATE, ObjectKind::Job)?;
        let pool = super::handle::resolve(&table, pool, Rights::CREATE, ObjectKind::MemoryPool)?;
        (Job::concrete(&parent)?, MemoryPool::concrete(&pool)?)
    };
    // 创建口闸门（链锁线性化）：占位先于一切可失败构造插入，失败路径
    // 统一回滚（capability 可见前完成不可失败的层级提交）。
    let (jid, reservation) = parent.gate_reserve_child()?;
    let child = match Job::child(jid, parent.jid, &parent, pool) {
        Ok(child) => child,
        Err(error) => {
            parent.rollback_child(reservation);
            return Err(error);
        }
    };
    let cap = match super::handle::mint(Job::object_ref(&child), rights) {
        Ok(cap) => cap,
        Err(error) => {
            parent.rollback_child(reservation);
            return Err(error);
        }
    };
    let caps = [cap];
    super::handle::install(
        thread,
        caps,
        |handles| crate::uaccess::write_user(thread, output, &handles[0]),
        || {
            parent.commit_child(reservation, child);
        },
    )
    .map(|_| ())
    .inspect_err(|_| {
        parent.rollback_child(reservation);
    })
}

/// JobSeal(control)：MANAGE；幂等封口——该 Job 及全部后代的创建/启动
/// 口经上行检查永久关闭。
pub fn seal(thread: &Thread, control: Handle) -> Result<(), SystemCallError> {
    let (job, _) = resolve_job_control(thread, control, Rights::MANAGE)?;
    job.seal();
    Ok(())
}

/// JobQuery(control, out)：READ；固定宽快照。
pub fn query(thread: &Thread, control: Handle, output: usize) -> Result<(), SystemCallError> {
    let (job, _) = resolve_job_control(thread, control, Rights::READ)?;
    let snapshot = job.snapshot();
    crate::uaccess::write_user(thread, output, &snapshot)
}

/// JobEnumerate(control, kind, cursor, buf, buf_len, out)：READ；单调 ID
/// 序游标分页。输出区间先行校验（P2-1：坏指针在扫描副作用之前拒绝）。
pub fn enumerate(
    thread: &Thread,
    control: Handle,
    kind_raw: u64,
    cursor: u64,
    buf: usize,
    buf_len: usize,
    output: usize,
) -> Result<(), SystemCallError> {
    let kind = parse_member_kind(kind_raw)?;
    if buf_len == 0 {
        return Err(SystemCallError::IllegalArgument);
    }
    let (job, _) = resolve_job_control(thread, control, Rights::READ)?;
    let cap = buf_len.min(JOB_ENUMERATE_MAX);
    let body = thread
        .process
        .lease_body()
        .ok_or(SystemCallError::ObjectClosed)?;
    {
        let mut vm = body.vm.space.lock();
        let space = vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?;
        space.check_range(buf, cap * core::mem::size_of::<u64>(), true)?;
        space.check_range(output, core::mem::size_of::<JobEnumerateResult>(), true)?;
    }
    let mut ids = [0u64; JOB_ENUMERATE_MAX];
    let (actual, more) = job.enumerate(kind, cursor, &mut ids[..cap]);
    let result = JobEnumerateResult {
        next_cursor: if actual > 0 { ids[actual - 1] } else { cursor },
        actual: actual as u32,
        more: u32::from(more),
    };
    let mut vm = body.vm.space.lock();
    let space = vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?;
    // SAFETY: ids[..actual] 是已初始化的 u64 切片；区间已在前面校验。
    let bytes = unsafe {
        core::slice::from_raw_parts(
            ids.as_ptr().cast::<u8>(),
            actual * core::mem::size_of::<u64>(),
        )
    };
    crate::uaccess::deliver_bytes(thread, space, buf, bytes)?;
    // SAFETY: JobEnumerateResult 字段全部初始化，无 padding；复检失败
    // 即杀本进程（deliver_output）。
    unsafe { crate::uaccess::deliver_output(thread, space, output, &result) }
}

/// JobDerive(control, kind, id, rights, out)：MANAGE；在直接成员域内按
/// ID 单目标派生 child JobControl / member ProcessControl。请求 rights
/// 必须是「源 Handle rights ∩ 目标角色 allowed_rights」的子集；目标不
/// 在直接成员表（含已完成移表）ObjectNotFound——ID 不复用保证 NotFound
/// 只意味着「已完成」，永不错指。
pub fn derive(
    thread: &Thread,
    control: Handle,
    kind_raw: u64,
    id: u64,
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let kind = parse_member_kind(kind_raw)?;
    let (job, source_rights) = resolve_job_control(thread, control, Rights::MANAGE)?;
    if !rights.is_known() || !rights.is_subset_of(source_rights) {
        return Err(SystemCallError::RightsDenied);
    }
    let object = match kind {
        JobMemberKind::ChildJobs => {
            let child = job.child_job(id).ok_or(SystemCallError::ObjectNotFound)?;
            Job::object_ref(&child)
        }
        JobMemberKind::MemberProcesses => {
            let process = job
                .member_process(id)
                .ok_or(SystemCallError::ObjectNotFound)?;
            ProcessShell::object_ref(&process)
        }
    };
    let caps = [super::handle::mint(object, rights)?];
    super::handle::install(
        thread,
        caps,
        |handles| crate::uaccess::write_user(thread, output, &handles[0]),
        || {},
    )
    .map(|_| ())
}
