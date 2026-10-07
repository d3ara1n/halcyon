//! 调度：域—类—执行点三层（notes/impls/task.md「调度」）+ 调度循环 + 期限表。
//!
//! 单一归属不变量：执行 owner 恰处于「类队列 | 本 hart current | 成员 Waiting」，
//! 全寿命容量随不可复制的 AdmittedThread 在容器间移动，离场才归还。
//! 公平性由 FIFO 队列的结构性质保证，不依赖额外记账字段。
//!
//! 调度域按「需求满足签名」推导（sched_domain crate）：域 = 一组能力
//! 兼容且策略相同的 hart，boot 构造后终身冻结；线程经进程绑定到唯一
//! compatible domain（ProcessStart 提交点冻结），只在所属域的类队列出现。

use core::{
    arch::asm,
    sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering},
};

use crate::task::resources::{MetadataCharge, MetadataFunding};
use alloc::{boxed::Box, sync::Arc, vec::Vec};
use erhino_shared::call::SystemCallError;
use ready_queue::{Admission, ReadyQueue, ReadyStorage, ReserveError};

use crate::sbi::DISARM;
use crate::sync::Spinlock;
use crate::task::{Thread, process::members::WakeTransition};
use crate::{
    hart, sbi,
    trap::{self, Outcome},
};
use erhino_shared::proc::Pid;
use ready_queue::Scan;

/// 执行容器唯一拥有的线程与全寿命容量；借用底层 Arc 不复制调度资格。
pub type AdmittedThread = ready_queue::Admitted<Arc<Thread>>;

/// 调度类负责自己的实际存储；首次批次和后续唤醒共享同一准入来源。
pub trait SchedClass: Sync {
    fn enqueue(&self, t: AdmittedThread);
    fn pick(&self) -> Option<AdmittedThread>;
    fn has_ready(&self) -> bool;
    fn ready_len(&self) -> usize;
    fn scan_ready_for(&self, pid: Pid) -> Scan<Arc<Thread>>;
    fn reserve_batch(&self, count: usize) -> Result<Admission, SystemCallError>;
}

/// 公平类：FIFO 轮转 + 固定量子。
pub struct FairClass {
    ready: Spinlock<ReadyQueue<Arc<Thread>, MetadataCharge>>,
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

    fn ready_len(&self) -> usize {
        self.ready.lock().len()
    }

    fn scan_ready_for(&self, pid: Pid) -> Scan<Arc<Thread>> {
        self.ready
            .lock()
            .scan_front_if(|thread| thread.process.pid == pid)
    }

    fn reserve_batch(&self, count: usize) -> Result<Admission, SystemCallError> {
        let mut prepared = ReadyStorage::new();
        loop {
            let required = {
                let mut ready = self.ready.lock();
                match ready.reserve(count, &mut prepared) {
                    Ok(admission) => return Ok(admission),
                    Err(ReserveError::CapacityChanged) => ready.growth_capacity(count),
                    Err(error) => Err(error),
                }
            }
            .map_err(|error| match error {
                ReserveError::Empty | ReserveError::CapacityOverflow => {
                    SystemCallError::IllegalArgument
                }
                ReserveError::AllocationFailed => SystemCallError::OutOfMemory,
                ReserveError::CapacityChanged => {
                    unreachable!("ready growth cannot require prepared storage")
                }
            })?;
            // 旧空块/落败候选在全部类锁外退款；系统共享容量不钉客户端预算。
            drop(prepared);
            let pool = crate::task::memory_pool::MemoryPool::system_pool();
            prepared =
                ReadyStorage::try_empty(required, &MetadataFunding(&pool)).map_err(|error| {
                    match error {
                        funded_storage::StorageError::Funding(error) => error,
                        funded_storage::StorageError::Allocation => SystemCallError::OutOfMemory,
                        funded_storage::StorageError::Layout => SystemCallError::ReachLimit,
                    }
                })?;
        }
    }
}

/// 调度域：一组能力兼容且策略相同的 hart 共享的类层次，按序查询、先到
/// 先得。域按「需求满足签名」推导（sched_domain crate，方向公理见
/// notes/ideas/task.md「线程」），boot 构造后终身冻结；域内 idle 位图是
/// IPI 门铃的目标集（slot 位图，经 registry 展开为 raw hartid，绝不把
/// 内部位图直接解释为 SBI hart mask）。
pub struct SchedDomain {
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

    /// 回收 Work 只扫描出生时的队列长度，后续入队不延长本轮。
    pub(crate) fn ready_len(&self) -> usize {
        self.classes[0].ready_len()
    }

