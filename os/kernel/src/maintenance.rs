//! 内核维护执行：已提交特权责任的无栈工作记录、全局队列与每 hart 轮转。
//!
//! 设计见 plans/todo-2026-09-29-kernel-maintenance-execution.md。
//! - 拥有体内嵌 [`WorkRecord`]，实现 [`Work::step`]；队列在 Queued/Running/
//!   Blocked 期间强持 `Arc<dyn Work>`，Done 后锁外 drop。
//! - 账户（资金账户 = Pool）内嵌 [`Account`]：`maintenance::Ledger` 与 Ready
//!   记录链；有 Ready 记录的账户挂在全局账户环上。
//! - 锁：全局一把 `ranks::MAINTENANCE` ticket 锁，叶锁，持有时不取其他锁、
//!   不分配（挂接全部为侵入式链接）。记录状态、链接与账本只在该锁下访问。
//! - 依赖：来源门内可用[`wake`]提交状态；返回的潜在最后根带出全部门后销毁，
//!   [`kick`]也在门外，不在来源/队列门内运行业务或Hint回调。

use alloc::sync::{Arc, Weak};
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU64, Ordering};

use maintenance::{
    account::{Grant, Ledger, Visit},
    record::{Action, Outcome, Record, State, WaitTicket},
    rotor::{Choice, Class, Rotor},
};

use crate::{hart, sync::Spinlock};

/// 每账户每轮信用，单次预扣另按 hart 上限分割；不是领域执行上界。
const ACCOUNT_QUANTUM_NS: u64 = 100_000;
/// 用户/维护时间政策，初始化时按平台唯一 Geometry 换算为 ticks。
const USER_QUANTUM_NS: u64 = 10_000_000;
const MAINTENANCE_QUANTUM_NS: u64 = 2_000_000;

/// 拥有体的一次有界原子推进。实际时间由执行器测量，领域只返回状态。
/// step 在任何锁之外调用，内部可取业务锁；不得递归 step 其他记录，
/// 级联只 publish 子记录。
pub(crate) trait Work: Send + Sync {
    /// 编译器拥有的静态名字引用，仅供费用诊断，不保活执行宿主。
    fn cost_name(&self) -> &'static &'static str {
        &const { core::any::type_name::<Self>() }
    }
    /// 只返回地址稳定的内嵌记录，不取业务锁、不分配、不回调；可在QUEUE门内调用。
    fn record(&self) -> &WorkRecord;
    fn step(self: Arc<Self>) -> Outcome;
}

/// 内嵌于拥有体的工作记录：状态机 + 侵入式 Ready 链接 + 所属账户。
pub(crate) struct WorkRecord {
    account: Arc<Account>,
    inner: UnsafeCell<RecordInner>,
}

struct RecordInner {
    state: Record,
    /// Ready 链后继；同时是 Queued 期间的强根。
    next: Option<Arc<dyn Work>>,
    /// Blocked 期间的强根；Running 根由执行器局部持有。
    root: Option<Arc<dyn Work>>,
}

// SAFETY: inner 只在全局 MAINTENANCE 锁下访问（模块契约）。
unsafe impl Sync for WorkRecord {}
unsafe impl Send for WorkRecord {}

impl WorkRecord {
    /// 出生即存在；准入由拥有体在创建时完成，记录本身零额外分配。
    pub(crate) fn new(account: Arc<Account>) -> Self {
        Self {
            account,
            inner: UnsafeCell::new(RecordInner {
                state: Record::new(),
                next: None,
                root: None,
            }),
        }
    }

    /// # Safety
    /// 调用者持有全局 MAINTENANCE 锁。
    #[allow(clippy::mut_from_ref)]
    unsafe fn inner(&self) -> &mut RecordInner {
        // SAFETY: 由调用者持锁保证独占。
        unsafe { &mut *self.inner.get() }
    }
}

