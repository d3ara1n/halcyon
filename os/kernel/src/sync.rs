//! 锁原语（见 notes/impls/internals.md「锁原语」、notes/impls/task.md
//! 「锁序契约」）。
//!
//! - [`RawTicketLock`]：唯一 FIFO 自旋原语；本地 IRQ 屏蔽与严格锁序
//!   配合，等待者不能被新请求无限插队。
//! - [`RankedRawSpinlock`]：带类型级锁序秩的原语，实现 `lock_api::RawMutex`
//!   供 talc 等库构造路径注入（TalcLock 控制实例化，秩无法以实例状态
//!   携带）；trait 路径也关闭 IRQ 并参与 ladder 断言。
//! - [`Spinlock<T>`]：内核自用容器，获取期间额外关闭本地中断——
//!   中断处理路径若触碰本 hart 持有的锁会同核死锁，关中断是正确性
//!   要求；并以实例级 rank 与链段 key 参与 Lock Ladder 断言。
//!
//! **Lock Ladder**（debug 构建）：per-hart 秩栈断言锁序单调——新秩须
//! 大于栈顶，或同秩且链段 key 严格递增（Job 链锁 = jid、HandleTable
//! caller→child = pid）；违规即 panic（经 RawWriter，不依赖堆与锁）。
//! release 构建零开销。秩分配表是唯一真值：[`ranks`]。

use core::{
    cell::UnsafeCell,
    fmt,
    ops::{Deref, DerefMut},
    sync::atomic::{AtomicU32, Ordering},
};

use crate::csr::SSTATUS_SIE;

/// 锁序秩：越小越外层。分配表与全部嵌套边见 notes/impls/task.md
/// 「锁序契约」。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Rank(pub u32);

/// 锁序秩分配。常量即契约——调整只改此处，全内核不出现裸数字。
pub mod ranks {
    use super::Rank;

    /// 收束批次仲裁：一次性覆盖最广（JobInner/lifecycle/对象/表/空间），
    /// 恒最先获取。
    pub const DRAIN_GATE: Rank = Rank(0);
    /// HandleTable 收束游标。
    pub const DRAIN_CURSOR: Rank = Rank(10);
    /// HandleTable：caller→child 嵌套沿 pid 递增（chained）。
    pub const HANDLE_TABLE: Rank = Rank(100);
    /// 无嵌套边的自由段：CONSOLE、REGISTRY、ROOT anchor、就绪队列、
    /// per-hart 期限表及 Thread 等待 epoch 的注册节点。
    pub const LEAF: Rank = Rank(150);
    /// Job 链锁：先父后子沿 jid 递增（chained），≤32 把同持。
    pub const JOB_INNER: Rank = Rank(200);
    /// Mailbox 状态锁（含电平与队列）。
    pub const MAILBOX: Rank = Rank(210);
    /// Tunnel Connection 状态锁。
    pub const CONNECTION: Rank = Rank(220);
    /// MemoryPool 四项额度状态；父子归还逐锁完成，不嵌套两把 Pool 锁。
    pub const MEMORY_POOL: Rank = Rank(230);
    /// MemoryObject 可执行状态与 affine WritePermit 账目。
    pub const MEMORY_OBJECT: Rank = Rank(250);
    /// 用户地址空间（页表树/帧/外部映射/drain 阶段）。
    pub const ADDRESS_SPACE: Rank = Rank(300);
    /// 空间的实物 View/写许可索引；从 VM 向 backing 单向获取。
    pub const VM_VIEWS: Rank = Rank(325);
    /// backing 段索引；空间解析可取此锁，帧/资金退款在释放两锁后进行。
    pub const BACKING: Rank = Rank(350);
    /// Notification 状态锁（唯一以 space 为外层的对象锁边）。
    pub const NOTIFICATION: Rank = Rank(400);
    /// 对象电平/壳锁：Job.wait、ProcessControl、Endpoint/Invitation、
    /// ProcessBuilder、Process.control 回指槽。
    pub const OBJECT_WAIT: Rank = Rank(500);
    /// Shell 准入/回执真值；可在 Job 链锁内进入，不嵌套 VM 锁。
    pub const LIFECYCLE: Rank = Rank(600);
    /// Body 线程稳定槽：出生登记允许 Shell → Members 单向锁边。
    pub const PROCESS_MEMBERS: Rank = Rank(610);
    /// 授权对象最后消散的单向寿命发布，不反向访问业务容器。
    /// 持久就绪集合；来源和此目标状态锁不得嵌套。
    pub const WAIT_SET: Rank = Rank(550);
    /// Commit 后完成槽：只在 lifecycle gate 内填充一次，完成方锁外取走后
    /// 才进入 AddressSpace 收束。
    /// Commit 后特权 work debt 的固定槽；取出后才进入业务容器锁。
    pub const WORK_DEBT: Rank = Rank(640);
    /// Remote Call 固定槽；Commit 可在 AddressSpace → lifecycle 内发布。
    /// 目标执行与完成回调均在锁外。
    pub const REMOTE_CALL: Rank = Rank(650);
    /// 维护执行全局队列（ticket 锁）：业务/对象锁之后的内层叶锁。持有期间
    /// 不取任何其他锁（含 HEAP，挂接零分配）；IPI 在解开全部锁后发送。
    pub const MAINTENANCE: Rank = Rank(660);
    /// talc 堆锁（RankedRawSpinlock 类型级注入；几乎被全部容器锁内
    /// 获取，故置顶）。
    pub const HEAP: Rank = Rank(900);
    /// 物理隔离的系统票据；talc Source 在 HEAP 锁内只做 O(1) 消费。
    pub const SYSTEM_SUPPLY: Rank = Rank(925);
    /// 物理帧池（HEAP 与空间锁的内层）。
    pub const POOL: Rank = Rank(950);
}

