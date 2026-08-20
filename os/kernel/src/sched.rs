//! 调度：域—类—执行点三层（notes/task.md「调度」）+ 调度循环 + 期限表。
//!
//! 单一归属不变量：线程任意时刻恰处于「类队列 | 本 hart current | 无容器」，
//! 全部转换经本模块入口（enqueue / pick / wake）在锁内完成。
//! 公平性由 FIFO 队列的结构性质保证，无记账字段（旧内核死因的免疫）。

use core::{
    arch::asm,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
};

use alloc::{collections::VecDeque, sync::Arc, vec::Vec};
use erhino_shared::proc::Pid;

use crate::sync::Spinlock;
use crate::{hart, sbi, trap::{self, Outcome}};
use crate::task::{table, Thread};

/// 调度类：一类线程的就绪容器 + 选择策略（可整体替换，见 notes/task.md）。
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
// 期限表（内核请求的等待对象，见 notes/call.md「异步调用」）
// ---------------------------------------------------------------------------

struct Deadline {
    at: u64,
    pid: Pid,
}

static TIMERS: Spinlock<Vec<Deadline>> = Spinlock::new(Vec::new());

/// 空闲 hart 位图（IPI 门铃的目标集）。
static IDLE_MASK: AtomicU64 = AtomicU64::new(0);

/// 每毫秒 tick 数（init 时按 timebase 换算）。
static TICKS_PER_MS: AtomicUsize = AtomicUsize::new(1);

/// 时间片量子（毫秒）。
const QUANTUM_MS: u64 = 10;

/// 定时器卸载值：远超 mtime 可达范围。
const DISARM: u64 = u64::MAX / 2;

pub fn init(timebase: usize) {
    TICKS_PER_MS.store((timebase / 1000).max(1), Ordering::Relaxed);
}

fn ticks_per_ms() -> u64 {
    TICKS_PER_MS.load(Ordering::Relaxed) as u64
}

/// 登记 sleep 期限并立即 arm 本 hart 定时器（唤醒所有权：期限的主人保证闹钟）。
pub fn sleep_register(ms: u64, pid: Pid) {
    if ms == 0 {
        return;
    }
    let at = sbi::read_time() + ms * ticks_per_ms();
    TIMERS.lock().push(Deadline { at, pid });
    arm_earliest();
}

/// 把定时器设到期限表最早期限（表空则不动）。
fn arm_earliest() {
    let timers = TIMERS.lock();
    if let Some(min) = timers.iter().map(|d| d.at).min() {
        sbi::set_timer(min);
    }
}