/// 资金账户的维护部分：DRR 账本与 Ready 记录链（侵入式，FIFO）。
pub(crate) struct Account {
    inner: UnsafeCell<AccountInner>,
    /// 维护欠账跟随账户寿命，晚于最后 Work 析构与在途 grant 结算。
    parent: Option<Arc<Account>>,
    /// 子账户独立寿命由父 Pool 付款；root 计入 boot 固定布局。
    metadata: Option<crate::task::resources::MetadataCharge>,
}

struct AccountInner {
    ledger: Ledger,
    head: Option<Arc<dyn Work>>,
    /// 尾指针仅作定位；强根由前驱 next/head 持有。
    tail: *const dyn Work,
    /// 账户环后继；挂环期间由前驱持有强根。
    ring_next: Option<Arc<Account>>,
    /// 弱前驱供 O(1) 任意节点摘环；强存活仍由 next/cursor/Work 提供。
    ring_prev: Weak<Account>,
    ring: Option<usize>,
}

// SAFETY: inner 只在全局 MAINTENANCE 锁下访问。
unsafe impl Sync for Account {}
unsafe impl Send for Account {}

impl Account {
    pub(crate) fn root() -> Self {
        Self::new(None, None)
    }

    pub(crate) fn funded(metadata: crate::task::resources::MetadataCharge) -> Self {
        let parent = metadata.payer().maintenance_account().clone();
        Self::new(Some(parent), Some(metadata))
    }

    fn new(
        parent: Option<Arc<Account>>,
        metadata: Option<crate::task::resources::MetadataCharge>,
    ) -> Self {
        Self {
            inner: UnsafeCell::new(AccountInner {
                ledger: Ledger::new(),
                head: None,
                tail: core::ptr::null::<Placeholder>() as *const dyn Work,
                ring_next: None,
                ring_prev: Weak::new(),
                ring: None,
            }),
            parent,
            metadata,
        }
    }

    /// # Safety
    /// 调用者持有全局 MAINTENANCE 锁。
    #[allow(clippy::mut_from_ref)]
    unsafe fn inner(&self) -> &mut AccountInner {
        // SAFETY: 由调用者持锁保证独占。
        unsafe { &mut *self.inner.get() }
    }
}

impl Drop for Account {
    fn drop(&mut self) {
        let state = self.inner.get_mut();
        assert!(
            state.ledger.is_quiescent() && state.head.is_none() && state.ring.is_none(),
            "finalizing active Account"
        );
        // 父 owner 阻止 metadata 返还触发付款 Pool 销毁时嵌套析构其账户；
        // 各祖先在本层结算结束后逐层进入自己的清理区间。
        let Some(parent) = self.parent.take() else {
            assert!(
                self.metadata.is_none(),
                "root Account carried child funding"
            );
            return;
        };
        let started = crate::clock::now_ticks().ok();
        let costs = &COSTS[hart::current().slot()];
        let in_maintenance = costs.in_maintenance.load(Ordering::Relaxed);
        drop(self.metadata.take());
        let actual = {
            let _q = QUEUE.lock();
            // SAFETY: 父账本共享状态在 QUEUE 下访问；子已有唯一析构借用。
            let ledger = unsafe { &mut parent.inner().ledger };
            ledger.absorb_debt(&mut state.ledger);
            let actual = started
                .zip(crate::clock::now_ticks().ok())
                .map(|(start, end)| end.saturating_sub(start));
            if let Some(actual) = actual.filter(|_| in_maintenance) {
                ledger.charge(actual);
            }
            actual
        };
        if let Some(actual) = actual {
            if in_maintenance {
                costs.cleanup_ticks.fetch_add(actual, Ordering::Relaxed);
            }
            record_max(&costs.cleanup_max, actual);
        } else {
            // 仍收束退款/旧欠账；锁外请求既有停止，不伪造正常公平结算。
            crate::runtime_stop::request();
        }
        drop(parent);
    }
}

/// 空尾指针的类型占位（只用于构造 null 胖指针，从不解引用）。
struct Placeholder;
impl Work for Placeholder {
    fn record(&self) -> &WorkRecord {
        unreachable!()
    }
    fn step(self: Arc<Self>) -> Outcome {
        unreachable!()
    }
}

