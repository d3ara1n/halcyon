//! 单 actor 执行运行体：持有来源登记寿命、迟到事件过滤、
//! 期限唤醒与公平任务推进。任务经 [`Requests`] 声明运行体请求，
//! advance 返回后统一应用；Assembly 独占集合接收，业务只声明登记/注销和推进。

use crate::work_queue::{FinalizingStep, InsertFailure, WorkQueue};
use crate::{
    ExecutionResource,
    source::{Registration, SourceOps},
};
use alloc::{boxed::Box, vec::Vec};
use core::pin::Pin;
use core::sync::atomic::{AtomicUsize, Ordering};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals},
    time::Deadline,
    wait::WaitReason,
    wait_set::{MemberRequest, ReadyRecord},
};
use libbudget::{AccountView, Charge};
use ordered_table::OrderedTable;
use timer_queue::{TimerNode, TimerQueue};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Runnable,
    Parked,
    Complete,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Advance {
    pub work_done: usize,
    pub step: Step,
}

/// 任务声明类别标签；含义由任务自定义。
pub type SourceKind = u64;

/// 运行体签发的来源身份；对任务是不透明值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceId {
    token: u64,
    task: u64,
}

impl SourceId {
    pub fn token(self) -> u64 {
        self.token
    }
}

/// 不透明的观察描述；只传递登记意图，不分配、不导出关闭权或数据访问权。
pub struct SourcePlan {
    handle: Handle,
    signals: ObjectSignals,
}

impl SourcePlan {
    pub fn new(handle: Handle, signals: ObjectSignals) -> Self {
        Self { handle, signals }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceEvent {
    pub source: SourceId,
    pub kind: SourceKind,
    pub observed: ObjectSignals,
}

/// 任务 advance 内可见的输入：已投递来源事件与期限命中。
/// 迭代即消费；未消费输入使 Parked 任务重新就绪。
pub struct Input<'a> {
    events: &'a [SourceEvent],
    cursor: usize,
    timed_out: bool,
    now_ns: u64,
}

impl Input<'_> {
    pub fn now_ns(&self) -> u64 {
        self.now_ns
    }
    /// 拉取下一条已投递事件；迭代即消费。
    pub fn pull(&mut self) -> Option<SourceEvent> {
        let event = self.events.get(self.cursor).copied();
        if event.is_some() {
            self.cursor += 1;
        }
        event
    }
    pub fn take_timeout(&mut self) -> bool {
        core::mem::take(&mut self.timed_out)
    }
    pub fn has_pending(&self) -> bool {
        self.cursor < self.events.len() || self.timed_out
    }
}

/// 运行体在 Gate 应用请求时同步返还的失败；派生失败携带完整任务 owner。
pub enum RequestFailure<T> {
    Spawn {
        task: T,
        error: SystemCallError,
    },
    Source {
        kind: SourceKind,
        error: SystemCallError,
    },
    Wake {
        task: u64,
        error: SystemCallError,
    },
}

/// 任务在 advance 内声明的运行体请求；advance 返回后按序应用。
pub struct Requests<T> {
    operations: Vec<RequestOperation<T>>,
}

const REQUEST_CAPACITY: usize = 16;

enum RequestOperation<T> {
    Spawn { task: T, max_sources: usize },
    Source(SourceRequest),
    Wake { task: u64 },
}

struct PendingGate {
    task: u64,
    step: Step,
    refused: bool,
    rejection: Option<SystemCallError>,
}

enum SourceRequest {
    /// 登记意图统一为值计划，cookie 由运行体注入任务身份。
    Register { plan: SourcePlan, kind: SourceKind },
    /// 撤销登记并释放其输入额度。
    Remove { source: SourceId },
}

impl<T> Requests<T> {
    fn new() -> Result<Self, SystemCallError> {
        let mut operations = Vec::new();
        operations
            .try_reserve_exact(REQUEST_CAPACITY)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(Self { operations })
    }

    /// 请求派生新任务；准入失败在本次 Gate 收尾时同步返还给父任务。
    pub fn spawn(&mut self, task: T, max_sources: usize) -> Result<(), T> {
        if self.operations.len() == REQUEST_CAPACITY {
            return Err(task);
        }
        self.operations
            .push(RequestOperation::Spawn { task, max_sources });
        Ok(())
    }

    pub fn add_source(
        &mut self,
        handle: Handle,
        signals: ObjectSignals,
        kind: SourceKind,
    ) -> Result<(), SystemCallError> {
        self.register(SourcePlan::new(handle, signals), kind)
            .map_err(|_| SystemCallError::ReachLimit)
    }

    pub fn register(&mut self, plan: SourcePlan, kind: SourceKind) -> Result<(), SourcePlan> {
        if self.operations.len() == REQUEST_CAPACITY {
            return Err(plan);
        }
        self.operations
            .push(RequestOperation::Source(SourceRequest::Register {
                plan,
                kind,
            }));
        Ok(())
    }

    pub fn remove(&mut self, source: SourceId) -> Result<(), SystemCallError> {
        if self.operations.len() == REQUEST_CAPACITY {
            return Err(SystemCallError::ReachLimit);
        }
        self.operations
            .push(RequestOperation::Source(SourceRequest::Remove { source }));
        Ok(())
    }

    /// 请求唤醒同一 Runtime 中的另一项活动任务；不持有任务或 Runtime 借用。
    pub fn wake(&mut self, task: u64) -> Result<(), SystemCallError> {
        if self.operations.len() == REQUEST_CAPACITY {
            return Err(SystemCallError::ReachLimit);
        }
        self.operations.push(RequestOperation::Wake { task });
        Ok(())
    }

    fn pop_front(&mut self) -> Option<RequestOperation<T>> {
        (!self.operations.is_empty()).then(|| self.operations.remove(0))
    }
}

pub struct SourceEntry {
    task: u64,
    kind: SourceKind,
    removing: bool,
    input_index: usize,
    input_queued: bool,
    input_previous: u64,
    input_next: u64,
    retire_timer: Pin<Box<TimerNode<u64>>>,
    pending: Option<ObjectSignals>,
    _charge: Charge,
}

const SOURCE_INPUT_BYTES: usize =
    ordered_table::PreparedEntry::<SourceEntry, ()>::allocation_bytes()
        + core::mem::size_of::<TimerNode<u64>>();

static RETIRED_CLEANUP_FAILURES: AtomicUsize = AtomicUsize::new(0);
const SOURCE_RETIRE_RETRY_NS: u64 = 1_000_000;