/// 唤醒全部到期线程（锁序单向：期限表锁 → 类锁，先收集后入队）。
fn wake_expired() {
    let now = sbi::read_time();
    let expired: Vec<Pid> = {
        let mut timers = TIMERS.lock();
        let (due, rest): (Vec<_>, Vec<_>) = timers.drain(..).partition(|d| d.at <= now);
        *timers = rest;
        due.into_iter().map(|d| d.pid).collect()
    };
    for pid in expired {
        // 进程可能已退出（惰性清理：找不到即丢弃过期登记）。
        if let Some(p) = table::get(pid) {
            if let Some(t) = p.main_thread() {
                // sleep 完成动作：写 ecall 响应（a0=NoError、sepc+4）后入队——
                // Wait 语义下 ecall 未前进，唤醒前补上结果（M4 泛化为内核
                // 请求的完成回调，见 notes/call.md）。
                // SAFETY: 线程处于 Waiting（不在任何容器、无 hart 执行），
                // 经进程表独占引用写其帧安全。
                let frame = unsafe { &mut *t.frame_ptr() };
                frame.x[10] = 0; // NoError
                frame.x[11] = 0;
                frame.sepc += 4;
                enqueue(t);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 入口：enqueue（新就绪 / 唤醒）与定时器事件
// ---------------------------------------------------------------------------

/// 线程入队并按门铃唤醒空闲 hart（IPI = 他方请求，见 notes/internals.md）。
pub fn enqueue(t: Arc<Thread>) {
    FAIR.enqueue(t);
    let mask = IDLE_MASK.load(Ordering::Relaxed);
    if mask != 0 {
        sbi::send_ipi(&mask);
    }
}

/// timer trap（量子耗尽或 sleep 到期）：卸载 → 唤醒到期；当前线程由
/// trap 出口 Requeue 轮转。
pub fn on_timer() {
    sbi::set_timer(DISARM);
    wake_expired();
}

/// 域内是否有就绪线程（SSIP 分支判断是否值得切走）。
pub fn domain_has_ready() -> bool {
    DOMAIN.has_ready()
}

/// 记录退出码（Exit / 异常终止共用；回收时随统计打印）。
pub fn report_exit(t: &Thread, code: i64) {
    *t.exit_code.lock() = Some(code);
}

// ---------------------------------------------------------------------------
// 调度循环（每 hart 常驻，见 notes/internals.md「trap 帧与上下文」）
// ---------------------------------------------------------------------------

/// hart 主循环：pick → 进用户态 → Switch 处置 → 循环；空则 idle。
pub fn run() -> ! {
    // 本 hart 内核现场初始化：SUM（直访用户页）与局部中断使能
    // （timer 量子/期限、IPI 门铃）。SUM 是 per-hart 位，各 hart 自开。
    crate::mm::enable_sum();
    // SAFETY: 仅置 sie.STIE(bit5)|SSIE(bit1) 位。
    unsafe { asm!("csrs sie, {bits}", bits = in(reg) 0b0010_0010) };

    let me = hart::current();
    loop {
        let Some(t) = DOMAIN.pick() else {
            idle();
            continue;
        };
        me.set_context(t.frame_ptr(), t.satp(), Arc::as_ptr(&t));
        t.switches.fetch_add(1, Ordering::Relaxed);
        arm_quantum();
        // SAFETY: 执行点已装好（帧/satp/线程），tp 不变量成立。
        let outcome = unsafe { trap::ret_to_user() };
        me.clear_context();
        match outcome {
            Outcome::Requeue => FAIR.enqueue(t),
            Outcome::Killed => reap(t),
            Outcome::Park => {} // 已登记内核请求（单一归属：无容器，等待 wake）
            Outcome::Resume => unreachable!("Resume 不经过调度循环"),
        }
    }
}

/// 量子装填：时间片与期限表最早期限取近（不睡过期）。
fn arm_quantum() {
    let quantum = sbi::read_time() + QUANTUM_MS * ticks_per_ms();
    let earliest = TIMERS.lock().iter().map(|d| d.at).min();
    sbi::set_timer(earliest.unwrap_or(quantum).min(quantum));
}

/// 回收退出线程：摘进程表 → 打统计 → drop 链释放地址空间（表帧/数据帧 RAII）。
///
/// 此刻线程不在任何容器、无其他 hart 能触达——本 hart 独占，无需额外锁。
fn reap(t: Arc<Thread>) {
    let pid = t.process.pid;
    let Some(process) = table::remove(pid) else {
        panic!("退出的 pid {} 不在进程表内", pid);
    };
    let now = sbi::read_time();
    let elapsed_ms = (now - t.created_tick) / ticks_per_ms();
    log!(
        Task,
        "pid {} 回收: exit={:?} {} 次调度, 存活 {} ms",
        pid,
        *t.exit_code.lock(),
        t.switches.load(Ordering::Relaxed),
        elapsed_ms
    );
    drop(t);
    drop(process); // 地址空间随 Drop 链归还全部帧
}

/// idle：登记空闲位 → 按期限表 arm（无期限则卸载）→ wfi。
/// 醒来（SIE=0，不 trap）清门铃后回主循环重查待办。
fn idle() {
    let bit = 1u64 << hart::hartid();
    IDLE_MASK.fetch_or(bit, Ordering::SeqCst);

    let earliest = TIMERS.lock().iter().map(|d| d.at).min();
    match earliest {
        Some(at) => sbi::set_timer(at),
        None => sbi::set_timer(DISARM),
    }
    // SAFETY: wfi 等待局部使能的中断 pending 唤醒。
    unsafe { asm!("wfi", options(nomem, preserves_flags)) };
    sbi::clear_ssip();
    IDLE_MASK.fetch_and(!bit, Ordering::SeqCst);

    if sip_stip_pending() {
        // 期限到达唤醒（回主循环前把到期线程入队）。
        sbi::set_timer(DISARM);
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
