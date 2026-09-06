//! 调度：域—类—执行点三层（notes/impls/task.md「调度」）+ 调度循环 + 期限表。
//!
//! 单一归属不变量：可调度 owner 恰处于「类队列 | 本 hart current | WaitContext」，
//! 全寿命容量随不可复制的 AdmittedThread 在容器间移动，离场才归还。
//! 公平性由 FIFO 队列的结构性质保证，不依赖额外记账字段。
//!
//! 调度域按「需求满足签名」推导（sched_domain crate）：域 = 一组能力
//! 兼容且策略相同的 hart，boot 构造后终身冻结；线程经进程绑定到唯一
//! compatible domain（ProcessStart 提交点冻结），只在所属域的类队列出现。

use core::{
    arch::asm,
    sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering},
};

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use ready_queue::{Admission, ReadyQueue};

use crate::sbi::DISARM;
use crate::sync::Spinlock;
use crate::task::{Thread, lifecycle::EnterRunning};
use crate::{
    deferred_work, hart, sbi,
    trap::{self, Outcome},
};

/// 执行容器唯一拥有的线程与全寿命容量；借用底层 Arc 不复制调度资格。
pub type AdmittedThread = ready_queue::Admitted<Arc<Thread>>;

/// 调度类负责自己的实际存储；首次批次和后续唤醒共享同一准入来源。
pub trait SchedClass: Sync {
    fn enqueue(&self, t: AdmittedThread);
    fn pick(&self) -> Option<AdmittedThread>;
    fn has_ready(&self) -> bool;
    fn reserve_batch(&self, count: usize) -> Result<Admission, ()>;
    /// 整批在一次类锁内交付；没有 marker、token 或存量扫描。
    fn publish_batch(&self, admission: Admission, threads: Vec<Arc<Thread>>);
}

/// 公平类：FIFO 轮转 + 固定量子。
pub struct FairClass {
    ready: Spinlock<ReadyQueue<Arc<Thread>>>,
}

impl FairClass {
    fn new() -> Self {
        Self {
            ready: Spinlock::new(
                crate::sync::ranks::LEAF,
                ReadyQueue::try_new().expect("scheduler capacity metadata allocation failed"),
            ),
        }
    }
}

impl SchedClass for FairClass {
    fn enqueue(&self, t: AdmittedThread) {
        self.ready.lock().enqueue(t);
    }

    fn pick(&self) -> Option<AdmittedThread> {
        self.ready.lock().pick()
    }

    fn has_ready(&self) -> bool {
        !self.ready.lock().is_empty()
    }

    fn reserve_batch(&self, count: usize) -> Result<Admission, ()> {
        self.ready.lock().reserve(count).map_err(|_| ())
    }

    fn publish_batch(&self, mut admission: Admission, threads: Vec<Arc<Thread>>) {
        assert_eq!(
            admission.remaining(),
            threads.len(),
            "ready batch/thread count mismatch"
        );
        let mut ready = self.ready.lock();
        for thread in threads {
            ready.enqueue(admission.admit(thread));
        }
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
    fn pick(&self) -> Option<AdmittedThread> {
        self.classes.iter().find_map(|c| c.pick())
    }

    fn has_ready(&self) -> bool {
        self.classes.iter().any(|c| c.has_ready())
    }

    pub(crate) fn index(&self) -> usize {
        self.index
    }

    /// 就绪入队（Requeue/wake 路径的公平类；今天单类，classes[0] 即公平类）。
    fn enqueue_fair(&self, t: AdmittedThread) {
        self.classes[0].enqueue(t);
    }

    /// 为目标公平类支付完整出生批次的全寿命存储；取消仅做原子退款。
    pub fn reserve_ready(&'static self, count: usize) -> Result<ReadyBatch, ()> {
        Ok(ReadyBatch {
            domain: self,
            admission: self.classes[0].reserve_batch(count)?,
        })
    }

    /// 唤醒本域一个空闲 hart（门铃只达本域 idle hart）。
    fn wake_one(&self) {
        let mask = self.idle_mask.load(Ordering::SeqCst);
        if mask != 0 {
            crate::registry::ipi_slots(mask);
        }
    }
}

/// 目标域和实际类存储的出生准入，未交付部分随 owner 析构取消。
pub struct ReadyBatch {
    domain: &'static SchedDomain,
    admission: Admission,
}

impl ReadyBatch {
    pub fn publish(self, threads: Vec<Arc<Thread>>) {
        assert!(
            threads
                .iter()
                .all(|thread| core::ptr::eq(thread.process.domain(), self.domain)),
            "ready batch belongs to another execution domain"
        );
        self.domain.classes[0].publish_batch(self.admission, threads);
        self.domain.wake_one();
    }

