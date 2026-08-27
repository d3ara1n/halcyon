//! Job：进程创建域与资源预算的结构根（notes/ideas/task.md「Job」）。
//!
//! 所有权图（单一生命周期根，无双向强持）：
//! 内核 static anchor ─strong→ root Job；parent Job ─strong→ child Jobs；
//! child ─weak→ parent；Job 直接成员表 ─strong→ 未 Dead 的 Process cores；
//! Process ─weak→ Job。JobControl Handle 只强持 authority 视图——关闭最后
//! 一个 JobControl 仅消散 authority，不影响层级根与成员。
//!
//! 成员表同时承载 ProcessCreate/ProcessStart 的事务 marker（reserve/
//! commit/rollback），Building process 在 Create 提交点即为成员。

use alloc::{sync::{Arc, Weak}, vec::Vec};
use core::any::Any;

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    proc::Pid,
};

use super::{
    Thread,
    object::{HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState, SubscribeResult},
    proc::Process,
    wait::Subscription,
};

/// 内核侧成员表条目：事务 marker 或强持的 Process core。
#[derive(Clone)]
pub(crate) enum MemberEntry {
    /// 事务预留：Create/Start 提交前占位，对枚举与查找不可见。
    Reserved { pid: Pid, token: u64 },
    /// 未 Dead 的 Process core（Building 起，Dead 发布时移除）。
    Process(Arc<Process>),
}

/// 内核侧 Job 层级条目：事务 marker 或强持的 child Job。
#[derive(Clone)]
enum ChildEntry {
    /// JobCreate 事务预留：对枚举/查找不可见。
    #[expect(dead_code, reason = "封口/完成计数接 layer 级查询时区分用途")]
    Reserved(u64),
    #[expect(dead_code, reason = "JobSeal/完成计数接入时读取")]
    Job(Arc<Job>),
}

pub(crate) struct JobState {
    /// 强持 child Jobs（层级是 Job 的生命周期根；含事务 marker）。
    children: Vec<ChildEntry>,
    /// 直接成员（Process cores 与事务 marker）。
    members: Vec<MemberEntry>,
    /// 封口状态；JobSeal syscall 与祖先链检查随 step 5 接入。
    #[expect(dead_code, reason = "JobSeal 接线时使用")]
    sealed: bool,
}

pub struct Job {
    header: ObjectHeader,
    #[expect(dead_code, reason = "ancestor seal 链沿此上行（step 5）")]
    parent: Option<Weak<Job>>,
    state: crate::sync::Spinlock<JobState>,
    wait: crate::sync::Spinlock<ObjectWaitState>,
}

/// root Job 的内核 static anchor（强持至本次启动结束）。
static ROOT: crate::sync::Spinlock<Option<Arc<Job>>> = crate::sync::Spinlock::new(None);

/// 下一个成员事务 token。
static NEXT_MEMBER_TOKEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

/// 成员表事务预留凭据。
pub(crate) struct MemberReservation {
    pid: Pid,
    token: u64,
}

impl Job {
    /// root Job：内核 static anchor 强持至本次启动结束；boot 交给 init 的
    /// Handle 只是 authority，不是生命周期根。
    pub fn root() -> Arc<Self> {
        let mut root = ROOT.lock();
        if root.is_none() {
            *root = Some(
                Arc::try_new(Self {
                    header: ObjectHeader::new(),
                    parent: None,
                    state: crate::sync::Spinlock::new(JobState {
                        children: Vec::new(),
                        members: Vec::new(),
                        sealed: false,
                    }),
                    wait: crate::sync::Spinlock::new(ObjectWaitState::new(ObjectSignals::NONE)),
                })
                .expect("root Job allocation failed"),
            );
        }
        root.clone().expect("root Job anchor is initialized")
    }

    fn child(parent: Arc<Self>) -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            header: ObjectHeader::new(),
            parent: Some(Arc::downgrade(&parent)),
            state: crate::sync::Spinlock::new(JobState {
                children: Vec::new(),
                members: Vec::new(),
                sealed: false,
            }),
            wait: crate::sync::Spinlock::new(ObjectWaitState::new(ObjectSignals::NONE)),
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub fn object_ref(job: &Arc<Self>) -> ObjectRef {
        job.clone()
    }

    /// 从 Handle 表条目取具体 Job（trait upcasting 后 downcast）。
    pub(crate) fn concrete(object: &ObjectRef) -> Result<Arc<Self>, SystemCallError> {
        let any: Arc<dyn Any + Send + Sync> = object.clone();
        any.downcast::<Self>().map_err(|_| SystemCallError::WrongObjectType)
    }

    /// 预留 child 层级槽位（JobCreate 事务：容量与位置一次锁定，
    /// 不受并发 try_reserve 消费影响）。
    fn reserve_child(&self, token: u64) -> Result<usize, SystemCallError> {
        let mut state = self.state.lock();
        state
            .children
            .try_reserve(1)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        state.children.push(ChildEntry::Reserved(token));
        Ok(state.children.len() - 1)
    }

    fn commit_child(&self, index: usize, job: Arc<Job>) {
        let mut state = self.state.lock();
        debug_assert!(matches!(state.children[index], ChildEntry::Reserved(_)));
        state.children[index] = ChildEntry::Job(job);
    }