/// Lock Ladder：per-hart 锁序栈（debug 构建）。帧只被所属 hart 在
/// Spinlock 关中断语义的临界区内经 tp 访问，自身无锁；跨 hart 无共享
/// 访问。
#[cfg(debug_assertions)]
pub(crate) mod ladder {
    use super::Rank;
    use crate::hart;
    use core::cell::{Cell, UnsafeCell};
    use core::sync::atomic::{AtomicBool, Ordering};

    /// 栈深上界：Job 链锁（≤32）加表/空间/堆/池若干级。
    const DEPTH: usize = 64;

    struct Frame {
        depth: Cell<usize>,
        ranks: UnsafeCell<[Rank; DEPTH]>,
        keys: UnsafeCell<[u64; DEPTH]>,
    }

    struct Ladder {
        frames: [Frame; hart::HART_NUM_LIMIT],
    }

    // SAFETY: 每帧只被所属 hart 在关中断（Spinlock 语义：获取即清
    // sstatus.SIE）的临界区内经 tp 访问，协作式内核下不存在同 hart
    // 嵌套中断；ladder 自身不取锁，跨 hart 访问互相独立。
    unsafe impl Sync for Ladder {}

    /// bootstrap 专用帧：formal entry 之前 tp 尚未建立（cold boot 是
    /// 独立临时环境，见 impls/execution-context.md），该阶段单核运行，
    /// 锁序断言照常工作于专用帧。
    struct BootstrapLadder {
        frame: Frame,
    }

    // SAFETY: 仅在单核 bootstrap 阶段（TP_READY 为假）被 boot hart 访问。
    unsafe impl Sync for BootstrapLadder {}

    static BOOTSTRAP: BootstrapLadder = BootstrapLadder {
        frame: Frame {
            depth: Cell::new(0),
            ranks: UnsafeCell::new([Rank(0); DEPTH]),
            keys: UnsafeCell::new([0; DEPTH]),
        },
    };

    /// tp 就绪标志：各 hart 的 formal entry 汇合点发布。此后 tp ≡
    /// HART_LOCALS 内本 hart 槽位（违例由 tp_slot 的断言暴露）。
    static TP_READY: AtomicBool = AtomicBool::new(false);

