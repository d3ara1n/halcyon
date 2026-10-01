//! 内核维护执行：已提交特权责任的无栈工作记录、全局队列与每 hart 轮转。
//!
//! 设计见 plans/todo-2026-09-29-kernel-maintenance-execution.md。
//! - 拥有体内嵌 [`WorkRecord`]，实现 [`Work::step`]；队列在 Queued/Running/
//!   Blocked 期间强持 `Arc<dyn Work>`，Done 后锁外 drop。
//! - 账户（资金账户 = Pool）内嵌 [`Account`]：`maintenance::Ledger` 与 Ready
//!   记录链；有 Ready 记录的账户挂在全局账户环上。
//! - 锁：全局一把 `ranks::MAINTENANCE` ticket 锁，叶锁，持有时不取其他锁、
//!   不分配（挂接全部为侵入式链接）。记录状态、链接与账本只在该锁下访问。
//! - 依赖：源锁内只摘 [`WakeToken`]，解源锁后 [`wake`]。

use alloc::sync::Arc;
use core::cell::UnsafeCell;

use maintenance::{
    account::{Grant, Ledger, Visit},
    record::{Action, Outcome, Record, WaitTicket},
    rotor::{Choice, Class, Rotor},
};

use crate::{hart, sync::TicketSpinlock};

/// 维护类每次仲裁的 DRR 量子与单次 grant 上限（qM，待定标政策参数）。
const ACCOUNT_QUANTUM: u64 = 1_000;
const MAX_GRANT: u64 = 1_000;
/// 用户类/维护类轮转量子（qU/qM，ticks；待定标政策参数）。
const USER_QUANTUM: u64 = 100_000;
const MAINTENANCE_QUANTUM: u64 = 20_000;

/// 拥有体的有界推进步。`grant` 为本步预算；返回实际成本与结果。
/// step 在任何锁之外调用，内部可取业务锁；不得递归 step 其他记录，
/// 级联只 publish 子记录。
pub(crate) trait Work: Send + Sync {
    fn record(&self) -> &WorkRecord;
    fn step(&self, grant: u64) -> (u64, Outcome);
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
    /// Running/Blocked 期间的强根（Queued 时由前驱 next 或账户 head 持有）。
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

    pub(crate) fn account(&self) -> &Arc<Account> {
        &self.account
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
}

struct AccountInner {
    ledger: Ledger,
    head: Option<Arc<dyn Work>>,
    /// 尾指针仅作定位；强根由前驱 next/head 持有。
    tail: *const dyn Work,
    /// 账户环后继；挂环期间由前驱持有强根。
    ring_next: Option<Arc<Account>>,
    on_ring: bool,
}

// SAFETY: inner 只在全局 MAINTENANCE 锁下访问。
unsafe impl Sync for Account {}
unsafe impl Send for Account {}

impl Account {
    pub(crate) fn new() -> Self {
        Self {
            inner: UnsafeCell::new(AccountInner {
                ledger: Ledger::new(),
                head: None,
                tail: core::ptr::null::<Placeholder>() as *const dyn Work,
                ring_next: None,
                on_ring: false,
            }),
        }
    }

    /// # Safety
    /// 调用者持有全局 MAINTENANCE 锁。
    #[allow(clippy::mut_from_ref)]
    unsafe fn inner(&self) -> &mut AccountInner {
        // SAFETY: 由调用者持锁保证独占。
        unsafe { &mut *self.inner.get() }
    }

