//! 授权域 Lifetime：authority/use 与观察存储分离，双零才关闭。

use super::{
    memory_pool::MemoryPool,
    object::{KernelObject, ObjectHeader, ObjectKind, ObjectRef},
    wait_source::{Hint, ObservationSource, Snapshot, Step, WaitNode, WaitQueue},
};
use crate::{
    maintenance::{self as mt, Work, WorkRecord},
    sync::{Spinlock, ranks},
};
use alloc::sync::{Arc, Weak};
use core::{
    any::Any,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
};
use erhino_shared::{
    call::SystemCallError,
    object::{ObjectSignals, Rights},
};

struct State {
    authorities: usize,
    uses: usize,
    closed: bool,
}

pub struct Lifetime {
    holders: capability::Holders,
    header: ObjectHeader,
    observed: u64,
    state: Spinlock<State>,
    source: WaitQueue,
    this: Spinlock<Weak<Self>>,
    _metadata: super::resources::MetadataCharge,
}

/// 仿射业务授权；last capability 和未发布回滚共享一次归还。
pub struct LifetimeOwner {
    domain: Arc<Lifetime>,
    active: AtomicBool,
}

/// 接受消息后随 Message/Delivery 移动，普通观察不产生此 lease。
pub struct LifetimeUse {
    domain: Arc<Lifetime>,
}

impl LifetimeOwner {
    pub fn new(observed: u64, pool: Arc<MemoryPool>) -> Result<(Self, ObjectRef), SystemCallError> {
        let metadata = super::resources::MetadataCharge::arc::<Lifetime>(&pool)?;
        let domain = Arc::try_new(Lifetime {
            holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            observed,
            state: Spinlock::new(
                ranks::OBJECT_WAIT,
                State {
                    authorities: 1,
                    uses: 0,
                    closed: false,
                },
            ),
            source: WaitQueue::new(pool.maintenance_account().clone()),
            this: Spinlock::new(ranks::LEAF, Weak::new()),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        *domain.this.lock() = Arc::downgrade(&domain);
        Ok((
            Self {
                domain: domain.clone(),
                active: AtomicBool::new(true),
            },
            domain,
        ))
    }

    pub fn derive(&self) -> Result<Self, SystemCallError> {
        let mut state = self.domain.state.lock();
        // 调用者在表锁下接受派生授权，或独占持有消费 claim；域终结后绝不重新开放。
        if !self.active.load(Ordering::Acquire) || state.closed {
            return Err(SystemCallError::ObjectClosed);
        }
        state.authorities = state
            .authorities
            .checked_add(1)
            .ok_or(SystemCallError::ReachLimit)?;
        Ok(Self {
            domain: self.domain.clone(),
            active: AtomicBool::new(true),
        })
    }

    pub fn accept(&self) -> Result<LifetimeUse, SystemCallError> {
        let mut state = self.domain.state.lock();
        if !self.active.load(Ordering::Acquire) || state.closed {
            return Err(SystemCallError::ObjectClosed);
        }
        state.uses = state
            .uses
            .checked_add(1)
            .ok_or(SystemCallError::ReachLimit)?;
        Ok(LifetimeUse {
            domain: self.domain.clone(),
        })
    }

    pub fn release(&self) {
        if self.active.swap(false, Ordering::AcqRel) {
            self.domain.release(true);
        }
    }
}
impl Drop for LifetimeOwner {
    fn drop(&mut self) {
        self.release();
    }
}
impl Drop for LifetimeUse {
    fn drop(&mut self) {
        self.domain.release(false);
    }
}

impl Lifetime {
    fn release(self: &Arc<Self>, authority: bool) {
        let notify = {
            let mut state = self.state.lock();
            let count = if authority {
                &mut state.authorities
            } else {
                &mut state.uses
            };
            *count = count.checked_sub(1).expect("Lifetime lease underflow");
            if state.authorities == 0 && state.uses == 0 {
                assert!(!state.closed, "Lifetime terminal published twice");
                state.closed = true;
                // SAFETY: 此来源 Arc 稳定，计数/公开电平/节点链共用 state 锁。
                unsafe { self.source.update(ObjectSignals::CLOSED, true) }
            } else {
                false
            }
        };
        if notify {
            let root: Arc<dyn Work> = self.clone();
            mt::publish(root);
            mt::kick();
        }
    }
}

impl ObservationSource for Lifetime {
    fn snapshot(&self) -> Snapshot {
        let _state = self.state.lock();
        // SAFETY: 来源链/电平的唯一同步域。
        unsafe { self.source.snapshot() }
    }
    unsafe fn attach(&self, node: Pin<&WaitNode>, interest: ObjectSignals, hint: Hint) {
        let _state = self.state.lock();
        // SAFETY: 调用者保持 pin 节点与观察 loan，到唯一 detach 返回。
        unsafe { self.source.attach(node, interest, hint) }
    }
    unsafe fn detach(&self, node: Pin<&WaitNode>) -> Hint {
        let _state = self.state.lock();
        // SAFETY: 同挂链约束；返回凭据不含节点引用。
        unsafe { self.source.detach(node) }
    }
}
impl Work for Lifetime {
    fn record(&self) -> &WorkRecord {
        self.source.record()
    }
    fn step(self: Arc<Self>) -> maintenance::record::Outcome {
        let step = {
            let _state = self.state.lock();
            // SAFETY: 持来源唯一同步域，每步复制一个无节点借用的凭据。
            unsafe { self.source.step() }
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
impl KernelObject for Lifetime {
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::Lifetime
    }
    fn related_id(&self) -> u64 {
        self.observed
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT)
    }
    fn allowed_signals(&self) -> Option<ObjectSignals> {
        Some(ObjectSignals::CLOSED)
    }

    fn wait_source(&self) -> Option<Arc<dyn ObservationSource>> {
        self.this
            .lock()
            .upgrade()
            .map(|source| source as Arc<dyn ObservationSource>)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}
