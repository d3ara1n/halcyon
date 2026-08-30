//! Process 生命周期状态机：Building → Running → Terminating → Dead 的
//! 唯一真值、线程成员表（含 Building 期预育条目）与终止待办
//! （notes/ideas/task.md「进程」）。
//!
//! 锁序契约（顶级）：**Job 链锁（先父后子，≤32 把）→ lifecycle 锁 →
//! 其他对象锁**。lifecycle 锁可整体嵌套于 Job 链锁内（ProcessStart
//! 提交闸门：链锁内上行检查祖先 seal 后同临界区调用 begin_running）；
//! lifecycle 锁内只改状态、终因、成员表、active mask 与 Building
//! 操作计数，锁内不调用 subscribe/offer/enqueue/IPI/对象 close/
//! uaccess/页表操作——这些动作经 [`TerminationTodo`] 与
//! [`Lifecycle::take_first_waiting`] 游标延迟到锁外执行。
//! 方向约束：lifecycle 锁内不得出游获取任何其他锁（对象锁、
//! WaitContext/期限表锁、调度类锁、地址空间/HandleTable 锁、Job 链
//! 锁）；反向的单向嵌套（如 ProcessControl 快照在对象锁内进入
//! lifecycle，或链锁内进入 lifecycle）因 lifecycle 不出游而安全，不
//! 构成环。例外：成员表 Vec 的 try_reserve/remove 会取 HEAP 锁
//! （LIFECYCLE→HEAP 合法秩）；锁内不 drop 线程强引用（Thread 无
//! Drop 副作用，但纪律上仍由锁外消费，见 take_first_staging）。
//!
//! 成员表是线程容器记录的真值：条目只在线程强引用真正消散后由持有方
//! 经 thread_departed 摘除：kill 不把仍在队列或等待竞争中的线程摘除，
//! 只组装触达待办。Staging 条目携带线程强引用（Building 期预育表：
//! 线程经组装者附入但尚未入册可运行）——引用与 Thread.process 的
//! 强回指构成环，环只存在于 Building 期且在 Terminating 后必被
//! take_first_staging 游标打破（REAPABLE 合取含表空，Dead 前无残留）。
//! 唤醒到再调度之间存在过渡窗口：自然完成的等待线程
//! 在被再次 dispatch 前记录仍为 Waiting（指向已完成的 context），由
//! enter_running 的覆盖写收编；终止路径对这类 stale 记录的取消 offer
//! 必然落败（单 outcome 仲裁），线程由 pick gate 吸收后 reap 摘除。
//! REAPABLE 严格晚于最后一个成员摘除与最后一个 Building 操作退出
//! （building_ops == 0）。

use core::sync::atomic::{AtomicUsize, Ordering};

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};

use erhino_shared::proc::{PROCESS_MAX_THREADS, ProcessExitReason, ProcessState, Tid};

use super::wait::WaitContext;

/// 成员表条目：线程容器状态（容器真值）。
#[derive(Clone)]
pub(crate) enum ThreadState {
    /// 在某调度类就绪队列中；线程强引用由队列持有。
    Ready,
    /// 已附入但尚未入册（Building 预育表）：线程强引用由成员表持有，
    /// Start 提交点整体转 Ready（强引用移交类队列）；终止路径经
    /// take_first_staging 摘除释放。
    Staging { thread: Arc<super::Thread> },
    /// 在某 hart 执行点上；线程强引用由调度循环持有，IPI 吸收。
    Running { slot: usize },
    /// 无容器等待中；线程强引用由 WaitContext 持有，经 weak 触达取消。
    Waiting { context: Weak<WaitContext> },
    /// 已冻结终因、正在退出路径上（自杀线程或终止取消接管；reap /
    /// 完成方收尾摘除）。
    Exiting,
}

/// attach_member 失败分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AttachFault {
    /// 已终止或已入册（非 Building）。
    Closed,
    /// 并发线程数已达 PROCESS_MAX_THREADS。
    Limit,
    /// 成员表容量预留失败。
    Oom,
}

/// begin_running 失败分类（活体门与计数一致性在同一锁内判定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BeginFault {
    /// 已终止（或 seal 后 gate 拒绝）；无线程的空表也归此（从未活过）。
    Closed,
    /// 预留的入册数与成员表长度不符（并发 attach 插队）；调用方以新
    /// 计数重试或报 ObjectBusy。
    StaleCount,
}

struct MemberEntry {
    tid: Tid,
    state: ThreadState,
}