    /// 返回 false 表示队列已空；命中者在队列锁外经过正常离场路径。
    pub(crate) fn cancel_ready_step(&self, pid: Pid) -> bool {
        match self.classes[0].scan_ready_for(pid) {
            Scan::Empty => false,
            Scan::Retained => true,
            Scan::Taken(thread) => {
                reap(thread);
                true
            }
        }
    }

    /// 就绪入队（Requeue/wake 路径的公平类；今天单类，classes[0] 即公平类）。
    pub(crate) fn enqueue_fair(&self, t: AdmittedThread) {
        self.classes[0].enqueue(t);
    }

    /// 为目标公平类支付完整出生批次的全寿命存储；取消仅做原子退款。
    pub fn reserve_ready(&'static self, count: usize) -> Result<ReadyBatch, SystemCallError> {
        Ok(ReadyBatch {
            domain: self,
            admission: self.classes[0].reserve_batch(count)?,
        })
    }

    /// 唤醒本域一个空闲 hart（门铃只达本域 idle hart）。
    fn wake_one(&self) {
        let mask = self.idle_mask.load(Ordering::SeqCst);
        if mask != 0 {
            let failed = crate::runtime_stop::notify_slots(mask);
            if failed != 0 {
                warn!(
                    Hart,
                    "Ready-queue doorbell failed for hart slot mask {failed:#x}; runtime stop requested"
                );
            }
        }
    }
}

/// 目标域和实际类存储的出生准入，未交付部分随 owner 析构取消。
pub struct ReadyBatch {
    domain: &'static SchedDomain,
    admission: Admission,
}