struct Ring {
    cursor: Option<Arc<Account>>,
    ring_len: usize,
}
/// 本轮必须全部结清才切环；新激活与已用完信用的账户只进下一轮。
struct Queue {
    rings: [Ring; 2],
    current: usize,
    /// 记录总数（Queued），供 idle/轮转快速判定。
    ready: usize,
}

static QUEUE: Spinlock<Queue> = Spinlock::new(
    crate::sync::ranks::MAINTENANCE,
    Queue {
        rings: [const {
            Ring {
                cursor: None,
                ring_len: 0,
            }
        }; 2],
        current: 0,
        ready: 0,
    },
);

/// 依赖完成方持有的唤醒凭证；来源门内可提交[`wake`]，潜在退休根必须门外释放。
pub(crate) struct WakeToken {
    work: Arc<dyn Work>,
    ticket: WaitTicket,
}

/// 记录在 step 内登记依赖：取得新等待代次的凭证，交给依赖源保存。
/// 随后若 step 返回 Blocked，早到的 wake 已由 notified 覆盖。
pub(crate) fn arm(work: &Arc<dyn Work>) -> WakeToken {
    let _q = QUEUE.lock();
    // SAFETY: 持全局锁。
    let inner = unsafe { work.record().inner() };
    let ticket = inner.state.arm().expect("arm outside Running");
    WakeToken {
        work: work.clone(),
        ticket,
    }
}

/// 激活记录：Idle 接管强根，Queued 合并，Running 只请求后续轮次。
/// 可在业务锁内调用（MAINTENANCE 为叶锁）；
/// 门铃由调用者在解开全部锁后经 [`kick`] 发送。
pub(crate) fn publish(work: Arc<dyn Work>) {
    let mut q = QUEUE.lock();
    // SAFETY: 持全局锁。
    let action = unsafe { work.record().inner() }
        .state
        .activate()
        .expect("activate of a blocked or terminal work record");
    if action == Action::Link {
        link(&mut q, work);
    } else {
        // Queued/Running的执行根只在同一锁下finish；此处Arc递减必非最后引用。
        // 先解锁会允许另一hart完成并释放根，把完整宿主析构带回通知调用栈。
        drop(work);
        drop(q);
    }
}

/// 依赖完成：活跃凭证在根受保护时归还。Idle/Done的潜在最后根交还调用者。
/// 可以在来源锁内提交状态；返回值必须带出全部领域锁后销毁。
#[must_use = "retired work roots must be dropped outside every domain lock"]
pub(crate) fn wake(token: WakeToken) -> Option<Arc<dyn Work>> {
    let WakeToken { work, ticket } = token;
    let mut q = QUEUE.lock();
    // SAFETY: 持全局锁。
    let inner = unsafe { work.record().inner() };
    let action = inner.state.wake(ticket);
    let active = matches!(
        inner.state.state(),
        State::Queued | State::Running { .. } | State::Blocked
    );
    if action == Action::Link {
        let root = inner.root.take().expect("blocked record without root");
        link(&mut q, root);
    }
    if active {
        // 当前执行根受QUEUE锁保护，非最后Arc Drop只做有界引用计数。
        drop(work);
        drop(q);
        None
    } else {
        // 不在这里运行潜在最后析构：调用者可能仍持来源锁。
        drop(q);
        Some(work)
    }
}

/// 解开全部锁后提醒空闲hart：合并由idle前复检闭合，发送失败进入runtime_stop。
pub(crate) fn kick() {
    crate::sched::wake_idle_harts();
}

/// 是否有可领取的维护记录（idle 复检与轮转仲裁用）。
pub(crate) fn has_ready() -> bool {
    QUEUE.lock().ready != 0
}

/// 挂入 Ready 链尾；账户首次有 Ready 记录时挂上账户环。
fn link(q: &mut Queue, work: Arc<dyn Work>) {
    let account = work.record().account.clone();
    // SAFETY: 持全局锁。
    let acc = unsafe { account.inner() };
    let raw = Arc::as_ptr(&work);
    if acc.head.is_none() {
        acc.head = Some(work);
    } else {
        // SAFETY: tail 指向链上记录，链上强根由前驱/head 持有；持全局锁。
        unsafe { (*acc.tail).record().inner() }.next = Some(work);
    }
    acc.tail = raw;
    q.ready += 1;
    if acc.ring.is_none() {
        ring_insert(q, q.current ^ 1, account);
    }
}

