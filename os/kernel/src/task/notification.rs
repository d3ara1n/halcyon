//! Notification Owner/Signaler；来源只维护电平并由出生 Work 有界发重试提示。

use alloc::sync::{Arc, Weak};
use core::{any::Any, pin::Pin};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, HandlePair, ObjectSignals, Rights},
};
use maintenance::record::Outcome;

use super::{
    Thread, handle,
    object::{KernelObject, ObjectHeader, ObjectKind},
    wait_source::{Hint, ObservationSource, Snapshot, Step, WaitNode, WaitQueue},
};
use crate::{
    maintenance::{self as mt, Work, WorkRecord},
    sync::{Spinlock, ranks},
};

struct CoreState {
    pending: u64,
    closed: bool,
}
struct NotificationCore {
    state: Spinlock<CoreState>,
    source: WaitQueue,
    this: Spinlock<Weak<Self>>,
    _metadata: super::resources::MetadataCharge,
}

impl NotificationCore {
    fn new(pool: Arc<super::memory_pool::MemoryPool>) -> Result<Arc<Self>, SystemCallError> {
        let metadata = super::resources::MetadataCharge::arc::<Self>(&pool)?;
        let account = pool.maintenance_account().clone();
        let core = Arc::try_new(Self {
            state: Spinlock::new(
                ranks::NOTIFICATION,
                CoreState {
                    pending: 0,
                    closed: false,
                },
            ),
            source: WaitQueue::new(account),
            this: Spinlock::new(ranks::LEAF, Weak::new()),
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)?;
        *core.this.lock() = Arc::downgrade(&core);
        Ok(core)
    }

    fn root(&self) -> Arc<dyn Work> {
        self.this
            .lock()
            .upgrade()
            .expect("notification source lost its core")
    }

    fn signal(&self, bits: u64) -> Result<(), SystemCallError> {
        if bits == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let root = self.root();
        let changed = {
            let mut state = self.state.lock();
            if state.closed {
                return Err(SystemCallError::ObjectClosed);
            }
            state.pending |= bits;
            // SAFETY: core 出生后地址稳定；电平与业务位同受 NOTIFICATION 锁保护。
            unsafe { self.source.update(ObjectSignals::READABLE, false) }
        };
        if changed {
            mt::publish(root);
            mt::kick();
        }
        Ok(())
    }

    fn take(&self, mask: u64) -> Result<u64, SystemCallError> {
        if mask == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let root = self.root();
        let (taken, changed) = {
            let mut state = self.state.lock();
            if state.closed {
                return Err(SystemCallError::ObjectClosed);
            }
            let taken = state.pending & mask;
            if taken == 0 {
                return Err(SystemCallError::ObjectNotAvailable);
            }
            state.pending &= !taken;
            let level = if state.pending == 0 {
                ObjectSignals::NONE
            } else {
                ObjectSignals::READABLE
            };
            // SAFETY: 同一稳定 core/来源同步域，不接触等待目标。
            (taken, unsafe { self.source.update(level, false) })
        };
        if changed {
            mt::publish(root);
            mt::kick();
        }
        Ok(taken)
    }

    fn shut(&self) {
        let root = self.root();
        let changed = {
            let mut state = self.state.lock();
            if state.closed {
                return;
            }
            state.closed = true;
            state.pending = 0;
            // SAFETY: core pin 稳定，持业务锁；关闭也只提交扫描责任。
            unsafe { self.source.update(ObjectSignals::CLOSED, true) }
        };
        if changed {
            mt::publish(root);
            mt::kick();
        }
    }
}

impl ObservationSource for NotificationCore {
    fn snapshot(&self) -> Snapshot {
        let _state = self.state.lock();
        // SAFETY: core 稳定、持来源业务锁。
        unsafe { self.source.snapshot() }
    }
    unsafe fn attach(&self, node: Pin<&WaitNode>, interest: ObjectSignals, hint: Hint) {
        let _state = self.state.lock();
        // SAFETY: 外层 ObservationSource::attach 契约给出节点寿命；持来源业务锁。
        unsafe {
            self.source.attach(node, interest, hint);
        }
    }
    unsafe fn detach(&self, node: Pin<&WaitNode>) -> Hint {
        let _state = self.state.lock();
        // SAFETY: 外层保证来自本来源，owner 保持 storage 至返回；同业务锁摘链。
        unsafe { self.source.detach(node) }
    }
}

impl Work for NotificationCore {
    fn record(&self) -> &WorkRecord {
        self.source.record()
    }
    fn step(self: Arc<Self>) -> Outcome {
        let next = {
            let _state = self.state.lock();
            // SAFETY: 持稳定 core 的来源锁；在锁内复制强在途凭证，无节点借用出锁。
            unsafe { self.source.step() }
        };
        match next {
            Step::Idle => Outcome::Idle,
            Step::Progress => Outcome::Ready,
            Step::Hint(delivery) => {
                delivery.send();
                Outcome::Ready
            }
        }
    }
}

pub struct NotificationOwner {
    holders: capability::Holders,
    header: ObjectHeader,
    core: Arc<NotificationCore>,
    _metadata: super::resources::MetadataCharge,
}
pub struct NotificationSignaler {
    holders: capability::Holders,
    header: ObjectHeader,
    core: Arc<NotificationCore>,
    _metadata: super::resources::MetadataCharge,
}
pub struct NotificationObservation {
    holders: capability::Holders,
    header: ObjectHeader,
    core: Arc<NotificationCore>,
    _metadata: super::resources::MetadataCharge,
}
macro_rules! identity {
    () => {
        fn holders(&self) -> &capability::Holders {
            &self.holders
        }
        fn header(&self) -> &ObjectHeader {
            &self.header
        }
        fn as_any(&self) -> &dyn Any {
            self
        }
    };
}
impl KernelObject for NotificationOwner {
    identity!();
    fn kind(&self) -> ObjectKind {
        ObjectKind::NotificationOwner
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT)
    }
    fn allowed_signals(&self) -> Option<ObjectSignals> {
        Some(ObjectSignals::READABLE | ObjectSignals::CLOSED)
    }

