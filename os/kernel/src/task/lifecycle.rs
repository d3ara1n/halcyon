//! Process 生命周期状态机：Building → Running → Terminating → Dead 的
//! 唯一真值、线程容器记录与终止待办（notes/ideas/task.md「进程」）。
//!
//! 锁序契约（顶级锁）：lifecycle 锁只改状态、终因、成员记录、active
//! mask 与 Building 操作计数，锁内不调用 subscribe/offer/enqueue/IPI/
//! 对象 close/uaccess/页表操作——这些动作经 [`TerminationTodo`] 延迟到
//! 锁外执行。方向约束：lifecycle 锁内不得出游获取任何其他锁（对象锁、
//! WaitContext/期限表锁、调度类锁、地址空间/HandleTable 锁）；反向的
//! 单向嵌套（如 ProcessControl 快照在对象锁内进入 lifecycle）因
//! lifecycle 不出游而安全，不构成环。
//!
//! 成员记录是线程容器的唯一真值，但 `Gone` 只在线程强引用真正消散后
//! 由持有方写入（pick 吸收的 reap / WaitContext 完成方 / Start 收尾方）：
//! kill 不把仍在队列或等待竞争中的线程记 Gone，只组装触达待办。
//! REAPABLE 严格晚于最后一个线程容器强引用释放与最后一个 Building
//! 操作退出（building_ops == 0）。

use core::sync::atomic::{AtomicUsize, Ordering};

use alloc::sync::Weak;

use erhino_shared::proc::{ProcessExitReason, ProcessState};

use super::wait::WaitContext;

/// 主线程的容器记录（多线程里程碑扩展为成员表）。
#[derive(Clone)]
pub(crate) enum ThreadRecord {
    /// 无线程且无线程所有权在途：Building（未 Start）或线程已离场。
    Gone,
    /// 在某调度类就绪队列中；线程强引用由队列持有。
    Ready,
    /// Start 已线性化（Building→Running）但主线程尚未入队；
    /// 线程强引用由 Start 调用方持有，收尾方负责 Gone 或转 Ready。
    Staging,
    /// 在某 hart 执行点上；线程强引用由调度循环持有，IPI 吸收。
    Running { slot: usize },
    /// 无容器等待中；线程强引用由 WaitContext 持有，经 weak 触达取消。
    Waiting { context: Weak<WaitContext> },
    /// 已冻结终因、正在退出路径上（调用线程即目标，reap 收尾）。
    Exiting,
}

/// 一次终止请求在锁内线性化后需要锁外执行的副作用。
#[derive(Default)]
pub(crate) struct TerminationTodo {
    /// 需要取消的等待上下文（原 Waiting 记录）；完成方负责收尾与离场确认。
    pub cancel_wait: Option<Weak<WaitContext>>,
    /// 需要 IPI 请求离开用户态的 hart slot 位图（原 Running 记录）。
    pub ipi_slots: u64,
    /// 无线程、无在途 Building 操作且 active 为零，可立即发布 REAPABLE
    /// （Building 目标的 kill/abandonment）。
    pub reapable: bool,
}

/// 生命周期内核侧状态（内嵌于 Process，不是独立对象）。
pub(crate) struct Lifecycle {
    /// 原子快速路径：trap 入口与调度 gate 只读，不走锁。
    /// 判别值与 `erhino_shared::proc::ProcessState` 一致。
    state: AtomicUsize,
    /// 顶级锁：终因冻结、成员记录、active mask 的复合转换。
    inner: crate::sync::Spinlock<LifecycleInner>,
}

struct LifecycleInner {
    reason: ProcessExitReason,
    code: i64,
    member: ThreadRecord,
    /// 本进程线程所在 hart 的 slot 位图：dispatch 前（进入用户 satp 前）
    /// 置位，Switch 出口归一 satp 后清除；全零且线程离场方可 REAPABLE。
    active: u64,
    /// 在途 Building 操作（builder 的 map/write/start）计数；
    /// 终止后归零才能发布 REAPABLE（操作退出方负责触发）。
    building_ops: usize,
}

fn state_index(state: ProcessState) -> usize {
    state as usize
}

impl Lifecycle {
    pub(crate) fn building() -> Self {
        Self {
            state: AtomicUsize::new(state_index(ProcessState::Building)),
            inner: crate::sync::Spinlock::new(LifecycleInner {
                reason: ProcessExitReason::None,
                code: 0,
                member: ThreadRecord::Gone,
                active: 0,
                building_ops: 0,
            }),
        }
    }