fn ring_insert(q: &mut Queue, index: usize, account: Arc<Account>) {
    // SAFETY: 持全局锁；已离环、没有旧链接。
    let state = unsafe { account.inner() };
    assert!(state.ring.is_none(), "account already linked");
    state.ring = Some(index);
    let ring = &mut q.rings[index];
    match ring.cursor.take() {
        None => {
            // SAFETY: 持全局锁。
            unsafe { account.inner() }.ring_next = Some(account.clone());
            unsafe { account.inner() }.ring_prev = Arc::downgrade(&account);
            ring.cursor = Some(account);
        }
        Some(cur) => {
            // 只插入指定环，下一轮身份由调用方确定。
            // SAFETY: 持全局锁。
            let next = unsafe { cur.inner() }.ring_next.take().expect("ring link");
            unsafe { account.inner() }.ring_prev = Arc::downgrade(&cur);
            unsafe { account.inner() }.ring_next = Some(next.clone());
            unsafe { next.inner() }.ring_prev = Arc::downgrade(&account);
            unsafe { cur.inner() }.ring_next = Some(account);
            ring.cursor = Some(cur);
        }
    }
    ring.ring_len += 1;
}

/// 已结清在途的账户 O(1) 结构摘链接；idle/end-round 由调用方决定。
fn ring_remove(q: &mut Queue, account: &Arc<Account>) -> (Arc<Account>, Option<Arc<Account>>) {
    let (index, next, previous) = {
        // SAFETY: 持全局锁，前驱在环上由强 next 保活。
        let c = unsafe { account.inner() };
        assert!(
            c.ring.is_some() && c.ledger.is_quiescent(),
            "active account unlinked"
        );
        let previous = c.ring_prev.upgrade().expect("ring predecessor");
        let index = c.ring.take().expect("account ring identity");
        c.ring_prev = Weak::new();
        (index, c.ring_next.take().expect("ring link"), previous)
    };
    let ring = &mut q.rings[index];
    ring.ring_len -= 1;
    if ring.ring_len == 0 {
        assert!(
            Arc::ptr_eq(&next, account),
            "singleton ring is not self-linked"
        );
        return (next, ring.cursor.take());
    }
    // SAFETY: 已结束本账户内层借用；非单元素时前驱/后继均不是本账户。
    let removed = unsafe { previous.inner() }
        .ring_next
        .replace(next.clone())
        .expect("ring predecessor link");
    assert!(
        Arc::ptr_eq(&removed, account),
        "ring predecessor lost target"
    );
    unsafe { next.inner() }.ring_prev = Arc::downgrade(&previous);
    let cursor = if ring
        .cursor
        .as_ref()
        .is_some_and(|cur| Arc::ptr_eq(cur, account))
    {
        ring.cursor.replace(next)
    } else {
        None
    };
    (removed, cursor)
}

type RetiredRoots = (Arc<Account>, Option<Arc<Account>>);
struct Pick {
    work: Option<(Arc<dyn Work>, Grant)>,
    retired: Option<RetiredRoots>,
}

