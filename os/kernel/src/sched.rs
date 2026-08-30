//! 调度：域—类—执行点三层（notes/impls/task.md「调度」）+ 调度循环 + 期限表。
//!
//! 单一归属不变量：线程任意时刻恰处于「类队列 | 本 hart current | 无容器」，
//! 全部转换经本模块入口（enqueue / pick / wake）在锁内完成。
//! 公平性由 FIFO 队列的结构性质保证，不依赖额外记账字段。
//!
//! 调度域按「需求满足签名」推导（sched_domain crate）：域 = 一组能力
//! 兼容且策略相同的 hart，boot 构造后终身冻结；线程经进程绑定到唯一
//! compatible domain（ProcessStart 提交点冻结），只在所属域的类队列出现。

use core::{
    arch::asm,
    sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering},
};

use alloc::{boxed::Box, collections::VecDeque, sync::Arc, vec::Vec};

use crate::sbi::DISARM;
use crate::sync::Spinlock;
use crate::task::{Thread, lifecycle::EnterRunning};
use crate::{
    hart, remote_call, sbi,
    trap::{self, Outcome},
};

/// 调度类：一类线程的就绪容器 + 选择策略（可整体替换，见 notes/impls/task.md）。
/// reserve/commit/rollback 是就绪容量的批量事务契约（协议四要素，见
/// notes/impls/task.md「reserve/commit/rollback 协议」）：整批占位对 pick/
/// has_ready 不可见，token 全局单调防错认，commit/rollback 凭 token 消费
/// 完整批次。容量必须预留在线程将要进入的目标容器（具体类队列）里，域层
/// 路由不替代本契约。
pub trait SchedClass: Sync {
    fn enqueue(&self, t: Arc<Thread>);
    fn pick(&self) -> Option<Arc<Thread>>;
    fn has_ready(&self) -> bool;
    /// 原子预留 count 个就绪容量占位；失败不留下部分预留。
    fn reserve_batch(&self, count: usize) -> Result<u64, ()>;
    /// 凭 token 提交完整线程批次（不可失败：容量已预留）。
    fn commit_batch(&self, token: u64, threads: Vec<Arc<Thread>>);
    /// 凭 token 回滚完整预留批次（不可失败：token 只被本事务消费）。
    fn rollback_batch(&self, token: u64, count: usize);
}

/// 公平类：FIFO 轮转 + 固定量子。
enum ReadyEntry {
    Reserved(u64),
    Thread(Arc<Thread>),
}

pub struct FairClass {
    ready: Spinlock<VecDeque<ReadyEntry>>,
}

impl FairClass {
    const fn new() -> Self {
        Self {
            ready: Spinlock::new(crate::sync::ranks::LEAF, VecDeque::new()),
        }
    }
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

    fn reserve_batch(&self, count: usize) -> Result<u64, ()> {
        if count == 0 {
            return Err(());
        }
        let token = NEXT_READY_RESERVATION.fetch_add(1, Ordering::Relaxed);
        if token == 0 {
            return Err(());
        }
        let mut ready = self.ready.lock();
        ready.try_reserve(count).map_err(|_| ())?;
        ready.extend((0..count).map(|_| ReadyEntry::Reserved(token)));
        Ok(token)
    }

    fn commit_batch(&self, token: u64, threads: Vec<Arc<Thread>>) {
        let expected = threads.len();
        let mut threads = threads.into_iter();
        let mut committed = 0;
        let mut ready = self.ready.lock();
        for entry in ready.iter_mut() {
            if matches!(entry, ReadyEntry::Reserved(reserved) if *reserved == token) {
                let thread = threads
                    .next()
                    .expect("ready reservation batch is too large");
                *entry = ReadyEntry::Thread(thread);
                committed += 1;
            }
        }
        assert_eq!(committed, expected, "ready reservation batch disappeared");
        assert!(
            threads.next().is_none(),
            "ready reservation batch is too small"
        );
    }

    fn rollback_batch(&self, token: u64, count: usize) {
        let mut removed = 0;
        let mut ready = self.ready.lock();
        ready.retain(|entry| {
            let matches = matches!(entry, ReadyEntry::Reserved(reserved) if *reserved == token);
            removed += usize::from(matches);
            !matches
        });
        assert_eq!(removed, count, "ready reservation batch disappeared");
    }
}