    /// formal entry 汇合点调用：宣布本 hart tp 已装配，ladder 切换至
    /// per-hart 帧。Release 语义：此后的任何 hart 在 Acquire 读取后
    /// 必见完整的帧数组。
    pub fn mark_tp_ready() {
        TP_READY.store(true, Ordering::Release);
    }

    static LADDER: Ladder = Ladder {
        frames: [const {
            Frame {
                depth: Cell::new(0),
                ranks: UnsafeCell::new([Rank(0); DEPTH]),
                keys: UnsafeCell::new([0; DEPTH]),
            }
        }; hart::HART_NUM_LIMIT],
    };

    fn frame() -> &'static Frame {
        if !TP_READY.load(Ordering::Acquire) {
            // 单核 bootstrap 阶段独占访问（BOOTSTRAP 的 Sync 论证）。
            return &BOOTSTRAP.frame;
        }
        &LADDER.frames[hart::tp_slot()]
    }

    /// 压栈并断言：新秩须大于栈顶，或同秩且双方链段 key 非零并严格
    /// 递增（链式锁沿创建序向下获取）。
    pub fn push(rank: Rank, key: u64, caller: Option<&core::panic::Location<'_>>) {
        let f = frame();
        let depth = f.depth.get();
        assert!(depth < DEPTH, "lock ladder overflow");
        // SAFETY: 关中断下独占访问。
        let (ranks, keys) = unsafe { (&mut *f.ranks.get(), &mut *f.keys.get()) };
        if depth > 0 {
            let (top, top_key) = (ranks[depth - 1], keys[depth - 1]);
            let chained = key != 0 && top_key != 0 && key > top_key;
            assert!(
                rank > top || (rank == top && chained),
                "lock-order violation: acquiring rank {rank:?} (key {key}) under rank {top:?} (key {top_key}); requested at {caller:?}",
            );
        }
        ranks[depth] = rank;
        keys[depth] = key;
        f.depth.set(depth + 1);
    }

    /// 弹栈，与 push 严格配对（Guard 的 drop / RawMutex unlock 负责）。
    pub fn pop() {
        let f = frame();
        let depth = f.depth.get();
        debug_assert!(depth > 0, "lock ladder underflow");
        f.depth.set(depth - 1);
    }
}

#[cfg(not(debug_assertions))]
pub(crate) mod ladder {
    use super::Rank;

    #[inline(always)]
    pub fn mark_tp_ready() {}

    #[inline(always)]
    pub fn push(_rank: Rank, _key: u64, _caller: Option<&core::panic::Location<'_>>) {}

    #[inline(always)]
    pub fn pop() {}
}

/// 获取前的本地中断状态。
#[must_use]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct InterruptState {
    sie_was_enabled: bool,
}

/// 关闭本地中断（sstatus.SIE 清零），返回此前的状态。
pub fn disable_interrupts() -> InterruptState {
    // SAFETY: 仅读写 sstatus.SIE 位；S 态特权指令。
    let old: usize;
    unsafe {
        core::arch::asm!(
            "csrrc {old}, sstatus, {mask}",
            old = out(reg) old,
            mask = in(reg) SSTATUS_SIE,
            options(nomem)
        )
    }
    InterruptState {
        sie_was_enabled: old & SSTATUS_SIE != 0,
    }
}

/// 恢复本地中断到 `state` 记录的状态。
pub fn restore_interrupts(state: InterruptState) {
    if state.sie_was_enabled {
        // SAFETY: 仅置回 sstatus.SIE 位。
        unsafe {
            core::arch::asm!(
                "csrrs x0, sstatus, {mask}",
                mask = in(reg) SSTATUS_SIE,
                options(nomem)
            )
        }
    }
}