/// 每次至多一个账户；下一轮只能在本轮空后开始，未结 grant 不能提前补信用。
fn pick(q: &mut Queue, quantum: u64) -> Pick {
    if q.rings[q.current].cursor.is_none() {
        q.current ^= 1;
    }
    let Some(cur) = q.rings[q.current].cursor.clone() else {
        return Pick {
            work: None,
            retired: None,
        };
    };
    // SAFETY: 持全局锁。
    let c = unsafe { cur.inner() };
    q.rings[q.current].cursor = c.ring_next.clone();
    if !c.ledger.round_started() {
        c.ledger.begin_round(quantum);
    }
    if c.head.is_none() {
        assert!(!c.ledger.is_quiescent(), "idle account remained on ring");
        return Pick {
            work: None,
            retired: None,
        };
    }
    // 给同一账户的多个 hart 留预扣空间；实际耗时可越额，完整入债。
    let cap = quantum.div_ceil(hart::HART_NUM_LIMIT as u64);
    let Visit::Grant(grant) = c.ledger.visit(cap) else {
        let retired = if c.ledger.round_complete() {
            c.ledger.finish_round();
            let roots = ring_remove(q, &cur);
            ring_insert(q, q.current ^ 1, cur);
            Some(roots)
        } else {
            None
        };
        return Pick {
            work: None,
            retired,
        };
    };
    let work = c.head.take().expect("nonempty ready chain");
    // SAFETY: 持全局锁。
    let w = unsafe { work.record().inner() };
    c.head = w.next.take();
    if c.head.is_none() {
        c.tail = core::ptr::null::<Placeholder>() as *const dyn Work;
    }
    w.state.pick().expect("queued record pick");
    q.ready -= 1;
    Pick {
        work: Some((work, grant)),
        retired: None,
    }
}

/// 锁内 visit→pick，锁外 step，锁内 finish，锁外析构记录，最后锁内结清账户。
/// 未领取时仍可能推进欠账；摘环及最终账户析构由外层 M 批计时覆盖。
fn advance_one(quantum: u64) -> bool {
    let Pick { work, retired } = pick(&mut QUEUE.lock(), quantum);
    // 移环保持新环强根；旧根仍统一在 QUEUE 外释放。
    drop(retired);
    let Some((work, grant)) = work else {
        return false;
    };
    let costs = &COSTS[hart::current().slot()];
    let cleanup_before = costs.cleanup_ticks.load(Ordering::Relaxed);
    let cost_name = work.cost_name();
    let started = execution_ticks();
    // 调用根由本执行原根保活，其析构不是最后引用；finish 后才释放执行原根。
    let outcome = work.clone().step();
    let account = work.record().account.clone();
    let mut q = QUEUE.lock();
    // SAFETY: 持全局锁；裸指针指向 `work` 内嵌记录，`work` 在本函数内存活，
    // 移入链/root 后由队列强持，分配不释放。
    let w: *mut RecordInner = unsafe { work.record().inner() };
    // SAFETY: 同上。
    let retired = match unsafe { (*w).state.finish(outcome) }.expect("finish outside Running") {
        Action::Link => {
            link(&mut q, work);
            None
        }
        Action::None => {
            // SAFETY: 同上。
            unsafe { (*w).root = Some(work) };
            None
        }
        Action::Retire => Some(work),
    };
    drop(q);
    // 本次 grant 尚未结清，账户仍在环或由 account 强持；记录析构可以发布其他工作。
    drop(retired);
    let removed = {
        let mut q = QUEUE.lock();
        // 时间覆盖本次等锁，但时钟失败必须先放锁再进入永久停驻。
        let ended = match crate::clock::now_ticks() {
            Ok(ended) => ended,
            Err(_) => {
                drop(q);
                crate::runtime_stop::request();
                crate::runtime_stop::check();
                unreachable!("runtime stop returned after clock epoch exhaustion");
            }
        };
        // SAFETY: 持全局锁；新一轮可已激活，但本 grant 的结算只修改账户账本。
        let acc = unsafe { account.inner() };
        let cleanup_after = costs.cleanup_ticks.load(Ordering::Relaxed);
        let elapsed = ended.saturating_sub(started);
        let separate = cleanup_after.wrapping_sub(cleanup_before);
        // 已归父的区间是外部测时内已结束且不重叠的子区间。
        let actual = elapsed
            .checked_sub(separate)
            .expect("Account cleanup metered twice");
        if actual == 0 {
            costs.zero_grants.fetch_add(1, Ordering::Relaxed);
        }
        if actual > costs.grant_max.load(Ordering::Relaxed) {
            costs.grant_max.store(actual, Ordering::Relaxed);
            costs
                .grant_name
                .store(core::ptr::from_ref(cost_name).cast_mut(), Ordering::Release);
        }
        acc.ledger.settle(grant, actual);
        if acc.head.is_none() && acc.ledger.is_quiescent() {
            acc.ledger.idle();
            Some(ring_remove(&mut q, &account))
        } else if acc.ledger.round_complete() {
            acc.ledger.finish_round();
            let roots = ring_remove(&mut q, &account);
            let next = q.current ^ 1;
            ring_insert(&mut q, next, account.clone());
            Some(roots)
        } else {
            None
        }
    };
    drop(removed);
    true
}