impl ReadyBatch {
    pub fn publish_one(self, thread: Arc<Thread>) {
        assert!(
            core::ptr::eq(thread.domain, self.domain),
            "ready batch belongs to another execution domain"
        );
        let mut admission = self.admission;
        self.domain.classes[0].enqueue(admission.admit(thread));
        self.domain.wake_one();
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
    for domain in domains.iter_mut().take(plan.domain_count()) {
        let fair: &'static FairClass = Box::leak(Box::new(FairClass::new()));
        *domain = Some(Box::leak(Box::new(SchedDomain {
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

// ---------------------------------------------------------------------------
// 等待模型（notes/impls/call.md「异步调用」）：等待条目 + 代数仲裁 + 发布时序
// ---------------------------------------------------------------------------

/// per-hart 期限队列：期限主人是登记 hart（唤醒所有权），登记、arm、
/// 到期弹出与 idle 装填只碰本 hart 队列。跨 hart detach 按节点 owner 锁住
/// owner queue 删除项，且不远程重编程 owner timer。
static HART_TIMERS: [Spinlock<timer_queue::TimerQueue<TimerDelivery>>; hart::HART_NUM_LIMIT] =
    [const { Spinlock::new(crate::sync::ranks::LEAF, timer_queue::TimerQueue::unbound()) };
        hart::HART_NUM_LIMIT];

/// 本 hart 的期限队列（slot 由 formal entry 设置，调用点均在调度循环或
/// 其 Park 发布路径内）。
#[inline]
fn timers() -> &'static Spinlock<timer_queue::TimerQueue<TimerDelivery>> {
    &HART_TIMERS[hart::current().slot()]
}

pub(crate) struct TimerDelivery {
    pub(crate) thread: Arc<Thread>,
    pub(crate) epoch: u64,
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

/// 在发起 hart 的期限队列登记等待，并立刻按新堆顶装填本地时钟。
pub(crate) fn register_wait_timeout(expires_at: u64, thread: Arc<Thread>, epoch: u64) {
    let owner_slot = hart::current().slot();
    let mut timers = timers().lock();
    assert!(
        timers.bind_owner(owner_slot),
        "timer queue bound to the wrong hart"
    );
    // SAFETY: Thread 在稳定 Arc 中；载荷强持 Thread 到彻底摘链，持 owner 队列锁。
    // 唯一执行 owner 在前一轮 detach 后才允许复用出生内嵌节点。
    unsafe {
        timers.register(
            core::pin::Pin::new_unchecked(&thread.timer),
            expires_at,
            TimerDelivery {
                thread: thread.clone(),
                epoch,
            },
        );
    }
    drop(timers);
    arm_or_pend();
}

/// 由任意完成 hart 注销 timeout。只移除 owner queue 项，不重编程远端
/// timer；最多引起一次提前中断，owner 会在下一装填点按堆顶恢复。
pub(crate) fn unregister_wait_timeout(thread: &Thread, epoch: u64) {
    if let Some(owner) = thread.timer.owner_slot() {
        let removed = {
            let mut queue = HART_TIMERS[owner].lock();
            // SAFETY: detach 持 Thread 执行 owner，选到相应队列锁后复检 owner。
            unsafe {
                queue.cancel_if(core::pin::Pin::new_unchecked(&thread.timer), |delivery| {
                    delivery.epoch == epoch
                })
            }
        };
        drop(removed);
    }
}

/// 调度量子同样从公共时钟几何取得，不能截断平台频率。
const QUANTUM_NS: u64 = 10_000_000;

/// 每次服务至多处理的到期项数（qT）。待定标政策参数，见维护执行分案；
/// 剩余到期项留作本 hart pending，由调度循环下一轮继续服务。
const TIMER_SERVICE_BATCH: usize = 16;

/// 本 hart 存在已到期但尚未服务的期限项。只由所属 hart 读写；参与
/// run/idle/arm 决策：有 pending 时不 WFI，不把已过期堆顶装填进硬件。
static HART_TIMER_PENDING: [AtomicBool; hart::HART_NUM_LIMIT] =
    [const { AtomicBool::new(false) }; hart::HART_NUM_LIMIT];

#[inline]
fn timer_pending() -> &'static AtomicBool {
    &HART_TIMER_PENDING[hart::current().slot()]
}

/// 统一装填入口：堆顶已到期 → 标 pending（不装填，避免中断风暴）；
/// 堆顶在未来 → 返回该期限供装填；队列空返回 None。
fn earliest_future() -> Option<u64> {
    let earliest = timers().lock().peek_expires_at()?;
    match crate::clock::now_ticks() {
        Ok(now) if earliest <= now => {
            timer_pending().store(true, Ordering::Relaxed);
            None
        }
        _ => Some(earliest),
    }
}

/// 把本 hart 定时器设到最早的未来到期点；已到期项转为 pending。
fn arm_or_pend() {
    if let Some(expires_at) = earliest_future() {
        sbi::require(sbi::set_timer(expires_at), "TIME.set_timer");
    }
}

/// 有界服务本 hart 到期项：至多 qT 项，锁内彻底摘链后交出强 Thread/epoch，
/// 锁外只发重试提示，不写 Timeout 结果。仍有
/// 到期项时保留 pending，由调度循环下一轮继续，不在此排空。
fn service_expired() {
    let pending = timer_pending();
    pending.store(false, Ordering::Relaxed);
    let Ok(now) = crate::clock::now_ticks() else {
        crate::runtime_stop::check();
        return;
    };
    for _ in 0..TIMER_SERVICE_BATCH {
        let due = timers().lock().pop_expired(now);
        let Some(delivery) = due else { return };
        crate::task::wait_source::HintTarget::hint(&*delivery.thread, delivery.epoch);
    }
    if timers()
        .lock()
        .peek_expires_at()
        .is_some_and(|at| at <= now)
    {
        pending.store(true, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// 入口：enqueue（新就绪 / 唤醒）与定时器事件
// ---------------------------------------------------------------------------

/// 线程入队并按门铃唤醒其所属域的空闲 hart（IPI = 他方请求，见
/// notes/impls/internals.md）。线程只在所属域的类队列出现。
pub fn enqueue(t: AdmittedThread) {
    let domain = t.domain;
    domain.enqueue_fair(t);
    domain.wake_one();
}

/// timer trap（量子耗尽或 sleep 到期）：卸载 → 唤醒到期；当前线程由
/// trap 出口 Requeue 轮转。
pub fn on_timer() {
    sbi::require(sbi::set_timer(DISARM), "TIME.set_timer");
    service_expired();
}

/// 维护队列门铃：提醒全部域的空闲 hart（合并、失败均安全：idle 前复检）。
pub(crate) fn wake_idle_harts() {
    let table = domains();
    for domain in table.domains.iter().flatten() {
        domain.wake_one();
    }
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
        crate::runtime_stop::check();
        // 内核 IRQ 关闭，持续 M 下硬件 STIP 不会进入 trap。
        // 每安全点复检期限电平，未来项到期不依赖已有软件 pending 或先 idle。
        let _ = earliest_future();
        if timer_pending().load(Ordering::Relaxed) {
            service_expired();
        }
        // idle 唤醒、门铃合并或先前 IPI 失败后，Pending 槽仍由安全点补消费。
        crate::vm::remote::drain_current(crate::vm::remote::SAFE_POINT_BATCH);
        // 非 Resume 出口已在汇编边界归一（kernel satp + 本地全量
        // SFENCE.VMA）：循环体结构性只运行于内核页表下。
        // 用户/维护轮转仲裁：维护类每次至多消耗一份轮转预算。
        match crate::maintenance::service(me_domain.has_ready()) {
            maintenance::rotor::Choice::Run {
                class: maintenance::rotor::Class::User,
                ..
            } => {}
            maintenance::rotor::Choice::Idle => {
                idle();
                continue;
            }
            // 维护批或欠账仲裁之后回到安全点，不能顺带执行未获准的 U。
            _ => continue,
        };
        let _user = crate::maintenance::begin_dispatch();
        let Some(mut t) = me_domain.pick() else {
            // 选择落败仍是已授予的 U 尝试；scope 先结费，再由下轮仲裁决定 idle。
            continue;
        };
        // eligibility 纵深防御：线程只能在其绑定域的 hart 上运行
        // （结构上 pick 只达本域队列，断言兜底域路由接线错误）。
        debug_assert!(
            core::ptr::eq(t.domain, me_domain),
            "thread must run in its bound domain"
        );
        // Shell 运行准入与 VM 入口分别仲裁；两锁不嵌套。
        let Some(mut running) = t.process.admit_running() else {
            reap(t);
            continue;
        };
        let entered = t
            .process
            .lease_receipt()
            .is_some_and(|body| body.members.enter_running(t.member(), me.slot()));
        if !entered {
            drop(running);
            reap(t);
            continue;
        }
        // 续体尚在内核页表，先归还运行准入再移交资格；任何重新发布之后
        // 都不触现场。完成者在下一段重新过 Shell/VM 门，不向用户态循环复检。
        if crate::task::wait::has_entry(&t) {
            drop(running);
            let Some(resumed) = crate::task::wait::resume(t, me.slot()) else {
                continue;
            };
            t = resumed;
            let Some(permit) = t.process.admit_running() else {
                reap(t);
                continue;
            };
            running = permit;
        }
        if !running.enter(me.slot()) {
            drop(running);
            reap(t);
            continue;
        }
        me.set_context(t.frame_ptr(), t.satp(), Arc::as_ptr(&t), t.uses_fp());
        // 先扣本次续体/准入已用时间，硬量子按同一 U 余额的剩余值装填。
        crate::maintenance::end_user();
        crate::maintenance::begin_user();
        arm_quantum(crate::maintenance::user_deadline());
        // 指令流同步由 AddressSpace instruction epoch 的远端请求，或
        // `_ret_to_user` 的统一地址空间切换出口负责；调度器不再重复执行 fence.i。
        // SAFETY: 执行点已装好（帧/satp/线程），tp 不变量成立。
        let outcome = unsafe { trap::ret_to_user() };
        me.clear_context();
        HART_QUANTUM_DEADLINE[me.slot()].store(0, Ordering::Relaxed);
        // trap 的终止吸收可能把已登记等待的 Park 改判为 Killed。每个
        // Switch 出口都先取走意图；Killed 放弃未安装计划，不遗留给下一线程。
        let wait_plan = HART_WAIT_PLANS[me.slot()].lock().take();
        // 非-Resume 出口的归一（内核 satp + 全量 SFENCE.VMA）已由汇编
        // 出口边界完成：active 位图与后续 teardown 不得在目标地址空间
        // 上进行。
        let slot = me.slot();
        running.leave();
        match outcome {
            Outcome::Requeue => {
                assert!(wait_plan.is_none(), "Requeue outcome carries a wait intent");
                let transition = t
                    .process
                    .lease_receipt()
                    .map(|body| body.members.requeue(&t.process, t.member(), slot));
                match transition {
                    Some(WakeTransition::Publish(ingress)) => {
                        ingress.publish_admitted(t);
                    }
                    Some(WakeTransition::Cancelled) | None => reap(t),
                }
            }
            Outcome::Killed => {
                drop(wait_plan);
                reap(t);
            }
            // 已离开执行点，此刻发布等待：完成方可安全触达该线程。
            Outcome::Park => {
                let plan = wait_plan.expect("Park outcome must carry a wait intent");
                crate::task::wait::install(t, plan);
            }
            Outcome::Resume => unreachable!("Resume never passes through the scheduling loop"),
        }
    }
}

// 只由所属 hart 读写；0 表示无用户现场，正的线程量子必产生非零未来期限。
// Resume 复用绝对期限，不能因重装硬件 timer 而重新发线程时间片。
static HART_QUANTUM_DEADLINE: [AtomicU64; hart::HART_NUM_LIMIT] =
    [const { AtomicU64::new(0) }; hart::HART_NUM_LIMIT];

/// 新 dispatch 的量子装填；期限随本次用户现场持有到 Switch。
fn arm_quantum(class_deadline: u64) {
    let quantum = match crate::clock::after_ns(QUANTUM_NS) {
        Ok(quantum) => quantum,
        Err(_) => {
            crate::runtime_stop::request();
            crate::runtime_stop::check();
            unreachable!("runtime stop did not park after clock epoch exhaustion");
        }
    };
    HART_QUANTUM_DEADLINE[hart::current().slot()].store(quantum, Ordering::Relaxed);
    arm_user_until(quantum, class_deadline);
}

fn quantum_deadline() -> u64 {
    let quantum = HART_QUANTUM_DEADLINE[hart::current().slot()].load(Ordering::Relaxed);
    assert!(quantum != 0, "user quantum accessed outside active context");
    quantum
}

pub(crate) fn quantum_expired() -> bool {
    let quantum = quantum_deadline();
    match crate::clock::now_ticks() {
        Ok(now) => now >= quantum,
        Err(_) => {
            crate::runtime_stop::request();
            crate::runtime_stop::check();
            unreachable!("runtime stop returned after clock epoch exhaustion");
        }
    }
}

/// Resume 重装保持本次线程绝对期限，类别使用原轮费用余额。
pub(crate) fn rearm_quantum(class_deadline: u64) {
    arm_user_until(quantum_deadline(), class_deadline);
}

/// 线程、类别与期限表取最早者，已过期来源由既有 pending 路径处理。
fn arm_user_until(quantum: u64, class_deadline: u64) {
    // 已到期堆顶不装填（转 pending，由调度循环服务）；量子本身即后续检查机会。
    let earliest = earliest_future();
    sbi::require(
        sbi::set_timer(earliest.unwrap_or(quantum).min(quantum).min(class_deadline)),
        "TIME.set_timer",
    );
}

/// 退出 owner 先经正常 detach 摘节点，再移交 Departure；回执不持 Thread。
/// committed Map 结果义务可延后成员摘除与 DONE，但不保留 Thread/UserContext。
pub(crate) fn reap(t: AdmittedThread) {
    crate::task::wait::detach(&t);
    t.finish_created_departure();
    drop(t);
}

/// idle：在本域登记空闲位 → 双重检查就绪工作 → 按期限表 arm（无期限则卸载）
/// → wfi。醒来（SIE=0，不trap）先吸收停止事实，再清门铃并复检待办。
fn idle() {
    crate::runtime_stop::check();
    let domain = current_domain();
    let bit = 1u64 << hart::current().slot();
    domain.idle_mask.fetch_or(bit, Ordering::SeqCst);
    // 入队与登记 idle 的交错由双重检查闭合；work debt 的 Pending 电平同样
    // 禁止 owner 带债入睡，即使对应门铃曾失败或被合并。
    if domain.has_ready()
        || timer_pending().load(Ordering::Relaxed)
        || crate::maintenance::has_ready()
        || crate::vm::remote::has_current()
    {
        domain.idle_mask.fetch_and(!bit, Ordering::SeqCst);
        return;
    }

    match earliest_future() {
        Some(at) => sbi::require(sbi::set_timer(at), "TIME.set_timer"),
        None if timer_pending().load(Ordering::Relaxed) => {
            domain.idle_mask.fetch_and(!bit, Ordering::SeqCst);
            return;
        }
        None => sbi::require(sbi::set_timer(DISARM), "TIME.set_timer"),
    };
    // SAFETY: wfi 等待局部使能的中断 pending 唤醒。
    unsafe { asm!("wfi", options(nomem, preserves_flags)) };
    crate::runtime_stop::check();
    sbi::clear_ssip();
    domain.idle_mask.fetch_and(!bit, Ordering::SeqCst);

    if sip_stip_pending() {
        // 期限到达唤醒（回主循环前把到期线程入队）。
        sbi::require(sbi::set_timer(DISARM), "TIME.set_timer");
        service_expired();
    }
}

/// 本 hart 时钟中断是否 pending（idle 醒来后查询用）。
fn sip_stip_pending() -> bool {
    let sip: usize;
    // SAFETY: 只读 sip。
    unsafe { asm!("csrr {}, sip", out(reg) sip, options(nomem, preserves_flags)) };
    sip & (1 << 5) != 0
}