    /// 子账户销毁时把剩余负 deficit 并入本账户（独立 debt transfer：不调用
    /// settle、不动 in_flight、不伪造 grant）。调用者保证 `child` 已无任何
    /// 在途记录。
    pub(crate) fn absorb_debt(&self, child: &Account) {
        let _q = QUEUE.lock();
        // SAFETY: 持全局锁。
        let (me, ch) = unsafe { (self.inner(), child.inner()) };
        assert!(ch.ledger.is_quiescent() && ch.head.is_none(), "debt transfer from active account");
        me.ledger.absorb_debt(&mut ch.ledger);
    }
}

/// 空尾指针的类型占位（只用于构造 null 胖指针，从不解引用）。
struct Placeholder;
impl Work for Placeholder {
    fn record(&self) -> &WorkRecord {
        unreachable!()
    }
    fn step(&self, _: u64) -> (u64, Outcome) {
        unreachable!()
    }
}

/// 全局账户环：轮转游标指向下一个被访问的账户。
struct Queue {
    cursor: Option<Arc<Account>>,
    ring_len: usize,
    /// 记录总数（Queued），供 idle/轮转快速判定。
    ready: usize,
}

static QUEUE: TicketSpinlock<Queue> = TicketSpinlock::new(
    crate::sync::ranks::MAINTENANCE,
    Queue {
        cursor: None,
        ring_len: 0,
        ready: 0,
    },
);

/// 依赖完成方持有的唤醒凭证；源锁内摘取，解源锁后 [`wake`]。
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

/// 发布记录：Idle → Queued。可在业务锁内调用（MAINTENANCE 为叶锁）；
/// 门铃由调用者在解开全部锁后经 [`kick`] 发送。
pub(crate) fn publish(work: Arc<dyn Work>) {
    let mut q = QUEUE.lock();
    // SAFETY: 持全局锁。
    let action = unsafe { work.record().inner() }
        .state
        .publish()
        .expect("publish of a non-idle work record");
    debug_assert_eq!(action, Action::Link);
    link(&mut q, work);
}

/// 依赖完成：按凭证代次唤醒；陈旧或重复凭证无效。凭证在锁外 drop。
pub(crate) fn wake(token: WakeToken) {
    let WakeToken { work, ticket } = token;
    let mut q = QUEUE.lock();
    // SAFETY: 持全局锁。
    let inner = unsafe { work.record().inner() };
    if inner.state.wake(ticket) == Action::Link {
        let root = inner.root.take().expect("blocked record without root");
        link(&mut q, root);
    }
    drop(q);
    drop(work);
}

/// 解开全部锁后提醒空闲 hart 有维护工作（合并、失败均安全：idle 前复检）。
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
    if !acc.on_ring {
        acc.on_ring = true;
        ring_insert(q, account);
    }
}

fn ring_insert(q: &mut Queue, account: Arc<Account>) {
    match q.cursor.take() {
        None => {
            // SAFETY: 持全局锁。
            unsafe { account.inner() }.ring_next = Some(account.clone());
            q.cursor = Some(account);
        }
        Some(cur) => {
            // 插在游标之后：新账户下一轮被访问，不插队于当前账户之前。
            // SAFETY: 持全局锁。
            let c = unsafe { cur.inner() };
            unsafe { account.inner() }.ring_next = c.ring_next.take();
            c.ring_next = Some(account);
            q.cursor = Some(cur);
        }
    }
    q.ring_len += 1;
}

/// 从环上摘除游标处账户（其 Ready 链已空），游标前移。
fn ring_remove_cursor(q: &mut Queue, prev: &Arc<Account>) {
    let cur = q.cursor.take().expect("ring cursor");
    // SAFETY: 持全局锁。
    let c = unsafe { cur.inner() };
    c.on_ring = false;
    c.ledger.idle();
    let next = c.ring_next.take().expect("ring link");
    q.ring_len -= 1;
    if q.ring_len == 0 {
        // 自环：next 即 cur 自身。
        drop(next);
        return;
    }
    // SAFETY: 持全局锁。
    unsafe { prev.inner() }.ring_next = Some(next.clone());
    q.cursor = Some(next);
}

