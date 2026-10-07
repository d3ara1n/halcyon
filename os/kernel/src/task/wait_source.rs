//! 等待来源的内核承载：纯电平链在 capability，来源的业务锁保护电平/游标。
//! 每个实际来源 core 出生内嵌本承载，实现 Work 并在锁外发送 Hint。

use alloc::sync::{Arc, Weak};
use capability::signal;
use core::{cell::UnsafeCell, marker::PhantomPinned, pin::Pin};
use erhino_shared::object::ObjectSignals;

use crate::{
    maintenance::{self as mt, Account, Work, WorkRecord},
    sync::{Spinlock, ranks},
};

/// 目标知识属于等待领域；提示不能提交 syscall 结果或摘来源节点。
pub trait HintTarget: Send + Sync {
    fn hint(&self, identity: u64);
}

pub struct Hint {
    target: Weak<dyn HintTarget>,
    identity: u64,
}

impl Hint {
    pub(crate) fn new(target: Weak<dyn HintTarget>, identity: u64) -> Self {
        Self { target, identity }
    }
}

/// 来源锁内复制出的在途凭证，不含节点引用；锁外最多发一次重试提示。
pub(crate) struct Delivery {
    target: Arc<dyn HintTarget>,
    identity: u64,
}

impl Delivery {
    pub(crate) fn send(self) {
        self.target.hint(self.identity);
    }
}

pub(crate) enum Step {
    Idle,
    Progress,
    Hint(Delivery),
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub(crate) signals: ObjectSignals,
    pub(crate) closed: bool,
}

pub type WaitNode = signal::Node<Hint>;

/// 节点 owner 强持这份观察存储，唯一释放入口是 detach，不能由来源代摘。
pub trait ObservationSource: Send + Sync {
    fn snapshot(&self) -> Snapshot;

    /// # Safety
    /// node 保持 pin/存活直到本 source.detach 返回；target 是 Weak，
    /// 节点 owner 保持本来源的强引用，所有节点操作由其同一执行 owner 仲裁。
    unsafe fn attach(&self, node: Pin<&WaitNode>, interest: ObjectSignals, hint: Hint);

    /// # Safety
    /// 节点来自本 source；正常返回与退出路径经同一 detach 入口结算一次。
    unsafe fn detach(&self, node: Pin<&WaitNode>) -> Hint;
}

/// 来源 core 只分配一次；record 在队列锁下可访问，chain 仅在业务锁下访问。
pub(crate) struct WaitQueue {
    record: WorkRecord,
    chain: UnsafeCell<signal::Source<Hint>>,
    _pin: PhantomPinned,
}

// SAFETY: 实际来源 core 的业务锁保护全部 chain 访问；record 由维护队列锁保护。
unsafe impl Send for WaitQueue {}
unsafe impl Sync for WaitQueue {}

impl WaitQueue {
    pub(crate) fn new(account: Arc<Account>) -> Self {
        Self {
            record: WorkRecord::new(account),
            chain: UnsafeCell::new(signal::Source::new()),
            _pin: PhantomPinned,
        }
    }

    pub(crate) fn record(&self) -> &WorkRecord {
        &self.record
    }

    /// # Safety
    /// 来源 core 已处于稳定 Arc 地址，调用者持它的业务锁。
    unsafe fn with_chain<R>(
        &self,
        action: impl for<'a> FnOnce(Pin<&'a mut signal::Source<Hint>>) -> R,
    ) -> R {
        // SAFETY: 稳定存储、来源同步域独占，HRTB 排除 chain 的可变引用逃逸。
        unsafe { action(Pin::new_unchecked(&mut *self.chain.get())) }
    }

    /// # Safety
    /// 同 chain 契约；返回 true 后 caller 在本记录的来源 Work 发布活动。
    pub(crate) unsafe fn update(&self, signals: ObjectSignals, closed: bool) -> bool {
        unsafe { self.with_chain(|chain| chain.update(signals.raw(), closed)) }
    }

    /// # Safety
    /// 同 chain 契约；snapshot 不接触任何 target。
    pub(crate) unsafe fn snapshot(&self) -> Snapshot {
        let chain = unsafe { &*self.chain.get() };
        Snapshot {
            signals: ObjectSignals::from_raw(chain.level()),
            closed: chain.is_closed(),
        }
    }

    /// # Safety
    /// 同来源同步域与 ObservationSource::attach 的节点存储契约。
    pub(crate) unsafe fn attach(&self, node: Pin<&WaitNode>, interest: ObjectSignals, hint: Hint) {
        unsafe {
            self.with_chain(|chain| chain.insert(node, interest.raw(), hint));
        }
    }

    /// # Safety
    /// 同来源同步域，节点必须来自本 queue；返回 target 凭据锁外析构。
    pub(crate) unsafe fn detach(&self, node: Pin<&WaitNode>) -> Hint {
        unsafe { self.with_chain(|chain| chain.remove(node)) }
    }

    /// # Safety
    /// 持来源业务锁；upgrade 在锁内完成，Delivery 送出前必须解锁。
    pub(crate) unsafe fn step(&self) -> Step {
        // SAFETY: chain 的短借用仅存续在本 closure；仅导出 Hint target 的强引用。
        unsafe {
            self.with_chain(|chain| match chain.step() {
                signal::Scan::Idle => Step::Idle,
                signal::Scan::Skipped | signal::Scan::Boundary => Step::Progress,
                signal::Scan::Candidate(hint) => match hint.target.upgrade() {
                    Some(target) => Step::Hint(Delivery {
                        target,
                        identity: hint.identity,
                    }),
                    None => Step::Progress,
                },
            })
        }
    }
}

