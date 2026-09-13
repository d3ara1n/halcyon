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
//! 「放子锁、取父锁」，单步有界（延迟触发安全：sealed ⇒ 无新成员，
//! 判定幂等）。
//!
//! 成员/子表是按 ID 有序的 fallible AVL：ID 在 owner 锁内分配并与占位
//! 插入同临界区，查找、提交、回滚和完成摘除均不分配且受树高硬界约束；
//! 枚举按 ID 单调分页，遇未决占位即终止本批（屏障窗口只存在于创建方的
//! 单个 syscall 内，枚举方重试不活锁）。

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use core::any::Any;

use ordered_table::{InsertError, OrderedTable};

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    proc::{
        JOB_ENUMERATE_MAX, JobEnumerateResult, JobId, JobMemberKind, JobSnapshot, JobState, Pid,
    },
};

use super::{
    Thread,
    object::{
        HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState,
        SubscribeResult,
    },
    proc::Process,
    wait::{Subscription, schedule_waiters},
};

/// Job 层级深度硬上限（含 root）。
pub(crate) const JOB_DEPTH_MAX: usize = 32;
/// 单个 Job 的直接成员或直接 child 数上限。它同时把 AVL 高度与完成路径
/// 的比较次数封成常量；容量耗尽在创建口返回 ReachLimit。
const JOB_DIRECT_ENTRY_LIMIT: usize = 65_536;

fn table_insert_error<V>(error: InsertError<V>) -> SystemCallError {
    match error {
        InsertError::Limit(_) => SystemCallError::ReachLimit,
        InsertError::Allocation(_) => SystemCallError::OutOfMemory,
    }
}

/// 内核侧成员表条目：事务 marker 或强持的 Process core。
#[derive(Clone)]
pub(crate) enum MemberEntry {
    /// 事务预留：Create 提交前占位，对枚举与派生查找不可见（屏障条目）。
    Reserved { token: u64 },
    /// 未 Dead 的 Process core（Building 起，Dead 发布时移除）。
    Process(Arc<Process>),
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

pub(crate) struct JobInner {
    /// 直接 child Jobs（按 JobId 升序；含事务占位）。
    children: OrderedTable<ChildEntry>,
    /// 直接成员（按 Pid 升序；含事务占位）。
    members: OrderedTable<MemberEntry>,
    /// 封口位：创建/启动提交点沿父链上行检查。
    sealed: bool,
    /// 完成位：sealed && 两表空时一次置位；此后两表恒空。
    dead: bool,
}

impl JobInner {
    /// 完成判定（持锁调用）：条件满足则置 dead 并返回 true；幂等。
    fn complete_if_ready(&mut self) -> bool {
        if self.sealed && !self.dead && self.members.is_empty() && self.children.is_empty() {
            self.dead = true;
            true
        } else {
            false
        }
    }
}

/// Job 完成传播的持久游标。
///
/// 一个游标只保留当前待发布/待摘除的 Job 强引用；每次 `advance` 最多
/// 完成一次发布或一次父成员摘除，ProcessDrain 可在预算边界安全暂停。
pub(crate) struct CompletionCursor {
    child: Arc<Job>,
    published: bool,
}

impl CompletionCursor {
    fn new(child: Arc<Job>) -> Self {
        Self {
            child,
            published: false,
        }
    }