    fn rollback_child(&self, index: usize) {
        let mut state = self.state.lock();
        debug_assert!(matches!(state.children[index], ChildEntry::Reserved(_)));
        state.children.swap_remove(index);
    }

    /// 预留成员表条目（ProcessCreate 事务）。
    pub(crate) fn reserve_member(&self, pid: Pid) -> Result<MemberReservation, SystemCallError> {
        let token = NEXT_MEMBER_TOKEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if token == 0 {
            return Err(SystemCallError::InternalError);
        }
        let mut state = self.state.lock();
        state.members.try_reserve(1).map_err(|_| SystemCallError::OutOfMemory)?;
        state.members.push(MemberEntry::Reserved { pid, token });
        Ok(MemberReservation { pid, token })
    }

    pub(crate) fn commit_member(&self, reservation: MemberReservation, process: Arc<Process>) {
        let mut state = self.state.lock();
        let entry = state
            .members
            .iter_mut()
            .find(|entry| matches!(entry, MemberEntry::Reserved { pid, token } if *pid == reservation.pid && *token == reservation.token))
            .expect("job member reservation disappeared");
        *entry = MemberEntry::Process(process);
    }

    pub(crate) fn rollback_member(&self, reservation: MemberReservation) {
        let mut state = self.state.lock();
        let index = state
            .members
            .iter()
            .position(|entry| matches!(entry, MemberEntry::Reserved { pid, token } if *pid == reservation.pid && *token == reservation.token))
            .expect("job member reservation disappeared");
        state.members.swap_remove(index);
    }

    /// 摘除成员（Dead 发布点调用）；返回被移除的 core。Sealed 且成员
    /// 均已收束时的 CLOSED 发布在 JobState 锁外执行（对象锁不与
    /// JobState 锁嵌套）；完成计数泛化随 step 5 接入。
    pub(crate) fn remove_member(&self, pid: Pid) -> Option<Arc<Process>> {
        let mut state = self.state.lock();
        let index = state
            .members
            .iter()
            .position(|entry| matches!(entry, MemberEntry::Process(process) if process.pid == pid))?;
        match state.members.swap_remove(index) {
            MemberEntry::Process(process) => Some(process),
            MemberEntry::Reserved { .. } => {
                unreachable!("reserved entries never match a pid lookup")
            }
        }
    }
}

impl KernelObject for Job {
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

    fn unsubscribe(&self, id: u64) {
        self.wait.lock().unsubscribe(id);
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

pub fn create(
    thread: &Thread,
    parent: Handle,
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let parent = {
        let table = thread.process.handles.lock();
        let entry = table.get(parent, Rights::CREATE).map_err(super::handle::map_error)?;
        if *entry.role() != HandleRole::JobControl || entry.object().kind() != ObjectKind::Job {
            return Err(SystemCallError::WrongObjectType);
        }
        if !rights.is_subset_of(entry.rights()) {
            return Err(SystemCallError::RightsDenied);
        }
        Job::concrete(entry.object())?
    };
    // 层级插入事务（F5）：真实 marker 槽位先行锁定（并发 try_reserve
    // 无法消费），capability 可见前完成不可失败的层级提交。
    let child_token = NEXT_MEMBER_TOKEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if child_token == 0 {
        return Err(SystemCallError::InternalError);
    }
    let child_slot = parent.reserve_child(child_token)?;
    let child = Job::child(parent.clone())?;
    let entry = super::handle::entry(Job::object_ref(&child), HandleRole::JobControl, rights)
        .map_err(|error| {
            parent.rollback_child(child_slot);
            super::handle::map_error(error)
        })?;
    install_one(thread, entry, output, || {
        parent.commit_child(child_slot, child);
    })
    .inspect_err(|_| {
        parent.rollback_child(child_slot);
    })
}

/// 持久 PID 分配器（单调不复用；全局进程表已退役，生命周期根在 Job）。
static NEXT_PID: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(1);

pub fn alloc_pid() -> Pid {
    NEXT_PID.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

fn install_one(
    thread: &Thread,
    entry: super::handle::ProcessHandleEntry,
    output: usize,
    publish: impl FnOnce(),
) -> Result<(), SystemCallError> {
    let mut entries = alloc::vec::Vec::new();
    entries.try_reserve(1).map_err(|_| SystemCallError::OutOfMemory)?;
    entries.push(entry);
    let token = super::handle::transaction_token();
    let mut table = thread.process.handles.lock();
    let reservation = table.reserve(1, token).map_err(super::handle::map_error)?;
    let handle = reservation.handles()[0];
    let mut space = thread.process.space.lock();
    if let Err(error) = space.check_range(output, core::mem::size_of::<Handle>(), true) {
        drop(space);
        table.rollback(reservation).expect("JobCreate reservation must remain owned");
        return Err(error.into());
    }
    // SAFETY: Handle 无 padding，输出已在同一 space 锁下校验。
    unsafe { crate::uaccess::write_user_value(&mut space, output, &handle) }
        .expect("validated JobCreate output must remain writable");
    drop(space);
    // 发布序（同 ProcessCreate）：输出值此刻仍是 Reserved 槽号（其他
    // 线程不可用）；先完成不可失败的层级提交，再公开 capability——
    // 窗口内其他线程即使拿到槽号也无法关闭唯一 JobControl。
    publish();
    table.commit(reservation, entries)
        .expect("JobCreate reservation count matches entry");
    Ok(())
}