    fn wait_source(&self) -> Option<Arc<dyn ObservationSource>> {
        Some(self.core.clone())
    }
    fn on_last_capability(&self) {
        self.core.shut();
    }
}
impl KernelObject for NotificationSignaler {
    identity!();
    fn kind(&self) -> ObjectKind {
        ObjectKind::NotificationSignaler
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(Rights::SIGNAL | Rights::TRANSIT | Rights::GRANT | Rights::DUPLICATE)
    }
    fn allowed_signals(&self) -> Option<ObjectSignals> {
        None
    }
}
impl KernelObject for NotificationObservation {
    identity!();
    fn kind(&self) -> ObjectKind {
        ObjectKind::NotificationObservation
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT)
    }
    fn allowed_signals(&self) -> Option<ObjectSignals> {
        Some(ObjectSignals::CLOSED)
    }
    fn wait_source(&self) -> Option<Arc<dyn ObservationSource>> {
        Some(self.core.clone())
    }
}

pub fn observe(thread: &Thread, source: Handle, output: usize) -> Result<(), SystemCallError> {
    let authority = super::handle::Authorization::accept(thread, source, Rights::NONE)?;
    let core = if let Some(owner) = authority
        .object()
        .as_any()
        .downcast_ref::<NotificationOwner>()
    {
        if !authority.rights().contains(Rights::READ) {
            return Err(SystemCallError::RightsDenied);
        }
        owner.core.clone()
    } else if let Some(signaler) = authority
        .object()
        .as_any()
        .downcast_ref::<NotificationSignaler>()
    {
        if !authority.rights().contains(Rights::SIGNAL) {
            return Err(SystemCallError::RightsDenied);
        }
        signaler.core.clone()
    } else {
        return Err(SystemCallError::WrongObjectType);
    };
    let pool = thread.process.funding_pool();
    let metadata = super::resources::MetadataCharge::arc::<NotificationObservation>(&pool)?;
    let observation = Arc::try_new(NotificationObservation {
        holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
        header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
        core,
        _metadata: metadata,
    })
    .map_err(|_| SystemCallError::OutOfMemory)?;
    let caps = [handle::mint(
        observation,
        Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT,
    )?];
    handle::install(
        thread,
        caps,
        |handles| crate::uaccess::write_user(thread, output, &handles[0]),
        || {},
    )
    .map(|_| ())
}

fn new_pair(
    pool: Arc<super::memory_pool::MemoryPool>,
) -> Result<(Arc<NotificationOwner>, Arc<NotificationSignaler>), SystemCallError> {
    let core = NotificationCore::new(pool.clone())?;
    let metadata = super::resources::MetadataCharge::arc::<NotificationOwner>(&pool)?;
    let owner = Arc::try_new(NotificationOwner {
        holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
        header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
        core: core.clone(),
        _metadata: metadata,
    })
    .map_err(|_| SystemCallError::OutOfMemory)?;
    let metadata = super::resources::MetadataCharge::arc::<NotificationSignaler>(&pool)?;
    let signaler = Arc::try_new(NotificationSignaler {
        holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
        header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
        core,
        _metadata: metadata,
    })
    .map_err(|_| SystemCallError::OutOfMemory)?;
    Ok((owner, signaler))
}

pub fn create(
    thread: &Thread,
    owner_rights: Rights,
    signaler_rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let (owner, signaler) = new_pair(thread.process.funding_pool())?;
    let owner_cap = super::handle::mint(owner, owner_rights)?;
    let signaler_cap = match super::handle::mint(signaler, signaler_rights) {
        Ok(cap) => cap,
        Err(error) => {
            super::handle::release_all([owner_cap]);
            return Err(error);
        }
    };
    let caps = [owner_cap, signaler_cap];
    super::handle::install(
        thread,
        caps,
        |handles| {
            crate::uaccess::write_user(
                thread,
                output,
                &HandlePair {
                    owner: handles[0],
                    peer: handles[1],
                },
            )
        },
        || {},
    )
    .map(|_| ())
}

fn resolve<T: Any + Send + Sync>(
    thread: &Thread,
    handle: Handle,
    rights: Rights,
    kind: ObjectKind,
) -> Result<Arc<T>, SystemCallError> {
    let object = {
        let body = thread
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let table = body.handles.lock();
        super::handle::resolve(&table, handle, rights, kind)?
    };
    let any: Arc<dyn Any + Send + Sync> = object;
    any.downcast::<T>()
        .map_err(|_| SystemCallError::WrongObjectType)
}

pub fn signal(thread: &Thread, handle: Handle, bits: u64) -> Result<(), SystemCallError> {
    resolve::<NotificationSignaler>(
        thread,
        handle,
        Rights::SIGNAL,
        ObjectKind::NotificationSignaler,
    )?
    .core
    .signal(bits)
}
pub fn take(
    thread: &Thread,
    handle: Handle,
    mask: u64,
    output: usize,
) -> Result<(), SystemCallError> {
    let owner =
        resolve::<NotificationOwner>(thread, handle, Rights::READ, ObjectKind::NotificationOwner)?;
    let taken = owner.core.take(mask)?;
    crate::uaccess::write_user(thread, output, &taken)
}