/// 仅有电平发布而没有消息缓冲/Take 的观察存储。业务终态发布到这里后，
/// 该同步域唯一拥有公开电平与节点链；不回调业务对象或持其私有 Body。
struct SignalState {
    internal_done: bool,
    waiter: Option<mt::WakeToken>,
}

pub(crate) struct SignalSource {
    queue: WaitQueue,
    /// 单一内部完成消费者；用户态观察仍使用出生节点链。
    state: Spinlock<SignalState>,
    _metadata: super::resources::MetadataCharge,
}

impl SignalSource {
    pub(crate) fn try_new(
        pool: Arc<super::memory_pool::MemoryPool>,
    ) -> Result<Arc<Self>, erhino_shared::call::SystemCallError> {
        let metadata = super::resources::MetadataCharge::arc::<Self>(&pool)?;
        let source = Arc::try_new(Self {
            queue: WaitQueue::new(pool.maintenance_account().clone()),
            state: Spinlock::new(
                ranks::OBJECT_WAIT,
                SignalState {
                    internal_done: false,
                    waiter: None,
                },
            ),
            _metadata: metadata,
        })
        .map_err(|_| erhino_shared::call::SystemCallError::OutOfMemory)?;
        Ok(source)
    }

    /// 调用者已结束业务事实的仲裁；不持高于 OBJECT_WAIT 的业务锁。
    pub(crate) fn publish(self: &Arc<Self>, signals: ObjectSignals, closed: bool) {
        let (activate, woke, retired) = {
            let mut guard = self.state.lock();
            // SAFETY: 来源稳定存储、本来源同步域；电平与链同域提交。
            let activate = unsafe { self.queue.update(signals, closed) };
            let wake = if signals.contains(ObjectSignals::DONE) {
                guard.waiter.take()
            } else {
                None
            };
            let woke = wake.is_some();
            // DONE仍在来源门内，等待者不能先读新电平、finish并释放执行根。
            // wake只提交队列状态/归还非最后引用，潜在退休根移到门外销毁。
            let retired = wake.and_then(mt::wake);
            (activate, woke, retired)
        };
        drop(retired);
        if woke {
            mt::kick();
        }
        if activate {
            let root: Arc<dyn Work> = self.clone();
            mt::publish(root);
            mt::kick();
        }
    }

    /// 内部唯一完成等待者先 arm 再复检，来源终态与登记共用同一同步域。
    pub(crate) fn wait_done(&self, token: mt::WakeToken) -> Option<mt::WakeToken> {
        let mut guard = self.state.lock();
        // SAFETY: 持来源锁，快照与终态发布同域。
        if guard.internal_done
            || unsafe { self.queue.snapshot() }
                .signals
                .contains(ObjectSignals::DONE)
        {
            return Some(token);
        }
        assert!(
            guard.waiter.is_none(),
            "duplicate internal completion waiter"
        );
        guard.waiter = Some(token);
        None
    }

    /// 内部退款里程碑与公开电平分别提交；复用持久观察宿主，不提前公开业务 DONE。
    pub(crate) fn complete_internal(&self) {
        let (woke, retired) = {
            let mut guard = self.state.lock();
            assert!(!guard.internal_done, "internal completion published twice");
            guard.internal_done = true;
            let wake = guard.waiter.take();
            (wake.is_some(), wake.and_then(mt::wake))
        };
        drop(retired);
        if woke {
            mt::kick();
        }
    }
}

impl ObservationSource for SignalSource {
    fn snapshot(&self) -> Snapshot {
        let _guard = self.state.lock();
        // SAFETY: 来源锁保护电平与 chain。
        unsafe { self.queue.snapshot() }
    }

    unsafe fn attach(&self, node: Pin<&WaitNode>, interest: ObjectSignals, hint: Hint) {
        let _guard = self.state.lock();
        // SAFETY: 调用者保证 node/loan 寿命，来源锁串行所有链操作。
        unsafe { self.queue.attach(node, interest, hint) }
    }

    unsafe fn detach(&self, node: Pin<&WaitNode>) -> Hint {
        let _guard = self.state.lock();
        // SAFETY: 同 attach 契约，返回 Hint 不含节点引用，调用者锁外释放。
        unsafe { self.queue.detach(node) }
    }
}

impl Work for SignalSource {
    fn record(&self) -> &WorkRecord {
        self.queue.record()
    }

    fn step(self: Arc<Self>) -> maintenance::record::Outcome {
        let step = {
            let _guard = self.state.lock();
            // SAFETY: 来源域内只检查一个候选并复制在途凭证。
            unsafe { self.queue.step() }
        };
        match step {
            Step::Idle => maintenance::record::Outcome::Idle,
            Step::Progress => maintenance::record::Outcome::Ready,
            Step::Hint(delivery) => {
                delivery.send();
                maintenance::record::Outcome::Ready
            }
        }
    }
}
