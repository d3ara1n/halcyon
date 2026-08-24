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
use erhino_shared::call::SystemCallError;
use num_traits::ToPrimitive;

use crate::context::UserContext;
use crate::sync::Spinlock;
use crate::{hart, mm, sbi, trap::{self, Outcome}};
use crate::sbi::DISARM;
use crate::task::{table, Thread};

/// 调度类：一类线程的就绪容器 + 选择策略（可整体替换，见 notes/impls/task.md）。
pub trait SchedClass: Sync {
    fn enqueue(&self, t: Arc<Thread>);
    fn pick(&self) -> Option<Arc<Thread>>;
    fn has_ready(&self) -> bool;
}

/// 公平类：FIFO 轮转 + 固定量子。
pub struct FairClass {
    ready: Spinlock<VecDeque<Arc<Thread>>>,
}

impl SchedClass for FairClass {
    fn enqueue(&self, t: Arc<Thread>) {
        self.ready.lock().push_back(t);
    }

    fn pick(&self) -> Option<Arc<Thread>> {
        self.ready.lock().pop_front()
    }

    fn has_ready(&self) -> bool {
        !self.ready.lock().is_empty()
    }
}

/// 调度域：一组 hart 共享的类层次，按序查询、先到先得。
/// 异构 hart（效能核/快核）即多域划分；当前单域单类。
pub struct SchedDomain {
    pub classes: [&'static dyn SchedClass; 1],
}

static FAIR: FairClass = FairClass {
    ready: Spinlock::new(VecDeque::new()),
};

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

/// 等待凭据：等待条目强持有等待中的线程（线程的强引用随容器走：
/// 就绪队列 / 执行点循环栈 / 等待条目，见 task.md「单一归属不变量」），
/// generation 是单次完成与取消仲裁的唯一凭据。
pub(crate) struct Waiter {
    pub(crate) thread: Arc<Thread>,
    pub(crate) generation: u64,
}

/// 期限表条目：到期时间 + 谁在等。当前仅 sleep 一种期限等待；新增
/// 期限类等待时再引入类别字段。
struct DeadlineEntry {
    at: u64,
    waiter: Waiter,
}

static TIMERS: Spinlock<Vec<DeadlineEntry>> = Spinlock::new(Vec::new());

/// 等待意图槽取值（HartLocal.park_kind；hart 私有槽无并发）。IPC 两类
/// 的参数是装箱的内核对象指针（park_arg 携带，发布时回收）。
const PARK_NONE: usize = 0;
const PARK_SLEEP: usize = 1;
const PARK_RECV: usize = 2;
const PARK_SIGNAL: usize = 3;

/// dispatcher 侧登记：把等待意图写入本 hart 槽。此刻**不碰任何全局
/// 结构**——发布由调度循环在线程离开执行点之后完成（park_publish），
/// 保证「可被唤醒」严格晚于「无容器」，完成方永远见不到仍在本 hart
/// 执行的线程。
pub fn park_request_sleep(ms: u64) {
    let me = hart::current();
    me.park_arg.store(ms as usize, Ordering::Relaxed);
    // Kind 最后写：与汇编侧「数据先于标记」同一发布纪律。
    me.park_kind.store(PARK_SLEEP, Ordering::Relaxed);
}

/// Receive 阻塞形态的登记：参数装箱，发布时由 ipc::publish_recv 回收。
pub fn park_request_recv(buf: usize, len: usize) {
    let me = hart::current();
    me.park_arg.store(
        alloc::boxed::Box::into_raw(alloc::boxed::Box::new((buf, len))) as usize,
        Ordering::Relaxed,
    );
    me.park_kind.store(PARK_RECV, Ordering::Relaxed);
}

/// SignalWait 阻塞形态的登记：目标表装箱，发布时由 ipc::publish_signal_wait 回收。
pub fn park_request_signal(targets: *mut Vec<(crate::task::ipc::SigTarget, u64)>) {
    let me = hart::current();
    me.park_arg.store(targets as usize, Ordering::Relaxed);
    me.park_kind.store(PARK_SIGNAL, Ordering::Relaxed);
}

/// 调度循环 Park 分支调用：消费意图槽，向全局期限表发布等待并 arm。
/// 发起 hart 即期限主人（唤醒所有权：立即 arm 自己的 timer）。
fn park_publish(t: &Arc<Thread>) {
    let me = hart::current();
    let kind = me.park_kind.swap(PARK_NONE, Ordering::Relaxed);
    match kind {
        PARK_SLEEP => {
            let ms = me.park_arg.load(Ordering::Relaxed) as u64;
            let generation = t.wait_gen.fetch_add(1, Ordering::AcqRel) + 1;
            let at = sbi::read_time().saturating_add(ms.saturating_mul(ticks_per_ms()));
            TIMERS.lock().push(DeadlineEntry {
                at,
                waiter: Waiter { thread: t.clone(), generation },
            });
            arm_earliest();
        }
        PARK_RECV => {
            let p = me.park_arg.load(Ordering::Relaxed) as *mut (usize, usize);
            // SAFETY: 指针由 park_request_recv 装箱产生，仅此处回收一次。
            let (buf, len) = unsafe { *Box::from_raw(p) };
            crate::task::ipc::publish_recv(t, buf, len);
        }
        PARK_SIGNAL => {
            let p = me.park_arg.load(Ordering::Relaxed)
                as *mut Vec<(crate::task::ipc::SigTarget, u64)>;
            // SAFETY: 指针由 syscall 分发路径装箱产生，仅此处回收一次。
            let targets = unsafe { *Box::from_raw(p) };
            crate::task::ipc::publish_signal_wait(t, targets);
        }
        _ => unreachable!("Park outcome must carry a wait intent"),
    }
}

/// 完成一次等待：代数仲裁（CAS 消费，单次完成）→ 写结果 → 入队。
/// 线程已死（upgrade 失败）或已被取消/完成（gen 不匹配）则惰性丢弃。
fn complete(entry: DeadlineEntry) {
    fulfill(entry.waiter.thread, entry.waiter.generation, |f| {
        f.x[10] = 0; // NoError
        f.x[11] = 0;
        f.sepc += 4;
    });
}

/// 完成一次 IPC/期限等待：代数仲裁（CAS 消费，单次完成）→ 回调写结果
/// → 入队。返回 false 表示条目已死（gen 不匹配，已被取消或在别处完成），
/// 条目连同其 Arc 一并丢弃——若这是最后一份强引用，线程随之消亡。
pub(crate) fn fulfill(thread: Arc<Thread>, generation: u64, f: impl FnOnce(&mut UserContext)) -> bool {
    if thread
        .wait_gen
        .compare_exchange(generation, generation + 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    // SAFETY: gen 已消费 ⇒ 该等待恰被本次完成；线程处于 Waiting
    //（无容器、无 hart 执行——发布时序保证），独占写帧安全。
    let frame = unsafe { &mut *thread.frame_ptr() };
    f(frame);
    enqueue(thread);
    true
}

/// 以错误码完成一次等待（[`fulfill`] 的错误帧便捷形式）。
pub(crate) fn respond_error_to(thread: Arc<Thread>, generation: u64, err: SystemCallError) -> bool {
    let code = err.to_usize().unwrap_or(1) as u64;
    fulfill(thread, generation, |f| {
        f.x[10] = code;
        f.sepc += 4;
    })
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

/// 把定时器设到期限表最早期限（表空则不动）。
fn arm_earliest() {
    let timers = TIMERS.lock();
    if let Some(min) = timers.iter().map(|d| d.at).min() {
        sbi::require(sbi::set_timer(min), "TIME.set_timer");
    }
}

/// 唤醒全部到期等待者（先收集后完成：锁序单向，期限内不做长工作）。
fn wake_expired() {
    let now = sbi::read_time();
    let due: Vec<DeadlineEntry> = {
        let mut timers = TIMERS.lock();
        let (due, rest): (Vec<_>, Vec<_>) = timers.drain(..).partition(|d| d.at <= now);
        *timers = rest;
        due
    };
    for entry in due {
        complete(entry);
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

/// 记录退出码（Exit / 异常终止共用；回收时随统计打印）。
pub fn report_exit(t: &Thread, code: i64) {
    // 死亡即刻归一到内核地址空间（mm::normalize_satp）：此刻进程 root
    // 仍完整，静态读取安全；此后 teardown/调度不得再依赖该 root——
    // AddressSpace::drop 会剥离内核顶层项，滞留其下任何 TLB miss 即 fatal。
    mm::normalize_satp();
    *t.exit_code.lock() = Some(code);
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
        me.set_context(t.frame_ptr(), t.satp(), Arc::as_ptr(&t), t.uses_fp());
        t.switches.fetch_add(1, Ordering::Relaxed);
        arm_quantum();
        // SAFETY: 执行点已装好（帧/satp/线程），tp 不变量成立。
        let outcome = unsafe { trap::ret_to_user() };
        me.clear_context();
        match outcome {
            Outcome::Requeue => FAIR.enqueue(t),
            Outcome::Killed => reap(t),
            // 已离开执行点，此刻发布等待：完成方可安全触达该线程。
            Outcome::Park => park_publish(&t),
            Outcome::Resume => unreachable!("Resume never passes through the scheduling loop"),
        }
    }
}

/// 量子装填：时间片与期限表最早期限取近（不睡过期）。
fn arm_quantum() {
    let quantum = sbi::read_time() + QUANTUM_MS * ticks_per_ms();
    let earliest = TIMERS.lock().iter().map(|d| d.at).min();
    sbi::require(
        sbi::set_timer(earliest.unwrap_or(quantum).min(quantum)),
        "TIME.set_timer",
    );
}

/// 回收退出线程：摘进程表 → 打统计 → drop 链释放地址空间（表帧/数据帧 RAII）。
///
/// 此刻线程不在任何容器、无其他 hart 能触达——本 hart 独占，无需额外锁。
fn reap(t: Arc<Thread>) {
    let pid = t.process.pid;
    let Some(process) = table::remove(pid) else {
        panic!("exited pid {} not in process table", pid);
    };
    let now = sbi::read_time();
    let elapsed_ms = (now - t.created_tick) / ticks_per_ms();
    log!(
        Task,
        "pid {} reaped: exit={:?}, {} switches, lifespan {} ms",
        pid,
        *t.exit_code.lock(),
        t.switches.load(Ordering::Relaxed),
        elapsed_ms
    );
    drop(t);
    drop(process); // 地址空间随 Drop 链归还全部帧
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

    let earliest = TIMERS.lock().iter().map(|d| d.at).min();
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
/// 产生工作：enqueue 只来自运行中的 hart）、就绪队列空、期限表空、
/// 无设备中断使能（当前无设备；接入后设备即主人，谓词自然失效）。
/// 新增等待源时本谓词必须同步扩展：每种 Waiting 都要有可枚举的主人，
/// 否则静默误判为停机。IPC 等待者（邮箱/信号）刻意**不**阻止静默：
/// hart 全 idle 时不存在能投递消息/信号的执行流，等待者永无主人，
/// 停机即正确终态。
fn is_quiescent() -> bool {
    IDLE_MASK.load(Ordering::SeqCst) == crate::registry::active_slot_mask()
        && !DOMAIN.has_ready()
        && TIMERS.lock().is_empty()
}
