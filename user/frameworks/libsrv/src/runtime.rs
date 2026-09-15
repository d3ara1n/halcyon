//! 单 actor 执行运行体：独占观察集合，拥有来源登记寿命、迟到事件过滤、
//! 期限唤醒与公平任务推进。任务经 [`Requests`] 声明运行体请求，
//! advance 返回后统一应用；业务不手工编排 token、rearm 或事件队列。

use crate::budget::{Account, Charge, CoreResource, ExecutionSlots, Taxonomy};
use crate::work_queue::{InsertFailure, WorkQueue};
use alloc::{sync::Arc, vec::Vec};
use core::sync::atomic::{AtomicUsize, Ordering};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals},
    time::Deadline,
    wait::WaitItem,
    wait_set::{RECEIVE_MAX, ReadyRecord},
};
use ordered_table::OrderedTable;
use timer_queue::{TimerQueue, TimerToken};

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
pub struct SourceId(u64);

impl SourceId {
    pub fn token(self) -> u64 {
        self.0
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

/// 运行体依赖的观察与时间环境；真实实现是内核 WaitSet，host 测试用替身。
pub trait SourceOps {
    fn register_item(&self, item: WaitItem) -> Result<u64, SystemCallError>;
    fn rearm(&self, token: u64) -> Result<u64, SystemCallError>;
    fn remove_source(&self, token: u64) -> Result<(), SystemCallError>;
    fn receive_into(&self, records: &mut [ReadyRecord]) -> Result<usize, SystemCallError>;
    fn wait(&self, deadline: Deadline) -> Result<(), SystemCallError>;
    fn now_ns(&self) -> Result<u64, SystemCallError>;
}

/// 可整体关闭的观察集合；真实实现即内核 WaitSet 的普通关闭。
pub trait SourceSet: SourceOps + Sized {
    fn close_set(self) -> Result<(), (Self, SystemCallError)>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceEvent {
    pub source: SourceId,
    pub kind: SourceKind,
    pub observed: ObjectSignals,
    pub error: u32,
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
}

/// 任务在 advance 内声明的运行体请求；advance 返回后按序应用。
pub struct Requests<T> {
    operations: Vec<RequestOperation<T>>,
}

const REQUEST_CAPACITY: usize = 16;

enum RequestOperation<T> {
    Spawn { task: T, max_sources: usize },
    Source(SourceRequest),
}

struct PendingGate {
    task: u64,
    step: Step,
    has_pending: bool,
    refused: bool,
    rejection: Option<SystemCallError>,
}

enum SourceRequest {
    /// 登记意图统一为值计划，cookie 由运行体注入任务身份。
    Register { plan: SourcePlan, kind: SourceKind },
    /// 重新武装一个仍需观察的来源；不声明即保持解除。
    Rearm { source: SourceId },
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
        self.arm_source(SourcePlan::new(handle, signals), kind)
            .map_err(|_| SystemCallError::ReachLimit)
    }

    pub fn arm_source(&mut self, plan: SourcePlan, kind: SourceKind) -> Result<(), SourcePlan> {
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

    pub fn rearm(&mut self, source: SourceId) -> Result<(), SystemCallError> {
        if self.operations.len() == REQUEST_CAPACITY {
            return Err(SystemCallError::ReachLimit);
        }
        self.operations
            .push(RequestOperation::Source(SourceRequest::Rearm { source }));
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

    fn pop_front(&mut self) -> Option<RequestOperation<T>> {
        (!self.operations.is_empty()).then(|| self.operations.remove(0))
    }
}

pub struct SourceEntry<K: Taxonomy> {
    task: u64,
    kind: SourceKind,
    generation: u64,
    armed: bool,
    removing: bool,
    input_index: usize,
    input_queued: bool,
    input_previous: u64,
    input_next: u64,
    retire_timer: TimerToken,
    pending: Option<SourceEvent>,
    _charge: Charge<K>,
}

static RETIRED_CLEANUP_FAILURES: AtomicUsize = AtomicUsize::new(0);

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

pub struct Runtime<T, S: SourceOps, K: Taxonomy = CoreResource> {
    queue: WorkQueue<T, K>,
    sources: OrderedTable<SourceEntry<K>>,
    set: Option<S>,
    account: Arc<Account<K>>,
    records: Vec<ReadyRecord>,
    scratch: Vec<SourceEvent>,
    phase: u8,
    retire_cursor: Option<u64>,
    maintenance_phase: u8,
    retire_phase: bool,
    retire_timers: TimerQueue<u64>,
    clock_now: u64,
    source_ready: bool,
    pending_gate: Option<PendingGate>,
    requests: Requests<T>,
    slots: ExecutionSlots,
    _input_charge: Charge<K>,
}

impl<T, S: SourceOps, K: Taxonomy> Runtime<T, S, K> {
    /// 接收、scratch 与唯一请求缓冲的预付存储。
    fn fixed_input_bytes(source_limit: usize) -> Result<usize, SystemCallError> {
        RECEIVE_MAX
            .checked_mul(core::mem::size_of::<ReadyRecord>())
            .and_then(|bytes| {
                source_limit
                    .checked_mul(core::mem::size_of::<SourceEvent>())
                    .and_then(|scratch| bytes.checked_add(scratch))
            })
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
                    .checked_mul(
                        core::mem::size_of::<SourceEntry<K>>()
                            + core::mem::size_of::<ReadyRecord>(),
                    )
                    .ok_or(SystemCallError::ReachLimit)?,
            )
            .ok_or(SystemCallError::ReachLimit)
    }

    pub fn new(
        set: S,
        task_limit: usize,
        source_limit: usize,
        slots: ExecutionSlots,
        account: &Arc<Account<K>>,
    ) -> Result<Self, SystemCallError> {
        if task_limit == 0 || source_limit == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let input_charge =
            account.acquire_at(slots.input_bytes, Self::fixed_input_bytes(source_limit)?)?;
        let requests = Requests::new()?;
        let mut records = Vec::new();
        records
            .try_reserve_exact(RECEIVE_MAX)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        records.resize(
            RECEIVE_MAX,
            ReadyRecord {
                token: 0,
                arm_generation: 0,
                cookie: 0,
                observed: ObjectSignals::NONE,
                reason: 0,
                error: 0,
            },
        );
        let mut scratch = Vec::new();
        scratch
            .try_reserve_exact(source_limit)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(Self {
            set: Some(set),
            queue: WorkQueue::new(task_limit)?,
            sources: OrderedTable::new(source_limit),
            account: account.clone(),
            records,
            scratch,
            phase: 0,
            retire_cursor: None,
            maintenance_phase: 0,
            retire_phase: false,
            retire_timers: TimerQueue::new(0),
            clock_now: 0,
            source_ready: false,
            pending_gate: None,
            requests,
            slots,
            _input_charge: input_charge,
        })
    }

    pub fn spawn(&mut self, task: T, max_sources: usize) -> Result<u64, InsertFailure<T>> {
        self.queue
            .insert(task, max_sources, &self.account, self.slots.task)
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
                        .as_ref()
                        .expect("runtime set already closed")
                        .now_ns()
                        .map_err(|error| TaskFailure { task: 0, error })?;
                    self.clock_now = now;
                    self.queue.expire(now, budget)
                }
                1 => self.receive(world, budget)?,
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
        } else if self.queue.has_ready()
            || self.pending_gate.is_some()
            || self.source_ready
            || self.maintenance_ready()
        {
            DriveState::Runnable
        } else {
            DriveState::Waiting(self.next_deadline())
        }
    }

    /// 外层组合等待获得集合就绪后调用；事件仍由本运行体独占接收。
    pub fn notified(&mut self) {
        self.source_ready = true;
    }

    /// 错误任务保留原输入、来源和额度；其他任务继续推进。
    pub fn defer_failed_task(
        &mut self,
        id: u64,
        retry_at: Option<u64>,
    ) -> Result<(), SystemCallError> {
        self.queue.defer(id, retry_at)
    }

    /// 同步门面仍使用与外层组合驱动相同的状态与绝对等待。
    pub fn run<W>(&mut self, world: &mut W, max_work: usize) -> Result<(), TaskFailure>
    where
        T: Task<W, Family = T>,
    {
        loop {
            if self.drive_state() == DriveState::Drained {
                return Ok(());
            }
            self.turn(world, max_work)?;
            match self.drive_state() {
                DriveState::Drained => return Ok(()),
                DriveState::Runnable => {}
                DriveState::Waiting(deadline) => {
                    self.set
                        .as_ref()
                        .expect("runtime set already closed")
                        .wait(deadline)
                        .map_err(|error| TaskFailure { task: 0, error })?;
                    self.notified();
                }
            }
        }
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
        if max_work == 0 {
            return Err(TaskFailure {
                task: 0,
                error: SystemCallError::IllegalArgument,
            });
        }
        let stopped = usize::from(
            self.queue
                .stop_next(world)
                .map_err(|error| TaskFailure { task: 0, error })?,
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
        reason = "关闭失败返还仍拥有任务的运行时，错误路径不分配"
    )]
    pub fn close(mut self) -> Result<(), (Self, SystemCallError)>
    where
        S: SourceSet,
    {
        if self.pending_gate.is_some() || !self.queue.is_empty() || !self.sources.is_empty() {
            return Err((self, SystemCallError::ObjectBusy));
        }
        let set = self.set.take().expect("runtime set already closed");
        match set.close_set() {
            Ok(()) => Ok(()),
            Err((set, error)) => {
                self.set = Some(set);
                Err((self, error))
            }
        }
    }

    fn receive<W>(&mut self, world: &mut W, budget: usize) -> Result<usize, TaskFailure>
    where
        T: Task<W, Family = T>,
    {
        let phase = self.maintenance_phase;
        self.maintenance_phase = (self.maintenance_phase + 1) % 3;
        if phase == 1 {
            return Ok(self.retry_removals(world, budget));
        }
        if phase == 2 {
            return Ok(usize::from(self.queue.retire_one()));
        }
        let capacity = budget.min(RECEIVE_MAX);
        let count = match self
            .set
            .as_ref()
            .expect("runtime set already closed")
            .receive_into(&mut self.records[..capacity])
        {
            Ok(count) => count,
            Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => 0,
            Err(error) => return Err(TaskFailure { task: 0, error }),
        };
        self.source_ready = count == capacity;
        let mut used = 0;
        for index in 0..count {
            let record = self.records[index];
            self.route(record);
            // 丢弃旧代次同样消费一个接收工作单位。
            used += 1;
        }
        Ok(used)
    }

    /// 按令牌路由就绪记录：迟到代次丢弃，当前代次写入预付槽并唤醒任务。
    fn route(&mut self, record: ReadyRecord) -> bool {
        let Some(entry) = self.sources.get_mut(record.token) else {
            return false;
        };
        if entry.removing {
            return false;
        }
        if entry.generation != record.arm_generation {
            return false;
        }
        if entry.pending.is_some() {
            // 不变量防御：预付槽未消费时不应收到当前代次记录。保持解除，
            // 消费后的重新武装会原子重观察电平，不覆盖未消费事实。
            return false;
        }
        entry.pending = Some(SourceEvent {
            source: SourceId(record.token),
            kind: entry.kind,
            observed: record.observed,
            error: record.error,
        });
        entry.armed = false;
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
        self.sources
            .get_mut(token)
            .expect("queued source remains owned")
            .pending
            .take()
    }

    /// 新退休来源的发现与已登记退休公平轮转；每次探查也计费。
    fn retry_removals<W>(&mut self, world: &mut W, budget: usize) -> usize
    where
        T: Task<W, Family = T>,
    {
        let mut used = 0;
        while used < budget {
            self.retire_phase = !self.retire_phase;
            if self.retire_phase && self.queue.has_unmarked_sources() {
                let next = self
                    .sources
                    .next_after(self.retire_cursor.as_ref())
                    .or_else(|| self.sources.next_after(None))
                    .map(|(&token, entry)| (token, entry.task, entry.removing));
                if let Some((token, task, removing)) = next {
                    self.retire_cursor = Some(token);
                    if !removing && self.queue.is_finalizing(task) {
                        self.begin_remove(token);
                    }
                }
                used += 1;
                continue;
            }
            let Some((_, at, &token)) = self.retire_timers.peek() else {
                break;
            };
            if at > self.clock_now {
                break;
            }
            let _ = self.drop_source(world, token);
            used += 1;
        }
        used
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
        entry.armed = false;
        let task = entry.task;
        let index = entry.input_index;
        let timer = entry.retire_timer;
        if let Some(moved) = self.queue.detach_source(task, index) {
            self.sources
                .get_mut(moved)
                .expect("moved source must remain owned")
                .input_index = index;
        }
        assert!(
            self.retire_timers.reschedule(timer, self.clock_now),
            "source retirement timer must exist"
        );
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
        let result = self
            .set
            .as_ref()
            .expect("runtime set already closed")
            .remove_source(token);
        match result {
            Ok(()) | Err(SystemCallError::ObjectNotFound) => {
                let entry = self
                    .sources
                    .remove(token)
                    .expect("retiring source must remain owned");
                self.retire_timers.cancel(entry.retire_timer);
                self.queue.source_retired(entry.task);
                self.queue
                    .get_task_mut(entry.task)
                    .expect("source task must remain owned")
                    .unregistered(world, entry.kind, SourceId(token));
                self.queue
                    .schedule(entry.task)
                    .expect("source task must remain owned");
                Ok(())
            }
            Err(error) => {
                let timer = self
                    .sources
                    .get(token)
                    .expect("failed source must remain owned")
                    .retire_timer;
                assert!(
                    self.retire_timers
                        .reschedule(timer, self.clock_now.saturating_add(1_000_000)),
                    "source retirement timer must exist"
                );
                RETIRED_CLEANUP_FAILURES.fetch_add(1, Ordering::Relaxed);
                Err(error)
            }
        }
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
            .as_ref()
            .expect("runtime set already closed")
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
                entry.pending = Some(event);
                self.enqueue_input(event.source.token(), true);
            }
        }
        if let Some(slot) = self.queue.slot(id) {
            slot.timed_out |= timed_out;
        }
        let has_pending = self.queue.slot(id).is_some_and(|slot| slot.input_head != 0) || timed_out;
        let deadline = self.task_deadline::<W>(id);
        let mut advance = match result {
            Ok(advance) if advance.work_done <= budget => advance,
            result => {
                let error = result.err().unwrap_or(SystemCallError::InternalError);
                // 即使进入失败 Gate/Hold，也保留任务当前的业务期限。
                let _ = self.queue.update_deadline(id, deadline);
                if self.requests.operations.is_empty() {
                    self.queue
                        .schedule(id)
                        .expect("failed task must remain owned");
                } else {
                    self.pending_gate = Some(PendingGate {
                        task: id,
                        step: Step::Runnable,
                        has_pending,
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
                has_pending,
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
            SourceRequest::Rearm { source } | SourceRequest::Remove { source } => source,
            _ => return false,
        };
        let Some(entry) = self.sources.get(source.token()) else {
            return false;
        };
        if entry.task == task {
            return false;
        }
        let kind = entry.kind;
        self.queue
            .get_task_mut(task)
            .expect("requesting task remains owned")
            .refused(
                world,
                RequestFailure::Source {
                    kind,
                    error: SystemCallError::ObjectNotFound,
                },
            );
        true
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
            RequestOperation::Source(request) => {
                if self.reject_foreign_source(task, world, &request) {
                    return;
                }
                let (kind, notify) = match request {
                    SourceRequest::Register { kind, .. } => (kind, true),
                    SourceRequest::Rearm { source } => (
                        self.sources
                            .get(source.token())
                            .map_or(0, |entry| entry.kind),
                        true,
                    ),
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
            .as_ref()
            .expect("runtime set remains owned")
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
        let has_pending = gate.has_pending
            || self
                .queue
                .slot(gate.task)
                .is_some_and(|slot| slot.timed_out);
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
            } => {
                match self
                    .queue
                    .insert(spawned, max_sources, &self.account, self.slots.task)
                {
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
                        SourceRequest::Rearm { source } => {
                            let kind = self
                                .sources
                                .get(source.token())
                                .map_or(0, |entry| entry.kind);
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
                    SourceRequest::Rearm { source } => {
                        let kind = self
                            .sources
                            .get(source.token())
                            .map_or(0, |entry| entry.kind);
                        match self.rearm_source(source.token()) {
                            Ok(()) => false,
                            Err(error) => {
                                self.queue
                                    .get_task_mut(task)
                                    .expect("requesting task disappeared before Gate refusal")
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
        generation: u64,
    ) -> Result<ordered_table::PreparedEntry<SourceEntry<K>>, SystemCallError> {
        let slot = self
            .queue
            .slot(task)
            .ok_or(SystemCallError::ObjectNotFound)?;
        if slot.sources.len() >= slot.max_sources {
            return Err(SystemCallError::ReachLimit);
        }
        let input_index = slot.sources.len();
        let charge = self.account.acquire_at(
            self.slots.input_bytes,
            core::mem::size_of::<SourceEntry<K>>() + core::mem::size_of::<ReadyRecord>(),
        )?;
        let retire_timer = self
            .retire_timers
            .try_register(0, 0)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        assert!(
            self.retire_timers.park(retire_timer),
            "new source timer must exist"
        );
        let entry = SourceEntry {
            task,
            kind,
            generation,
            armed: true,
            removing: false,
            input_index,
            input_queued: false,
            input_previous: 0,
            input_next: 0,
            retire_timer,
            pending: None,
            _charge: charge,
        };
        self.sources.prepare_insert(0, entry).map_err(|failure| {
            let (entry, error) = match failure {
                ordered_table::InsertError::Limit(entry) => (entry, SystemCallError::ReachLimit),
                ordered_table::InsertError::Allocation(entry) => {
                    (entry, SystemCallError::OutOfMemory)
                }
            };
            self.retire_timers.cancel(entry.retire_timer);
            error
        })
    }

    fn cancel_prepared_source(&mut self, prepared: ordered_table::PreparedEntry<SourceEntry<K>>) {
        self.retire_timers
            .cancel(prepared.into_value().retire_timer);
    }

    fn add_source(
        &mut self,
        task: u64,
        plan: SourcePlan,
        kind: SourceKind,
    ) -> Result<SourceId, SystemCallError> {
        if !self.queue.accepts_sources(task) {
            return Err(SystemCallError::ObjectClosed);
        }
        let prepared = self.prepare_source(task, kind, 1)?;
        match self
            .set
            .as_ref()
            .expect("runtime set already closed")
            .register_item(WaitItem::new(plan.handle, plan.signals, task))
        {
            Ok(token) => self.adopt_entry(task, token, prepared),
            Err(error) => {
                self.cancel_prepared_source(prepared);
                Err(error)
            }
        }
    }

    fn adopt_entry(
        &mut self,
        task: u64,
        token: u64,
        mut prepared: ordered_table::PreparedEntry<SourceEntry<K>>,
    ) -> Result<SourceId, SystemCallError> {
        *self
            .retire_timers
            .value_mut(prepared.value_mut().retire_timer)
            .expect("prepared source timer must exist") = token;
        self.sources.insert_prepared(prepared.with_key(token));
        self.queue
            .slot(task)
            .expect("prepared source task disappeared")
            .sources
            .push(token);
        Ok(SourceId(token))
    }

    fn rearm_source(&mut self, token: u64) -> Result<(), SystemCallError> {
        let entry = self
            .sources
            .get(token)
            .ok_or(SystemCallError::ObjectNotFound)?;
        if entry.armed || entry.removing || entry.pending.is_some() {
            return Ok(());
        }
        match self
            .set
            .as_ref()
            .expect("runtime set already closed")
            .rearm(token)
        {
            Ok(generation) => {
                let entry = self
                    .sources
                    .get_mut(token)
                    .expect("rearm entry disappeared");
                entry.generation = generation;
                entry.armed = true;
                Ok(())
            }
            Err(error) => {
                // 武装失败作为来源错误事件投递，任务决定退休或降级。
                let entry = self
                    .sources
                    .get_mut(token)
                    .expect("rearm entry disappeared");
                let task = entry.task;
                if entry.pending.is_none() {
                    let source = SourceId(token);
                    let kind = entry.kind;
                    entry.pending = Some(SourceEvent {
                        source,
                        kind,
                        observed: ObjectSignals::NONE,
                        error: error as u32,
                    });
                }
                self.enqueue_input(token, false);
                self.queue
                    .schedule(task)
                    .expect("rearm-failure task must remain owned");
                Ok(())
            }
        }
    }
}

impl<T, S: SourceOps, K: Taxonomy> Drop for Runtime<T, S, K> {
    fn drop(&mut self) {
        // 非空放弃即缺陷信号：任务 owner 各自 Drop 有界收束，来源额度随表
        // 析构释放；内核登记由集合 Drop 的一次关闭兜底（进程边界接管）。
    }
}

#[cfg(target_arch = "riscv64")]
impl<T, K: Taxonomy> Runtime<T, rinlib::ipc::wait_set::WaitSet, K> {
    /// 仅借出集合观察目标，不转移接收、登记或关闭权。
    pub fn wait_item(&self, cookie: u64) -> WaitItem {
        WaitItem::new(
            self.set
                .as_ref()
                .expect("runtime set already closed")
                .handle(),
            ObjectSignals::READABLE | ObjectSignals::CLOSED,
            cookie,
        )
    }
}

#[cfg(target_arch = "riscv64")]
impl SourceOps for rinlib::ipc::wait_set::WaitSet {
    fn register_item(&self, item: WaitItem) -> Result<u64, SystemCallError> {
        rinlib::ipc::wait_set::WaitSet::register(self, item)
    }
    fn rearm(&self, token: u64) -> Result<u64, SystemCallError> {
        rinlib::ipc::wait_set::WaitSet::rearm(self, token)
    }
    fn remove_source(&self, token: u64) -> Result<(), SystemCallError> {
        rinlib::ipc::wait_set::WaitSet::remove(self, token)
    }
    fn receive_into(&self, records: &mut [ReadyRecord]) -> Result<usize, SystemCallError> {
        rinlib::ipc::wait_set::WaitSet::receive_into(self, records)
    }
    fn wait(&self, deadline: Deadline) -> Result<(), SystemCallError> {
        rinlib::ipc::wait_set::WaitSet::wait(self, deadline).map(|_| ())
    }
    fn now_ns(&self) -> Result<u64, SystemCallError> {
        rinlib::time::snapshot().map(|snapshot| snapshot.now_ns)
    }
}

#[cfg(target_arch = "riscv64")]
impl SourceSet for rinlib::ipc::wait_set::WaitSet {
    fn close_set(self) -> Result<(), (Self, SystemCallError)> {
        rinlib::ipc::wait_set::WaitSet::close(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{collections::BTreeMap, collections::VecDeque, rc::Rc, vec};
    use core::cell::{Cell, RefCell};

    struct FakeShared {
        inner: RefCell<FakeInner>,
        now: Cell<u64>,
    }

    struct FakeInner {
        next_token: u64,
        generations: BTreeMap<u64, u64>,
        consumed: BTreeMap<u64, bool>,
        queued: BTreeMap<u64, bool>,
        delivered: VecDeque<ReadyRecord>,
        remove_busy: usize,
        waits: Vec<Deadline>,
        rearms: Vec<u64>,
        removes: Vec<u64>,
        registered: Vec<(Handle, ObjectSignals, u64)>,
    }

    impl FakeShared {
        fn fire_at(&self, token: u64, observed: ObjectSignals, shift: i64) {
            let mut inner = self.inner.borrow_mut();
            let generation = *inner.generations.get(&token).expect("fired unknown token");
            let generation = (generation as i64 + shift).max(0) as u64;
            inner.delivered.push_back(ReadyRecord {
                token,
                arm_generation: generation,
                cookie: 0,
                observed,
                reason: 1,
                error: 0,
            });
            if shift == 0 {
                inner.queued.insert(token, true);
            }
        }

        fn generation(&self, token: u64) -> u64 {
            *self
                .inner
                .borrow()
                .generations
                .get(&token)
                .expect("unknown token")
        }

        fn rearms(&self) -> Vec<u64> {
            self.inner.borrow().rearms.clone()
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

    /// 观察集合替身：脚本化就绪记录、可控时钟与调用记录。
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
                generations: BTreeMap::new(),
                consumed: BTreeMap::new(),
                queued: BTreeMap::new(),
                delivered: VecDeque::new(),
                remove_busy: 0,
                waits: Vec::new(),
                rearms: Vec::new(),
                removes: Vec::new(),
                registered: Vec::new(),
            }),
            now: Cell::new(0),
        })
    }

    impl SourceOps for FakeSet {
        fn register_item(&self, item: WaitItem) -> Result<u64, SystemCallError> {
            let mut inner = self.shared.inner.borrow_mut();
            let token = inner.next_token;
            inner.next_token += 1;
            inner.generations.insert(token, 1);
            inner.consumed.insert(token, false);
            inner.queued.insert(token, false);
            inner
                .registered
                .push((item.handle, item.signals, item.cookie));
            Ok(token)
        }
        fn rearm(&self, token: u64) -> Result<u64, SystemCallError> {
            let mut inner = self.shared.inner.borrow_mut();
            if inner.queued.get(&token).copied().unwrap_or(false)
                || !inner.consumed.get(&token).copied().unwrap_or(false)
            {
                return Err(SystemCallError::ObjectBusy);
            }
            let generation = inner
                .generations
                .get_mut(&token)
                .ok_or(SystemCallError::ObjectNotFound)?;
            *generation += 1;
            let current = *generation;
            inner.consumed.insert(token, false);
            inner.queued.insert(token, false);
            inner.rearms.push(token);
            Ok(current)
        }
        fn remove_source(&self, token: u64) -> Result<(), SystemCallError> {
            let mut inner = self.shared.inner.borrow_mut();
            if inner.remove_busy != 0 {
                inner.remove_busy -= 1;
                return Err(SystemCallError::ObjectBusy);
            }
            inner.generations.remove(&token);
            inner.consumed.remove(&token);
            inner.queued.remove(&token);
            inner.removes.push(token);
            Ok(())
        }
        fn receive_into(&self, records: &mut [ReadyRecord]) -> Result<usize, SystemCallError> {
            let mut inner = self.shared.inner.borrow_mut();
            let mut count = 0;
            while count < records.len() {
                let Some(record) = inner.delivered.pop_front() else {
                    break;
                };
                records[count] = record;
                if inner.generations.get(&record.token) == Some(&record.arm_generation) {
                    inner.consumed.insert(record.token, true);
                    inner.queued.insert(record.token, false);
                }
                count += 1;
            }
            Ok(count)
        }
        fn wait(&self, deadline: Deadline) -> Result<(), SystemCallError> {
            self.shared.inner.borrow_mut().waits.push(deadline);
            if let Ok(Some(at)) = deadline.instant() {
                self.shared.now.set(at);
            }
            Ok(())
        }
        fn now_ns(&self) -> Result<u64, SystemCallError> {
            Ok(self.shared.now.get())
        }
    }

    #[test]
    fn fake_waitset_models_consumption_and_busy_removal() {
        let shared = fake_shared();
        let set = FakeSet::new(&shared);
        let token = set
            .register_item(WaitItem::new(
                Handle::from_parts(9, 1),
                ObjectSignals::READABLE,
                0,
            ))
            .unwrap();
        assert_eq!(set.rearm(token), Err(SystemCallError::ObjectBusy));
        let mut records = [ReadyRecord {
            token: 0,
            arm_generation: 0,
            cookie: 0,
            observed: ObjectSignals::NONE,
            reason: 0,
            error: 0,
        }];
        assert_eq!(set.receive_into(&mut records).unwrap(), 0);
        assert_eq!(set.rearm(token), Err(SystemCallError::ObjectBusy));
        shared.fire_at(token, ObjectSignals::READABLE, 0);
        assert_eq!(set.receive_into(&mut records).unwrap(), 1);
        assert_eq!(set.rearm(token).unwrap(), 2);
        shared.set_remove_busy(1);
        assert_eq!(set.remove_source(token), Err(SystemCallError::ObjectBusy));
        assert_eq!(shared.generation(token), 2);
        assert_eq!(set.remove_source(token), Ok(()));
    }

    #[test]
    fn input_preserves_unconsumed_events_and_timeout() {
        let events = [
            SourceEvent {
                source: SourceId(1),
                kind: 1,
                observed: ObjectSignals::READABLE,
                error: 0,
            },
            SourceEvent {
                source: SourceId(2),
                kind: 2,
                observed: ObjectSignals::CLOSED,
                error: 7,
            },
        ];
        let mut input = Input {
            events: &events,
            cursor: 0,
            timed_out: true,
            now_ns: 0,
        };
        assert_eq!(input.pull().unwrap().source, SourceId(1));
        assert!(input.has_pending());
        assert!(input.take_timeout());
        assert_eq!(input.pull().unwrap().source, SourceId(2));
        assert!(!input.has_pending());
    }

    #[derive(Default)]
    struct World {
        order: Vec<u64>,
        events: Vec<(SourceKind, ObjectSignals, u32)>,
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

    use crate::budget::Budget;

    fn account() -> Arc<Account<CoreResource>> {
        Budget::new(&[8, 4096], 1)
            .unwrap()
            .account(&[8, 4096])
            .unwrap()
    }

    #[test]
    fn yielding_tasks_alternate_fairly() {
        let shared = fake_shared();
        let mut rt: Runtime<ToyTask, FakeSet> = Runtime::new(
            FakeSet::new(&shared),
            4,
            8,
            CoreResource::EXECUTION_SLOTS,
            &account(),
        )
        .unwrap();
        let mut world = World::default();
        rt.spawn(ToyTask { turns: 2 }, 0).unwrap();
        rt.spawn(ToyTask { turns: 2 }, 0).unwrap();
        rt.run(&mut world, 1).unwrap();
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
        let mut rt: Runtime<WaitingTask, FakeSet> = Runtime::new(
            FakeSet::new(&shared),
            4,
            8,
            CoreResource::EXECUTION_SLOTS,
            &account(),
        )
        .unwrap();
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
        rt.turn(&mut world, 3).unwrap();
        assert!(!rt.has_ready());
        shared.now.set(20);
        rt.turn(&mut world, 1).unwrap();
        assert!(rt.has_ready());
        rt.run(&mut world, 1).unwrap();
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
        let mut rt: Runtime<InvalidDeadlineTask, FakeSet> = Runtime::new(
            FakeSet::new(&shared),
            2,
            2,
            CoreResource::EXECUTION_SLOTS,
            &account(),
        )
        .unwrap();
        let mut world = World::default();
        let id = rt.spawn(InvalidDeadlineTask, 0).unwrap();
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
        let mut rt: Runtime<StopWaitTask, FakeSet> = Runtime::new(
            FakeSet::new(&shared),
            2,
            2,
            CoreResource::EXECUTION_SLOTS,
            &account(),
        )
        .unwrap();
        let mut world = World::default();
        rt.spawn(StopWaitTask { stopping: false }, 0).unwrap();
        rt.seal();
        rt.shutdown_turn(&mut world, 1).unwrap();
        assert!(!rt.is_empty());
        shared.now.set(20);
        rt.run(&mut world, 1).unwrap();
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
                world.events.push((event.kind, event.observed, event.error));
                self.events_seen += 1;
                if self.events_seen < self.complete_after {
                    requests
                        .rearm(event.source)
                        .expect("source request capacity");
                }
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
                RequestFailure::Spawn { error, .. } | RequestFailure::Source { error, .. } => error,
            };
            world.refusals.push(error);
        }
        fn stop(&mut self, _world: &mut World) {}
    }

    fn source_runtime(shared: &Rc<FakeShared>) -> Runtime<SourceTask, FakeSet> {
        Runtime::new(
            FakeSet::new(shared),
            4,
            8,
            CoreResource::EXECUTION_SLOTS,
            &account(),
        )
        .unwrap()
    }

    #[test]
    fn source_routes_events_and_rearms() {
        let shared = fake_shared();
        let mut rt = source_runtime(&shared);
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
        rt.turn(&mut world, 3).unwrap();
        // Gate 下一工作面提交来源登记。
        rt.turn(&mut world, 3).unwrap();
        let registered = shared.registered();
        assert_eq!(registered.len(), 1);
        assert_ne!(registered[0].2, 0);
        let token = 1;
        // 第一次事件投递后任务请求 rearm；第二次事件后完成并撤销来源。
        shared.fire_at(token, ObjectSignals::READABLE, 0);
        for _ in 0..4 {
            rt.turn(&mut world, 3).unwrap();
        }
        assert_eq!(world.events.len(), 1);
        assert_eq!(world.events[0].0, 7);
        assert!(shared.rearms().contains(&token));
        shared.fire_at(token, ObjectSignals::CLOSED, 0);
        rt.run(&mut world, 3).unwrap();
        assert_eq!(world.events.len(), 2);
        assert!(rt.is_empty());
        assert!(shared.removes().contains(&token));
    }

    #[test]
    fn completed_task_waits_for_busy_source_retirement() {
        let shared = fake_shared();
        let mut rt = source_runtime(&shared);
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
        rt.turn(&mut world, 3).unwrap();
        rt.turn(&mut world, 3).unwrap();
        shared.fire_at(1, ObjectSignals::READABLE, 0);
        rt.turn(&mut world, 3).unwrap();
        rt.turn(&mut world, 3).unwrap();
        assert_eq!(rt.pending_sources(), 1);
        shared.set_remove_busy(1);
        rt.run(&mut world, 3).unwrap();
        assert_eq!(rt.pending_sources(), 0, "removes={:?}", shared.removes());
        assert!(rt.is_empty());
        assert!(shared.waits().contains(&Deadline::at(1_000_000)));
    }

    #[test]
    fn stale_generation_records_are_dropped() {
        let shared = fake_shared();
        let mut rt = source_runtime(&shared);
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
        rt.turn(&mut world, 3).unwrap();
        rt.turn(&mut world, 3).unwrap();
        shared.fire_at(1, ObjectSignals::READABLE, -1);
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
        let mut rt: Runtime<Spawner, FakeSet> = Runtime::new(
            FakeSet::new(&shared),
            2,
            8,
            CoreResource::EXECUTION_SLOTS,
            &account(),
        )
        .unwrap();
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
        rt.run(&mut world, 3).unwrap();
        assert!(rt.is_empty());
        assert_eq!(world.refusals.len(), 1);
        assert!(matches!(world.refusals[0], SystemCallError::ReachLimit));
    }

    #[test]
    fn max_work_one_advances_gate_one_operation_at_a_time() {
        let shared = fake_shared();
        let mut rt: Runtime<Spawner, FakeSet> = Runtime::new(
            FakeSet::new(&shared),
            4,
            4,
            CoreResource::EXECUTION_SLOTS,
            &account(),
        )
        .unwrap();
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
        rt.turn(&mut world, 1).unwrap();
        rt.turn(&mut world, 1).unwrap();
        rt.turn(&mut world, 1).unwrap();
        assert!(rt.has_pending_gate());
        assert_eq!(rt.pending_tasks(), 1);
        rt.turn(&mut world, 1).unwrap();
        rt.turn(&mut world, 1).unwrap();
        rt.turn(&mut world, 1).unwrap();
        assert_eq!(rt.pending_tasks(), 2);
        assert!(rt.has_pending_gate());
        rt.turn(&mut world, 1).unwrap();
        rt.turn(&mut world, 1).unwrap();
        rt.turn(&mut world, 1).unwrap();
        assert_eq!(rt.pending_tasks(), 3);
        assert!(!rt.has_pending_gate());
    }

    #[test]
    fn finalizing_task_is_not_requeued_by_shutdown() {
        let shared = fake_shared();
        let mut rt: Runtime<ToyTask, FakeSet> = Runtime::new(
            FakeSet::new(&shared),
            2,
            2,
            CoreResource::EXECUTION_SLOTS,
            &account(),
        )
        .unwrap();
        let mut world = World::default();
        rt.spawn(ToyTask { turns: 1 }, 0).unwrap();
        rt.turn(&mut world, 3).unwrap();
        assert!(!rt.has_ready());
        rt.seal();
        rt.shutdown_turn(&mut world, 1).unwrap();
        rt.run(&mut world, 1).unwrap();
        assert!(rt.is_empty());
    }

    #[test]
    fn max_work_one_allows_ready_task_and_source_retirement_to_progress() {
        let shared = fake_shared();
        let mut rt: Runtime<SourceTask, FakeSet> = source_runtime(&shared);
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
            rt.turn(&mut world, 1).unwrap();
        }
        shared.fire_at(1, ObjectSignals::READABLE, 0);
        shared.fire_at(2, ObjectSignals::READABLE, 0);
        for _ in 0..100 {
            rt.turn(&mut world, 1).unwrap();
            if rt.is_empty() {
                break;
            }
        }
        assert_eq!(world.events.len(), 2);
        assert!(rt.is_empty());
        assert_eq!(rt.pending_sources(), 0);
    }

    impl SourceSet for FakeSet {
        fn close_set(self) -> Result<(), (Self, SystemCallError)> {
            assert!(self.shared.inner.borrow().generations.is_empty());
            Ok(())
        }
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
                            world.events.push((event.kind, event.observed, event.error));
                            requests.rearm(event.source)?;
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
        let account = account();
        let mut rt: Runtime<MixedTask, FakeSet> = Runtime::new(
            FakeSet::new(&shared),
            8,
            8,
            CoreResource::EXECUTION_SLOTS,
            &account,
        )
        .unwrap();
        let baseline = account.usage(CoreResource::InputBytes).0;
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
                inner.generations.contains_key(&1) && !inner.consumed[&1] && !inner.queued[&1]
            };
            if armed {
                shared.fire_at(1, ObjectSignals::READABLE, 0);
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
        assert_eq!(account.usage(CoreResource::Task).0, 1);
        shared.set_remove_busy(1);
        rt.seal();
        rt.shutdown_turn(&mut world, 1).unwrap();
        rt.run(&mut world, 1).unwrap();
        assert!(rt.is_empty());
        assert_eq!(rt.pending_sources(), 0);
        assert_eq!(rt.retire_timers.len(), 0);
        assert_eq!(account.usage(CoreResource::Task).0, 0);
        assert_eq!(account.usage(CoreResource::InputBytes).0, baseline);
        assert!(shared.waits().contains(&Deadline::at(1_000_000)));
        assert!(rt.close().is_ok());
        assert_eq!(account.usage(CoreResource::InputBytes).0, 0);
    }

    #[test]
    fn receive_consumes_only_returned_records() {
        let shared = fake_shared();
        let set = FakeSet::new(&shared);
        for n in 1..=2 {
            set.register_item(WaitItem::new(
                Handle::from_raw(n),
                ObjectSignals::READABLE,
                n,
            ))
            .unwrap();
            shared.fire_at(n, ObjectSignals::READABLE, 0);
        }
        let mut out = [ReadyRecord {
            token: 0,
            arm_generation: 0,
            cookie: 0,
            observed: ObjectSignals::NONE,
            reason: 0,
            error: 0,
        }];
        assert_eq!(set.receive_into(&mut out).unwrap(), 1);
        assert!(set.rearm(1).is_ok());
        assert_eq!(set.rearm(2), Err(SystemCallError::ObjectBusy));
        assert_eq!(set.receive_into(&mut out).unwrap(), 1);
        assert!(set.rearm(2).is_ok());
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
        let mut rt = Runtime::new(
            FakeSet::new(&shared),
            1,
            2,
            CoreResource::EXECUTION_SLOTS,
            &account(),
        )
        .unwrap();
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
            rt.turn(&mut world, 1).unwrap();
        }
        shared.fire_at(1, ObjectSignals::READABLE, 0);
        shared.fire_at(2, ObjectSignals::READABLE, 0);
        for _ in 0..160 {
            rt.turn(&mut world, 1).unwrap();
        }
        assert_eq!(world.order, [1, 2]);
        assert!(rt.is_empty());
        assert!(rt.close().is_ok());
    }

    #[test]
    fn removing_one_source_does_not_erase_another_sources_retry() {
        let shared = fake_shared();
        let mut rt = Runtime::new(
            FakeSet::new(&shared),
            1,
            2,
            CoreResource::EXECUTION_SLOTS,
            &account(),
        )
        .unwrap();
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
        rt.run(&mut world, 1).unwrap();
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
        let mut rt = Runtime::new(
            FakeSet::new(&shared),
            2,
            2,
            CoreResource::EXECUTION_SLOTS,
            &account(),
        )
        .unwrap();
        let mut world = World::default();
        let failed = rt.spawn(FaultTask::Failing(false), 0).unwrap();
        let healthy = rt.spawn(FaultTask::Healthy, 0).unwrap();
        let failure = rt.turn(&mut world, 3).unwrap_err();
        assert_eq!(failure.task, failed);
        rt.defer_failed_task(failed, Some(50)).unwrap();
        rt.run(&mut world, 1).unwrap();
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
            let mut rt = Runtime::new(
                FakeSet::new(&shared),
                1,
                1,
                CoreResource::EXECUTION_SLOTS,
                &account(),
            )
            .unwrap();
            let mut world = World::default();
            let id = rt.spawn(DeadlineGateTask { phase: 0 }, 1).unwrap();
            rt.turn(&mut world, 3).unwrap();
            rt.wake(id).unwrap();
            assert_eq!(rt.turn(&mut world, 3).unwrap_err().task, id);
            rt.defer_failed_task(id, None).unwrap();
            for _ in 0..12 {
                rt.turn(&mut world, 1).unwrap();
            }
            assert_eq!(rt.next_deadline(), Deadline::at(50));
            assert_eq!(world.refusals, [SystemCallError::InternalError]);
            assert!(shared.registered().is_empty());
            rt.defer_failed_task(id, Some(retry)).unwrap();
            if retry < 50 {
                shared.now.set(100);
                rt.turn(&mut world, 1).unwrap();
                assert!(rt.has_ready(), "execution retry is due first");
                assert!(
                    !rt.queue.slot(id).unwrap().timed_out,
                    "business timer has not been popped yet"
                );
            }
            rt.run(&mut world, 1).unwrap();
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
                requests.rearm(source)?;
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
            let mut rt = Runtime::new(
                FakeSet::new(&shared),
                2,
                2,
                CoreResource::EXECUTION_SLOTS,
                &account(),
            )
            .unwrap();
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
            assert_eq!(world.denied, 2);
            assert_eq!(world.removed, 0);
            assert_eq!(rt.pending_sources(), 1);
            assert!(shared.rearms().is_empty());
            assert!(shared.removes().is_empty());
            rt.seal();
            rt.shutdown_turn(&mut world, 1).unwrap();
            rt.run(&mut world, 1).unwrap();
            assert_eq!(world.removed, 1);
            assert!(rt.close().is_ok());
        }
    }

    #[derive(Debug)]
    struct PlanTask {
        adopted: bool,
    }

    impl Task<World> for PlanTask {
        type Family = PlanTask;

        fn advance(
            &mut self,
            _id: u64,
            _world: &mut World,
            requests: &mut Requests<Self>,
            input: &mut Input<'_>,
            _budget: usize,
        ) -> Result<Advance, SystemCallError> {
            if !self.adopted {
                requests
                    .arm_source(
                        SourcePlan::new(Handle::from_parts(5, 1), ObjectSignals::DATA),
                        9,
                    )
                    .unwrap_or_else(|_| panic!("source request capacity"));
                self.adopted = true;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            if input.pull().is_some() {
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
        fn refused(&mut self, _world: &mut World, failure: RequestFailure<Self>) {
            let error = match failure {
                RequestFailure::Spawn { error, .. } | RequestFailure::Source { error, .. } => error,
            };
            panic!("PlanTask Gate request failed: {error:?}")
        }
        fn stop(&mut self, _world: &mut World) {}
    }

    #[test]
    fn arm_plan_adopts_initial_generation() {
        let shared = fake_shared();
        let mut rt: Runtime<PlanTask, FakeSet> = Runtime::new(
            FakeSet::new(&shared),
            4,
            8,
            CoreResource::EXECUTION_SLOTS,
            &account(),
        )
        .unwrap();
        let mut world = World::default();
        rt.spawn(PlanTask { adopted: false }, 2).unwrap();
        rt.turn(&mut world, 3).unwrap();
        rt.turn(&mut world, 3).unwrap();
        let token = 1;
        // 新登记直接采用 WaitSet 的初始代次，不对未消费记录错误 rearm。
        assert_eq!(shared.generation(token), 1);
        // 旧代次记录被丢弃，当前代次记录正常投递。
        shared.fire_at(token, ObjectSignals::DATA, -1);
        rt.turn(&mut world, 3).unwrap();
        assert!(!rt.is_empty());
        shared.fire_at(token, ObjectSignals::DATA, 0);
        rt.run(&mut world, 3).unwrap();
        assert!(rt.is_empty());
    }

    #[test]
    fn seal_rejects_spawn_and_stop_next_advances() {
        let shared = fake_shared();
        let mut rt: Runtime<ToyTask, FakeSet> = Runtime::new(
            FakeSet::new(&shared),
            4,
            8,
            CoreResource::EXECUTION_SLOTS,
            &account(),
        )
        .unwrap();
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
        rt.run(&mut world, 1).unwrap();
        assert!(rt.is_empty());
    }
}