    /// 推进一步，返回本条祖先链是否已完全收束。
    pub(crate) fn advance(&mut self) -> bool {
        if !self.published {
            self.child.publish_closed();
            self.published = true;
            return false;
        }

        let Some(parent) = self
            .child
            .parent
            .as_ref()
            .map(|weak| weak.upgrade().expect("ancestor jobs outlive their members"))
        else {
            return true;
        };
        let (removed, parent_completed) = {
            let mut state = parent.state.lock();
            let removed = state
                .children
                .remove(self.child.jid)
                .expect("job child disappeared before ancestor removal");
            let completed = state.complete_if_ready();
            (removed, completed)
        };
        assert!(
            matches!(removed, ChildEntry::Job(_)),
            "job child entry kind changed"
        );
        drop(removed);
        if !parent_completed {
            return true;
        }
        self.child = parent;
        self.published = false;
        false
    }
}

pub struct Job {
    header: ObjectHeader,
    /// 全局单调不复用；root 是首个分配者，恒为 1。
    jid: JobId,
    /// 创建时冻结的父 JobId（root 为 0）；不依赖 weak 父的存活。
    parent_jid: JobId,
    parent: Option<Weak<Job>>,
    state: crate::sync::Spinlock<JobInner>,
    wait: crate::sync::Spinlock<ObjectWaitState>,
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
fn collect_chain(owner: &Arc<Job>) -> Result<Vec<Arc<Job>>, SystemCallError> {
    let mut upward = Vec::new();
    let mut current = owner.clone();
    loop {
        upward
            .try_reserve(1)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        upward.push(current.clone());
        if upward.len() > JOB_DEPTH_MAX {
            return Err(SystemCallError::IllegalArgument);
        }
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
) -> Result<Vec<crate::sync::SpinlockGuard<'_, JobInner>>, SystemCallError> {
    let mut guards = Vec::new();
    guards
        .try_reserve(chain.len())
        .map_err(|_| SystemCallError::OutOfMemory)?;
    for job in chain {
        guards.push(job.state.lock());
    }
    Ok(guards)
}

impl Job {
    /// root Job：内核 static anchor 强持至本次启动结束；boot 交给 init 的
    /// Handle 只是 authority，不是生命周期根。完成发 CLOSED 但不移除
    /// 不释放（boot 生命周期）。
    pub fn root() -> Arc<Self> {
        let mut root = ROOT.lock();
        if root.is_none() {
            let jid = alloc_jid().expect("root Job identity exhausted");
            *root = Some(
                Arc::try_new(Self {
                    header: ObjectHeader::new(),
                    jid,
                    parent_jid: 0,
                    parent: None,
                    state: crate::sync::Spinlock::chained(
                        crate::sync::ranks::JOB_INNER,
                        jid,
                        JobInner {
                            children: OrderedTable::new(JOB_DIRECT_ENTRY_LIMIT),
                            members: OrderedTable::new(JOB_DIRECT_ENTRY_LIMIT),
                            sealed: false,
                            dead: false,
                        },
                    ),
                    wait: crate::sync::Spinlock::new(
                        crate::sync::ranks::OBJECT_WAIT,
                        ObjectWaitState::new(ObjectSignals::NONE),
                    ),
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
    ) -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            jid,
            parent_jid,
            parent: Some(Arc::downgrade(parent)),
            state: crate::sync::Spinlock::chained(
                crate::sync::ranks::JOB_INNER,
                jid,
                JobInner {
                    children: OrderedTable::new(JOB_DIRECT_ENTRY_LIMIT),
                    members: OrderedTable::new(JOB_DIRECT_ENTRY_LIMIT),
                    sealed: false,
                    dead: false,
                },
            ),
            wait: crate::sync::Spinlock::new(
                crate::sync::ranks::OBJECT_WAIT,
                ObjectWaitState::new(ObjectSignals::NONE),
            ),
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
        let chain = collect_chain(self)?;
        let mut guards = lock_chain(&chain)?;
        if guards.iter().any(|state| state.sealed) {
            return Err(SystemCallError::ObjectClosed);
        }
        let token = next_member_token()?;
        let pid = alloc_pid().ok_or(SystemCallError::ReachLimit)?;
        let owner = guards.last_mut().expect("chain always contains the owner");
        owner
            .members
            .try_insert(pid, MemberEntry::Reserved { token })
            .map_err(table_insert_error)?;
        Ok((pid, MemberReservation { pid, token }))
    }

    /// JobCreate 创建口闸门：链锁内上行检查祖先 seal 与深度上限、锁内
    /// 分配 JobId 并把占位插入子表。child 深度 = 链长 + 1 ≤ 上限。
    pub(crate) fn gate_reserve_child(
        self: &Arc<Self>,
    ) -> Result<(JobId, ChildReservation), SystemCallError> {
        let chain = collect_chain(self)?;
        if chain.len() >= JOB_DEPTH_MAX {
            return Err(SystemCallError::IllegalArgument);
        }
        let mut guards = lock_chain(&chain)?;
        if guards.iter().any(|state| state.sealed) {
            return Err(SystemCallError::ObjectClosed);
        }
        let token = next_member_token()?;
        let jid = alloc_jid().ok_or(SystemCallError::ReachLimit)?;
        let owner = guards.last_mut().expect("chain always contains the owner");
        owner
            .children
            .try_insert(jid, ChildEntry::Reserved { token })
            .map_err(table_insert_error)?;
        Ok((jid, ChildReservation { jid, token }))
    }

    /// 预留成员表条目（bootstrap 路径：pid 由调用方持有；启动期单线程、
    /// 无 seal 可能，不走链锁闸门）。syscall 路径一律走
    /// [`Self::gate_reserve_member`]。
    pub(crate) fn reserve_member(&self, pid: Pid) -> Result<MemberReservation, SystemCallError> {
        let token = next_member_token()?;
        let mut state = self.state.lock();
        state
            .members
            .try_insert(pid, MemberEntry::Reserved { token })
            .map_err(table_insert_error)?;
        Ok(MemberReservation { pid, token })
    }

    /// 在 Job owner 锁内发布成员并执行调用方的不可失败外部提交。调用方可先持
    /// 更外层的 HandleTable 锁，使 capability 与 Job 枚举在释放任一锁前同时可见。
    pub(crate) fn commit_member_with<R>(
        &self,
        reservation: MemberReservation,
        process: Arc<Process>,
        commit: impl FnOnce() -> R,
    ) -> R {
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
        commit()
    }

    pub(crate) fn rollback_member(&self, reservation: MemberReservation) {
        let removed = self
            .state
            .lock()
            .members
            .remove(reservation.pid)
            .expect("job member reservation disappeared");
        assert!(
            matches!(removed, MemberEntry::Reserved { token } if token == reservation.token),
            "job member reservation token changed"
        );
    }

    fn commit_child(&self, reservation: ChildReservation, job: Arc<Job>) {
        let mut state = self.state.lock();
        let entry = state
            .children
            .get_mut(reservation.jid)
            .expect("job child reservation disappeared");
        assert!(
            matches!(entry, ChildEntry::Reserved { token } if *token == reservation.token),
            "job child reservation token changed"
        );
        *entry = ChildEntry::Job(job);
    }

    fn rollback_child(&self, reservation: ChildReservation) {
        let removed = self
            .state
            .lock()
            .children
            .remove(reservation.jid)
            .expect("job child reservation disappeared");
        assert!(
            matches!(removed, ChildEntry::Reserved { token } if token == reservation.token),
            "job child reservation token changed"
        );
    }

    /// JobSeal：O(1) 置位（幂等，不扫表）；已空则完成并返回传播游标。
    /// 已 Dead 的 Job 上是幂等无操作。
    fn seal(self: &Arc<Self>) {
        let completed = {
            let mut state = self.state.lock();
            if state.dead {
                return;
            }
            state.sealed = true;
            state.complete_if_ready()
        };
        if completed {
            self.finish_completion();
        }
    }

    /// 摘除成员（Dead 发布点调用）。完成判定与 JobInner 锁内线性化；若
    /// Job 因此满足 sealed && 空，返回持有祖先传播责任的游标，交由
    /// ProcessDrain 按统一预算推进。
    pub(crate) fn remove_member(self: &Arc<Self>, pid: Pid) -> Option<CompletionCursor> {
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
            matches!(removed, MemberEntry::Process(_)),
            "job process entry kind changed"
        );
        drop(removed);
        completed.then(|| CompletionCursor::new(self.clone()))
    }

    /// 按 Pid 查直接成员（仅可见 Process 条目；占位对派生不可见）。
    pub(crate) fn member_process(&self, pid: Pid) -> Option<Arc<Process>> {
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
        let state_value = if state.dead {
            JobState::Dead
        } else if state.sealed {
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
        self.wait
            .lock()
            .update(ObjectSignals::NONE, ObjectSignals::CLOSED);
        schedule_waiters(&self.wait);
    }

    /// 完成收尾（JobInner 锁外）：发布自身 CLOSED，再沿父链传播——逐级
    /// 「放子锁、取父锁」移除与再判定，单步有界。root 无父仅发布。
    /// 父必先于子存活（子在其 children 表内直到本调用移除），升级失败
    /// 即所有权图违约。
    fn finish_completion(self: &Arc<Self>) {
        let mut cursor = CompletionCursor::new(self.clone());
        while !cursor.advance() {}
    }

    /// ProcessStart 提交闸门：链锁内上行检查祖先未 sealed，并在同一
    /// 临界区完成 lifecycle 的 Building → Running 线性化（lifecycle 锁
    /// 嵌套于链锁内，锁序规范允许方向）。失败（seal 先到或进程已
    /// 终止）返回 ObjectClosed，调用方整体回滚。
    /// ProcessStart 提交闸门：链锁内「上行检查祖先 seal + Building →
    /// Running（含预育提取）」（活体门、计数一致性与 Staging 提取在
    /// begin_running 锁内原子完成；lifecycle 锁嵌套于链锁内，锁序
    /// Job 链锁 → lifecycle 锁）。expected 是调用方预留就绪容量的成员
    /// 计数，out 容量同由调用方预预留；并发 attach 插队报 ObjectBusy
    /// （调用方以新计数重试）。
    pub(crate) fn start_commit_gate(
        process: &Arc<Process>,
        expected: usize,
        out: &mut Vec<Arc<crate::task::Thread>>,
    ) -> Result<(), SystemCallError> {
        let job = process.job();
        let chain = collect_chain(&job)?;
        let guards = lock_chain(&chain)?;
        if guards.iter().any(|state| state.sealed) {
            return Err(SystemCallError::ObjectClosed);
        }
        match process.lifecycle.begin_running(expected, out) {
            Ok(()) => Ok(()),
            Err(super::lifecycle::BeginFault::Closed) => Err(SystemCallError::ObjectClosed),
            Err(super::lifecycle::BeginFault::StaleCount | super::lifecycle::BeginFault::Busy) => {
                Err(SystemCallError::ObjectBusy)
            }
        }
    }
}

impl KernelObject for Job {
    fn complete_waiter_drain(
        &self,
        reservation: super::notify_work::Reservation,
    ) -> super::notify_work::Completion {
        self.wait.lock().complete_notification(reservation)
    }

    fn drain_waiters(&self, budget: usize) -> (usize, bool) {
        super::wait::drain_waiters(&self.wait, budget)
    }

    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::Job
    }

    fn allowed_rights(&self, role: HandleRole) -> Option<Rights> {
        (role == HandleRole::JobControl).then_some(
            Rights::CREATE
                | Rights::MANAGE
                | Rights::READ
                | Rights::WAIT
                | Rights::DUPLICATE
                | Rights::TRANSIT
                | Rights::GRANT,
        )
    }

    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals> {
        (role == HandleRole::JobControl).then_some(ObjectSignals::CLOSED)
    }

    fn signals(&self) -> ObjectSignals {
        self.wait.lock().signals()
    }

    fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
        self.wait.lock().subscribe(subscription)
    }

    fn rearm_observer(&self, id: u64) -> Result<super::object::ObserverRearm, SystemCallError> {
        self.wait.lock().rearm_observer(id)
    }

    fn cancel_observer(&self, id: u64) -> Option<super::object::CancelledObservation> {
        self.wait.lock().cancel_observer(id)
    }

    fn unsubscribe(&self, id: u64) {
        let retired = self.wait.lock().unsubscribe(id);
        drop(retired);
    }

    fn close_handle(&self, role: HandleRole, _owner: &Process, _exiting: bool) {
        debug_assert!(role == HandleRole::JobControl);
        // JobControl 丢失只消散 authority，不隐式终止成员（所有权图见模块注释）。
    }

    fn close_transit(&self, role: HandleRole) {
        debug_assert!(role == HandleRole::JobControl);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 解析 JobControl Handle（rights 前置校验 + 类型/role 核对）；
/// 返回 (Job, 该 Handle 的 rights)——派生权的「源 rights」基准。
fn resolve_job_control(
    thread: &Thread,
    handle: Handle,
    rights: Rights,
) -> Result<(Arc<Job>, Rights), SystemCallError> {
    let table = thread.process.handles.lock();
    let entry = table
        .get(handle, rights)
        .map_err(super::handle::map_error)?;
    if *entry.role() != HandleRole::JobControl || entry.object().kind() != ObjectKind::Job {
        return Err(SystemCallError::WrongObjectType);
    }
    let source_rights = entry.rights();
    Ok((Job::concrete(entry.object())?, source_rights))
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
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let parent = {
        let table = thread.process.handles.lock();
        let entry = table
            .get(parent, Rights::CREATE)
            .map_err(super::handle::map_error)?;
        if *entry.role() != HandleRole::JobControl || entry.object().kind() != ObjectKind::Job {
            return Err(SystemCallError::WrongObjectType);
        }
        if !rights.is_subset_of(entry.rights()) {
            return Err(SystemCallError::RightsDenied);
        }
        Job::concrete(entry.object())?
    };
    // 创建口闸门（链锁线性化）：占位先于一切可失败构造插入，失败路径
    // 统一回滚（capability 可见前完成不可失败的层级提交）。
    let (jid, reservation) = parent.gate_reserve_child()?;
    let child = match Job::child(jid, parent.jid, &parent) {
        Ok(child) => child,
        Err(error) => {
            parent.rollback_child(reservation);
            return Err(error);
        }
    };
    let entry = match super::handle::entry(Job::object_ref(&child), HandleRole::JobControl, rights)
    {
        Ok(entry) => entry,
        Err(error) => {
            parent.rollback_child(reservation);
            return Err(super::handle::map_error(error));
        }
    };
    super::handle::install_one(thread, entry, output, || {
        parent.commit_child(reservation, child);
    })
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
    let mut space = thread.process.space.lock();
    space.check_range(output, core::mem::size_of::<JobSnapshot>(), true)?;
    // SAFETY: JobSnapshot 字段与 reserved 全部初始化，结构无 padding；
    // 复检失败即杀本进程（deliver_output）。
    unsafe { crate::uaccess::deliver_output(thread, &mut space, output, &snapshot) }
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
    {
        let mut space = thread.process.space.lock();
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
    let mut space = thread.process.space.lock();
    // SAFETY: ids[..actual] 是已初始化的 u64 切片；区间已在前面校验。
    let bytes = unsafe {
        core::slice::from_raw_parts(
            ids.as_ptr().cast::<u8>(),
            actual * core::mem::size_of::<u64>(),
        )
    };
    crate::uaccess::copy_to_user(&mut space, buf, bytes)?;
    // SAFETY: JobEnumerateResult 字段全部初始化，无 padding；复检失败
    // 即杀本进程（deliver_output）。
    unsafe { crate::uaccess::deliver_output(thread, &mut space, output, &result) }
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
    let entry = match kind {
        JobMemberKind::ChildJobs => {
            let child = job.child_job(id).ok_or(SystemCallError::ObjectNotFound)?;
            super::handle::entry(Job::object_ref(&child), HandleRole::JobControl, rights)
                .map_err(super::handle::map_error)?
        }
        JobMemberKind::MemberProcesses => {
            let process = job
                .member_process(id)
                .ok_or(SystemCallError::ObjectNotFound)?;
            // 单一 shell 身份：存活 shell 复用同一对象；消散后从 core
            // 铸造并在铸造点重放已达成的电平（REAPABLE），派生兜底由此
            // 接上 drain 入口。
            let shell = process.revive_control()?;
            super::handle::entry(
                super::process::ProcessControl::object_ref(&shell),
                HandleRole::ProcessControl,
                rights,
            )
            .map_err(super::handle::map_error)?
        }
    };
    super::handle::install_one(thread, entry, output, || {})
}