/// 一次锁内领取：沿账户环做 DRR 访问，取得 grant 与记录。环长有限，
/// 每个账户每轮至少补一次量子，结构上至多绕环两圈即可领取或判定无可领。
fn pick(q: &mut Queue) -> Option<(Arc<dyn Work>, Grant)> {
    let bound = q.ring_len.saturating_mul(2) + 1;
    for _ in 0..bound {
        let cur = q.cursor.clone()?;
        // SAFETY: 持全局锁。
        let c = unsafe { cur.inner() };
        if c.head.is_none() {
            let prev = ring_prev(q, &cur);
            ring_remove_cursor(q, &prev);
            continue;
        }
        match c.ledger.visit(ACCOUNT_QUANTUM, MAX_GRANT) {
            Visit::Skip => {
                q.cursor = c.ring_next.clone();
            }
            Visit::Grant(grant) => {
                let work = c.head.take().expect("nonempty ready chain");
                // SAFETY: 持全局锁。
                let w = unsafe { work.record().inner() };
                c.head = w.next.take();
                if c.head.is_none() {
                    c.tail = core::ptr::null::<Placeholder>() as *const dyn Work;
                }
                w.state.pick().expect("queued record pick");
                q.ready -= 1;
                q.cursor = c.ring_next.clone();
                return Some((work, grant));
            }
        }
    }
    None
}

/// 环上前驱（环长有界；只在摘除空账户时调用）。
fn ring_prev(q: &Queue, target: &Arc<Account>) -> Arc<Account> {
    let mut p = target.clone();
    for _ in 0..q.ring_len {
        // SAFETY: 持全局锁。
        let n = unsafe { p.inner() }.ring_next.clone().expect("ring link");
        if Arc::ptr_eq(&n, target) {
            return p;
        }
        p = n;
    }
    unreachable!("account ring is not closed")
}

/// 执行一次维护推进：锁内 visit→pick，锁外 step，锁内 finish+settle；
/// Done 后锁外 drop 强根。返回实际成本（0 表示无可领）。
fn advance_one() -> u64 {
    let Some((work, grant)) = pick(&mut QUEUE.lock()) else {
        return 0;
    };
    let (cost, outcome) = work.step(grant.ticks());
    let mut q = QUEUE.lock();
    // SAFETY: 持全局锁。
    let acc = unsafe { work.record().account.inner() };
    acc.ledger.settle(grant, cost);
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
    drop(retired);
    cost.max(1)
}

/// 每 hart 用户/维护轮转器（hart 私有，无锁）。
struct HartRotor(UnsafeCell<Option<Rotor>>);
// SAFETY: 每项只被所属 hart 在调度循环/trap 返回边界访问。
unsafe impl Sync for HartRotor {}

static ROTORS: [HartRotor; hart::HART_NUM_LIMIT] =
    [const { HartRotor(UnsafeCell::new(None)) }; hart::HART_NUM_LIMIT];

fn rotor() -> &'static mut Rotor {
    // SAFETY: 本 hart 私有槽，调用点不重入（模块契约）。
    let slot = unsafe { &mut *ROTORS[hart::current().slot()].0.get() };
    slot.get_or_insert_with(|| Rotor::new(USER_QUANTUM, MAINTENANCE_QUANTUM))
}

/// 调度循环安全点（离场处理后、pick 前）：按轮转器仲裁并推进维护。
/// 维护类每次运行至多消耗一份轮转预算，之后回到循环顶重新仲裁。
pub(crate) fn service(user_ready: bool) {
    let maint_ready = has_ready();
    let r = rotor();
    if let Choice::Run {
        class: Class::Maintenance,
        budget,
    } = r.choose(user_ready, maint_ready)
    {
        let mut spent = 0u64;
        while spent < budget {
            let cost = advance_one();
            if cost == 0 {
                break;
            }
            spent = spent.saturating_add(cost);
        }
        r.charge(Class::Maintenance, spent);
    }
}

/// trap 返回边界结算用户类实际占用；额度耗尽且维护可运行时返回 true，
/// 调用方转 Requeue。
pub(crate) fn charge_user(elapsed: u64) -> bool {
    let r = rotor();
    r.charge(Class::User, elapsed);
    r.balance(Class::User) <= 0 && has_ready()
}
