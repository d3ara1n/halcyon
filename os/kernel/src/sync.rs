//! 锁原语（见 notes/impls/internals.md「锁原语」、notes/impls/task.md
//! 「锁序契约」）。
//!
//! - [`RawSpinlock`]：CAS 自旋原语（无中断语义、无 ladder），是
//!   [`Spinlock`] 与 [`RankedRawSpinlock`] 的内部实现。
//! - [`RankedRawSpinlock`]：带类型级锁序秩的原语，实现 `lock_api::RawMutex`
//!   供 talc 等库构造路径注入（TalcLock 控制实例化，秩无法以实例状态
//!   携带）；trait 路径参与 ladder 断言。
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
    /// per-hart 期限表、WaitContext 的 thread/registrations。
    pub const LEAF: Rank = Rank(150);
    /// Job 链锁：先父后子沿 jid 递增（chained），≤32 把同持。
    pub const JOB_INNER: Rank = Rank(200);
    /// Mailbox 状态锁（含电平与队列）。
    pub const MAILBOX: Rank = Rank(210);
    /// Tunnel Connection 状态锁。
    pub const CONNECTION: Rank = Rank(220);
    /// MemoryObject 可执行状态与 affine WritePermit 账目。
    pub const MEMORY_OBJECT: Rank = Rank(250);
    /// 用户地址空间（页表树/帧/外部映射/drain 阶段）。
    pub const ADDRESS_SPACE: Rank = Rank(300);
    /// Notification 状态锁（唯一以 space 为外层的对象锁边）。
    pub const NOTIFICATION: Rank = Rank(400);
    /// 对象电平/壳锁：Job.wait、ProcessControl、Endpoint/Invitation、
    /// ProcessBuilder、Process.control 回指槽。
    pub const OBJECT_WAIT: Rank = Rank(500);
    /// 进程生命周期顶级锁（从不出游；被链锁/对象壳在锁内进入）。
    pub const LIFECYCLE: Rank = Rank(600);
    /// Commit 后完成槽：只在 lifecycle gate 内填充一次，完成方锁外取走后
    /// 才进入 AddressSpace 收束。
    pub const MEMORY_COMPLETION: Rank = Rank(625);
    /// Remote Call 固定槽；Commit 可在 AddressSpace → lifecycle 内发布。
    /// 目标执行与完成回调均在锁外。
    pub const REMOTE_CALL: Rank = Rank(650);
    /// talc 堆锁（RankedRawSpinlock 类型级注入；几乎被全部容器锁内
    /// 获取，故置顶）。
    pub const HEAP: Rank = Rank(900);
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

/// CAS 自旋原语（无中断语义、无 ladder）。内存序由原子语义背书：
/// acquire 成功路径 Acquire、release Release，跨核临界区可见性不依赖
/// 手写栅栏。
pub struct RawSpinlock {
    locked: AtomicU32,
}

impl RawSpinlock {
    pub const fn new() -> Self {
        Self {
            locked: AtomicU32::new(0),
        }
    }

    /// 自旋直到获取。
    fn acquire(&self) {
        while self
            .locked
            .compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
    }

    /// 释放。仅允许持有者调用。
    fn release(&self) {
        self.locked.store(0, Ordering::Release);
    }

    /// 尝试获取，成功返回 true。
    fn try_lock(&self) -> bool {
        self.locked
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
    }
}

/// 带类型级锁序秩的自旋锁：实现 `lock_api::RawMutex`，供 talc 等库
/// 构造路径注入（锁实例由库控制构造，秩只能走 const 泛型）。trait
/// 路径参与 ladder 断言。
pub struct RankedRawSpinlock<const RANK: u32> {
    raw: RawSpinlock,
}

impl<const RANK: u32> RankedRawSpinlock<RANK> {
    pub const fn new() -> Self {
        Self {
            raw: RawSpinlock::new(),
        }
    }
}

unsafe impl<const RANK: u32> lock_api::RawMutex for RankedRawSpinlock<RANK> {
    const INIT: Self = Self::new();
    type GuardMarker = lock_api::GuardNoSend;

    #[inline]
    fn lock(&self) {
        self.raw.acquire();
        ladder::push(Rank(RANK), 0, None);
    }

    #[inline]
    fn try_lock(&self) -> bool {
        if self.raw.try_lock() {
            ladder::push(Rank(RANK), 0, None);
            true
        } else {
            false
        }
    }

    #[inline]
    unsafe fn unlock(&self) {
        ladder::pop();
        self.raw.release();
    }
}

/// 内核自旋锁容器：获取期间关本地中断，并以实例级 rank 参与 ladder
/// 断言。
pub struct Spinlock<T> {
    raw: RawSpinlock,
    rank: Rank,
    chain_key: u64,
    data: UnsafeCell<T>,
}

/// SAFETY: 数据仅能在持有锁（本 hart 关中断 + 自旋互斥）时访问。
unsafe impl<T: Send> Sync for Spinlock<T> {}
unsafe impl<T: Send> Send for Spinlock<T> {}

/// [`Spinlock`] 的守卫，drop 时弹出 ladder、释放锁并恢复中断状态。
pub struct SpinlockGuard<'a, T> {
    lock: &'a Spinlock<T>,
    state: InterruptState,
}

impl<T> Spinlock<T> {
    pub const fn new(rank: Rank, value: T) -> Self {
        Self {
            raw: RawSpinlock::new(),
            rank,
            chain_key: 0,
            data: UnsafeCell::new(value),
        }
    }

    /// 链式锁：同秩多把沿 `chain_key` 严格递增获取（Job 链锁 = jid、
    /// HandleTable caller→child = pid）。非链式锁（key 为零）同秩连持
    /// 即断言违规。
    pub const fn chained(rank: Rank, chain_key: u64, value: T) -> Self {
        Self {
            raw: RawSpinlock::new(),
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

    /// 获取锁：关本地中断后自旋等待，成功即压入 ladder。
    #[track_caller]
    pub fn lock(&self) -> SpinlockGuard<'_, T> {
        let state = disable_interrupts();
        self.raw.acquire();
        ladder::push(
            self.rank,
            self.chain_key,
            Some(core::panic::Location::caller()),
        );
        SpinlockGuard { lock: self, state }
    }

    /// 尝试获取锁，失败返回 `None`（同样关中断，drop 时恢复）。
    #[track_caller]
    pub fn try_lock(&self) -> Option<SpinlockGuard<'_, T>> {
        let state = disable_interrupts();
        if self.raw.try_lock() {
            ladder::push(
                self.rank,
                self.chain_key,
                Some(core::panic::Location::caller()),
            );
            Some(SpinlockGuard { lock: self, state })
        } else {
            restore_interrupts(state);
            None
        }
    }
}

impl<T> Deref for SpinlockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: 持有锁，独占访问。
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinlockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: 持有锁，独占访问。
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinlockGuard<'_, T> {
    fn drop(&mut self) {
        ladder::pop();
        self.lock.raw.release();
        restore_interrupts(self.state);
    }
}

impl<T: fmt::Debug> fmt::Debug for Spinlock<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.try_lock() {
            Some(guard) => guard.fmt(f),
            None => f.write_str("<locked>"),
        }
    }
}