    /// 将跨调用层交付的线程与同一准入责任绑定；发布入口只接受该 owner。
    pub fn admit(&mut self, thread: Arc<Thread>) -> AdmittedThread {
        assert!(
            core::ptr::eq(thread.process.domain(), self.domain),
            "thread belongs to another execution domain"
        );
        self.admission.admit(thread)
    }
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

/// 每 hart 固定的内联等待意图；与期限表同属 Rust 调度状态，不进入 trap 锚布局。
static HART_WAIT_PLANS: [Spinlock<Option<crate::task::wait::WaitPlan>>; hart::HART_NUM_LIMIT] =
    [const { Spinlock::new(crate::sync::ranks::LEAF, None) }; hart::HART_NUM_LIMIT];

/// dispatcher 侧登记：把等待意图写入本 hart 槽。此刻**不碰任何全局
/// 结构**——发布由调度循环在线程离开执行点之后完成，
/// 保证「可被唤醒」严格晚于「无容器」，完成方永远见不到仍在本 hart
/// 执行的线程。
/// 新对象 ABI 的统一等待意图；计划已在 syscall 入口解析 Handle 并保留授权。
pub fn park_request_wait(plan: crate::task::wait::WaitPlan) {
    let mut slot = HART_WAIT_PLANS[hart::current().slot()].lock();
    assert!(slot.is_none(), "hart wait intent is already occupied");
    *slot = Some(plan);
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
pub fn enqueue(t: AdmittedThread) {
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
        deferred_work::drain_current();
        crate::task::notify_work::drain_current();
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
        // ProcessWrite 可经另一 hart 的直映射回填可执行页；除上面的
        // execution gate epoch 同步外，每次新 dispatch 还执行本地 fence.i。
        // SAFETY: fence.i 是本 hart 指令流同步，不触碰内存。
        unsafe { asm!("fence.i", options(nostack, preserves_flags)) };
        // SAFETY: 执行点已装好（帧/satp/线程），tp 不变量成立。
        let outcome = unsafe { trap::ret_to_user() };
        me.clear_context();
        // trap 的终止吸收可能把已登记等待的 Park 改判为 Killed。每个
        // Switch 出口都先取走意图；Killed 放弃未安装计划，不遗留给下一线程。
        let wait_plan = HART_WAIT_PLANS[me.slot()].lock().take();
        // 非-Resume 出口的归一（内核 satp + 全量 SFENCE.VMA）已由汇编
        // 出口边界完成：active 位图与后续 teardown 不得在目标地址空间
        // 上进行。
        let slot = me.slot();
        match outcome {
            Outcome::Requeue => {
                assert!(wait_plan.is_none(), "Requeue outcome carries a wait intent");
                loop {
                    deferred_work::drain_current();
                    crate::task::notify_work::drain_current();
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
                    deferred_work::drain_current();
                    crate::task::notify_work::drain_current();
                    let epochs = t.process.space.epochs();
                    if t.process
                        .lifecycle
                        .clear_active_if(slot, || t.process.space.local_is_current(epochs))
                    {
                        break;
                    }
                }
                drop(wait_plan);
                reap(t);
            }
            // 已离开执行点，此刻发布等待：完成方可安全触达该线程。
            Outcome::Park => {
                loop {
                    deferred_work::drain_current();
                    crate::task::notify_work::drain_current();
                    let epochs = t.process.space.epochs();
                    if t.process
                        .lifecycle
                        .clear_active_if(slot, || t.process.space.local_is_current(epochs))
                    {
                        break;
                    }
                }
                let plan = wait_plan.expect("Park outcome must carry a wait intent");
                crate::task::wait::install(t, plan);
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
fn reap(t: AdmittedThread) {
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
    // 入队与登记 idle 的交错由双重检查闭合；work debt 的 Pending 电平同样
    // 禁止 owner 带债入睡，即使对应门铃曾失败或被合并。
    if domain.has_ready() || deferred_work::has_current() || crate::task::notify_work::has_current()
    {
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