pub fn retired_cleanup_failures() -> usize {
    RETIRED_CLEANUP_FAILURES.load(Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskFailure {
    pub task: u64,
    pub error: SystemCallError,
}

/// 组合驱动与同步 run 共用同一可推进状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveState {
    Runnable,
    Waiting(Deadline),
    Drained,
}

/// 稳定任务契约：advance 消费输入、按预算推进并报告下一步。
/// 停止意图经 stop 冻结，实际退休仍在后续 advance 内完成。
/// [`Task::Family`] 是本服务的任务族类型（通常是枚举）：派生请求以族
/// 类型进入运行体，由驱动点类型强制 Family 与队列元素一致。
pub trait Task<W>: Sized {
    type Family: Task<W>;

    fn advance(
        &mut self,
        id: u64,
        world: &mut W,
        requests: &mut Requests<Self::Family>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError>;
    /// Gate 请求被拒时同步接回 owner；调用返回后任务会再次进入 ready 队尾。
    fn refused(&mut self, world: &mut W, failure: RequestFailure<Self::Family>);
    /// 登记成功回执；任务可在首次事件前撤销来源（例如等待超时）。
    fn registered(&mut self, _world: &mut W, _kind: SourceKind, _source: SourceId) {}
    /// 注销已实际完成，允许任务兑现被观察对象的关闭责任。
    fn unregistered(&mut self, _world: &mut W, _kind: SourceKind, _source: SourceId) {}
    fn stop(&mut self, world: &mut W);
    fn deadline(&self) -> Deadline {
        Deadline::INFINITE
    }
}

pub struct Runtime<T, S: SourceOps> {
    queue: WorkQueue<T>,
    sources: OrderedTable<SourceEntry, ()>,
    set: Registration<S>,
    account: AccountView<ExecutionResource>,
    scratch: Vec<SourceEvent>,
    phase: u8,
    maintenance_phase: u8,
    retire_timers: TimerQueue<u64>,
    clock_now: u64,
    pending_gate: Option<PendingGate>,
    requests: Requests<T>,
    _input_charge: Charge,
}

impl<T, S: SourceOps> Runtime<T, S> {
    fn fixed_input_bytes(source_limit: usize) -> Result<usize, SystemCallError> {
        source_limit
            .checked_mul(core::mem::size_of::<SourceEvent>())
            .and_then(|bytes| {
                REQUEST_CAPACITY
                    .checked_mul(core::mem::size_of::<RequestOperation<T>>())
                    .and_then(|gate| bytes.checked_add(gate))
            })
            .ok_or(SystemCallError::ReachLimit)
    }

    /// 计算固定存储与全部可准入来源的输入额度，供装配者建立账户。
    pub fn input_budget(source_limit: usize) -> Result<usize, SystemCallError> {
        Self::fixed_input_bytes(source_limit)?
            .checked_add(
                source_limit
                    .checked_mul(SOURCE_INPUT_BYTES)
                    .ok_or(SystemCallError::ReachLimit)?,
            )
            .ok_or(SystemCallError::ReachLimit)
    }

    pub fn new(
        set: Registration<S>,
        task_limit: usize,
        source_limit: usize,
        account: &AccountView<ExecutionResource>,
    ) -> Result<Self, SystemCallError> {
        if task_limit == 0 || source_limit == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        if set.failed() {
            return Err(SystemCallError::ObjectClosed);
        }
        let input_charge = account.acquire(
            ExecutionResource::InputBytes,
            Self::fixed_input_bytes(source_limit)?,
        )?;
        let requests = Requests::new()?;
        let mut scratch = Vec::new();
        scratch
            .try_reserve_exact(source_limit)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        let queue = WorkQueue::new(task_limit)?;
        Ok(Self {
            set,
            queue,
            sources: OrderedTable::new(source_limit),
            account: account.clone(),
            scratch,
            phase: 0,
            maintenance_phase: 0,
            retire_timers: TimerQueue::new(0),
            clock_now: 0,
            pending_gate: None,
            requests,
            _input_charge: input_charge,
        })
    }

    pub(crate) fn binding(&self) -> &Registration<S> {
        &self.set
    }

    pub fn spawn(&mut self, task: T, max_sources: usize) -> Result<u64, InsertFailure<T>> {
        if self.set.failed() {
            return Err(InsertFailure {
                task,
                error: SystemCallError::ObjectClosed,
            });
        }
        self.queue.insert(task, max_sources, &self.account)
    }

    pub fn get_task_mut(&mut self, id: u64) -> Option<&mut T> {
        self.queue.get_task_mut(id)
    }

    pub fn wake(&mut self, id: u64) -> Result<(), SystemCallError> {
        self.queue.schedule(id)
    }

    pub fn seal(&mut self) {
        self.queue.seal();
    }

    pub fn is_sealed(&self) -> bool {
        self.queue.is_sealed()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn pending_tasks(&self) -> usize {
        self.queue.len()
    }

    pub fn pending_sources(&self) -> usize {
        self.sources.len()
    }

    pub fn has_pending_gate(&self) -> bool {
        self.pending_gate.is_some()
    }

    pub fn has_ready(&self) -> bool {
        self.queue.has_ready()
    }

    /// 三工作面轮转推进：期限、输入与任务各按份额推进，空面计一单位。
    pub fn turn<W>(&mut self, world: &mut W, max_work: usize) -> Result<usize, TaskFailure>
    where
        T: Task<W, Family = T>,
    {
        if self.set.failed() {
            return Err(TaskFailure {
                task: 0,
                error: SystemCallError::ObjectClosed,
            });
        }
        if max_work == 0 {
            return Err(TaskFailure {
                task: 0,
                error: SystemCallError::IllegalArgument,
            });
        }
        let slice = max_work.div_ceil(3);
        let mut used = 0;
        while used < max_work {
            let budget = slice.min(max_work - used);
            let phase = self.phase;
            self.phase = (self.phase + 1) % 3;
            let actual = match phase {
                0 => {
                    let now = self
                        .set
                        .set()
                        .now_ns()
                        .map_err(|error| TaskFailure { task: 0, error })?;
                    self.clock_now = now;
                    self.queue.expire(now, budget)
                }
                1 => self.retire(world, budget)?,
                _ => self
                    .advance_one(world, budget)?
                    .map_or(0, |advance| advance.work_done),
            };
            // 查询空工作面也有固定成本，计一单位并让下一轮从后继工作面开始。
            used += actual.max(1);
        }
        Ok(used)
    }

    pub fn drive_state(&self) -> DriveState {
        if self.queue.is_empty() && self.sources.is_empty() && self.pending_gate.is_none() {
            DriveState::Drained
        } else if self.queue.has_ready() || self.pending_gate.is_some() || self.maintenance_ready()
        {
            DriveState::Runnable
        } else {
            DriveState::Waiting(self.next_deadline())
        }
    }

    /// 错误任务保留原输入、来源和额度；其他任务继续推进。
    pub fn defer_failed_task(
        &mut self,
        id: u64,
        retry_at: Option<u64>,
    ) -> Result<(), SystemCallError> {
        self.queue.defer(id, retry_at)
    }

    fn maintenance_ready(&self) -> bool {
        self.queue.has_unmarked_sources()
            || self.queue.has_retire_ready()
            || self
                .retire_timers
                .peek_expires_at()
                .is_some_and(|at| at <= self.clock_now)
    }

    /// 预付期限堆提供 O(1) 最近期限，等待决策不扫描来源表。
    pub fn next_deadline(&self) -> Deadline {
        let task_at = self
            .queue
            .next_deadline()
            .instant()
            .expect("task deadlines are validated before installation");
        match (task_at, self.retire_timers.peek_expires_at()) {
            (Some(a), Some(b)) => Deadline::at(a.min(b)),
            (Some(at), None) | (None, Some(at)) => Deadline::at(at),
            (None, None) => Deadline::INFINITE,
        }
    }

    /// Seal 后每步只发出一个停止意图，任务自身仍通过 advance 按预算退休。
    pub fn shutdown_turn<W>(&mut self, world: &mut W, max_work: usize) -> Result<usize, TaskFailure>
    where
        T: Task<W, Family = T>,
    {
        if self.set.failed() {
            return Err(TaskFailure {
                task: 0,
                error: SystemCallError::ObjectClosed,
            });
        }
        if max_work == 0 {
            return Err(TaskFailure {
                task: 0,
                error: SystemCallError::IllegalArgument,
            });
        }
        let now = self
            .set
            .set()
            .now_ns()
            .map_err(|error| TaskFailure { task: 0, error })?;
        self.clock_now = now;
        let stopped = usize::from(
            self.queue
                .stop_next(world, now)
                .map_err(|(task, error)| TaskFailure { task, error })?,
        );
        if stopped == max_work {
            return Ok(stopped);
        }
        self.turn(world, max_work - stopped)
            .map(|used| used + stopped)
    }

    /// 任务清空且来源已全部退休后收束观察集合；失败原样返还运行体。
    #[expect(
        clippy::result_large_err,
        reason = "关闭失败返还仍拥有任务的运行体，错误路径不分配"
    )]
    pub fn close(self) -> Result<(), (Self, SystemCallError)> {
        if self.pending_gate.is_some() || !self.queue.is_empty() || !self.sources.is_empty() {
            return Err((self, SystemCallError::ObjectBusy));
        }
        Ok(())
    }

    fn retire<W>(&mut self, world: &mut W, budget: usize) -> Result<usize, TaskFailure>
    where
        T: Task<W, Family = T>,
    {
        let mut used = 0;
        while used < budget {
            let mut progressed = false;
            for _ in 0..2 {
                self.maintenance_phase ^= 1;
                if self.maintenance_phase == 0 {
                    match self.queue.finalize_one() {
                        FinalizingStep::Idle => continue,
                        FinalizingStep::Source(token) => self.begin_remove(token),
                        FinalizingStep::Progress => {}
                    }
                } else {
                    let Some((at, &token)) = self.retire_timers.peek() else {
                        continue;
                    };
                    if at > self.clock_now {
                        continue;
                    }
                    let task = self
                        .sources
                        .get(token)
                        .expect("retiring source remains owned")
                        .task;
                    if let Err(error) = self.drop_source(world, token)
                        && self.sources.get(token).is_none()
                    {
                        // 来源已真实退休：此时错误属于回调修改后的任务期限，
                        // 不能伪装成仍需重试的内核 Remove，也不能吞掉原任务身份。
                        return Err(TaskFailure { task, error });
                    }
                }
                progressed = true;
                break;
            }
            if !progressed {
                break;
            }
            used += 1;
        }
        Ok(used)
    }

    /// Assembly 交付纯电平提示；cookie 是全局任务身份，token 在同一集合不复用。
    pub fn feed(&mut self, record: ReadyRecord) -> bool {
        let Some(entry) = self.sources.get_mut(record.token) else {
            return false;
        };
        if entry.removing || entry.task != record.cookie || record.reserved != 0 {
            return false;
        }
        let observed = match WaitReason::from_u32(record.reason) {
            Some(WaitReason::Signaled) => record.observed,
            Some(WaitReason::Closed) => record.observed | ObjectSignals::CLOSED,
            _ => return false,
        };
        entry.pending = Some(entry.pending.unwrap_or(ObjectSignals::NONE) | observed);
        let task = entry.task;
        self.enqueue_input(record.token, false);
        self.queue
            .schedule(task)
            .expect("source task disappeared before delivery");
        true
    }

    fn enqueue_input(&mut self, token: u64, front: bool) {
        let entry = self.sources.get(token).expect("input source remains owned");
        if entry.input_queued {
            return;
        }
        let task = entry.task;
        let slot = self.queue.slot(task).expect("input task remains owned");
        let (previous, next) = if front {
            (0, slot.input_head)
        } else {
            (slot.input_tail, 0)
        };
        if previous == 0 {
            slot.input_head = token;
        }
        if next == 0 {
            slot.input_tail = token;
        }
        if previous != 0 {
            self.sources
                .get_mut(previous)
                .expect("input predecessor exists")
                .input_next = token;
        }
        if next != 0 {
            self.sources
                .get_mut(next)
                .expect("input successor exists")
                .input_previous = token;
        }
        let entry = self
            .sources
            .get_mut(token)
            .expect("input source remains owned");
        entry.input_queued = true;
        entry.input_previous = previous;
        entry.input_next = next;
    }

    fn unlink_input(&mut self, token: u64) {
        let Some(entry) = self.sources.get_mut(token) else {
            return;
        };
        if !entry.input_queued {
            return;
        }
        let (task, previous, next) = (entry.task, entry.input_previous, entry.input_next);
        entry.input_queued = false;
        entry.input_previous = 0;
        entry.input_next = 0;
        let slot = self.queue.slot(task).expect("input task remains owned");
        if previous == 0 {
            slot.input_head = next;
        }
        if next == 0 {
            slot.input_tail = previous;
        }
        if previous != 0 {
            self.sources
                .get_mut(previous)
                .expect("input predecessor exists")
                .input_next = next;
        }
        if next != 0 {
            self.sources
                .get_mut(next)
                .expect("input successor exists")
                .input_previous = previous;
        }
    }

    fn pop_input(&mut self, task: u64) -> Option<SourceEvent> {
        let token = self.queue.slot(task)?.input_head;
        if token == 0 {
            return None;
        }
        self.unlink_input(token);
        let entry = self
            .sources
            .get_mut(token)
            .expect("queued source remains owned");
        entry.pending.take().map(|observed| SourceEvent {
            source: SourceId {
                token,
                task: entry.task,
            },
            kind: entry.kind,
            observed,
        })
    }

    /// 从活动输入槽摘除，保留独立退休记录与额度直到真实注销成功。
    fn begin_remove(&mut self, token: u64) {
        self.unlink_input(token);
        let Some(entry) = self.sources.get_mut(token) else {
            return;
        };
        if entry.removing {
            return;
        }
        entry.removing = true;
        entry.pending = None;
        let task = entry.task;
        let index = entry.input_index;
        if let Some(moved) = self.queue.detach_source(task, index) {
            self.sources
                .get_mut(moved)
                .expect("moved source must remain owned")
                .input_index = index;
        }
        let node = self
            .sources
            .get(token)
            .expect("retiring source lost storage")
            .retire_timer
            .as_ref();
        // SAFETY: actor 独占 runtime，节点 Box pin 在来源出生时预付，取消后再注册。
        unsafe {
            self.retire_timers.cancel(node);
            self.retire_timers.register(node, self.clock_now, token);
        }
    }

    /// 一次注销；失败只改期该来源自己的预付槽，不影响其他来源。
    fn drop_source<W>(&mut self, world: &mut W, token: u64) -> Result<(), SystemCallError>
    where
        T: Task<W, Family = T>,
    {
        if self.sources.get(token).is_none() {
            return Ok(());
        }
        self.begin_remove(token);
        let result = self.set.set().remove_source(token);
        match result {
            Ok(()) | Err(SystemCallError::ObjectNotFound) => {
                let task = self
                    .sources
                    .get(token)
                    .expect("removing source remains owned")
                    .task;
                let callback_at = if self.queue.accepts_sources(task) {
                    match self.set.set().now_ns() {
                        Ok(now) => {
                            self.clock_now = now;
                            Some(now)
                        }
                        Err(error) => {
                            // 内核成员已移除，用户态责任仍保留；下次 Remove 的
                            // ObjectNotFound 会继续此处，回执只在取样成功后交付一次。
                            self.retry_source_removal(token);
                            return Err(error);
                        }
                    }
                } else {
                    None
                };
                let entry = self
                    .sources
                    .remove(token)
                    .map(ordered_table::PreparedEntry::into_value)
                    .expect("retiring source must remain owned");
                // SAFETY: entry 已摘但 Box 节点仍存活，队列由本 actor 独占。
                unsafe {
                    self.retire_timers.cancel(entry.retire_timer.as_ref());
                }
                self.queue.source_retired(entry.task);
                if let Some(now) = callback_at {
                    self.queue.mature_deadline(entry.task, now);
                }
                self.queue
                    .get_task_mut(entry.task)
                    .expect("source task must remain owned")
                    .unregistered(
                        world,
                        entry.kind,
                        SourceId {
                            token,
                            task: entry.task,
                        },
                    );
                self.queue
                    .schedule(entry.task)
                    .expect("source task must remain owned");
                if self.queue.accepts_sources(entry.task) {
                    let deadline = self.task_deadline::<W>(entry.task);
                    self.queue.update_deadline(entry.task, deadline)?;
                }
                Ok(())
            }
            Err(error) => {
                self.retry_source_removal(token);
                Err(error)
            }
        }
    }

    fn retry_source_removal(&mut self, token: u64) {
        let node = self
            .sources
            .get(token)
            .expect("failed source must remain owned")
            .retire_timer
            .as_ref();
        // SAFETY: actor 独占，来源记录保持 Box 存活，取消/注册不分配。
        unsafe {
            self.retire_timers.cancel(node);
            self.retire_timers.register(
                node,
                self.clock_now.saturating_add(SOURCE_RETIRE_RETRY_NS),
                token,
            );
        }
        RETIRED_CLEANUP_FAILURES.fetch_add(1, Ordering::Relaxed);
    }

    fn advance_one<W>(
        &mut self,
        world: &mut W,
        budget: usize,
    ) -> Result<Option<Advance>, TaskFailure>
    where
        T: Task<W, Family = T>,
    {
        if budget == 0 {
            return Err(TaskFailure {
                task: 0,
                error: SystemCallError::IllegalArgument,
            });
        }
        if self.pending_gate.is_some() {
            return self.advance_gate(world, budget);
        }
        let now = self
            .set
            .set()
            .now_ns()
            .map_err(|error| TaskFailure { task: 0, error })?;
        self.clock_now = now;
        let Some(id) = self.queue.pop_ready() else {
            return Ok(None);
        };
        self.queue.mature_deadline(id, now);
        // 输入FIFO只弹出本次预算允许的记录，不遍历任务的全部登记。
        self.scratch.clear();
        let timed_out = core::mem::take(
            &mut self
                .queue
                .slot(id)
                .expect("ready task remains owned")
                .timed_out,
        );
        for _ in 0..budget {
            let Some(event) = self.pop_input(id) else {
                break;
            };
            self.scratch.push(event);
        }
        let mut input = Input {
            events: &self.scratch,
            cursor: 0,
            timed_out,
            now_ns: now,
        };
        let result = {
            let task = &mut self.queue.slot(id).expect("ready task remains owned").task;
            task.advance(id, world, &mut self.requests, &mut input, budget)
        };
        let consumed = input.cursor;
        let timed_out = input.timed_out;
        let _ = input;
        for index in (consumed..self.scratch.len()).rev() {
            let event = self.scratch[index];
            if let Some(entry) = self.sources.get_mut(event.source.token()) {
                debug_assert!(
                    entry.pending.is_none(),
                    "pending source input restored twice"
                );
                entry.pending = Some(event.observed);
                self.enqueue_input(event.source.token(), true);
            }
        }
        if let Some(slot) = self.queue.slot(id) {
            slot.timed_out |= timed_out;
        }
        let has_pending = self.queue.slot(id).is_some_and(|slot| slot.input_head != 0) || timed_out;
        let deadline = self.task_deadline::<W>(id);
        let updated = self.queue.update_deadline(id, deadline);
        let result = match (result, updated) {
            (Ok(advance), Ok(())) => Ok(advance),
            (Err(error), _) | (_, Err(error)) => Err(error),
        };
        let mut advance = match result {
            Ok(advance) if advance.work_done <= budget => advance,
            result => {
                let error = result.err().unwrap_or(SystemCallError::InternalError);
                if self.requests.operations.is_empty() {
                    self.queue
                        .schedule(id)
                        .expect("failed task must remain owned");
                } else {
                    self.pending_gate = Some(PendingGate {
                        task: id,
                        step: Step::Runnable,
                        refused: false,
                        rejection: Some(error),
                    });
                }
                return Err(TaskFailure { task: id, error });
            }
        };
        advance.work_done = advance.work_done.max(1);
        if !self.requests.operations.is_empty() {
            self.pending_gate = Some(PendingGate {
                task: id,
                step: advance.step,
                refused: false,
                rejection: None,
            });
            return Ok(Some(advance));
        }
        if let Err(error) = self
            .queue
            .finish_advance(id, deadline, advance.step, has_pending)
        {
            return Err(TaskFailure { task: id, error });
        }
        Ok(Some(advance))
    }

    fn task_deadline<W>(&mut self, task: u64) -> Result<Option<u64>, SystemCallError>
    where
        T: Task<W, Family = T>,
    {
        self.queue
            .get_task_mut(task)
            .ok_or(SystemCallError::ObjectNotFound)?
            .deadline()
            .instant()
            .map_err(|_| SystemCallError::IllegalArgument)
    }

    fn reject_foreign_source<W>(
        &mut self,
        task: u64,
        world: &mut W,
        request: &SourceRequest,
    ) -> bool
    where
        T: Task<W, Family = T>,
    {
        let source = match request {
            SourceRequest::Remove { source } => source,
            _ => return false,
        };
        if source.task != task {
            self.queue
                .get_task_mut(task)
                .expect("source requester remains owned")
                .refused(
                    world,
                    RequestFailure::Source {
                        kind: 0,
                        error: SystemCallError::ObjectNotFound,
                    },
                );
            return true;
        }
        let Some(entry) = self.sources.get(source.token()) else {
            return false;
        };
        assert_eq!(entry.task, task, "source identity changed owner");
        false
    }

    fn reject_operation<W>(
        &mut self,
        task: u64,
        world: &mut W,
        operation: RequestOperation<T>,
        error: SystemCallError,
    ) where
        T: Task<W, Family = T>,
    {
        match operation {
            RequestOperation::Spawn { task: spawned, .. } => {
                self.queue
                    .get_task_mut(task)
                    .expect("requesting task disappeared while rejecting requests")
                    .refused(
                        world,
                        RequestFailure::Spawn {
                            task: spawned,
                            error,
                        },
                    );
            }
            RequestOperation::Wake { task: target } => {
                self.queue
                    .get_task_mut(task)
                    .expect("requesting task disappeared while rejecting wake")
                    .refused(
                        world,
                        RequestFailure::Wake {
                            task: target,
                            error,
                        },
                    );
            }
            RequestOperation::Source(request) => {
                if self.reject_foreign_source(task, world, &request) {
                    return;
                }
                let (kind, notify) = match request {
                    SourceRequest::Register { kind, .. } => (kind, true),
                    SourceRequest::Remove { source } => {
                        let _ = self.drop_source(world, source.token());
                        (0, false)
                    }
                };
                if notify {
                    self.queue
                        .get_task_mut(task)
                        .expect("requesting task disappeared while rejecting requests")
                        .refused(world, RequestFailure::Source { kind, error });
                }
            }
        }
    }

    fn advance_gate<W>(
        &mut self,
        world: &mut W,
        _budget: usize,
    ) -> Result<Option<Advance>, TaskFailure>
    where
        T: Task<W, Family = T>,
    {
        let now = self
            .set
            .set()
            .now_ns()
            .map_err(|error| TaskFailure { task: 0, error })?;
        self.clock_now = now;
        let mut gate = self.pending_gate.take().expect("gate remains owned");
        self.queue.mature_deadline(gate.task, now);
        if let Some(operation) = self.requests.pop_front() {
            if let Some(error) = gate.rejection {
                self.reject_operation(gate.task, world, operation, error);
                gate.refused = true;
            } else if self.apply_operation(gate.task, world, operation, gate.step != Step::Complete)
            {
                gate.refused = true;
            }
        }
        let deadline = self.task_deadline::<W>(gate.task);
        if let Err(error) = self.queue.update_deadline(gate.task, deadline) {
            gate.rejection = Some(error);
            let task = gate.task;
            self.pending_gate = Some(gate);
            return Err(TaskFailure { task, error });
        }
        if !self.requests.operations.is_empty() {
            self.pending_gate = Some(gate);
            return Ok(Some(Advance {
                work_done: 1,
                step: Step::Runnable,
            }));
        }
        let step = if gate.refused {
            Step::Runnable
        } else {
            gate.step
        };
        let deadline = self.task_deadline::<W>(gate.task);
        let has_pending = self
            .queue
            .slot(gate.task)
            .is_some_and(|slot| slot.input_head != 0 || slot.timed_out);
        self.queue
            .finish_advance(gate.task, deadline, step, has_pending)
            .map_err(|error| TaskFailure {
                task: gate.task,
                error,
            })?;
        Ok(Some(Advance { work_done: 1, step }))
    }

    fn apply_operation<W>(
        &mut self,
        task: u64,
        world: &mut W,
        operation: RequestOperation<T>,
        allow_sources: bool,
    ) -> bool
    where
        T: Task<W, Family = T>,
    {
        match operation {
            RequestOperation::Spawn {
                task: spawned,
                max_sources,
            } => match self.queue.insert(spawned, max_sources, &self.account) {
                Ok(_) => false,
                Err(failure) => {
                    self.queue
                        .get_task_mut(task)
                        .expect("requesting task disappeared before Gate refusal")
                        .refused(
                            world,
                            RequestFailure::Spawn {
                                task: failure.task,
                                error: failure.error,
                            },
                        );
                    true
                }
            },
            RequestOperation::Wake { task: target } => {
                if let Err(error) = self.queue.schedule(target) {
                    self.queue
                        .get_task_mut(task)
                        .expect("requesting task disappeared before wake refusal")
                        .refused(
                            world,
                            RequestFailure::Wake {
                                task: target,
                                error,
                            },
                        );
                    true
                } else {
                    false
                }
            }
            RequestOperation::Source(request) => {
                if self.reject_foreign_source(task, world, &request) {
                    return true;
                }
                if !allow_sources {
                    return match request {
                        SourceRequest::Remove { source } => {
                            let _ = self.drop_source(world, source.token());
                            false
                        }
                        SourceRequest::Register { kind, .. } => {
                            self.queue
                                .get_task_mut(task)
                                .expect("requesting task disappeared before Complete refusal")
                                .refused(
                                    world,
                                    RequestFailure::Source {
                                        kind,
                                        error: SystemCallError::IllegalArgument,
                                    },
                                );
                            true
                        }
                    };
                }
                match request {
                    SourceRequest::Register { plan, kind } => {
                        match self.add_source(task, plan, kind) {
                            Ok(source) => {
                                self.queue
                                    .get_task_mut(task)
                                    .expect("registered task must exist")
                                    .registered(world, kind, source);
                                false
                            }
                            Err(error) => {
                                self.queue
                                    .get_task_mut(task)
                                    .expect("source task remains owned")
                                    .refused(world, RequestFailure::Source { kind, error });
                                true
                            }
                        }
                    }
                    SourceRequest::Remove { source } => {
                        let _ = self.drop_source(world, source.token());
                        false
                    }
                }
            }
        }
    }

    fn prepare_source(
        &mut self,
        task: u64,
        kind: SourceKind,
    ) -> Result<ordered_table::PreparedEntry<SourceEntry, ()>, SystemCallError> {
        let slot = self
            .queue
            .slot(task)
            .ok_or(SystemCallError::ObjectNotFound)?;
        if slot.sources.len() >= slot.max_sources {
            return Err(SystemCallError::ReachLimit);
        }
        let input_index = slot.sources.len();
        let charge = self
            .account
            .acquire(ExecutionResource::InputBytes, SOURCE_INPUT_BYTES)?;
        let retire_timer = Box::into_pin(
            Box::try_new(TimerNode::new()).map_err(|_| SystemCallError::OutOfMemory)?,
        );
        let entry = SourceEntry {
            task,
            kind,
            removing: false,
            input_index,
            input_queued: false,
            input_previous: 0,
            input_next: 0,
            retire_timer,
            pending: None,
            _charge: charge,
        };
        self.sources
            .prepare_insert(0, entry, &libbudget::AllocatorBacked)
            .map_err(|failure| {
                let (entry, error) = match failure {
                    ordered_table::InsertError::Limit(entry) => {
                        (entry, SystemCallError::ReachLimit)
                    }
                    ordered_table::InsertError::Storage(error, entry) => {
                        (entry, libbudget::allocator_error(error))
                    }
                };
                drop(entry);
                error
            })
    }

    fn add_source(
        &mut self,
        task: u64,
        plan: SourcePlan,
        kind: SourceKind,
    ) -> Result<SourceId, SystemCallError> {
        if self.set.failed() || !self.queue.accepts_sources(task) {
            return Err(SystemCallError::ObjectClosed);
        }
        let prepared = self.prepare_source(task, kind)?;
        match self
            .set
            .set()
            .register_item(MemberRequest::new(plan.handle, plan.signals, task))
        {
            Ok(token) => Ok(self.adopt_entry(task, token, prepared)),
            Err(error) => {
                drop(prepared);
                Err(error)
            }
        }
    }

    fn adopt_entry(
        &mut self,
        task: u64,
        token: u64,
        prepared: ordered_table::PreparedEntry<SourceEntry, ()>,
    ) -> SourceId {
        self.sources.insert_prepared(prepared.with_key(token));
        self.queue
            .slot(task)
            .expect("prepared source task disappeared")
            .sources
            .push(token);
        SourceId { token, task }
    }
}

impl<T, S: SourceOps> Drop for Runtime<T, S> {
    fn drop(&mut self) {
        // 先解除侵入式 timer 对节点的访问，再允许来源表释放各自 pin Box。
        while self.retire_timers.pop_expired(u64::MAX).is_some() {}
        // 非空放弃是装配误用，整个接收装配 fail-closed；业务失败必须由原运行体退休。
        if !self.queue.is_empty() || !self.sources.is_empty() || self.pending_gate.is_some() {
            self.set.invalidate();
        }
        while let Some((&token, _)) = self.sources.next_after(None) {
            if let Err(error) = self.set.set().remove_source(token)
                && error != SystemCallError::ObjectNotFound
            {
                RETIRED_CLEANUP_FAILURES.fetch_add(1, Ordering::Relaxed);
            }
            self.sources
                .remove(token)
                .map(ordered_table::PreparedEntry::into_value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assembly::Assembly;
    use alloc::{collections::BTreeMap, collections::VecDeque, rc::Rc, vec};
    use core::cell::{Cell, RefCell};

    struct FakeShared {
        inner: RefCell<FakeInner>,
        now: Cell<u64>,
    }

    struct FakeInner {
        next_token: u64,
        members: BTreeMap<u64, u64>,
        delivered: VecDeque<ReadyRecord>,
        remove_busy: usize,
        waits: Vec<Deadline>,
        removes: Vec<u64>,
        registered: Vec<(Handle, ObjectSignals, u64)>,
        persistent: bool,
    }
    impl FakeShared {
        fn fire_at(&self, token: u64, observed: ObjectSignals, shift: i64) {
            let mut inner = self.inner.borrow_mut();
            let cookie = inner.members[&token].wrapping_add_signed(shift);
            inner.delivered.push_back(ReadyRecord {
                token,
                cookie,
                observed,
                reason: 0,
                reserved: 0,
            });
        }
        fn removes(&self) -> Vec<u64> {
            self.inner.borrow().removes.clone()
        }
        fn registered(&self) -> Vec<(Handle, ObjectSignals, u64)> {
            self.inner.borrow().registered.clone()
        }
        fn waits(&self) -> Vec<Deadline> {
            self.inner.borrow().waits.clone()
        }
        fn set_remove_busy(&self, attempts: usize) {
            self.inner.borrow_mut().remove_busy = attempts;
        }
    }
    struct FakeSet {
        shared: Rc<FakeShared>,
    }
    impl FakeSet {
        fn new(shared: &Rc<FakeShared>) -> Self {
            Self {
                shared: shared.clone(),
            }
        }
    }
    fn fake_shared() -> Rc<FakeShared> {
        Rc::new(FakeShared {
            inner: RefCell::new(FakeInner {
                next_token: 1,
                members: BTreeMap::new(),
                delivered: VecDeque::new(),
                remove_busy: 0,
                waits: Vec::new(),
                removes: Vec::new(),
                registered: Vec::new(),
                persistent: false,
            }),
            now: Cell::new(0),
        })
    }
    impl SourceOps for FakeSet {
        fn register_item(&self, item: MemberRequest) -> Result<u64, SystemCallError> {
            let mut inner = self.shared.inner.borrow_mut();
            let token = inner.next_token;
            inner.next_token += 1;
            inner.members.insert(token, item.cookie);
            inner
                .registered
                .push((item.handle, item.signals, item.cookie));
            Ok(token)
        }
        fn remove_source(&self, token: u64) -> Result<(), SystemCallError> {
            let mut inner = self.shared.inner.borrow_mut();
            if inner.remove_busy > 0 {
                inner.remove_busy -= 1;
                return Err(SystemCallError::ObjectBusy);
            }
            inner
                .members
                .remove(&token)
                .ok_or(SystemCallError::ObjectNotFound)?;
            inner.removes.push(token);
            Ok(())
        }
        fn receive_into(&self, records: &mut [ReadyRecord]) -> Result<usize, SystemCallError> {
            let mut inner = self.shared.inner.borrow_mut();
            if inner.persistent && inner.delivered.is_empty() {
                let members: Vec<_> = inner
                    .members
                    .iter()
                    .map(|(&token, &cookie)| (token, cookie))
                    .collect();
                for (token, cookie) in members {
                    inner.delivered.push_back(ReadyRecord {
                        token,
                        cookie,
                        observed: ObjectSignals::READABLE,
                        reason: 0,
                        reserved: 0,
                    });
                }
            }
            let mut count = 0;
            for slot in records {
                let Some(record) = inner.delivered.pop_front() else {
                    break;
                };
                *slot = record;
                count += 1;
            }
            Ok(count)
        }
        fn wait(&self, deadline: Deadline) -> Result<(), SystemCallError> {
            self.shared.inner.borrow_mut().waits.push(deadline);
            if deadline != Deadline::INFINITE {
                self.shared
                    .now
                    .set(deadline.instant().unwrap().expect("finite deadline"));
            }
            Ok(())
        }
        fn now_ns(&self) -> Result<u64, SystemCallError> {
            Ok(self.shared.now.get())
        }
        fn close_set(self) -> Result<(), (Self, SystemCallError)> {
            self.shared.inner.borrow_mut().members.clear();
            self.shared.inner.borrow_mut().delivered.clear();
            Ok(())
        }
    }

    #[test]
    fn input_preserves_unconsumed_events_and_timeout() {
        let events = [
            SourceEvent {
                source: SourceId { token: 1, task: 9 },
                kind: 1,
                observed: ObjectSignals::READABLE,
            },
            SourceEvent {
                source: SourceId { token: 2, task: 9 },
                kind: 2,
                observed: ObjectSignals::CLOSED,
            },
        ];
        let mut input = Input {
            events: &events,
            cursor: 0,
            timed_out: true,
            now_ns: 0,
        };
        assert_eq!(input.pull().unwrap().source, SourceId { token: 1, task: 9 });
        assert!(input.has_pending());
        assert!(input.take_timeout());
        assert_eq!(input.pull().unwrap().source, SourceId { token: 2, task: 9 });
        assert!(!input.has_pending());
    }

    #[derive(Default)]
    struct World {
        order: Vec<u64>,
        events: Vec<(SourceKind, ObjectSignals)>,
        refusals: Vec<SystemCallError>,
    }

    #[derive(Debug)]
    struct ToyTask {
        turns: usize,
    }

    impl Task<World> for ToyTask {
        type Family = ToyTask;

        fn advance(
            &mut self,
            id: u64,
            world: &mut World,
            _requests: &mut Requests<Self>,
            _input: &mut Input<'_>,
            _budget: usize,
        ) -> Result<Advance, SystemCallError> {
            world.order.push(id);
            self.turns -= 1;
            Ok(Advance {
                work_done: 1,
                step: if self.turns == 0 {
                    Step::Complete
                } else {
                    Step::Runnable
                },
            })
        }
        fn refused(&mut self, _world: &mut World, _failure: RequestFailure<Self>) {
            panic!("ToyTask does not submit Gate requests")
        }
        fn stop(&mut self, world: &mut World) {
            world.order.push(u64::MAX);
        }
    }

    use libbudget::Budget;

    fn account() -> AccountView<ExecutionResource> {
        let budget = Budget::new(&[8, 4096], 1).unwrap();
        let account = budget.account(&[8, 4096]).unwrap();
        account
            .view(&[budget.slot(0).unwrap(), budget.slot(1).unwrap()])
            .unwrap()
    }

    #[test]
    fn constructor_refusal_returns_source_set_and_refunds_budget() {
        let shared = fake_shared();
        let budget = Budget::new(&[8, 1], 1).unwrap();
        let tiny = budget
            .account(&[8, 1])
            .unwrap()
            .view(&[budget.slot(0).unwrap(), budget.slot(1).unwrap()])
            .unwrap();
        let before = tiny.usage(ExecutionResource::InputBytes);
        let (set, error) = match Assembly::new(FakeSet::new(&shared), &tiny) {
            Ok(_) => panic!("insufficient receive budget accepted"),
            Err(failure) => failure,
        };
        assert_eq!(error, SystemCallError::QuotaExceeded);
        assert!(Rc::ptr_eq(&set.shared, &shared));
        assert_eq!(tiny.usage(ExecutionResource::InputBytes), before);
    }

    #[test]
    fn yielding_tasks_alternate_fairly() {
        let shared = fake_shared();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
        let mut rt: Runtime<ToyTask, FakeSet> =
            Runtime::new(assembly.bind().unwrap(), 4, 8, &account()).unwrap();
        let mut world = World::default();
        rt.spawn(ToyTask { turns: 2 }, 0).unwrap();
        rt.spawn(ToyTask { turns: 2 }, 0).unwrap();
        assembly.run(&mut rt, &mut world, 1).unwrap();
        let first = world.order[0];
        let second = world.order[1];
        assert_eq!(world.order, vec![first, second, first, second]);
        assert!(rt.is_empty());
    }

    #[derive(Debug)]
    struct WaitingTask {
        deadline_at: u64,
        armed: bool,
    }

    impl Task<World> for WaitingTask {
        type Family = WaitingTask;

        fn advance(
            &mut self,
            _id: u64,
            _world: &mut World,
            _requests: &mut Requests<Self>,
            input: &mut Input<'_>,
            _budget: usize,
        ) -> Result<Advance, SystemCallError> {
            if input.take_timeout() {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            self.armed = true;
            Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            })
        }
        fn refused(&mut self, _world: &mut World, _failure: RequestFailure<Self>) {
            panic!("WaitingTask does not submit Gate requests")
        }
        fn stop(&mut self, _world: &mut World) {}
        fn deadline(&self) -> Deadline {
            Deadline::at(u64::from(self.armed) * self.deadline_at)
        }
    }

    #[test]
    fn deadline_wakes_parked_task() {
        let shared = fake_shared();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
        let mut rt: Runtime<WaitingTask, FakeSet> =
            Runtime::new(assembly.bind().unwrap(), 4, 8, &account()).unwrap();
        let mut world = World::default();
        rt.spawn(
            WaitingTask {
                deadline_at: 20,
                armed: false,
            },
            0,
        )
        .unwrap();
        // 首轮覆盖三个工作面：任务登记期限并停驻。
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 3).unwrap();
        assert!(!rt.has_ready());
        shared.now.set(20);
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 1).unwrap();
        assert!(rt.has_ready());
        assembly.run(&mut rt, &mut world, 1).unwrap();
        assert!(rt.is_empty());
    }

    #[derive(Debug)]
    struct InvalidDeadlineTask;

    impl Task<World> for InvalidDeadlineTask {
        type Family = InvalidDeadlineTask;

        fn advance(
            &mut self,
            _id: u64,
            _world: &mut World,
            _requests: &mut Requests<Self>,
            _input: &mut Input<'_>,
            _budget: usize,
        ) -> Result<Advance, SystemCallError> {
            Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            })
        }

        fn refused(&mut self, _world: &mut World, _failure: RequestFailure<Self>) {
            panic!("invalid deadline task does not submit requests")
        }

        fn stop(&mut self, _world: &mut World) {}

        fn deadline(&self) -> Deadline {
            Deadline {
                kind: 99,
                reserved: 0,
                at_ns: 0,
            }
        }
    }

    #[test]
    fn invalid_deadline_is_reported_without_dropping_task() {
        let shared = fake_shared();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
        let mut rt: Runtime<InvalidDeadlineTask, FakeSet> =
            Runtime::new(assembly.bind().unwrap(), 2, 2, &account()).unwrap();
        let mut world = World::default();
        let id = rt.spawn(InvalidDeadlineTask, 0).unwrap();
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        assert_eq!(
            rt.turn(&mut world, 3).unwrap_err(),
            TaskFailure {
                task: id,
                error: SystemCallError::IllegalArgument,
            }
        );
        assert!(rt.has_ready());
        assert!(!rt.is_empty());
    }

    #[derive(Debug)]
    struct StopWaitTask {
        stopping: bool,
    }

    impl Task<World> for StopWaitTask {
        type Family = StopWaitTask;

        fn advance(
            &mut self,
            _id: u64,
            _world: &mut World,
            _requests: &mut Requests<Self>,
            input: &mut Input<'_>,
            _budget: usize,
        ) -> Result<Advance, SystemCallError> {
            if self.stopping && input.take_timeout() {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            })
        }

        fn refused(&mut self, _world: &mut World, _failure: RequestFailure<Self>) {
            panic!("stop wait task does not submit requests")
        }

        fn stop(&mut self, _world: &mut World) {
            self.stopping = true;
        }

        fn deadline(&self) -> Deadline {
            if self.stopping {
                Deadline::at(20)
            } else {
                Deadline::INFINITE
            }
        }
    }

    #[test]
    fn stopping_task_can_park_until_cleanup_deadline() {
        let shared = fake_shared();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
        let mut rt: Runtime<StopWaitTask, FakeSet> =
            Runtime::new(assembly.bind().unwrap(), 2, 2, &account()).unwrap();
        let mut world = World::default();
        rt.spawn(StopWaitTask { stopping: false }, 0).unwrap();
        rt.seal();
        rt.shutdown_turn(&mut world, 1).unwrap();
        assert!(!rt.is_empty());
        shared.now.set(20);
        assembly.run(&mut rt, &mut world, 1).unwrap();
        assert!(rt.is_empty());
    }

    /// 订阅来源、按请求重 arm，见 N 个事件后完成。
    #[derive(Debug)]
    struct SourceTask {
        handle: Handle,
        requested: bool,
        events_seen: usize,
        complete_after: usize,
    }

    impl Task<World> for SourceTask {
        type Family = SourceTask;

        fn advance(
            &mut self,
            _id: u64,
            world: &mut World,
            requests: &mut Requests<Self>,
            input: &mut Input<'_>,
            _budget: usize,
        ) -> Result<Advance, SystemCallError> {
            if !self.requested {
                self.requested = true;
                requests
                    .add_source(
                        self.handle,
                        ObjectSignals::READABLE | ObjectSignals::CLOSED,
                        7,
                    )
                    .expect("source request capacity");
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            while let Some(event) = input.pull() {
                world.events.push((event.kind, event.observed));
                self.events_seen += 1;
            }
            if self.events_seen >= self.complete_after {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            })
        }
        fn refused(&mut self, world: &mut World, failure: RequestFailure<Self>) {
            let error = match failure {
                RequestFailure::Spawn { error, .. }
                | RequestFailure::Source { error, .. }
                | RequestFailure::Wake { error, .. } => error,
            };
            world.refusals.push(error);
            self.events_seen = self.complete_after;
        }
        fn stop(&mut self, _world: &mut World) {}
    }

    fn source_runtime(
        shared: &Rc<FakeShared>,
    ) -> (Runtime<SourceTask, FakeSet>, Assembly<FakeSet>) {
        let assembly = Assembly::new(FakeSet::new(shared), &account())
            .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
        let runtime = Runtime::new(assembly.bind().unwrap(), 4, 8, &account()).unwrap();
        (runtime, assembly)
    }

    #[test]
    fn source_limit_refusal_returns_task_and_refunds_slot() {
        let shared = fake_shared();
        let funding = account();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
        let mut rt = Runtime::new(assembly.bind().unwrap(), 2, 1, &funding).unwrap();
        let baseline_task = funding.usage(ExecutionResource::Task);
        let baseline_input = funding.usage(ExecutionResource::InputBytes);
        let mut world = World::default();
        rt.spawn(
            SourceTask {
                handle: Handle::from_parts(3, 1),
                requested: false,
                events_seen: 0,
                complete_after: 1,
            },
            0,
        )
        .unwrap();
        assembly.run(&mut rt, &mut world, 3).unwrap();
        assert_eq!(world.refusals, [SystemCallError::ReachLimit]);
        assert!(shared.registered().is_empty());
        assert_eq!(rt.pending_sources(), 0);
        assert!(rt.is_empty());
        assert_eq!(funding.usage(ExecutionResource::Task), baseline_task);
        assert_eq!(funding.usage(ExecutionResource::InputBytes), baseline_input);
    }

    #[test]
    fn persistent_source_routes_multiple_prompts() {
        let shared = fake_shared();
        let (mut rt, mut assembly) = source_runtime(&shared);
        let mut world = World::default();
        rt.spawn(
            SourceTask {
                handle: Handle::from_parts(3, 1),
                requested: false,
                events_seen: 0,
                complete_after: 2,
            },
            2,
        )
        .unwrap();
        // 首轮覆盖三个工作面：任务登记来源并停驻。
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 3).unwrap();
        // Gate 下一工作面提交来源登记。
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 3).unwrap();
        let registered = shared.registered();
        assert_eq!(registered.len(), 1);
        assert_ne!(registered[0].2, 0);
        let token = 1;
        // 第一次事件投递后任务请求 rearm；第二次事件后完成并撤销来源。
        shared.fire_at(token, ObjectSignals::READABLE, 0);
        for _ in 0..4 {
            for &record in assembly.receive(64).unwrap() {
                rt.feed(record);
            }
            rt.turn(&mut world, 3).unwrap();
        }
        assert_eq!(world.events.len(), 1);
        assert_eq!(world.events[0].0, 7);
        shared.fire_at(token, ObjectSignals::CLOSED, 0);
        assembly.run(&mut rt, &mut world, 3).unwrap();
        assert_eq!(world.events.len(), 2);
        assert!(rt.is_empty());
        assert!(shared.removes().contains(&token));
    }

    #[test]
    fn completed_task_waits_for_busy_source_retirement() {
        let shared = fake_shared();
        let (mut rt, mut assembly) = source_runtime(&shared);
        let mut world = World::default();
        rt.spawn(
            SourceTask {
                handle: Handle::from_parts(8, 1),
                requested: false,
                events_seen: 0,
                complete_after: 1,
            },
            1,
        )
        .unwrap();
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 3).unwrap();
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 3).unwrap();
        shared.fire_at(1, ObjectSignals::READABLE, 0);
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 3).unwrap();
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 3).unwrap();
        assert_eq!(rt.pending_sources(), 1);
        shared.set_remove_busy(1);
        assembly.run(&mut rt, &mut world, 3).unwrap();
        assert_eq!(rt.pending_sources(), 0, "removes={:?}", shared.removes());
        assert!(rt.is_empty());
        assert!(shared.waits().contains(&Deadline::at(1_000_000)));
    }

    #[test]
    fn stale_cookie_records_are_dropped() {
        let shared = fake_shared();
        let (mut rt, mut assembly) = source_runtime(&shared);
        let mut world = World::default();
        rt.spawn(
            SourceTask {
                handle: Handle::from_parts(4, 1),
                requested: false,
                events_seen: 0,
                complete_after: 1,
            },
            2,
        )
        .unwrap();
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 3).unwrap();
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 3).unwrap();
        shared.fire_at(1, ObjectSignals::READABLE, -1);
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 3).unwrap();
        assert_eq!(world.events.len(), 0);
        assert!(!rt.has_ready());
    }

    /// 父任务一次 advance 连派两个子任务；表限使第二个被拒，经输入报告。
    #[derive(Debug)]
    struct Spawner {
        spawned: bool,
        leaf: bool,
        refused: bool,
    }

    impl Task<World> for Spawner {
        type Family = Spawner;
        fn advance(
            &mut self,
            _id: u64,
            _world: &mut World,
            requests: &mut Requests<Self>,
            _input: &mut Input<'_>,
            _budget: usize,
        ) -> Result<Advance, SystemCallError> {
            if self.leaf || self.refused {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            if !self.spawned {
                self.spawned = true;
                requests
                    .spawn(
                        Spawner {
                            spawned: true,
                            leaf: true,
                            refused: false,
                        },
                        0,
                    )
                    .expect("spawn request capacity");
                requests
                    .spawn(
                        Spawner {
                            spawned: true,
                            leaf: true,
                            refused: false,
                        },
                        0,
                    )
                    .expect("spawn request capacity");
            }
            Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            })
        }
        fn refused(&mut self, world: &mut World, failure: RequestFailure<Self>) {
            let RequestFailure::Spawn { task, error } = failure else {
                panic!("Spawner only submits spawn requests")
            };
            let _ = task;
            world.refusals.push(error);
            self.refused = true;
        }
        fn stop(&mut self, _world: &mut World) {}
    }

    #[test]
    fn gate_spawn_applies_and_refusal_is_reported() {
        // 任务表容量 2：父 + 首个子任务占满，第二个派生被表限拒绝。
        let shared = fake_shared();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
        let mut rt: Runtime<Spawner, FakeSet> =
            Runtime::new(assembly.bind().unwrap(), 2, 8, &account()).unwrap();
        let mut world = World::default();
        rt.spawn(
            Spawner {
                spawned: false,
                leaf: false,
                refused: false,
            },
            0,
        )
        .unwrap();
        assembly.run(&mut rt, &mut world, 3).unwrap();
        assert!(rt.is_empty());
        assert_eq!(world.refusals.len(), 1);
        assert!(matches!(world.refusals[0], SystemCallError::ReachLimit));
    }

    #[test]
    fn max_work_one_advances_gate_one_operation_at_a_time() {
        let shared = fake_shared();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
        let mut rt: Runtime<Spawner, FakeSet> =
            Runtime::new(assembly.bind().unwrap(), 4, 4, &account()).unwrap();
        let mut world = World::default();
        rt.spawn(
            Spawner {
                spawned: false,
                leaf: false,
                refused: false,
            },
            0,
        )
        .unwrap();
        // 期限、输入、任务三个工作面各占一次推进；第四次才结算
        // 第一个 spawn，不能在一次 max_work=1 中批量应用两个请求。
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 1).unwrap();
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 1).unwrap();
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 1).unwrap();
        assert!(rt.has_pending_gate());
        assert_eq!(rt.pending_tasks(), 1);
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 1).unwrap();
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 1).unwrap();
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 1).unwrap();
        assert_eq!(rt.pending_tasks(), 2);
        assert!(rt.has_pending_gate());
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 1).unwrap();
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 1).unwrap();
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 1).unwrap();
        assert_eq!(rt.pending_tasks(), 3);
        assert!(!rt.has_pending_gate());
    }

    #[test]
    fn finalizing_task_is_not_requeued_by_shutdown() {
        let shared = fake_shared();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
        let mut rt: Runtime<ToyTask, FakeSet> =
            Runtime::new(assembly.bind().unwrap(), 2, 2, &account()).unwrap();
        let mut world = World::default();
        rt.spawn(ToyTask { turns: 1 }, 0).unwrap();
        for &record in assembly.receive(64).unwrap() {
            rt.feed(record);
        }
        rt.turn(&mut world, 3).unwrap();
        assert!(!rt.has_ready());
        rt.seal();
        rt.shutdown_turn(&mut world, 1).unwrap();
        assembly.run(&mut rt, &mut world, 1).unwrap();
        assert!(rt.is_empty());
    }

    #[test]
    fn max_work_one_allows_ready_task_and_source_retirement_to_progress() {
        let shared = fake_shared();
        let (mut rt, mut assembly) = source_runtime(&shared);
        let mut world = World::default();
        rt.spawn(
            SourceTask {
                handle: Handle::from_parts(10, 1),
                requested: false,
                events_seen: 0,
                complete_after: 1,
            },
            1,
        )
        .unwrap();
        rt.spawn(
            SourceTask {
                handle: Handle::from_parts(11, 1),
                requested: false,
                events_seen: 0,
                complete_after: 1,
            },
            1,
        )
        .unwrap();
        for _ in 0..20 {
            for &record in assembly.receive(64).unwrap() {
                rt.feed(record);
            }
            rt.turn(&mut world, 1).unwrap();
        }
        shared.fire_at(1, ObjectSignals::READABLE, 0);
        shared.fire_at(2, ObjectSignals::READABLE, 0);
        for _ in 0..100 {
            for &record in assembly.receive(64).unwrap() {
                rt.feed(record);
            }
            rt.turn(&mut world, 1).unwrap();
            if rt.is_empty() {
                break;
            }
        }
        assert_eq!(world.events.len(), 2);
        assert!(rt.is_empty());
        assert_eq!(rt.pending_sources(), 0);
    }

    #[derive(Debug)]
    enum MixedTask {
        Source { registered: bool, stopping: bool },
        Parent,
        Child,
    }

    impl Task<World> for MixedTask {
        type Family = Self;
        fn advance(
            &mut self,
            id: u64,
            world: &mut World,
            requests: &mut Requests<Self>,
            input: &mut Input<'_>,
            _budget: usize,
        ) -> Result<Advance, SystemCallError> {
            let step = match self {
                Self::Source {
                    registered,
                    stopping,
                } => {
                    if *stopping {
                        Step::Complete
                    } else {
                        if !*registered {
                            requests.add_source(
                                Handle::from_parts(77, 1),
                                ObjectSignals::READABLE,
                                77,
                            )?;
                            *registered = true;
                        }
                        while let Some(event) = input.pull() {
                            world.events.push((event.kind, event.observed));
                        }
                        Step::Parked
                    }
                }
                Self::Parent => {
                    requests
                        .spawn(Self::Child, 0)
                        .expect("prepaid request capacity");
                    requests
                        .spawn(Self::Child, 0)
                        .expect("prepaid request capacity");
                    *self = Self::Child;
                    Step::Complete
                }
                Self::Child => {
                    world.order.push(id);
                    Step::Complete
                }
            };
            Ok(Advance { work_done: 1, step })
        }
        fn refused(&mut self, _world: &mut World, _failure: RequestFailure<Self>) {
            panic!("mixed workload must remain within admission limits");
        }
        fn stop(&mut self, _world: &mut World) {
            if let Self::Source { stopping, .. } = self {
                *stopping = true;
            }
        }
    }

    #[test]
    fn persistent_source_gate_retirement_and_stop_refund_with_one_work_unit() {
        let shared = fake_shared();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
        let account = account();
        let mut rt: Runtime<MixedTask, FakeSet> =
            Runtime::new(assembly.bind().unwrap(), 8, 8, &account).unwrap();
        let baseline = account.usage(ExecutionResource::InputBytes).0;
        let mut world = World::default();
        rt.spawn(
            MixedTask::Source {
                registered: false,
                stopping: false,
            },
            1,
        )
        .unwrap();
        rt.spawn(MixedTask::Parent, 0).unwrap();
        for _ in 0..360 {
            let armed = {
                let inner = shared.inner.borrow();
                inner.members.contains_key(&1)
            };
            if armed {
                shared.fire_at(1, ObjectSignals::READABLE, 0);
            }
            for &record in assembly.receive(64).unwrap() {
                rt.feed(record);
            }
            assert_eq!(rt.turn(&mut world, 1).unwrap(), 1);
        }
        assert!(
            world.events.len() > 2,
            "persistent input must continue during retirement"
        );
        assert_eq!(world.order.len(), 2, "both Gate children must execute");
        assert_eq!(
            rt.pending_tasks(),
            1,
            "completed tasks must retire while the source stays alive"
        );
        assert_eq!(account.usage(ExecutionResource::Task).0, 1);
        shared.set_remove_busy(1);
        rt.seal();
        rt.shutdown_turn(&mut world, 1).unwrap();
        assembly.run(&mut rt, &mut world, 1).unwrap();
        assert!(rt.is_empty());
        assert_eq!(rt.pending_sources(), 0);
        assert_eq!(rt.retire_timers.len(), 0);
        assert_eq!(account.usage(ExecutionResource::Task).0, 0);
        assert_eq!(account.usage(ExecutionResource::InputBytes).0, baseline);
        assert!(shared.waits().contains(&Deadline::at(1_000_000)));
        assert!(rt.close().is_ok());
        assert_eq!(account.usage(ExecutionResource::InputBytes).0, 0);
    }

    #[derive(Debug)]
    struct PartialTask {
        armed: bool,
        skip: bool,
        seen: usize,
    }
    impl Task<World> for PartialTask {
        type Family = Self;
        fn advance(
            &mut self,
            _id: u64,
            world: &mut World,
            requests: &mut Requests<Self>,
            input: &mut Input<'_>,
            _budget: usize,
        ) -> Result<Advance, SystemCallError> {
            if !self.armed {
                requests.add_source(Handle::from_raw(11), ObjectSignals::READABLE, 1)?;
                requests.add_source(Handle::from_raw(12), ObjectSignals::READABLE, 2)?;
                self.armed = true;
            } else if self.skip {
                self.skip = false;
            } else {
                while let Some(event) = input.pull() {
                    world.order.push(event.kind);
                    self.seen += 1;
                }
            }
            Ok(Advance {
                work_done: 1,
                step: if self.seen == 2 {
                    Step::Complete
                } else {
                    Step::Parked
                },
            })
        }
        fn refused(&mut self, _: &mut World, _: RequestFailure<Self>) {
            panic!("partial input fixture must fit admission");
        }
        fn stop(&mut self, _: &mut World) {}
    }

    #[test]
    fn unconsumed_input_returns_to_the_front_without_scanning_all_sources() {
        let shared = fake_shared();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
        let mut rt = Runtime::new(assembly.bind().unwrap(), 1, 2, &account()).unwrap();
        let mut world = World::default();
        rt.spawn(
            PartialTask {
                armed: false,
                skip: true,
                seen: 0,
            },
            2,
        )
        .unwrap();
        for _ in 0..30 {
            for &record in assembly.receive(64).unwrap() {
                rt.feed(record);
            }
            rt.turn(&mut world, 1).unwrap();
        }
        shared.fire_at(1, ObjectSignals::READABLE, 0);
        shared.fire_at(2, ObjectSignals::READABLE, 0);
        for _ in 0..160 {
            for &record in assembly.receive(64).unwrap() {
                rt.feed(record);
            }
            rt.turn(&mut world, 1).unwrap();
        }
        assert_eq!(world.order, [1, 2]);
        assert!(rt.is_empty());
        assert!(rt.close().is_ok());
    }

    #[test]
    fn removing_one_source_does_not_erase_another_sources_retry() {
        let shared = fake_shared();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
        let mut rt = Runtime::new(assembly.bind().unwrap(), 1, 2, &account()).unwrap();
        let mut world = World::default();
        let task = rt.spawn(ToyTask { turns: 1 }, 2).unwrap();
        let first = rt
            .add_source(
                task,
                SourcePlan::new(Handle::from_raw(1), ObjectSignals::READABLE),
                1,
            )
            .unwrap();
        let second = rt
            .add_source(
                task,
                SourcePlan::new(Handle::from_raw(2), ObjectSignals::READABLE),
                2,
            )
            .unwrap();
        shared.set_remove_busy(1);
        assert_eq!(
            rt.drop_source(&mut world, first.token()),
            Err(SystemCallError::ObjectBusy)
        );
        assert!(rt.drop_source(&mut world, second.token()).is_ok());
        assert_eq!(rt.next_deadline(), Deadline::at(1_000_000));
        assembly.run(&mut rt, &mut world, 1).unwrap();
        assert!(rt.close().is_ok());
        assert!(shared.waits().contains(&Deadline::at(1_000_000)));
    }

    #[derive(Debug)]
    enum FaultTask {
        Failing(bool),
        Healthy,
    }
    impl Task<World> for FaultTask {
        type Family = Self;
        fn advance(
            &mut self,
            id: u64,
            world: &mut World,
            _: &mut Requests<Self>,
            input: &mut Input<'_>,
            _: usize,
        ) -> Result<Advance, SystemCallError> {
            if let Self::Failing(failed) = self {
                if !*failed {
                    *failed = true;
                    return Err(SystemCallError::InternalError);
                }
                assert!(
                    !input.take_timeout(),
                    "execution retry must not fabricate a business timeout"
                );
                assert_eq!(input.now_ns(), 50);
            }
            world.order.push(id);
            Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            })
        }
        fn refused(&mut self, _: &mut World, _: RequestFailure<Self>) {
            panic!("fault fixture does not submit requests");
        }
        fn stop(&mut self, _: &mut World) {}
    }

    #[test]
    fn failed_task_retains_state_while_healthy_task_runs_and_retry_is_not_timeout() {
        let shared = fake_shared();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
        let mut rt = Runtime::new(assembly.bind().unwrap(), 2, 2, &account()).unwrap();
        let mut world = World::default();
        let failed = rt.spawn(FaultTask::Failing(false), 0).unwrap();
        let healthy = rt.spawn(FaultTask::Healthy, 0).unwrap();
        let failure = rt.turn(&mut world, 3).unwrap_err();
        assert_eq!(failure.task, failed);
        rt.defer_failed_task(failed, Some(50)).unwrap();
        assembly.run(&mut rt, &mut world, 1).unwrap();
        assert_eq!(world.order, [healthy, failed]);
        assert!(rt.close().is_ok());
    }

    #[derive(Debug)]
    struct DeadlineGateTask {
        phase: u8,
    }
    impl Task<World> for DeadlineGateTask {
        type Family = Self;
        fn advance(
            &mut self,
            _: u64,
            _: &mut World,
            requests: &mut Requests<Self>,
            input: &mut Input<'_>,
            _: usize,
        ) -> Result<Advance, SystemCallError> {
            match self.phase {
                0 => {
                    self.phase = 1;
                    Ok(Advance {
                        work_done: 1,
                        step: Step::Parked,
                    })
                }
                1 => {
                    self.phase = 2;
                    requests.add_source(Handle::from_raw(8), ObjectSignals::READABLE, 8)?;
                    Err(SystemCallError::InternalError)
                }
                2 => {
                    assert!(
                        input.take_timeout(),
                        "failure Gate must preserve the business deadline while held"
                    );
                    self.phase = 3;
                    Ok(Advance {
                        work_done: 1,
                        step: Step::Runnable,
                    })
                }
                _ => {
                    assert!(!input.take_timeout(), "business timeout is delivered once");
                    Ok(Advance {
                        work_done: 1,
                        step: Step::Complete,
                    })
                }
            }
        }
        fn deadline(&self) -> Deadline {
            if self.phase == 3 {
                Deadline::INFINITE
            } else {
                Deadline::at(50)
            }
        }
        fn refused(&mut self, world: &mut World, failure: RequestFailure<Self>) {
            if let RequestFailure::Source { error, .. } = failure {
                world.refusals.push(error);
            }
        }
        fn stop(&mut self, _: &mut World) {}
    }
    #[test]
    fn failed_gate_preserves_business_deadline_across_hold_and_execution_retry() {
        for retry in [60, 10] {
            let shared = fake_shared();
            let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
                .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
            let mut rt = Runtime::new(assembly.bind().unwrap(), 1, 1, &account()).unwrap();
            let mut world = World::default();
            let id = rt.spawn(DeadlineGateTask { phase: 0 }, 1).unwrap();
            for &record in assembly.receive(64).unwrap() {
                rt.feed(record);
            }
            rt.turn(&mut world, 3).unwrap();
            rt.wake(id).unwrap();
            assert_eq!(rt.turn(&mut world, 3).unwrap_err().task, id);
            rt.defer_failed_task(id, None).unwrap();
            for _ in 0..12 {
                for &record in assembly.receive(64).unwrap() {
                    rt.feed(record);
                }
                rt.turn(&mut world, 1).unwrap();
            }
            assert_eq!(rt.next_deadline(), Deadline::at(50));
            assert_eq!(world.refusals, [SystemCallError::InternalError]);
            assert!(shared.registered().is_empty());
            rt.defer_failed_task(id, Some(retry)).unwrap();
            if retry < 50 {
                shared.now.set(100);
                for &record in assembly.receive(64).unwrap() {
                    rt.feed(record);
                }
                rt.turn(&mut world, 1).unwrap();
                assert!(rt.has_ready(), "execution retry is due first");
                assert!(
                    !rt.queue.slot(id).unwrap().timed_out,
                    "business timer has not been popped yet"
                );
            }
            assembly.run(&mut rt, &mut world, 1).unwrap();
            assert!(rt.close().is_ok());
        }
    }

    #[derive(Default)]
    struct SourceOwnershipWorld {
        source: Option<SourceId>,
        denied: usize,
        removed: usize,
    }
    #[derive(Debug)]
    struct SourceOwnershipTask {
        owner: bool,
        started: bool,
        done: bool,
        mode: u8,
    }
    impl Task<SourceOwnershipWorld> for SourceOwnershipTask {
        type Family = Self;
        fn advance(
            &mut self,
            _: u64,
            world: &mut SourceOwnershipWorld,
            requests: &mut Requests<Self>,
            _: &mut Input<'_>,
            _: usize,
        ) -> Result<Advance, SystemCallError> {
            if self.done {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                });
            }
            if self.owner {
                if !self.started {
                    requests.add_source(Handle::from_raw(9), ObjectSignals::READABLE, 9)?;
                    self.started = true;
                }
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            if let Some(source) = world.source {
                self.done = true;

                requests.remove(source)?;
                if self.mode == 2 {
                    return Err(SystemCallError::InternalError);
                }
                return Ok(Advance {
                    work_done: 1,
                    step: if self.mode == 1 {
                        Step::Complete
                    } else {
                        Step::Parked
                    },
                });
            }
            Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            })
        }
        fn registered(
            &mut self,
            world: &mut SourceOwnershipWorld,
            _: SourceKind,
            source: SourceId,
        ) {
            world.source = Some(source);
        }
        fn unregistered(&mut self, world: &mut SourceOwnershipWorld, _: SourceKind, _: SourceId) {
            world.removed += 1;
        }
        fn refused(&mut self, world: &mut SourceOwnershipWorld, failure: RequestFailure<Self>) {
            let RequestFailure::Source { error, .. } = failure else {
                panic!("fixture does not spawn")
            };
            assert_eq!(error, SystemCallError::ObjectNotFound);
            world.denied += 1;
        }
        fn stop(&mut self, _: &mut SourceOwnershipWorld) {
            self.done = true;
        }
    }
    #[test]
    fn foreign_source_is_rejected_in_normal_complete_and_failed_gates() {
        for mode in 0..3 {
            let shared = fake_shared();
            let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
                .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
            let mut rt = Runtime::new(assembly.bind().unwrap(), 2, 2, &account()).unwrap();
            let mut world = SourceOwnershipWorld::default();
            rt.spawn(
                SourceOwnershipTask {
                    owner: true,
                    started: false,
                    done: false,
                    mode,
                },
                1,
            )
            .unwrap();
            rt.spawn(
                SourceOwnershipTask {
                    owner: false,
                    started: false,
                    done: false,
                    mode,
                },
                0,
            )
            .unwrap();
            for _ in 0..80 {
                let _ = rt.turn(&mut world, 1);
            }
            assert_eq!(world.denied, 1);
            assert_eq!(world.removed, 0);
            assert_eq!(rt.pending_sources(), 1);
            assert!(shared.removes().is_empty());
            rt.seal();
            rt.shutdown_turn(&mut world, 1).unwrap();
            assembly.run(&mut rt, &mut world, 1).unwrap();
            assert_eq!(world.removed, 1);
            assert!(rt.close().is_ok());
        }
    }

    #[derive(Default)]
    struct MultiSourceWorld {
        registered: usize,
        unregistered: usize,
        owner_closed: bool,
    }

    struct MultiSourceTask {
        phase: u8,
        sources: [Option<SourceId>; 4],
    }

    impl Task<MultiSourceWorld> for MultiSourceTask {
        type Family = Self;

        fn advance(
            &mut self,
            _: u64,
            world: &mut MultiSourceWorld,
            requests: &mut Requests<Self>,
            _: &mut Input<'_>,
            _: usize,
        ) -> Result<Advance, SystemCallError> {
            match self.phase {
                0 => {
                    for kind in 1..=4 {
                        requests.add_source(
                            Handle::from_raw(20 + kind),
                            ObjectSignals::READABLE,
                            kind,
                        )?;
                    }
                    self.phase = 1;
                }
                1 if world.registered == 4 => {
                    for source in self.sources.into_iter().flatten() {
                        requests.remove(source)?;
                    }
                    self.phase = 2;
                }
                2 if world.unregistered == 4 => {
                    world.owner_closed = true;
                    self.phase = 3;
                }
                _ => {}
            }
            Ok(Advance {
                work_done: 1,
                step: if self.phase == 3 {
                    Step::Complete
                } else if self.phase == 2 {
                    Step::Parked
                } else {
                    Step::Runnable
                },
            })
        }

        fn refused(&mut self, _: &mut MultiSourceWorld, failure: RequestFailure<Self>) {
            match failure {
                RequestFailure::Source { kind, error } => {
                    panic!("four-source registration {kind} failed: {error:?}")
                }
                RequestFailure::Spawn { error, .. } => {
                    panic!("four-source task spawn failed: {error:?}")
                }
                _ => panic!("four-source admission returned unexpected failure"),
            }
        }

        fn registered(&mut self, world: &mut MultiSourceWorld, kind: SourceKind, source: SourceId) {
            assert!(self.sources[(kind - 1) as usize].replace(source).is_none());
            world.registered += 1;
        }

        fn unregistered(
            &mut self,
            world: &mut MultiSourceWorld,
            kind: SourceKind,
            source: SourceId,
        ) {
            assert_eq!(self.sources[(kind - 1) as usize].take(), Some(source));
            assert!(
                !world.owner_closed,
                "owner closed before source removal callback"
            );
            world.unregistered += 1;
        }

        fn stop(&mut self, _: &mut MultiSourceWorld) {}
    }

    #[test]
    fn four_sources_retire_before_their_owner_closes() {
        let shared = fake_shared();
        let input_bytes = Runtime::<MultiSourceTask, FakeSet>::input_budget(4).unwrap();
        let limits = [8, input_bytes];
        let budget = Budget::new(&limits, 1).unwrap();
        let account = budget.account(&limits).unwrap();
        let view = account
            .view(&[budget.slot(0).unwrap(), budget.slot(1).unwrap()])
            .unwrap();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &self::account())
            .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
        let mut rt = Runtime::new(assembly.bind().unwrap(), 1, 4, &view).unwrap();
        let mut world = MultiSourceWorld::default();
        rt.spawn(
            MultiSourceTask {
                phase: 0,
                sources: [None; 4],
            },
            4,
        )
        .unwrap_or_else(|_| panic!("four-source task admission failed"));
        for _ in 0..80 {
            if world.owner_closed && rt.is_empty() {
                break;
            }
            for &record in assembly.receive(64).unwrap() {
                rt.feed(record);
            }
            rt.turn(&mut world, 1).unwrap();
        }
        assert_eq!(world.registered, 4);
        assert_eq!(world.unregistered, 4);
        assert!(world.owner_closed);
        assert_eq!(shared.removes().len(), 4);
        assert_eq!(rt.pending_sources(), 0);
        assert!(rt.close().is_ok());
    }

    #[test]
    fn seal_rejects_spawn_and_stop_next_advances() {
        let shared = fake_shared();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &self::account())
            .unwrap_or_else(|(_, error)| panic!("assembly failed: {error:?}"));
        let mut rt: Runtime<ToyTask, FakeSet> =
            Runtime::new(assembly.bind().unwrap(), 4, 8, &self::account()).unwrap();
        let mut world = World::default();
        rt.spawn(ToyTask { turns: 100 }, 0).unwrap();
        rt.seal();
        assert!(matches!(
            rt.spawn(ToyTask { turns: 1 }, 0),
            Err(InsertFailure {
                error: SystemCallError::ObjectClosed,
                ..
            })
        ));
        rt.shutdown_turn(&mut world, 1).unwrap();
        // stop 意图使 ToyTask 下一次 advance Complete。
        assembly.run(&mut rt, &mut world, 1).unwrap();
        assert!(rt.is_empty());
    }
    #[test]
    fn one_receive_routes_two_runtimes_without_nested_sources() {
        let shared = fake_shared();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|_| panic!("assembly failed"));
        let mut first = Runtime::new(assembly.bind().unwrap(), 2, 2, &account()).unwrap();
        let mut second = Runtime::new(assembly.bind().unwrap(), 2, 2, &account()).unwrap();
        let mut a = World::default();
        let mut b = World::default();
        for rt in [&mut first, &mut second] {
            rt.spawn(
                SourceTask {
                    handle: Handle::from_parts(3, 1),
                    requested: false,
                    events_seen: 0,
                    complete_after: 1,
                },
                1,
            )
            .unwrap();
        }
        for _ in 0..6 {
            first.turn(&mut a, 3).unwrap();
            second.turn(&mut b, 3).unwrap();
        }
        assert_eq!(shared.registered().len(), 2);
        shared.fire_at(1, ObjectSignals::READABLE, 0);
        shared.fire_at(2, ObjectSignals::CLOSED, 0);
        let records = assembly.receive(64).unwrap();
        assert_eq!(records.len(), 2);
        assert!(first.feed(records[0]));
        assert!(!second.feed(records[0]));
        assert!(!first.feed(records[1]));
        assert!(second.feed(records[1]));
        for _ in 0..12 {
            first.turn(&mut a, 3).unwrap();
            second.turn(&mut b, 3).unwrap();
        }
        assert_eq!(a.events, [(7, ObjectSignals::READABLE)]);
        assert_eq!(b.events, [(7, ObjectSignals::CLOSED)]);
        assert!(first.is_empty() && second.is_empty());
        assert!(first.close().is_ok());
        assert!(second.close().is_ok());
        assert!(assembly.close().is_ok());
    }

    #[test]
    fn repeated_level_hints_merge_in_one_prepaid_input() {
        let shared = fake_shared();
        let (mut rt, mut assembly) = source_runtime(&shared);
        let mut world = World::default();
        rt.spawn(
            SourceTask {
                handle: Handle::from_parts(3, 1),
                requested: false,
                events_seen: 0,
                complete_after: 1,
            },
            1,
        )
        .unwrap();
        for _ in 0..6 {
            rt.turn(&mut world, 3).unwrap();
        }
        shared.fire_at(1, ObjectSignals::READABLE, 0);
        shared.fire_at(1, ObjectSignals::CLOSED, 0);
        for &record in assembly.receive(64).unwrap() {
            assert!(rt.feed(record));
        }
        for _ in 0..12 {
            rt.turn(&mut world, 3).unwrap();
        }
        assert_eq!(
            world.events,
            [(7, ObjectSignals::READABLE | ObjectSignals::CLOSED)]
        );
        assert!(rt.close().is_ok());
        assert!(assembly.close().is_ok());
    }

    #[test]
    fn abandoning_live_runtime_invalidates_shared_assembly() {
        let shared = fake_shared();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|_| panic!("assembly failed"));
        let mut rt = Runtime::new(assembly.bind().unwrap(), 2, 2, &account()).unwrap();
        let healthy: Runtime<SourceTask, FakeSet> =
            Runtime::new(assembly.bind().unwrap(), 2, 2, &account()).unwrap();
        let mut world = World::default();
        rt.spawn(
            SourceTask {
                handle: Handle::from_parts(3, 1),
                requested: false,
                events_seen: 0,
                complete_after: 1,
            },
            1,
        )
        .unwrap();
        for _ in 0..6 {
            rt.turn(&mut world, 3).unwrap();
        }
        drop(rt);
        assert!(assembly.is_failed());
        assert_eq!(
            assembly.receive(64).unwrap_err(),
            SystemCallError::ObjectClosed
        );
        assert_eq!(shared.removes(), [1]);
        assert!(healthy.close().is_ok());
        assert!(assembly.close().is_ok());
    }
    #[derive(Debug)]
    struct CancelDeadlineTask {
        phase: u8,
        callback_cancel: bool,
    }
    impl Task<World> for CancelDeadlineTask {
        type Family = Self;
        fn advance(
            &mut self,
            _: u64,
            _: &mut World,
            requests: &mut Requests<Self>,
            input: &mut Input<'_>,
            _: usize,
        ) -> Result<Advance, SystemCallError> {
            assert!(
                !input.take_timeout(),
                "cancelled deadline produced a timeout"
            );
            match self.phase {
                0 => {
                    self.phase = 1;
                    Ok(Advance {
                        work_done: 1,
                        step: Step::Parked,
                    })
                }
                1 => {
                    self.phase = 2;
                    requests.add_source(Handle::from_raw(8), ObjectSignals::READABLE, 8)?;
                    requests.add_source(Handle::from_raw(9), ObjectSignals::READABLE, 9)?;
                    Ok(Advance {
                        work_done: 1,
                        step: Step::Runnable,
                    })
                }
                _ => Ok(Advance {
                    work_done: 1,
                    step: Step::Complete,
                }),
            }
        }
        fn deadline(&self) -> Deadline {
            if self.phase < 2 || (self.callback_cancel && self.phase == 2) {
                Deadline::at(50)
            } else {
                Deadline::INFINITE
            }
        }
        fn registered(&mut self, _: &mut World, _: SourceKind, _: SourceId) {
            if self.callback_cancel {
                self.phase = 3;
            }
        }
        fn refused(&mut self, _: &mut World, _: RequestFailure<Self>) {
            panic!("unexpected refusal");
        }
        fn stop(&mut self, _: &mut World) {}
    }
    #[test]
    fn successful_gate_and_callbacks_cancel_deadline_before_later_clock_sample() {
        for callback_cancel in [false, true] {
            let shared = fake_shared();
            let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
                .unwrap_or_else(|_| panic!("assembly failed"));
            let mut rt = Runtime::new(assembly.bind().unwrap(), 1, 2, &account()).unwrap();
            let mut world = World::default();
            let id = rt
                .spawn(
                    CancelDeadlineTask {
                        phase: 0,
                        callback_cancel,
                    },
                    2,
                )
                .unwrap();
            rt.turn(&mut world, 3).unwrap();
            shared.now.set(20);
            rt.wake(id).unwrap();
            while !rt.has_pending_gate() {
                rt.turn(&mut world, 1).unwrap();
            }
            if callback_cancel {
                while rt.get_task_mut(id).unwrap().phase < 3 {
                    rt.turn(&mut world, 1).unwrap();
                }
                assert!(rt.has_pending_gate());
            }
            shared.now.set(60);
            assembly.run(&mut rt, &mut world, 1).unwrap();
            assert!(rt.close().is_ok());
            assert!(assembly.close().is_ok());
        }
    }

    #[derive(Debug)]
    struct CallbackDeadlineTask {
        registration: crate::SourceRegistration,
        deadline: Deadline,
        cancel_to: Deadline,
    }

    impl Task<Vec<bool>> for CallbackDeadlineTask {
        type Family = Self;

        fn advance(
            &mut self,
            _: u64,
            timeouts: &mut Vec<bool>,
            requests: &mut Requests<Self>,
            input: &mut Input<'_>,
            _: usize,
        ) -> Result<Advance, SystemCallError> {
            timeouts.push(input.take_timeout());
            if self.deadline == Deadline::INFINITE {
                self.registration.remove(requests)?;
            } else {
                self.registration.install(
                    requests,
                    SourcePlan::new(Handle::from_raw(8), ObjectSignals::READABLE),
                    8,
                )?;
            }
            Ok(Advance {
                work_done: 1,
                step: if self.deadline == Deadline::INFINITE && self.registration.is_empty() {
                    Step::Complete
                } else {
                    Step::Parked
                },
            })
        }

        fn registered(&mut self, _: &mut Vec<bool>, kind: SourceKind, source: SourceId) {
            self.registration.registered(kind, source);
        }

        fn unregistered(&mut self, _: &mut Vec<bool>, _: SourceKind, source: SourceId) {
            assert!(self.registration.unregistered(source));
            self.deadline = self.cancel_to;
        }

        fn stop(&mut self, _: &mut Vec<bool>) {
            self.deadline = self.cancel_to;
            self.registration.cancel();
        }

        fn deadline(&self) -> Deadline {
            self.deadline
        }

        fn refused(&mut self, _: &mut Vec<bool>, _: RequestFailure<Self>) {
            panic!("unexpected callback deadline refusal");
        }
    }

    #[test]
    fn non_gate_callbacks_cancel_only_unmatured_deadlines() {
        for stop in [false, true] {
            for cancel_at in [20, 3_000_000] {
                let shared = fake_shared();
                let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
                    .unwrap_or_else(|_| panic!("assembly failed"));
                let mut rt = Runtime::new(assembly.bind().unwrap(), 1, 1, &account()).unwrap();
                let mut timeouts = Vec::new();
                let id = rt
                    .spawn(
                        CallbackDeadlineTask {
                            registration: crate::SourceRegistration::new(),
                            deadline: Deadline::at(2_000_000),
                            cancel_to: Deadline::INFINITE,
                        },
                        1,
                    )
                    .unwrap();
                while rt.pending_sources() == 0 || rt.has_pending_gate() {
                    rt.turn(&mut timeouts, 1).unwrap();
                }
                shared.now.set(cancel_at);
                if stop {
                    rt.seal();
                    rt.shutdown_turn(&mut timeouts, 1).unwrap();
                } else {
                    let source = rt.get_task_mut(id).unwrap().registration.source().unwrap();
                    shared.set_remove_busy(1);
                    assert_eq!(
                        rt.drop_source(&mut timeouts, source.token()),
                        Err(SystemCallError::ObjectBusy)
                    );
                    shared.now.set(cancel_at + SOURCE_RETIRE_RETRY_NS);
                    while rt.pending_sources() != 0 {
                        rt.turn(&mut timeouts, 1).unwrap();
                    }
                }
                shared.now.set(5_000_000);
                assembly.run(&mut rt, &mut timeouts, 1).unwrap();
                assert_eq!(
                    timeouts.iter().filter(|&&timeout| timeout).count(),
                    usize::from(cancel_at >= 2_000_000),
                    "only a deadline matured before its cancellation may be delivered"
                );
                assert!(rt.close().is_ok());
                assert!(assembly.close().is_ok());
            }
        }
    }

    #[test]
    fn invalid_retirement_callback_deadline_preserves_and_identifies_the_task() {
        let shared = fake_shared();
        let mut assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|_| panic!("assembly failed"));
        let mut rt = Runtime::new(assembly.bind().unwrap(), 1, 1, &account()).unwrap();
        let mut timeouts = Vec::new();
        let id = rt
            .spawn(
                CallbackDeadlineTask {
                    registration: crate::SourceRegistration::new(),
                    deadline: Deadline::at(2_000_000),
                    cancel_to: Deadline {
                        kind: 99,
                        reserved: 0,
                        at_ns: 0,
                    },
                },
                1,
            )
            .unwrap();
        while rt.pending_sources() == 0 || rt.has_pending_gate() {
            rt.turn(&mut timeouts, 1).unwrap();
        }
        let source = rt.get_task_mut(id).unwrap().registration.source().unwrap();
        shared.set_remove_busy(1);
        assert_eq!(
            rt.drop_source(&mut timeouts, source.token()),
            Err(SystemCallError::ObjectBusy)
        );
        shared.now.set(SOURCE_RETIRE_RETRY_NS);
        let failure = loop {
            if let Err(failure) = rt.turn(&mut timeouts, 1) {
                break failure;
            }
        };
        assert_eq!(failure.task, id);
        assert_eq!(failure.error, SystemCallError::IllegalArgument);
        assert_eq!(rt.pending_sources(), 0);
        assert_eq!(rt.pending_tasks(), 1);
        let task = rt.get_task_mut(id).unwrap();
        assert!(task.registration.is_empty());
        task.deadline = Deadline::INFINITE;
        assembly.run(&mut rt, &mut timeouts, 1).unwrap();
        assert!(rt.close().is_ok());
        assert!(assembly.close().is_ok());
    }

    #[test]
    fn older_task_retires_under_continuous_new_completion() {
        let shared = fake_shared();
        let assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|_| panic!("assembly failed"));
        let mut rt = Runtime::new(assembly.bind().unwrap(), 4, 2, &account()).unwrap();
        let old = rt.spawn(ToyTask { turns: 1 }, 0).unwrap();
        let later = rt.spawn(ToyTask { turns: 1 }, 0).unwrap();
        rt.queue.begin_finalizing(later);
        rt.queue.finalize_one();
        rt.queue.begin_finalizing(old);
        let mut newcomers = Vec::new();
        for _ in 0..4 {
            let id = rt.spawn(ToyTask { turns: 1 }, 0).unwrap();
            newcomers.push(id);
            rt.queue.begin_finalizing(id);
            rt.queue.finalize_one();
            if rt.queue.slot(old).is_none() {
                break;
            }
        }
        assert!(
            rt.queue.slot(old).is_none(),
            "new completions starved the older owner"
        );
        while !rt.is_empty() {
            rt.queue.finalize_one();
        }
        assert!(rt.close().is_ok());
        assert!(assembly.close().is_ok());
    }

    #[test]
    fn always_ready_receive_keeps_task_quantum() {
        let shared = fake_shared();
        let (mut rt, mut assembly) = source_runtime(&shared);
        let mut world = World::default();
        let id = rt
            .spawn(
                SourceTask {
                    handle: Handle::from_parts(3, 1),
                    requested: false,
                    events_seen: 0,
                    complete_after: 100,
                },
                1,
            )
            .unwrap();
        for _ in 0..6 {
            rt.turn(&mut world, 3).unwrap();
        }
        shared.inner.borrow_mut().persistent = true;
        for _ in 0..12 {
            for &record in assembly.receive(1).unwrap() {
                rt.feed(record);
            }
            rt.turn(&mut world, 1).unwrap();
        }
        assert!(
            !world.events.is_empty(),
            "persistent Receive starved task advancement"
        );
        rt.queue.begin_finalizing(id);
        for _ in 0..20 {
            rt.turn(&mut world, 1).unwrap();
        }
        assert!(rt.close().is_ok());
        assert!(assembly.close().is_ok());
    }

    #[test]
    fn failed_assembly_rejects_admission_and_stop_callbacks() {
        let shared = fake_shared();
        let assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|_| panic!("assembly failed"));
        let mut rt = Runtime::new(assembly.bind().unwrap(), 2, 2, &account()).unwrap();
        let id = rt.spawn(ToyTask { turns: 1 }, 0).unwrap();
        drop(assembly);
        let failure = rt.spawn(ToyTask { turns: 9 }, 0).unwrap_err();
        assert_eq!(failure.error, SystemCallError::ObjectClosed);
        assert_eq!(failure.task.turns, 9);
        rt.seal();
        let mut world = World::default();
        assert_eq!(
            rt.shutdown_turn(&mut world, 1).unwrap_err().error,
            SystemCallError::ObjectClosed
        );
        assert_eq!(rt.get_task_mut(id).unwrap().turns, 1);
        assert_eq!(
            rt.shutdown_turn(&mut world, 2).unwrap_err().error,
            SystemCallError::ObjectClosed
        );
        assert_eq!(rt.get_task_mut(id).unwrap().turns, 1);
        assert!(world.order.is_empty(), "failed assembly ran business stop");
    }

    #[test]
    fn registration_cancelled_before_install_receipt_is_retired_before_task_drop() {
        let shared = fake_shared();
        let assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|_| panic!("assembly failed"));
        let mut rt = Runtime::new(assembly.bind().unwrap(), 1, 1, &account()).unwrap();
        let mut timeouts = Vec::new();
        let id = rt
            .spawn(
                CallbackDeadlineTask {
                    registration: crate::SourceRegistration::new(),
                    deadline: Deadline::at(2_000_000),
                    cancel_to: Deadline::INFINITE,
                },
                1,
            )
            .unwrap();
        while !rt.has_pending_gate() {
            rt.turn(&mut timeouts, 1).unwrap();
        }
        assert_eq!(rt.pending_sources(), 0);
        rt.seal();
        rt.shutdown_turn(&mut timeouts, 1).unwrap();
        assert!(rt.get_task_mut(id).unwrap().registration.is_installing());
        while rt.pending_tasks() != 0 {
            rt.shutdown_turn(&mut timeouts, 1).unwrap();
        }
        assert_eq!(rt.pending_sources(), 0);
        assert!(rt.close().is_ok());
        assert!(assembly.close().is_ok());
    }

    #[test]
    fn old_retirement_receipt_cannot_clear_a_reinstalled_source() {
        let shared = fake_shared();
        let assembly = Assembly::new(FakeSet::new(&shared), &account())
            .unwrap_or_else(|_| panic!("assembly failed"));
        let mut rt = Runtime::new(assembly.bind().unwrap(), 1, 1, &account()).unwrap();
        let mut timeouts = Vec::new();
        let id = rt
            .spawn(
                CallbackDeadlineTask {
                    registration: crate::SourceRegistration::new(),
                    deadline: Deadline::at(2_000_000),
                    cancel_to: Deadline::at(2_000_000),
                },
                1,
            )
            .unwrap();
        while rt.pending_sources() == 0 || rt.has_pending_gate() {
            rt.turn(&mut timeouts, 1).unwrap();
        }
        let old = rt.get_task_mut(id).unwrap().registration.source().unwrap();
        rt.drop_source(&mut timeouts, old.token()).unwrap();
        while rt.pending_sources() == 0 || rt.has_pending_gate() {
            rt.turn(&mut timeouts, 1).unwrap();
        }
        let current = rt.get_task_mut(id).unwrap().registration.source().unwrap();
        assert_ne!(old, current);
        assert!(!rt.get_task_mut(id).unwrap().registration.unregistered(old));
        assert_eq!(
            rt.get_task_mut(id).unwrap().registration.source(),
            Some(current)
        );
        rt.get_task_mut(id).unwrap().cancel_to = Deadline::INFINITE;
        rt.seal();
        while rt.pending_tasks() != 0 {
            rt.shutdown_turn(&mut timeouts, 1).unwrap();
        }
        assert_eq!(rt.pending_sources(), 0);
        assert!(rt.close().is_ok());
        assert!(assembly.close().is_ok());
    }
}
