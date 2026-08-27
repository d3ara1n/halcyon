//! Notification：显式消费的 OR 位集合，等待只观察 READABLE 电平。

use alloc::sync::Arc;
use core::any::Any;

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, HandlePair, ObjectSignals, Rights},
};

use crate::sync::Spinlock;

use super::{
    object::{
        HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState,
        SubscribeResult,
    },
    proc::Process,
    Thread,
    wait::{Subscription, finish_offered},
};

struct NotificationState {
    wait: ObjectWaitState,
    pending: u64,
    closed: bool,
}

pub struct Notification {
    #[expect(dead_code, reason = "KernelObject 共同头供后续对象诊断使用")]
    header: ObjectHeader,
    state: Spinlock<NotificationState>,
}

impl Notification {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            header: ObjectHeader::new(),
            state: Spinlock::new(crate::sync::ranks::NOTIFICATION, NotificationState {
                wait: ObjectWaitState::new(ObjectSignals::NONE),
                pending: 0,
                closed: false,
            }),
        })
    }

    pub fn object_ref(this: &Arc<Self>) -> ObjectRef {
        this.clone()
    }

    pub fn signal(&self, bits: u64) -> Result<(), SystemCallError> {
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

    pub fn take(&self, mask: u64) -> Result<u64, SystemCallError> {
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

    fn close_owner(&self) {
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
        loop {
            let context = self.state.lock().wait.take_completer();
            let Some(context) = context else { break };
            finish_offered(context);
        }
    }
}

impl KernelObject for Notification {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::Notification
    }

    fn allowed_rights(&self, role: HandleRole) -> Option<Rights> {
        match role {
            HandleRole::NotificationOwner => {
                Some(Rights::READ | Rights::WAIT | Rights::MANAGE | Rights::GRANT)
            }
            HandleRole::NotificationSignaler => Some(
                Rights::SIGNAL
                    | Rights::WAIT
                    | Rights::TRANSIT
                    | Rights::GRANT
                    | Rights::DUPLICATE,
            ),
            _ => None,
        }
    }

    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals> {
        match role {
            HandleRole::NotificationOwner => Some(ObjectSignals::READABLE | ObjectSignals::CLOSED),
            HandleRole::NotificationSignaler => Some(ObjectSignals::CLOSED),
            _ => None,
        }
    }

    fn signals(&self) -> ObjectSignals {
        self.state.lock().wait.signals()
    }

    fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
        self.state.lock().wait.subscribe(subscription)
    }

    fn unsubscribe(&self, id: u64) {
        self.state.lock().wait.unsubscribe(id);
    }

    fn close_handle(&self, role: HandleRole, _owner: &Process, _exiting: bool) {
        if role == HandleRole::NotificationOwner {
            self.close_owner();
        }
    }

    fn close_transit(&self, role: HandleRole) {
        debug_assert!(role == HandleRole::NotificationSignaler);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn create(
    thread: &Thread,
    owner_rights: Rights,
    signaler_rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let notification = Notification::new();
    let object = Notification::object_ref(&notification);
    let mut entries = alloc::vec::Vec::new();
    entries.try_reserve(2).map_err(|_| SystemCallError::OutOfMemory)?;
    entries.push(
        super::handle::entry(object.clone(), HandleRole::NotificationOwner, owner_rights)
            .map_err(super::handle::map_error)?,
    );
    entries.push(
        super::handle::entry(object, HandleRole::NotificationSignaler, signaler_rights)
            .map_err(super::handle::map_error)?,
    );

    let token = super::handle::transaction_token();
    let mut table = thread.process.handles.lock();
    let reservation = table.reserve(2, token).map_err(super::handle::map_error)?;
    let pair = HandlePair::new(reservation.handles()[0], reservation.handles()[1]);
    let mut space = thread.process.space.lock();
    if let Err(error) = space.check_range(output, core::mem::size_of::<HandlePair>(), true) {
        drop(space);
        table.rollback(reservation).expect("NotificationCreate reservation must remain owned");
        return Err(error.into());
    }
    // SAFETY: HandlePair 无 padding；复检失败即杀本进程（deliver_output），
    // 未提交的预留随进程消亡。
    unsafe { crate::uaccess::deliver_output(thread, &mut space, output, &pair) }?;
    drop(space);
    table.commit(reservation, entries).expect("NotificationCreate reservation must remain owned");
    Ok(())
}

pub fn signal(thread: &Thread, handle: Handle, bits: u64) -> Result<(), SystemCallError> {
    let object = resolve(thread, handle, Rights::SIGNAL, HandleRole::NotificationSignaler)?;
    concrete(&object)?.signal(bits)
}

pub fn take(
    thread: &Thread,
    handle: Handle,
    mask: u64,
    output: usize,
) -> Result<(), SystemCallError> {
    let object = resolve(thread, handle, Rights::READ, HandleRole::NotificationOwner)?;
    let notification = concrete(&object)?;
    let mut space = thread.process.space.lock();
    space.check_range(output, core::mem::size_of::<u64>(), true)?;
    let taken = notification.take(mask)?;
    // SAFETY: u64 无 padding，且 space 锁使预校验到复制之间无映射变化。
    unsafe { crate::uaccess::write_user_value(&mut space, output, &taken) }?;
    Ok(())
}

fn resolve(
    thread: &Thread,
    handle: Handle,
    rights: Rights,
    role: HandleRole,
) -> Result<ObjectRef, SystemCallError> {
    let table = thread.process.handles.lock();
    let entry = table.get(handle, rights).map_err(super::handle::map_error)?;
    if *entry.role() != role || entry.object().kind() != ObjectKind::Notification {
        return Err(SystemCallError::WrongObjectType);
    }
    Ok(entry.object().clone())
}

fn concrete(object: &ObjectRef) -> Result<&Notification, SystemCallError> {
    object
        .as_any()
        .downcast_ref::<Notification>()
        .ok_or(SystemCallError::WrongObjectType)
}