/// 每 hart 的类别余额与当前 U 计时区间；只由所属 hart 在非重入边界访问。
struct Execution {
    rotor: Rotor,
    user_started: Option<u64>,
    /// Geometry在本次boot内不可变，策略常量只在执行器出生时换算。
    account_quantum: u64,
}
struct HartRotor(UnsafeCell<Option<Execution>>);
// SAFETY: 每项只被所属 hart 在调度循环/trap 返回边界访问。
unsafe impl Sync for HartRotor {}

static ROTORS: [HartRotor; hart::HART_NUM_LIMIT] =
    [const { HartRotor(UnsafeCell::new(None)) }; hart::HART_NUM_LIMIT];

// 不重借 service 持有的 &mut Execution。只由所属 hart 累加；
// 回绕差要求一个 grant 区间短于完整 u64 计数周期。
struct HartCosts {
    in_maintenance: AtomicBool,
    cleanup_ticks: AtomicU64,
    cleanup_max: AtomicU64,
    grant_max: AtomicU64,
    grant_name: AtomicPtr<&'static str>,
    zero_grants: AtomicU64,
    atomic_max: AtomicU64,
}
static COSTS: [HartCosts; hart::HART_NUM_LIMIT] = [const {
    HartCosts {
        in_maintenance: AtomicBool::new(false),
        cleanup_ticks: AtomicU64::new(0),
        cleanup_max: AtomicU64::new(0),
        grant_max: AtomicU64::new(0),
        grant_name: AtomicPtr::new(core::ptr::null_mut()),
        zero_grants: AtomicU64::new(0),
        atomic_max: AtomicU64::new(0),
    }
}; hart::HART_NUM_LIMIT];

fn record_max(cell: &AtomicU64, actual: u64) {
    // 所属 hart 是唯一 writer，不用有潜在重试环的 compare_exchange；
    // 其他 hart 的退出诊断只作原子读取。
    cell.store(cell.load(Ordering::Relaxed).max(actual), Ordering::Relaxed);
}

/// 与既有 heap/资金退出诊断同行；观测最大值用于政策定标，不冒充 WCET。
pub(crate) fn log_costs() {
    let mut atomic_max = 0;
    let mut grant_max = 0;
    let mut cleanup_max = 0;
    let mut cleanup_ticks = 0u64;
    let mut zero_grants = 0u64;
    for (slot, costs) in COSTS.iter().enumerate() {
        atomic_max = atomic_max.max(costs.atomic_max.load(Ordering::Relaxed));
        grant_max = grant_max.max(costs.grant_max.load(Ordering::Relaxed));
        cleanup_max = cleanup_max.max(costs.cleanup_max.load(Ordering::Relaxed));
        cleanup_ticks = cleanup_ticks.saturating_add(costs.cleanup_ticks.load(Ordering::Relaxed));
        zero_grants = zero_grants.saturating_add(costs.zero_grants.load(Ordering::Relaxed));
        let name = costs.grant_name.load(Ordering::Acquire);
        if !name.is_null() {
            // SAFETY: 只发布编译器提升的 &'static str 静态对象地址；从不释放/修改。
            let name = unsafe { *name };
            log!(
                Task,
                "maintenance grant hart_slot {slot}: max {} owner {name}",
                costs.grant_max.load(Ordering::Relaxed)
            );
        }
    }
    log!(
        Task,
        "maintenance ticks: atomic_max {atomic_max} grant_max {grant_max} cleanup_max {cleanup_max} cleanup_total {cleanup_ticks} zero_grants {zero_grants}"
    );
}