/// 调度域：一组能力兼容且策略相同的 hart 共享的类层次，按序查询、先到
/// 先得。域按「需求满足签名」推导（sched_domain crate，方向公理见
/// notes/ideas/task.md「线程」），boot 构造后终身冻结；域内 idle 位图是
/// IPI 门铃的目标集（slot 位图，经 registry 展开为 raw hartid，绝不把
/// 内部位图直接解释为 SBI hart mask）。
pub struct SchedDomain {
    index: usize,
    classes: [&'static dyn SchedClass; 1],
    idle_mask: AtomicU64,
}

impl SchedDomain {
    fn pick(&self) -> Option<Arc<Thread>> {
        self.classes.iter().find_map(|c| c.pick())
    }

    fn has_ready(&self) -> bool {
        self.classes.iter().any(|c| c.has_ready())
    }

    pub(crate) fn index(&self) -> usize {
        self.index
    }

    /// 就绪入队（Requeue/wake 路径的公平类；今天单类，classes[0] 即公平类）。
    fn enqueue_fair(&self, t: Arc<Thread>) {
        self.classes[0].enqueue(t);
    }

    /// 唤醒本域一个空闲 hart（门铃只达本域 idle hart）。
    fn wake_one(&self) {
        let mask = self.idle_mask.load(Ordering::SeqCst);
        if mask != 0 {
            crate::registry::ipi_slots(mask);
        }
    }
}

static NEXT_READY_RESERVATION: AtomicU64 = AtomicU64::new(1);

/// Start 事务的域批量预留凭据（域 + token + 数量）。
pub struct ReadyBatch {
    domain: &'static SchedDomain,
    token: u64,
    count: usize,
}

/// 在目标域的公平类原子预留完整就绪批次。
pub fn reserve_ready_batch(domain: &'static SchedDomain, count: usize) -> Result<ReadyBatch, ()> {
    let token = domain.classes[0].reserve_batch(count)?;
    Ok(ReadyBatch {
        domain,
        token,
        count,
    })
}

pub fn commit_ready_batch(batch: ReadyBatch, threads: Vec<Arc<Thread>>) {
    assert_eq!(
        batch.count,
        threads.len(),
        "ready batch/thread count mismatch"
    );
    batch.domain.classes[0].commit_batch(batch.token, threads);
    batch.domain.wake_one();
}

pub fn rollback_ready_batch(batch: ReadyBatch) {
    batch.domain.classes[0].rollback_batch(batch.token, batch.count);
}

// ---------------------------------------------------------------------------
// 域表（boot 冻结）：划分真值 + slot/域下标 → 域对象
// ---------------------------------------------------------------------------

/// 域数上界：签名等价类数 ≤ admitted hart 数。
const MAX_DOMAINS: usize = hart::HART_NUM_LIMIT;

struct DomainTable {
    /// 域划分（resolve 的唯一真值；需求位序见 sched_domain）。
    plan: sched_domain::DomainPlan,
    /// 域下标 → 域对象。
    domains: [Option<&'static SchedDomain>; MAX_DOMAINS],
    /// slot → 所属域。
    by_slot: [Option<&'static SchedDomain>; hart::HART_NUM_LIMIT],
}

static DOMAINS: AtomicPtr<DomainTable> = AtomicPtr::new(core::ptr::null_mut());

/// 域表访问（Release/Acquire 发布；未构造即访问是时序错误）。
fn domains() -> &'static DomainTable {
    let ptr = DOMAINS.load(Ordering::Acquire);
    // SAFETY: boot 构造后终身有效（泄漏不释放）。
    unsafe { ptr.as_ref() }.expect("domain table not built")
}

/// boot 单核构造域表（bring_up_runtime，全员 Online 后、初始任务装载
/// 前）。域对象泄漏为 'static 终身冻结；hart→域归属与 caps 同属 boot
/// 事实（运行中不变，见 notes/ideas/task.md「线程」绑定冻结点）。
pub fn build_domains() {
    let mut caps = Vec::new();
    crate::registry::with_registry(|reg| {
        for (slot, _) in reg.records() {
            caps.push(crate::registry::load_caps(slot));
        }
    });
    let plan = sched_domain::plan(&caps);
    let mut domains: [Option<&'static SchedDomain>; MAX_DOMAINS] = [const { None }; MAX_DOMAINS];
    for index in 0..plan.domain_count() {
        let fair: &'static FairClass = Box::leak(Box::new(FairClass::new()));
        domains[index] = Some(Box::leak(Box::new(SchedDomain {
            index,
            classes: [fair],
            idle_mask: AtomicU64::new(0),
        })));
    }
    let mut by_slot: [Option<&'static SchedDomain>; hart::HART_NUM_LIMIT] =
        [const { None }; hart::HART_NUM_LIMIT];
    for slot in 0..caps.len() {
        by_slot[slot] = domains[plan.slot_domain(slot)];
    }
    // 拓扑快照（验收观测行）：每域满足的需求与成员 slot。
    for (index, _) in domains.iter().enumerate().take(plan.domain_count()) {
        let members: Vec<usize> = (0..caps.len())
            .filter(|slot| plan.slot_domain(*slot) == index)
            .collect();
        let labels: Vec<&str> = sched_domain::REQUIREMENTS
            .iter()
            .enumerate()
            .filter(|(bit, _)| plan.signature(index) & (1 << bit) != 0)
            .map(|(bit, _)| sched_domain::requirement_label(bit))
            .collect();
        log!(
            Sched,
            "domain {} [{}] -> harts {:?}",
            index,
            labels.join("+"),
            members
        );
    }
    let table = Box::leak(Box::new(DomainTable {
        plan,
        domains,
        by_slot,
    }));
    DOMAINS.store(table as *const _ as *mut _, Ordering::Release);
}

/// 本 hart 所属域（tp → slot → 域表；调度循环与 trap 路径专用）。
#[inline]
fn current_domain() -> &'static SchedDomain {
    domains().by_slot[hart::current().slot()].expect("domain table not built")
}

/// requirement → 兼容域中最弱者（默认放置政策，见 sched_domain）。
/// 无兼容域（平台事实）返回 None；Base64 恒有解（准入 ⇒ 基线 ⇒
/// Base64 兼容的准入不变量）。
pub fn resolve_domain(requirement: elf::IsaRequirement) -> Option<&'static SchedDomain> {
    let table = domains();
    table
        .plan
        .resolve(requirement)
        .and_then(|index| table.domains[index])
}

pub(crate) fn domain_by_index(index: usize) -> &'static SchedDomain {
    domains()
        .domains
        .get(index)
        .and_then(|domain| *domain)
        .expect("process execution binding names an unknown scheduler domain")
}

// ---------------------------------------------------------------------------
// 等待模型（notes/impls/call.md「异步调用」）：等待条目 + 代数仲裁 + 发布时序
// ---------------------------------------------------------------------------

/// per-hart 期限队列：期限主人是登记 hart（唤醒所有权），登记、arm、
/// 到期弹出与 idle 装填只碰本 hart 队列。跨 hart 完成仅按 token 锁住
/// owner queue 删除项，且不远程重编程 owner timer。
static HART_TIMERS: [Spinlock<timer_queue::TimerQueue<Arc<crate::task::wait::WaitContext>>>;
    hart::HART_NUM_LIMIT] =
    [const { Spinlock::new(crate::sync::ranks::LEAF, timer_queue::TimerQueue::unbound()) };
        hart::HART_NUM_LIMIT];

/// 本 hart 的期限队列（slot 由 formal entry 设置，调用点均在调度循环或
/// 其 Park 发布路径内）。
#[inline]
fn timers() -> &'static Spinlock<timer_queue::TimerQueue<Arc<crate::task::wait::WaitContext>>> {
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
    me.park_arg
        .store(Box::into_raw(Box::new(plan)) as usize, Ordering::Relaxed);
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

pub fn expires_after_ms(timeout_ms: u64) -> u64 {
    sbi::read_time().saturating_add(timeout_ms.saturating_mul(ticks_per_ms()))
}

/// 在发起 hart 的期限队列登记等待，并立刻按新堆顶装填本地时钟。
pub(crate) fn register_wait_timeout(
    expires_at: u64,
    context: Arc<crate::task::wait::WaitContext>,
) -> Result<timer_queue::TimerToken, ()> {
    let owner_slot = hart::current().slot();
    let mut timers = timers().lock();
    assert!(
        timers.bind_owner(owner_slot),
        "timer queue bound to the wrong hart"
    );
    let token = timers.try_register(expires_at, context).map_err(|_| ())?;
    drop(timers);
    arm_earliest();
    Ok(token)
}

/// 由任意完成 hart 注销 timeout。只移除 owner queue 项，不重编程远端
/// timer；最多引起一次提前中断，owner 会在下一装填点按堆顶恢复。
pub(crate) fn unregister_wait_timeout(token: timer_queue::TimerToken) {
    let removed = HART_TIMERS[token.owner_slot()].lock().cancel(token);
    // WaitContext 的最后一个强引用不得在期限队列锁内析构。
    drop(removed);
}

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

/// 把本 hart 定时器设到期限队列最早到期点（队列空则不动）。
fn arm_earliest() {
    let timers = timers().lock();
    if let Some(expires_at) = timers.peek_expires_at() {
        sbi::require(sbi::set_timer(expires_at), "TIME.set_timer");
    }
}

/// 弹出本 hart 全部已到期项后，在锁外以 token 通知 context。弹出与
/// 注销竞争时只有成功退休 token 的路径参与 Timeout outcome 仲裁。
fn wake_expired() {
    let now = sbi::read_time();
    loop {
        let due = timers().lock().pop_expired(now);
        let Some((token, context)) = due else { break };
        context.expire(token);
    }
}

// ---------------------------------------------------------------------------
// 入口：enqueue（新就绪 / 唤醒）与定时器事件
// ---------------------------------------------------------------------------

/// 线程入队并按门铃唤醒其所属域的空闲 hart（IPI = 他方请求，见
/// notes/impls/internals.md）。线程只在所属域的类队列出现。
pub fn enqueue(t: Arc<Thread>) {
    let domain = t.process.domain();
    domain.enqueue_fair(t);
    domain.wake_one();
}

/// timer trap（量子耗尽或 sleep 到期）：卸载 → 唤醒到期；当前线程由
/// trap 出口 Requeue 轮转。
pub fn on_timer() {
    sbi::require(sbi::set_timer(DISARM), "TIME.set_timer");
    wake_expired();
}

/// 本 hart 所属域是否有就绪线程（SSIP 分支判断是否值得切走）。
pub fn domain_has_ready() -> bool {
    current_domain().has_ready()
}

// ---------------------------------------------------------------------------
// 调度循环（每 hart 常驻，见 notes/impls/internals.md「trap 帧与上下文」）
// ---------------------------------------------------------------------------

/// hart 主循环：pick → 进用户态 → Switch 处置 → 循环；空则 idle。
/// pick 只从本 hart 所属域取（域内类按优先级序），线程的域绑定与
/// hart 的域归属在 boot/Start 各自冻结，运行期不迁移。
pub fn run() -> ! {
    // 内核现场（sie/SUM/FS 稳态）已由 formal entry 集中建立；
    // 本循环只维护执行点与量子。域终身冻结，循环外取一次。
    let me = hart::current();
    let me_domain = current_domain();
    loop {
        // idle 唤醒、门铃合并或先前 IPI 失败后，Pending 槽仍由安全点补消费。
        remote_call::drain_current();
        // 非 Resume 出口已在汇编边界归一（kernel satp + 本地全量
        // SFENCE.VMA）：循环体结构性只运行于内核页表下。
        let Some(t) = me_domain.pick() else {
            idle();
            continue;
        };
        // eligibility 纵深防御：线程只能在其绑定域的 hart 上运行
        // （结构上 pick 只达本域队列，断言兜底域路由接线错误）。
        debug_assert!(
            core::ptr::eq(t.process.domain(), me_domain),
            "thread must run in its bound domain"
        );
        // lifecycle gate：Terminating 线程不进用户态（惰性撤销）。
        let entered = loop {
            let epochs = t.process.space.synchronize_local();
            match t.process.lifecycle.enter_running_if(t.tid, me.slot(), || {
                t.process.space.local_is_current(epochs)
            }) {
                EnterRunning::Entered => break true,
                EnterRunning::Retry => continue,
                EnterRunning::Closed => break false,
            }
        };
        if !entered {
            reap(t);
            continue;
        }
        if t.process.pid == 1 {
            t.process.space.selftest_shootdown(&t.process.lifecycle);
        }
        me.set_context(t.frame_ptr(), t.satp(), Arc::as_ptr(&t), t.uses_fp());
        arm_quantum();
        // ProcessWrite 可经另一 hart 的直映射回填刚分配的可执行页。active
        // bitmap 当前只服务终止屏障，尚无代码代次，因此每次新 dispatch
        // 执行 fence.i，确保首次执行及迁移不观察旧 I-cache 内容。
        // SAFETY: fence.i 是本 hart 指令流同步，不触碰内存。
        unsafe { asm!("fence.i", options(nostack, preserves_flags)) };
        // SAFETY: 执行点已装好（帧/satp/线程），tp 不变量成立。
        let outcome = unsafe { trap::ret_to_user() };
        me.clear_context();
        // 非-Resume 出口的归一（内核 satp + 全量 SFENCE.VMA）已由汇编
        // 出口边界完成：active 位图与后续 teardown 不得在目标地址空间
        // 上进行。
        let slot = me.slot();
        match outcome {
            Outcome::Requeue => {
                loop {
                    remote_call::drain_current();
                    let epochs = t.process.space.epochs();
                    if t.process
                        .lifecycle
                        .on_requeue_if(t.tid, slot, || t.process.space.local_is_current(epochs))
                    {
                        break;
                    }
                }
                if t.process.lifecycle.is_terminating() {
                    reap(t);
                } else {
                    // 轮转回所属域的公平类（不打门铃：本 hart 忙，
                    // Requeue 线程由下一次 pick 自然推进）。
                    t.process.domain().enqueue_fair(t);
                }
            }
            Outcome::Killed => {
                loop {
                    remote_call::drain_current();
                    let epochs = t.process.space.epochs();
                    if t.process
                        .lifecycle
                        .clear_active_if(slot, || t.process.space.local_is_current(epochs))
                    {
                        break;
                    }
                }
                reap(t);
            }
            // 已离开执行点，此刻发布等待：完成方可安全触达该线程。
            Outcome::Park => {
                loop {
                    remote_call::drain_current();
                    let epochs = t.process.space.epochs();
                    if t.process
                        .lifecycle
                        .clear_active_if(slot, || t.process.space.local_is_current(epochs))
                    {
                        break;
                    }
                }
                park_publish(&t);
            }
            Outcome::Resume => unreachable!("Resume never passes through the scheduling loop"),
        }
    }
}

/// 量子装填：时间片与本 hart 期限表最早期限取近（不睡过期）。
fn arm_quantum() {
    let quantum = sbi::read_time() + QUANTUM_MS * ticks_per_ms();
    let earliest = timers().lock().peek_expires_at();
    sbi::require(
        sbi::set_timer(earliest.unwrap_or(quantum).min(quantum)),
        "TIME.set_timer",
    );
}

/// 回收终止线程：先移除执行容器强引用，再向独立 departure state 请求离场。
/// committed Map 结果义务可延后成员摘除与 DONE，但不保留 Thread/UserContext。
fn reap(t: Arc<Thread>) {
    // ThreadDeparture 只 weak 引用 Process；成员根可能已经摘除，必须把 core
    // 强持到 departure 完成成员确认。
    let process = t.process.clone();
    let departure = t.departure();
    let departure_kind = t.departure_kind();
    drop(t);
    departure.request(departure_kind);
    drop(process);
}

/// idle：在本域登记空闲位 → 双重检查就绪工作 → 按期限表 arm（无期限则卸载）
/// → wfi。醒来（SIE=0，不 trap）清门铃后回主循环重查待办。
fn idle() {
    let domain = current_domain();
    let bit = 1u64 << hart::current().slot();
    domain.idle_mask.fetch_or(bit, Ordering::SeqCst);
    // 入队与登记 idle 的交错由双重检查闭合：登记后若本域已有工作，
    // 说明入队发生在第一次 pick 之后，立即撤销 idle 身份并重试。
    if domain.has_ready() {
        domain.idle_mask.fetch_and(!bit, Ordering::SeqCst);
        return;
    }

    let earliest = timers().lock().peek_expires_at();
    match earliest {
        Some(at) => sbi::require(sbi::set_timer(at), "TIME.set_timer"),
        None => sbi::require(sbi::set_timer(DISARM), "TIME.set_timer"),
    };
    // SAFETY: wfi 等待局部使能的中断 pending 唤醒。
    unsafe { asm!("wfi", options(nomem, preserves_flags)) };
    sbi::clear_ssip();
    domain.idle_mask.fetch_and(!bit, Ordering::SeqCst);

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
