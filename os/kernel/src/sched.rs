//! 调度：域—类—执行点三层（notes/impls/task.md「调度」）+ 调度循环 + 期限表。
//!
//! 单一归属不变量：线程任意时刻恰处于「类队列 | 本 hart current | 无容器」，
//! 全部转换经本模块入口（enqueue / pick / wake）在锁内完成。
//! 公平性由 FIFO 队列的结构性质保证，无记账字段（旧内核死因的免疫）。

use core::{
    arch::asm,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use alloc::{boxed::Box, collections::VecDeque, sync::Arc, vec::Vec};

use crate::sync::Spinlock;
use crate::{hart, mm, sbi, trap::{self, Outcome}};
use crate::sbi::DISARM;
use crate::task::Thread;

/// 调度类：一类线程的就绪容器 + 选择策略（可整体替换，见 notes/impls/task.md）。
pub trait SchedClass: Sync {
    fn enqueue(&self, t: Arc<Thread>);
    fn pick(&self) -> Option<Arc<Thread>>;
    fn has_ready(&self) -> bool;
}

/// 公平类：FIFO 轮转 + 固定量子。
enum ReadyEntry {
    Reserved(u64),
    Thread(Arc<Thread>),
}

pub struct FairClass {
    ready: Spinlock<VecDeque<ReadyEntry>>,
}

impl SchedClass for FairClass {
    fn enqueue(&self, t: Arc<Thread>) {
        self.ready.lock().push_back(ReadyEntry::Thread(t));
    }

    fn pick(&self) -> Option<Arc<Thread>> {
        let mut ready = self.ready.lock();
        let count = ready.len();
        for _ in 0..count {
            match ready.pop_front()? {
                ReadyEntry::Thread(thread) => return Some(thread),
                reserved @ ReadyEntry::Reserved(_) => ready.push_back(reserved),
            }
        }
        None
    }

    fn has_ready(&self) -> bool {
        self.ready
            .lock()
            .iter()
            .any(|entry| matches!(entry, ReadyEntry::Thread(_)))
    }
}

/// 调度域：一组 hart 共享的类层次，按序查询、先到先得。
/// 异构 hart（效能核/快核）即多域划分；当前单域单类。
pub struct SchedDomain {
    pub classes: [&'static dyn SchedClass; 1],
}

static FAIR: FairClass = FairClass {
    ready: Spinlock::new(crate::sync::ranks::LEAF, VecDeque::new()),
};
static NEXT_READY_RESERVATION: AtomicU64 = AtomicU64::new(1);

pub struct ReadyReservation(u64);

pub fn reserve_ready() -> Result<ReadyReservation, ()> {
    let token = NEXT_READY_RESERVATION.fetch_add(1, Ordering::Relaxed);
    if token == 0 {
        return Err(());
    }
    let mut ready = FAIR.ready.lock();
    ready.try_reserve(1).map_err(|_| ())?;
    ready.push_back(ReadyEntry::Reserved(token));
    Ok(ReadyReservation(token))
}

pub fn commit_ready(reservation: ReadyReservation, thread: Arc<Thread>) {
    let mut ready = FAIR.ready.lock();
    let entry = ready
        .iter_mut()
        .find(|entry| matches!(entry, ReadyEntry::Reserved(token) if *token == reservation.0))
        .expect("ready reservation disappeared");
    *entry = ReadyEntry::Thread(thread);
    drop(ready);
    wake_idle_one();
}

pub fn rollback_ready(reservation: ReadyReservation) {
    let mut ready = FAIR.ready.lock();
    let index = ready
        .iter()
        .position(|entry| matches!(entry, ReadyEntry::Reserved(token) if *token == reservation.0))
        .expect("ready reservation disappeared");
    ready.remove(index);
}

/// 系统调度域（M3 单域；多域时由 HartKind 划分，执行点持域指针）。
pub static DOMAIN: SchedDomain = SchedDomain { classes: [&FAIR] };

impl SchedDomain {
    pub fn pick(&self) -> Option<Arc<Thread>> {
        self.classes.iter().find_map(|c| c.pick())
    }

    pub fn has_ready(&self) -> bool {
        self.classes.iter().any(|c| c.has_ready())
    }
}

// ---------------------------------------------------------------------------
// 等待模型（notes/impls/call.md「异步调用」）：等待条目 + 代数仲裁 + 发布时序
// ---------------------------------------------------------------------------

/// 期限表强持 WaitContext；到期只竞争其单一 outcome。
struct DeadlineEntry {
    at: u64,
    context: Arc<crate::task::wait::WaitContext>,
}

/// per-hart 期限表：期限主人是登记 hart（唤醒所有权），登记、arm、
/// 到期扫描与 idle 装填都只碰本 hart 表；唯一跨 hart 访问是静默谓词的
/// 只读遍历。锁防跨核并行下与该遍历的交错，本 hart 内无争用。
static HART_TIMERS: [Spinlock<Vec<DeadlineEntry>>; hart::HART_NUM_LIMIT] =
    [const { Spinlock::new(crate::sync::ranks::LEAF, Vec::new()) }; hart::HART_NUM_LIMIT];

/// 本 hart 的期限表（slot 由 formal entry 设置，调用点均在调度循环或
/// 其 Park 发布路径内）。
#[inline]
fn timers() -> &'static Spinlock<Vec<DeadlineEntry>> {
    &HART_TIMERS[hart::current().slot()]
}

/// 等待意图槽取值（HartLocal.park_kind；hart 私有槽无并发）。IPC 两类
/// 的参数是装箱的内核对象指针（park_arg 携带，发布时回收）。
const PARK_NONE: usize = 0;
const PARK_WAIT: usize = 1;

/// dispatcher 侧登记：把等待意图写入本 hart 槽。此刻**不碰任何全局
/// 结构**——发布由调度循环在线程离开执行点之后完成（park_publish），
/// 保证「可被唤醒」严格晚于「无容器」，完成方永远见不到仍在本 hart
/// 执行的线程。
/// 新对象 ABI 的统一等待意图；计划已在 syscall 入口解析 Handle 并保留授权。
pub fn park_request_wait(plan: crate::task::wait::WaitPlan) {
    let me = hart::current();
    me.park_arg.store(
        Box::into_raw(Box::new(plan)) as usize,
        Ordering::Relaxed,
    );
    me.park_kind.store(PARK_WAIT, Ordering::Relaxed);
}

/// 调度循环 Park 分支调用：消费意图槽，向本 hart 期限表发布等待并 arm。
/// 发起 hart 即期限主人（唤醒所有权：立即 arm 自己的 timer）。
fn park_publish(t: &Arc<Thread>) {
    let me = hart::current();
    let kind = me.park_kind.swap(PARK_NONE, Ordering::Relaxed);
    match kind {
        PARK_WAIT => {
            let p = me.park_arg.load(Ordering::Relaxed) as *mut crate::task::wait::WaitPlan;
            // SAFETY: 指针由 park_request_wait 装箱产生，仅此处回收一次。
            let plan = unsafe { *Box::from_raw(p) };
            crate::task::wait::install(t.clone(), plan);
        }
        _ => unreachable!("Park outcome must carry a wait intent"),
    }
}

pub fn deadline_after_ms(ms: u64) -> u64 {
    sbi::read_time().saturating_add(ms.saturating_mul(ticks_per_ms()))
}

pub(crate) fn register_wait_deadline(
    at: u64,
    context: Arc<crate::task::wait::WaitContext>,
) -> Result<(), ()> {
    let mut timers = timers().lock();
    timers.try_reserve(1).map_err(|_| ())?;
    timers.push(DeadlineEntry { at, context });
    drop(timers);
    arm_earliest();
    Ok(())
}
/// 空闲 hart 位图（IPI 门铃的目标集）。
static IDLE_MASK: AtomicU64 = AtomicU64::new(0);

/// 每毫秒 tick 数（init 时按 timebase 换算）。
static TICKS_PER_MS: AtomicUsize = AtomicUsize::new(1);

/// 时间片量子（毫秒）。
const QUANTUM_MS: u64 = 10;

pub fn init(timebase: usize) {
    TICKS_PER_MS.store((timebase / 1000).max(1), Ordering::Relaxed);
}

fn ticks_per_ms() -> u64 {
    TICKS_PER_MS.load(Ordering::Relaxed) as u64
}

/// 每秒 tick 数（bring_up_runtime 的上线超时计算用）。
pub fn ticks_per_sec() -> u64 {
    ticks_per_ms() * 1000
}

/// 把本 hart 定时器设到期限表最早期限（表空则不动）。
fn arm_earliest() {
    let timers = timers().lock();
    if let Some(min) = timers.iter().map(|d| d.at).min() {
        sbi::require(sbi::set_timer(min), "TIME.set_timer");
    }
}

/// 唤醒本 hart 期限表中全部到期等待者（先收集后完成：锁序单向，
/// 期限内不做长工作）。
fn wake_expired() {
    let now = sbi::read_time();
    let due: Vec<DeadlineEntry> = {
        let mut timers = timers().lock();
        let (due, rest): (Vec<_>, Vec<_>) = timers.drain(..).partition(|d| d.at <= now);
        *timers = rest;
        due
    };
    for entry in due {
        entry.context.expire();
    }
}

// ---------------------------------------------------------------------------
// 入口：enqueue（新就绪 / 唤醒）与定时器事件
// ---------------------------------------------------------------------------

/// 线程入队并按门铃唤醒空闲 hart（IPI = 他方请求，见 notes/impls/internals.md）。
/// IDLE_MASK 是 slot 位图；SBI 边界经 registry 展开为 raw hartid 逐个发送，
/// 绝不把内部位图直接解释为 SBI hart mask。
pub fn enqueue(t: Arc<Thread>) {
    FAIR.enqueue(t);
    wake_idle_one();
}

fn wake_idle_one() {
    let mask = IDLE_MASK.load(Ordering::SeqCst);
    if mask != 0 {
        crate::registry::ipi_slots(mask);
    }
}

/// timer trap（量子耗尽或 sleep 到期）：卸载 → 唤醒到期；当前线程由
/// trap 出口 Requeue 轮转。
pub fn on_timer() {
    sbi::require(sbi::set_timer(DISARM), "TIME.set_timer");
    wake_expired();
}

/// 域内是否有就绪线程（SSIP 分支判断是否值得切走）。
pub fn domain_has_ready() -> bool {
    DOMAIN.has_ready()
}

// ---------------------------------------------------------------------------
// 调度循环（每 hart 常驻，见 notes/impls/internals.md「trap 帧与上下文」）
// ---------------------------------------------------------------------------

/// hart 主循环：pick → 进用户态 → Switch 处置 → 循环；空则 idle。
pub fn run() -> ! {
    // 内核现场（sie/SUM/FS 稳态）已由 formal entry 集中建立；
    // 本循环只维护执行点与量子。
    let me = hart::current();
    loop {
        // 归一到内核地址空间（见 mm::normalize_satp）：刚结束的线程
        // root 可能已被回收，调度循环不得依赖它的内核映射。
        mm::normalize_satp();
        let Some(t) = DOMAIN.pick() else {
            idle();
            continue;
        };
        // lifecycle gate：Terminating 线程不进用户态（惰性撤销）。
        if !t.process.lifecycle.enter_running(me.slot()) {
            reap(t);
            continue;
        }
        me.set_context(t.frame_ptr(), t.satp(), Arc::as_ptr(&t), t.uses_fp());
        t.switches.fetch_add(1, Ordering::Relaxed);
        arm_quantum();
        // ProcessWrite 可经另一 hart 的直映射回填刚分配的可执行页。当前未
        // 建代码代次/active-hart 集合，因此每次新 dispatch 在本 hart 执行
        // fence.i，确保首次执行及迁移都不观察帧复用前的旧 I-cache 内容。
        // SAFETY: fence.i 是本 hart 指令流同步，不触碰内存。
        unsafe { asm!("fence.i", options(nostack, preserves_flags)) };
        // SAFETY: 执行点已装好（帧/satp/线程），tp 不变量成立。
        let outcome = unsafe { trap::ret_to_user() };
        me.clear_context();
        // 非-Resume 出口统一先归一内核 satp（含全量 SFENCE.VMA），再处置
        // 线程：active 位图与后续 teardown 不得在目标地址空间上进行。
        mm::normalize_satp();
        let slot = me.slot();
        match outcome {
            Outcome::Requeue => {
                t.process.lifecycle.on_requeue(slot);
                if t.process.lifecycle.is_terminating() {
                    reap(t);
                } else {
                    FAIR.enqueue(t);
                }
            }
            Outcome::Killed => {
                t.process.lifecycle.clear_active(slot);
                reap(t);
            }
            // 已离开执行点，此刻发布等待：完成方可安全触达该线程。
            Outcome::Park => {
                t.process.lifecycle.clear_active(slot);
                park_publish(&t);
            }
            Outcome::Resume => unreachable!("Resume never passes through the scheduling loop"),
        }
    }
}

/// 量子装填：时间片与本 hart 期限表最早期限取近（不睡过期）。
fn arm_quantum() {
    let quantum = sbi::read_time() + QUANTUM_MS * ticks_per_ms();
    let earliest = timers().lock().iter().map(|d| d.at).min();
    sbi::require(
        sbi::set_timer(earliest.unwrap_or(quantum).min(quantum)),
        "TIME.set_timer",
    );
}

/// 回收终止线程：先 drop 线程强引用（REAPABLE 严格晚于最后一个容器
/// 引用释放），再离场确认（全部离场则 REAPABLE 持续发布）。Dead 只由
/// 管理者 ProcessDrain 的 Complete 分支发布（资源屏障语义）。
fn reap(t: Arc<Thread>) {
    let process = t.process.clone();
    let pid = process.pid;
    let switches = t.switches.load(Ordering::Relaxed);
    let now = sbi::read_time();
    let elapsed_ms = (now - t.created_tick) / ticks_per_ms();
    drop(t);
    crate::task::process::confirm_departure(&process);
    log!(
        Task,
        "pid {} reaped: {} switches, lifespan {} ms",
        pid,
        switches,
        elapsed_ms
    );
    drop(process);
}

/// idle：登记空闲位 → 静默检测 → 按期限表 arm（无期限则卸载）→ wfi。
/// 醒来（SIE=0，不 trap）清门铃后回主循环重查待办。
fn idle() {
    let bit = 1u64 << hart::current().slot();
    IDLE_MASK.fetch_or(bit, Ordering::SeqCst);
    // 入队与登记 idle 的交错由双重检查闭合：登记后若已有工作，
    // 说明入队发生在第一次 pick 之后，立即撤销 idle 身份并重试。
    if DOMAIN.has_ready() {
        IDLE_MASK.fetch_and(!bit, Ordering::SeqCst);
        return;
    }
    if is_quiescent() {
        log!(
            Sched,
            "system quiescent (no waker), powering off; {} frame(s) free",
            crate::frame::free_frames()
        );
        sbi::shutdown();
    }

    let earliest = timers().lock().iter().map(|d| d.at).min();
    match earliest {
        Some(at) => sbi::require(sbi::set_timer(at), "TIME.set_timer"),
        None => sbi::require(sbi::set_timer(DISARM), "TIME.set_timer"),
    };
    // SAFETY: wfi 等待局部使能的中断 pending 唤醒。
    unsafe { asm!("wfi", options(nomem, preserves_flags)) };
    sbi::clear_ssip();
    IDLE_MASK.fetch_and(!bit, Ordering::SeqCst);

    if sip_stip_pending() {
        // 期限到达唤醒（回主循环前把到期线程入队）。
        sbi::require(sbi::set_timer(DISARM), "TIME.set_timer");
        wake_expired();
    }
}

/// 本 hart 时钟中断是否 pending（idle 醒来后查询用）。
fn sip_stip_pending() -> bool {
    let sip: usize;
    // SAFETY: 只读 sip。
    unsafe { asm!("csrr {}, sip", out(reg) sip, options(nomem, preserves_flags)) };
    sip & (1 << 5) != 0
}

/// 终端静默：无任何唤醒主人——预期 hart 全部已进 idle（无人能再
/// 产生工作：enqueue 只来自运行中的 hart）、就绪队列空、各 hart 期限
/// 表皆空、无设备中断使能（当前无设备；接入后设备即主人，谓词自然
/// 失效）。新增等待源时本谓词必须同步扩展：每种 Waiting 都要有可
/// 枚举的主人，否则静默误判为停机。IPC 等待者（邮箱/信号）刻意**不**
/// 阻止静默：hart 全 idle 时不存在能投递消息/信号的执行流，等待者
/// 永无主人，停机即正确终态。
fn is_quiescent() -> bool {
    IDLE_MASK.load(Ordering::SeqCst) == crate::registry::active_slot_mask()
        && !DOMAIN.has_ready()
        && HART_TIMERS.iter().all(|t| t.lock().is_empty())
}