/// 按 tid 升序表内定位（Ok = 命中，Err = 插入点）。
fn position(members: &[MemberEntry], tid: Tid) -> Result<usize, usize> {
    members.binary_search_by(|entry| entry.tid.cmp(&tid))
}

/// 一次终止请求在锁内线性化后需要锁外执行的纯量副作用（零分配）：
/// 等待取消不随 todo 携带——由 [`Lifecycle::take_first_waiting`] 游标
/// 在锁外逐条驱动。
#[derive(Default)]
pub(crate) struct TerminationTodo {
    /// 需要 IPI 请求离开用户态的 hart slot 位图（冻结时刻的 active
    /// 快照；自杀路径排除本 hart）。
    pub ipi_slots: u64,
    /// 无成员、无在途 Building 操作且 active 为零，可立即发布 REAPABLE
    /// （Building 目标的 kill/abandonment）。
    pub reapable: bool,
}

/// Running 事务在 Commit 前复检的 execution gate 快照。sequence 使 active
/// 位图离开后又恢复相同值的 ABA 仍可见。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExecutionSnapshot {
    sequence: u64,
    active: u64,
}

impl ExecutionSnapshot {
    pub(crate) const fn active(self) -> u64 {
        self.active
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExecutionChanged;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EnterRunning {
    Entered,
    Retry,
    Closed,
}

/// 生命周期内核侧状态（内嵌于 Process，不是独立对象）。
pub(crate) struct Lifecycle {
    /// 原子快速路径：trap 入口与调度 gate 只读，不走锁。
    /// 判别值与 `erhino_shared::proc::ProcessState` 一致。
    state: AtomicUsize,
    /// 顶级锁：终因冻结、成员表、active mask 的复合转换。
    inner: crate::sync::Spinlock<LifecycleInner>,
}

struct LifecycleInner {
    reason: ProcessExitReason,
    code: i64,
    /// 线程成员表：按 tid 升序、二分定位；离场即摘除，表空即无线程。
    /// 插入容量在锁内 try_reserve（原子可失败插入，失败无副作用）。
    members: Vec<MemberEntry>,
    /// 进程内线程号：单调不复用，从 1 起（0 = 非身份值，与 pid/JobId
    /// 哨兵纪律对齐）；首线程 tid 1。
    next_tid: Tid,
    /// 本进程线程所在 hart 的 slot 位图：dispatch 前（进入用户 satp 前）
    /// 置位，Switch 出口归一 satp 后清除；全零且表空方可 REAPABLE。
    active: u64,
    /// active、Running 准入或终止截止每次变化都单调前进；零值不使用。
    execution_sequence: u64,
    /// 在途 Building 操作（builder 的 map/write/attach/grant/start）
    /// 计数；终止后归零才能发布 REAPABLE（操作退出方负责触发）。
    building_ops: usize,
}

fn advance_execution(inner: &mut LifecycleInner) {
    inner.execution_sequence = inner
        .execution_sequence
        .checked_add(1)
        .expect("process execution sequence exhausted");
}

fn state_index(state: ProcessState) -> usize {
    state as usize
}

impl Lifecycle {
    /// 构造 Building 状态。成员表容量不在此预留——附线程经
    /// attach_member 的锁内 try_reserve 原子插入（失败无副作用）。
    pub(crate) fn building() -> Self {
        Self {
            state: AtomicUsize::new(state_index(ProcessState::Building)),
            inner: crate::sync::Spinlock::new(
                crate::sync::ranks::LIFECYCLE,
                LifecycleInner {
                    reason: ProcessExitReason::None,
                    code: 0,
                    members: Vec::new(),
                    next_tid: 1,
                    active: 0,
                    execution_sequence: 1,
                    building_ops: 0,
                },
            ),
        }
    }

    /// trap 入口 / 调度 gate 的快速谓词（原子读，无锁）。
    pub fn is_terminating(&self) -> bool {
        self.state.load(Ordering::Acquire) >= state_index(ProcessState::Terminating)
    }

    /// Reserve 阶段取得 Running active 集合与 ABA 序列。
    pub(crate) fn snapshot_running(&self) -> Option<ExecutionSnapshot> {
        let inner = self.inner.lock();
        (self.state.load(Ordering::Acquire) == state_index(ProcessState::Running)).then_some(
            ExecutionSnapshot {
                sequence: inner.execution_sequence,
                active: inner.active,
            },
        )
    }

    /// Commit 在 AddressSpace 锁内重进 execution gate；闭包只允许执行已经
    /// Reserve 完成、不可失败的短发布，锁外工作由返回 token 承接。
    pub(crate) fn commit_if_current<R>(
        &self,
        snapshot: ExecutionSnapshot,
        commit: impl FnOnce(u64) -> R,
    ) -> Result<R, ExecutionChanged> {
        let inner = self.inner.lock();
        if self.state.load(Ordering::Acquire) != state_index(ProcessState::Running)
            || inner.execution_sequence != snapshot.sequence
            || inner.active != snapshot.active
        {
            return Err(ExecutionChanged);
        }
        Ok(commit(inner.active))
    }

    /// Building 操作准入（builder 的 map/write/start 入口）：
    /// 未终止则登记在途并放行；终止则拒绝。
    pub(crate) fn enter_building_op(&self) -> bool {
        let mut inner = self.inner.lock();
        if self.is_terminating() {
            return false;
        }
        inner.building_ops += 1;
        true
    }

    /// Building 操作退出：计数归零且已终止、表空、无 active 时返回
    /// true——调用方负责锁外发布 REAPABLE。
    pub(crate) fn leave_building_op(&self) -> bool {
        let mut inner = self.inner.lock();
        inner.building_ops -= 1;
        self.is_terminating()
            && inner.members.is_empty()
            && inner.active == 0
            && inner.building_ops == 0
    }

    /// 附入线程（ProcessAttach / bootstrap 内嵌组装）：锁内分配 tid、
    /// 经闭包构造线程（Arc 分配取 HEAP 锁，LIFECYCLE→HEAP 合法秩）并
    /// 插入 Staging 条目（强引用由成员表持有）。原子可失败：
    /// Closed/Limit/Oom 均无副作用（tid 未消耗）。仅 Building 态接受。
    pub(crate) fn attach_member(
        &self,
        build: impl FnOnce(Tid) -> Result<Arc<super::Thread>, AttachFault>,
    ) -> Result<Tid, AttachFault> {
        let mut inner = self.inner.lock();
        if self.state.load(Ordering::Acquire) != state_index(ProcessState::Building) {
            return Err(AttachFault::Closed);
        }
        if inner.members.len() >= PROCESS_MAX_THREADS {
            return Err(AttachFault::Limit);
        }
        let tid = inner.next_tid;
        let thread = build(tid)?;
        inner.members.try_reserve(1).map_err(|_| AttachFault::Oom)?;
        inner.next_tid = tid.checked_add(1).expect("thread id space exhausted");
        inner.members.push(MemberEntry {
            tid,
            state: ThreadState::Staging { thread },
        });
        Ok(tid)
    }

    /// 成员表当前长度（Start 活体门与入册预留的计数输入；Building 期
    /// 成员全部是 Staging 预育条目）。
    pub(crate) fn member_count(&self) -> usize {
        self.inner.lock().members.len()
    }

    /// ProcessStart 线性化（含预育提取）：Building → Running。同一
    /// lifecycle 锁内完成：活体门（表非空）、计数一致性（members.len()
    /// == expected，防御并发 attach 插队）、building_ops 消费、状态
    /// 发布、全部 Staging 条目转 Ready 并把线程强引用交出到 `out`
    /// （容量由调用方预预留；锁内零分配零 drop）。提取与转换原子
    /// 合并消除了「gate 后、入队前」窗口内异 hart kill 游标摘走 Staging
    /// 条目的竞态——begin_running 成功后表内不再有 Staging。
    pub(crate) fn begin_running(
        &self,
        expected: usize,
        out: &mut Vec<Arc<super::Thread>>,
    ) -> Result<(), BeginFault> {
        let mut inner = self.inner.lock();
        if self.state.load(Ordering::Acquire) != state_index(ProcessState::Building) {
            return Err(BeginFault::Closed);
        }
        if inner.members.is_empty() {
            // 活体门：无线程的进程从未活过，不允许入册。
            return Err(BeginFault::Closed);
        }
        if inner.members.len() != expected {
            return Err(BeginFault::StaleCount);
        }
        debug_assert!(inner.building_ops > 0, "start must hold a building op");
        inner.building_ops -= 1;
        for entry in inner.members.iter_mut() {
            let state = core::mem::replace(&mut entry.state, ThreadState::Ready);
            match state {
                ThreadState::Staging { thread } => out.push(thread),
                _ => unreachable!("Building member must be Staging before start commit"),
            }
        }
        advance_execution(&mut inner);
        self.state
            .store(state_index(ProcessState::Running), Ordering::Release);
        Ok(())
    }

    /// 终止路径游标（锁外逐条驱动）：摘取首个 Staging 预育条目并交出
    /// 线程强引用（调用方在锁外 drop）。预育线程从未进入容器，无需
    /// 离场确认——条目摘除即完成。摘定后表空可触发 REAPABLE 判定
    /// （run_termination_todo 尾部轮询 is_reapable）。
    pub(crate) fn take_first_staging(&self) -> Option<Arc<super::Thread>> {
        let mut inner = self.inner.lock();
        let index = inner
            .members
            .iter()
            .position(|entry| matches!(entry.state, ThreadState::Staging { .. }))?;
        let entry = inner.members.remove(index);
        match entry.state {
            ThreadState::Staging { thread } => Some(thread),
            _ => unreachable!("position matched a Staging entry"),
        }
    }

    /// 请求终止：首次到达者冻结终因并组装锁外待办；后续请求幂等返回空。
    /// `exiting = Some(tid)`：调用线程即目标（Exit/fault/自杀 kill，条目
    /// 转 Exiting，IPI 排除本 hart——本 hart 已在内核且即将走 Killed
    /// 出口）；`None`：外部触达（kill/abandonment，IPI 目标 = 冻结时刻
    /// active 位图：覆盖仍在用户态与 Resume 热路径循环中的全部 hart，
    /// 冻结后 enter_running 拒绝、位只减不增）。Ready/Staging 成员无需
    /// 触达（pick gate / Start 收尾方吸收）；Waiting 成员由锁外游标逐条
    /// 取消。
    pub(crate) fn request_termination(
        &self,
        reason: ProcessExitReason,
        code: i64,
        exiting: Option<Tid>,
    ) -> TerminationTodo {
        let mut todo = TerminationTodo::default();
        let mut inner = self.inner.lock();
        if self.state.load(Ordering::Acquire) >= state_index(ProcessState::Terminating) {
            return todo; // 终因已冻结：幂等，后续事件只协助收束
        }
        inner.reason = reason;
        inner.code = code;
        match exiting {
            Some(tid) => {
                let slot = crate::hart::current().slot();
                let index =
                    position(&inner.members, tid).expect("self-exiting thread must be a member");
                // 自杀线程必在执行点上（Staging 预育线程从未进入容器，
                // 不可能发起 syscall；覆盖写丢弃的旧值不含强引用）。
                debug_assert!(matches!(
                    inner.members[index].state,
                    ThreadState::Running { .. }
                ));
                inner.members[index].state = ThreadState::Exiting;
                todo.ipi_slots = inner.active & !(1u64 << slot);
            }
            None => todo.ipi_slots = inner.active,
        }
        todo.reapable = inner.members.is_empty() && inner.active == 0 && inner.building_ops == 0;
        advance_execution(&mut inner);
        self.state
            .store(state_index(ProcessState::Terminating), Ordering::Release);
        todo
    }

    /// park 发布线性化：Running → Waiting；已 Terminating 返回 false，
    /// 调用方不得发布等待，改走 Abandoned 取消。
    pub(crate) fn park_waiting(&self, tid: Tid, context: &alloc::sync::Arc<WaitContext>) -> bool {
        let mut inner = self.inner.lock();
        if self.is_terminating() {
            return false;
        }
        let index = position(&inner.members, tid).expect("parking thread must be a member");
        if let ThreadState::Running { slot } = inner.members[index].state {
            // park 发布由调度循环在刚离开执行点的 hart 上完成：记录的
            // slot 必然就是本 hart（单一归属，dispatch 后不迁移）。
            debug_assert_eq!(slot, crate::hart::current().slot());
        } else {
            debug_assert!(false, "parking thread must be Running");
        }
        inner.members[index].state = ThreadState::Waiting {
            context: alloc::sync::Arc::downgrade(context),
        };
        true
    }

    /// Switch 出口（Requeue 分支）：只有本 hart 已确认当前 epoch 才清 active，
    /// 随后 Running → Ready。false 要求调用者锁外消费 Pending 后重试。
    pub(crate) fn on_requeue_if(
        &self,
        tid: Tid,
        slot: usize,
        synchronized: impl FnOnce() -> bool,
    ) -> bool {
        let mut inner = self.inner.lock();
        if !synchronized() {
            return false;
        }
        assert!(
            inner.active & (1u64 << slot) != 0,
            "requeued hart must be active for process"
        );
        inner.active &= !(1u64 << slot);
        advance_execution(&mut inner);
        if !self.is_terminating() {
            if let Ok(index) = position(&inner.members, tid) {
                if matches!(inner.members[index].state, ThreadState::Running { .. }) {
                    inner.members[index].state = ThreadState::Ready;
                }
            }
        }
        true
    }

    /// 非-Resume 出口（Killed/Park）：确认当前 epoch 后清 active。false 要求
    /// 调用者锁外消费 Pending 后重试。
    pub(crate) fn clear_active_if(&self, slot: usize, synchronized: impl FnOnce() -> bool) -> bool {
        let mut inner = self.inner.lock();
        if !synchronized() {
            return false;
        }
        assert!(
            inner.active & (1u64 << slot) != 0,
            "departing hart must be active for process"
        );
        inner.active &= !(1u64 << slot);
        advance_execution(&mut inner);
        true
    }

    /// 调度循环 dispatch 前：调用者先在锁外同步当前 epoch，再在 gate 内复检；
    /// epoch 变化返回 Retry，Terminating 返回 Closed，只有 Entered 才登记 active。
    pub(crate) fn enter_running_if(
        &self,
        tid: Tid,
        slot: usize,
        synchronized: impl FnOnce() -> bool,
    ) -> EnterRunning {
        let mut inner = self.inner.lock();
        if self.is_terminating() {
            return EnterRunning::Closed;
        }
        if !synchronized() {
            return EnterRunning::Retry;
        }
        let index = position(&inner.members, tid).expect("dispatched thread must be a member");
        inner.members[index].state = ThreadState::Running { slot };
        assert!(
            inner.active & (1u64 << slot) == 0,
            "hart cannot enter one process twice"
        );
        inner.active |= 1u64 << slot;
        advance_execution(&mut inner);
        EnterRunning::Entered
    }

    /// 线程离场完成（调用方已 drop 线程强引用：reap / WaitContext 完成
    /// 方 / Start 防御失败路径）：摘除成员；返回是否可发布 REAPABLE
    /// （末离场者发布）。
    pub(crate) fn thread_departed(&self, tid: Tid) -> bool {
        let mut inner = self.inner.lock();
        let index = position(&inner.members, tid).expect("departing thread must be a member");
        inner.members.remove(index);
        self.is_terminating()
            && inner.members.is_empty()
            && inner.active == 0
            && inner.building_ops == 0
    }

    /// 终止触达游标（锁外逐条驱动）：摘取首个 Waiting 成员的 weak
    /// context 并转 Exiting——offer 胜者的 finish 负责 thread_departed
    /// 摘除；败者说明自然完成方已接管，线程经 enqueue → pick gate 吸收
    /// 后由 reap 摘除。冻结后 park_waiting 拒绝、Waiting 集合单调不增，
    /// 游标必然收敛。
    pub(crate) fn take_first_waiting(&self) -> Option<Weak<WaitContext>> {
        let mut inner = self.inner.lock();
        let index = inner
            .members
            .iter()
            .position(|entry| matches!(entry.state, ThreadState::Waiting { .. }))?;
        match core::mem::replace(&mut inner.members[index].state, ThreadState::Exiting) {
            ThreadState::Waiting { context } => Some(context),
            _ => unreachable!("position matched a Waiting entry"),
        }
    }

    /// REAPABLE 条件谓词（派生铸造新 shell 的电平重放用）：已终止、
    /// 表空、无在途 Building 操作、无 active hart——与
    /// thread_departed/leave_building_op 的发布判定同一合取。
    pub(crate) fn is_reapable(&self) -> bool {
        let inner = self.inner.lock();
        self.is_terminating()
            && inner.members.is_empty()
            && inner.active == 0
            && inner.building_ops == 0
    }

    /// 固定宽快照（ProcessQuery）：state 与终因在同一临界区内取得，
    /// 不返回「Running + 已冻结终因」的混合视图。
    pub(crate) fn snapshot(&self) -> (ProcessState, ProcessExitReason, i64) {
        let inner = self.inner.lock();
        let state = match self.state.load(Ordering::Acquire) {
            0 => ProcessState::Building,
            1 => ProcessState::Running,
            2 => ProcessState::Terminating,
            _ => ProcessState::Dead,
        };
        (state, inner.reason, inner.code)
    }

    /// Dead 发布（收束完成后调用）：冻结终态并返回终因。
    pub(crate) fn mark_dead(&self) -> (ProcessExitReason, i64) {
        let inner = self.inner.lock();
        let frozen = (inner.reason, inner.code);
        self.state
            .store(state_index(ProcessState::Dead), Ordering::Release);
        frozen
    }
}
