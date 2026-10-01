//! Notification：显式消费的 OR 位集合。角色即对象类型（capability 分案设计轮次 4）：
//! - [`NotificationOwner`]：取走待决位、观察 READABLE；最后一份 capability 消失即关闭；
//! - [`NotificationSignaler`]：提交待决位；只保活 core，不保活 Owner 权能。
//! signal 与 Owner shut 在同一 core 锁下判定，shut 后拒绝迟到 signal。

use alloc::sync::Arc;
use core::any::Any;

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, HandlePair, ObjectSignals, Rights},
};

use crate::sync::Spinlock;

use super::{
    Thread,
    object::{KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState, SubscribeResult},
    wait::Subscription,
};

struct CoreState {
    wait: ObjectWaitState,
    pending: u64,
    closed: bool,
}

/// Owner 与 Signaler 共享的同步域。
struct NotificationCore {
    state: Spinlock<CoreState>,
}

impl NotificationCore {
    fn signal(&self, bits: u64) -> Result<(), SystemCallError> {
        if bits == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        {
            let mut state = self.state.lock();
            if state.closed {
                return Err(SystemCallError::ObjectClosed);
            }
            state.pending |= bits;
            state.wait.update(ObjectSignals::NONE, ObjectSignals::READABLE);
        }
        self.finish_waiters();
        Ok(())
    }

    fn take(&self, mask: u64) -> Result<u64, SystemCallError> {
        if mask == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let mut state = self.state.lock();
        if state.closed {
            return Err(SystemCallError::ObjectClosed);
        }
        let taken = state.pending & mask;
        if taken == 0 {
            return Err(SystemCallError::ObjectNotAvailable);
        }
        state.pending &= !taken;
        if state.pending == 0 {
            state.wait.update(ObjectSignals::READABLE, ObjectSignals::NONE);
        }
        Ok(taken)
    }

    fn shut(&self) {
        {
            let mut state = self.state.lock();
            if state.closed {
                return;
            }
            state.closed = true;
            state.pending = 0;
            state.wait.update(ObjectSignals::READABLE, ObjectSignals::CLOSED);
        }
        self.finish_waiters();
    }

    fn finish_waiters(&self) {
        let pending = self.state.lock().wait.take_notification();
        if let Some((reservation, target)) = pending {
            reservation.publish(target);
        }
    }
}

pub struct NotificationOwner {
    holders: capability::Holders,
    header: ObjectHeader,
    core: Arc<NotificationCore>,
}

pub struct NotificationSignaler {
    holders: capability::Holders,
    header: ObjectHeader,
    core: Arc<NotificationCore>,
}

/// 两个对象共用的等待委托（只有 Owner 承载等待源）。
macro_rules! common_object {
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
    common_object!();

    fn kind(&self) -> ObjectKind {
        ObjectKind::NotificationOwner
    }

    fn allowed_rights(&self) -> Option<Rights> {
        Some(Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT)
    }

    fn allowed_signals(&self) -> Option<ObjectSignals> {
        Some(ObjectSignals::READABLE | ObjectSignals::CLOSED)
    }

    fn signals(&self) -> ObjectSignals {
        self.core.state.lock().wait.signals()
    }

    fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
        self.core.state.lock().wait.subscribe(subscription)
    }

    fn rearm_observer(&self, id: u64) -> Result<super::object::ObserverRearm, SystemCallError> {
        self.core.state.lock().wait.rearm_observer(id)
    }

    fn cancel_observer(&self, id: u64) -> Option<super::object::CancelledObservation> {
        self.core.state.lock().wait.cancel_observer(id)
    }

    fn unsubscribe(&self, id: u64) {
        let retired = self.core.state.lock().wait.unsubscribe(id);
        drop(retired);
    }

    fn advance_waiter(&self) -> super::object::WaitAdvance {
        self.core.state.lock().wait.advance_waiter()
    }

    fn complete_waiter_drain(
        &self,
        reservation: super::notify_work::Reservation,
    ) -> super::notify_work::Completion {
        self.core.state.lock().wait.complete_notification(reservation)
    }

    fn on_last_capability(&self) {
        self.core.shut();
    }
}

impl KernelObject for NotificationSignaler {
    common_object!();

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

fn new_pair() -> Result<(Arc<NotificationOwner>, Arc<NotificationSignaler>), SystemCallError> {
    let core = Arc::try_new(NotificationCore {
        state: Spinlock::new(
            crate::sync::ranks::NOTIFICATION,
            CoreState {
                wait: ObjectWaitState::new(ObjectSignals::NONE),
                pending: 0,
                closed: false,
            },
        ),
    })
    .map_err(|_| SystemCallError::OutOfMemory)?;
    let owner = Arc::try_new(NotificationOwner {
        holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
        header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
        core: core.clone(),
    })
    .map_err(|_| SystemCallError::OutOfMemory)?;
    let signaler = Arc::try_new(NotificationSignaler {
        holders: capability::Holders::new(capability::ZeroPolicy::Terminal),
        header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
        core,
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
    let (owner, signaler) = new_pair()?;
    let owner_ref: ObjectRef = owner;
    let signaler_ref: ObjectRef = signaler;
    let owner_cap = super::handle::mint(owner_ref, owner_rights)?;
    let signaler_cap = match super::handle::mint(signaler_ref, signaler_rights) {
        Ok(cap) => cap,
        Err(error) => {
            super::handle::release_all([owner_cap]);
            return Err(error);
        }
    };
    let mut caps = alloc::vec::Vec::new();
    if caps.try_reserve_exact(2).is_err() {
        super::handle::release_all([owner_cap, signaler_cap]);
        return Err(SystemCallError::OutOfMemory);
    }
    caps.push(owner_cap);
    caps.push(signaler_cap);
    super::handle::install(
        thread,
        caps,
        |handles| {
            let pair = HandlePair::new(handles[0], handles[1]);
            crate::uaccess::write_user(thread, output, &pair)
        },
        || {},
    )
    .map(|_| ())
}

pub fn signal(thread: &Thread, handle: Handle, bits: u64) -> Result<(), SystemCallError> {
    let object = super::handle::resolve(
        &thread.process.handles.lock(),
        handle,
        Rights::SIGNAL,
        ObjectKind::NotificationSignaler,
    )?;
    let signaler = object
        .as_any()
        .downcast_ref::<NotificationSignaler>()
        .ok_or(SystemCallError::WrongObjectType)?;
    signaler.core.signal(bits)
}

pub fn take(thread: &Thread, handle: Handle, mask: u64, output: usize) -> Result<(), SystemCallError> {
    let object = super::handle::resolve(
        &thread.process.handles.lock(),
        handle,
        Rights::READ,
        ObjectKind::NotificationOwner,
    )?;
    let owner = object
        .as_any()
        .downcast_ref::<NotificationOwner>()
        .ok_or(SystemCallError::WrongObjectType)?;
    crate::uaccess::check_user_output::<u64>(thread, output)?;
    let taken = owner.core.take(mask)?;
    // 已消费的位写回失败终止发起进程（uaccess 契约）。
    crate::uaccess::write_user(thread, output, &taken)
}