    /// trap 入口 / 调度 gate 的快速谓词（原子读，无锁）。
    pub fn is_terminating(&self) -> bool {
        self.state.load(Ordering::Acquire) >= state_index(ProcessState::Terminating)
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

    /// Building 操作退出：计数归零且已终止、无线程、无 active 时返回
    /// true——调用方负责锁外发布 REAPABLE。
    pub(crate) fn leave_building_op(&self) -> bool {
        let mut inner = self.inner.lock();
        inner.building_ops -= 1;
        self.is_terminating()
            && matches!(inner.member, ThreadRecord::Gone)
            && inner.active == 0
            && inner.building_ops == 0
    }

    /// ProcessStart 线性化：Building → Running，成员转 Staging（线程
    /// 所有权在调用方手里）；同时消费 Building 操作登记。失败表示已
    /// 终止，调用方不得继续（退出操作登记并按 ObjectClosed 回滚）。
    pub(crate) fn begin_running(&self) -> bool {
        let mut inner = self.inner.lock();
        if self.state.load(Ordering::Acquire) != state_index(ProcessState::Building) {
            return false;
        }
        debug_assert!(matches!(inner.member, ThreadRecord::Gone));
        debug_assert!(inner.building_ops > 0, "start must hold a building op");
        inner.building_ops -= 1;
        inner.member = ThreadRecord::Staging;
        self.state.store(state_index(ProcessState::Running), Ordering::Release);
        true
    }

    /// Start 成功提交：主线程入就绪队列后 Staging → Ready。
    pub(crate) fn staging_ready(&self) {
        let mut inner = self.inner.lock();
        if matches!(inner.member, ThreadRecord::Staging) {
            inner.member = ThreadRecord::Ready;
        }
    }

    /// 请求终止：首次到达者冻结终因并组装锁外待办；后续请求幂等返回空。
    /// `exiting_self`：调用线程即目标（Exit/fault 退出路径，member → Exiting）；
    /// 否则按成员记录组装触达待办——不改变 Ready/Waiting/Staging 记录，
    /// 线程离场与 Gone 记录由实际持有线程所有权的路径完成。
    pub(crate) fn request_termination(
        &self,
        reason: ProcessExitReason,
        code: i64,
        exiting_self: bool,
    ) -> TerminationTodo {
        let mut todo = TerminationTodo::default();
        let mut inner = self.inner.lock();
        if self.state.load(Ordering::Acquire) >= state_index(ProcessState::Terminating) {
            return todo; // 终因已冻结：幂等，后续事件只协助收束
        }
        inner.reason = reason;
        inner.code = code;
        if exiting_self {
            inner.member = ThreadRecord::Exiting;
        } else {
            match inner.member.clone() {
                ThreadRecord::Gone => {
                    if inner.building_ops == 0 && inner.active == 0 {
                        todo.reapable = true;
                    }
                    // 在途 Building 操作：由 leave_building_op 触发 REAPABLE。
                }
                // 队列持有线程：pick gate 吸收后 reap 完成离场确认。
                ThreadRecord::Ready => {}
                // Start 调用方持有线程：收尾方（成功路径 gate / 失败路径
                // 防御终止）完成离场确认。
                ThreadRecord::Staging => {}
                // 调度循环持有线程：IPI 请求离开用户态，trap 入口吸收。
                ThreadRecord::Running { slot } => {
                    todo.ipi_slots = 1u64 << slot;
                }
                // WaitContext 持有线程：offer(Abandoned) 竞争，胜者收尾。
                ThreadRecord::Waiting { context } => {
                    todo.cancel_wait = Some(context);
                }
                ThreadRecord::Exiting => {}
            }
        }
        self.state.store(state_index(ProcessState::Terminating), Ordering::Release);
        todo
    }

    /// park 发布线性化：Running → Waiting；已 Terminating 返回 false，
    /// 调用方不得发布等待，改走 Abandoned 取消。
    pub(crate) fn park_waiting(&self, context: &alloc::sync::Arc<WaitContext>) -> bool {
        let mut inner = self.inner.lock();
        if self.is_terminating() {
            return false;
        }
        debug_assert!(matches!(inner.member, ThreadRecord::Running { .. }));
        inner.member = ThreadRecord::Waiting {
            context: alloc::sync::Arc::downgrade(context),
        };
        true
    }

    /// Switch 出口（Requeue 分支）：清 active，Running → Ready。
    pub(crate) fn on_requeue(&self, slot: usize) {
        let mut inner = self.inner.lock();
        inner.active &= !(1u64 << slot);
        if !self.is_terminating() {
            if matches!(inner.member, ThreadRecord::Running { .. }) {
                inner.member = ThreadRecord::Ready;
            }
        }
    }

    /// 非-Resume 出口（Killed/Park）只清 active 位：线程已离开用户
    /// 执行点，成员记录由各自后续路径（reap/park 发布）接管。
    pub(crate) fn clear_active(&self, slot: usize) {
        self.inner.lock().active &= !(1u64 << slot);
    }

    /// 调度循环 dispatch 前：Running 记录 + active 置位。
    /// Terminating 返回 false（惰性收束，不进入用户态）。
    pub(crate) fn enter_running(&self, slot: usize) -> bool {
        let mut inner = self.inner.lock();
        if self.is_terminating() {
            return false;
        }
        inner.member = ThreadRecord::Running { slot };
        inner.active |= 1u64 << slot;
        true
    }

    /// 线程离场完成（调用方已 drop 线程强引用：reap / WaitContext 完成
    /// 方 / Start 防御失败路径）：member → Gone；返回是否可发布 REAPABLE。
    pub(crate) fn thread_departed(&self) -> bool {
        let mut inner = self.inner.lock();
        inner.member = ThreadRecord::Gone;
        self.is_terminating()
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
        self.state.store(state_index(ProcessState::Dead), Ordering::Release);
        frozen
    }
}