/// 原始互斥原语接口：[`Lock`] 容器只依赖获取/释放/尝试三项，中断与 ladder
/// 语义由容器统一承担。
pub trait RawLock {
    /// 自旋直到获取。
    fn acquire(&self);
    /// 释放。仅允许持有者调用。
    fn release(&self);
    /// 尝试获取，成功返回 true。
    fn try_lock(&self) -> bool;
}

/// FIFO ticket 原语：`next` 发号、`serving` 叫号。等锁上界：获取期间关本地
/// 中断且协作式内核每 hart 至多一个待获取请求，故至多排在 H−1 个短临界区
/// 之后（维护执行分案）。计数按 u32 回绕：同时在途票数 ≤ H 远小于 2^32，
/// 相等比较不受回绕影响。
pub struct RawTicketLock {
    next: AtomicU32,
    serving: AtomicU32,
}

impl RawTicketLock {
    pub const fn new() -> Self {
        Self {
            next: AtomicU32::new(0),
            serving: AtomicU32::new(0),
        }
    }
}

impl RawLock for RawTicketLock {
    fn acquire(&self) {
        let ticket = self.next.fetch_add(1, Ordering::Relaxed);
        while self.serving.load(Ordering::Acquire) != ticket {
            core::hint::spin_loop();
        }
    }

    fn release(&self) {
        // 仅持有者写 serving，读改写无竞争；Release 发布临界区写入。
        let current = self.serving.load(Ordering::Relaxed);
        self.serving
            .store(current.wrapping_add(1), Ordering::Release);
    }

    fn try_lock(&self) -> bool {
        // 只在无人排队时取号：next == serving 时 CAS next 前进一格。
        // 获取前 owner 对 serving 的 Release；next 的 CAS 只预留票号，
        // 不能替代对临界区写入的 Acquire。
        let serving = self.serving.load(Ordering::Acquire);
        self.next
            .compare_exchange(
                serving,
                serving.wrapping_add(1),
                Ordering::Acquire,
                Ordering::Relaxed,
            )
            .is_ok()
    }
}

/// 带类型级锁序秩的自旋锁：实现 `lock_api::RawMutex`，供 talc 等库
/// 构造路径注入（锁实例由库控制构造，秩只能走 const 泛型）。trait
/// 路径参与 ladder 断言。
pub struct RankedRawSpinlock<const RANK: u32> {
    raw: RawTicketLock,
    interrupt: UnsafeCell<InterruptState>,
}

// SAFETY: interrupt 只由当前票的持有者读写；等待者先关本地 IRQ，
// 同 hart 不重入；RawMutex GuardNoSend 保证在原 hart 归还。
unsafe impl<const RANK: u32> Sync for RankedRawSpinlock<RANK> {}

impl<const RANK: u32> RankedRawSpinlock<RANK> {
    pub const fn new() -> Self {
        Self {
            raw: RawTicketLock::new(),
            interrupt: UnsafeCell::new(InterruptState {
                sie_was_enabled: false,
            }),
        }
    }
}

// SAFETY: FIFO 票提供唯一 owner，Acquire/Release 发布数据，trait 的
// unlock 仅由持票 owner 调用；中断状态由同一 owner 保存/恢复。
unsafe impl<const RANK: u32> lock_api::RawMutex for RankedRawSpinlock<RANK> {
    const INIT: Self = Self::new();
    type GuardMarker = lock_api::GuardNoSend;

    #[inline]
    fn lock(&self) {
        let state = disable_interrupts();
        self.raw.acquire();
        // SAFETY: 已取当前票，仅持有者可访问中断保存槽。
        unsafe { *self.interrupt.get() = state };
        ladder::push(Rank(RANK), 0, None);
    }

    #[inline]
    fn try_lock(&self) -> bool {
        let state = disable_interrupts();
        if self.raw.try_lock() {
            // SAFETY: 取票成功，本 hart 唯一持有。
            unsafe { *self.interrupt.get() = state };
            ladder::push(Rank(RANK), 0, None);
            true
        } else {
            restore_interrupts(state);
            false
        }
    }