fn execution() -> &'static mut Execution {
    // SAFETY: 本 hart 私有槽，调用点不重入（模块契约）。
    let slot = unsafe { &mut *ROTORS[hart::current().slot()].0.get() };
    slot.get_or_insert_with(|| Execution {
        rotor: Rotor::new(
            execution_duration(USER_QUANTUM_NS),
            execution_duration(MAINTENANCE_QUANTUM_NS),
        ),
        user_started: None,
        account_quantum: execution_duration(ACCOUNT_QUANTUM_NS),
    })
}

fn execution_ticks() -> u64 {
    crate::clock::now_ticks().unwrap_or_else(|_| {
        crate::runtime_stop::request();
        crate::runtime_stop::check();
        unreachable!("runtime stop returned after clock epoch exhaustion")
    })
}

fn execution_duration(ns: u64) -> u64 {
    crate::clock::duration_ticks(ns).unwrap_or_else(|_| {
        crate::runtime_stop::request();
        crate::runtime_stop::check();
        unreachable!("runtime stop returned after clock policy range failure")
    })
}

/// 用户现场与本次 syscall/离场使用同一类别余额；Resume 不补量子。
pub(crate) fn begin_user() {
    let state = execution();
    assert!(
        state.user_started.is_none(),
        "user accounting interval already open"
    );
    state.user_started = Some(execution_ticks());
}

/// 选定 U 类后的线程选择、续体、准入失败与最后释放归同一次 dispatch。
/// trap 可暂停/重开计时；此 scope 只保证各出口最终关闭，没有独占状态借用。
pub(crate) struct UserDispatchScope;
impl Drop for UserDispatchScope {
    fn drop(&mut self) {
        end_user();
    }
}
pub(crate) fn begin_dispatch() -> UserDispatchScope {
    begin_user();
    UserDispatchScope
}

/// 活动 U 段的剩余费用截止；装填本身也归此段，不从装填时刻延长余额。
pub(crate) fn user_deadline() -> u64 {
    let state = execution();
    // 调度入口/Resume 都先打开 U 段；此处不消费机会，也不重复结算费用。
    let started = state
        .user_started
        .expect("user deadline outside active accounting interval");
    started
        .checked_add(state.rotor.balance(Class::User).max(0) as u64)
        .unwrap_or_else(|| {
            crate::runtime_stop::request();
            crate::runtime_stop::check();
            unreachable!("runtime stop returned after clock epoch exhaustion")
        })
}

/// 紧急服务与调度仲裁前关闭区间；调用两次不重复扣账。
pub(crate) fn end_user() {
    let state = execution();
    if let Some(started) = state.user_started.take() {
        state
            .rotor
            .charge(Class::User, execution_ticks().saturating_sub(started));
    }
}

/// 下一次 Resume 领取原轮机会，不补费用/机会，也不重新开启计时段。
pub(crate) fn resume_user() -> bool {
    execution().rotor.resume(Class::User)
}

/// 调度循环安全点（离场处理后、pick 前）：按轮转器仲裁并推进维护。
/// 维护类每次运行至多消耗一份轮转预算，之后回到循环顶重新仲裁。
pub(crate) fn service(user_ready: bool) -> Choice {
    let maint_ready = has_ready();
    let state = execution();
    assert!(
        state.user_started.is_none(),
        "arbitration during user accounting interval"
    );
    let choice = state.rotor.choose(user_ready, maint_ready);
    if let Choice::Run {
        class: Class::Maintenance,
        ..
    } = choice
    {
        let costs = &COSTS[hart::current().slot()];
        // 只发布只读类别事实；Dtor 不借 ROTORS 的独占执行状态。
        costs.in_maintenance.store(true, Ordering::Relaxed);
        let started = execution_ticks();
        let _ = advance_one(state.account_quantum);
        let actual = execution_ticks().saturating_sub(started);
        record_max(&costs.atomic_max, actual);
        state.rotor.charge(Class::Maintenance, actual);
        costs.in_maintenance.store(false, Ordering::Relaxed);
    }
    choice
}