    #[inline]
    unsafe fn unlock(&self) {
        // SAFETY: trait 要求当前 owner；此槽在发放下一票前读取完成。
        let state = unsafe { *self.interrupt.get() };
        ladder::pop();
        self.raw.release();
        restore_interrupts(state);
    }
}

/// 内核锁容器：获取期间关本地中断，并以实例级 rank 参与 ladder 断言；
/// 互斥原语由 `R` 决定。
pub struct Lock<R: RawLock, T> {
    raw: R,
    rank: Rank,
    chain_key: u64,
    data: UnsafeCell<T>,
}

/// 内核共享同步统一 FIFO；链式 rank 与 IRQ 规则由容器拥有。
pub type Spinlock<T> = Lock<RawTicketLock, T>;
pub type SpinlockGuard<'a, T> = LockGuard<'a, RawTicketLock, T>;

/// SAFETY: 数据仅能在持有锁（本 hart 关中断 + 互斥）时访问。
unsafe impl<R: RawLock + Sync, T: Send> Sync for Lock<R, T> {}
unsafe impl<R: RawLock + Send, T: Send> Send for Lock<R, T> {}

/// [`Lock`] 的守卫，drop 时弹出 ladder、释放锁并恢复中断状态。
pub struct LockGuard<'a, R: RawLock, T> {
    lock: &'a Lock<R, T>,
    state: InterruptState,
}

impl<T> Lock<RawTicketLock, T> {
    pub const fn new(rank: Rank, value: T) -> Self {
        Self::with_raw(RawTicketLock::new(), rank, 0, value)
    }

    /// 链式锁：同秩多把沿 `chain_key` 严格递增获取（Job 链锁 = jid、
    /// HandleTable caller→child = pid）。非链式锁（key 为零）同秩连持
    /// 即断言违规。
    pub const fn chained(rank: Rank, chain_key: u64, value: T) -> Self {
        Self::with_raw(RawTicketLock::new(), rank, chain_key, value)
    }
}

impl<R: RawLock, T> Lock<R, T> {
    const fn with_raw(raw: R, rank: Rank, chain_key: u64, value: T) -> Self {
        Self {
            raw,
            rank,
            chain_key,
            data: UnsafeCell::new(value),
        }
    }

    /// 持有容器唯一借用时直接访问数据，不需要加锁。
    pub fn get_mut(&mut self) -> &mut T {
        // SAFETY: `&mut self` 保证不存在任何并发守卫或其它访问者。
        unsafe { &mut *self.data.get() }
    }

    /// 获取锁：关本地中断后等待，成功即压入 ladder。
    #[track_caller]
    pub fn lock(&self) -> LockGuard<'_, R, T> {
        let state = disable_interrupts();
        self.raw.acquire();
        ladder::push(
            self.rank,
            self.chain_key,
            Some(core::panic::Location::caller()),
        );
        LockGuard { lock: self, state }
    }

    /// 尝试获取锁，失败返回 `None`（同样关中断，失败立即恢复）。
    #[track_caller]
    pub fn try_lock(&self) -> Option<LockGuard<'_, R, T>> {
        let state = disable_interrupts();
        if self.raw.try_lock() {
            ladder::push(
                self.rank,
                self.chain_key,
                Some(core::panic::Location::caller()),
            );
            Some(LockGuard { lock: self, state })
        } else {
            restore_interrupts(state);
            None
        }
    }
}

impl<R: RawLock, T> Deref for LockGuard<'_, R, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: 持有锁，独占访问。
        unsafe { &*self.lock.data.get() }
    }
}

impl<R: RawLock, T> DerefMut for LockGuard<'_, R, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: 持有锁，独占访问。
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<R: RawLock, T> Drop for LockGuard<'_, R, T> {
    fn drop(&mut self) {
        ladder::pop();
        self.lock.raw.release();
        restore_interrupts(self.state);
    }
}

impl<R: RawLock, T: fmt::Debug> fmt::Debug for Lock<R, T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.try_lock() {
            Some(guard) => guard.fmt(f),
            None => f.write_str("<locked>"),
        }
    }
}
