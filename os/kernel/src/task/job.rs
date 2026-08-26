//! Job capability：进程创建域与未来预算/故障收束的结构根。

use alloc::sync::Arc;
use core::any::Any;

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
};

use super::{
    Thread,
    object::{HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState, SubscribeResult},
    proc::Process,
    wait::Subscription,
};

pub struct Job {
    header: ObjectHeader,
    #[expect(dead_code, reason = "Job 层级/预算接线时使用")]
    parent: Option<ObjectRef>,
    wait: crate::sync::Spinlock<ObjectWaitState>,
}

impl Job {
    pub fn root() -> Arc<Self> {
        Arc::try_new(Self {
            header: ObjectHeader::new(),
            parent: None,
            wait: crate::sync::Spinlock::new(ObjectWaitState::new(ObjectSignals::NONE)),
        })
        .expect("root Job allocation failed")
    }

    fn child(parent: ObjectRef) -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            header: ObjectHeader::new(),
            parent: Some(parent),
            wait: crate::sync::Spinlock::new(ObjectWaitState::new(ObjectSignals::NONE)),
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub fn object_ref(job: &Arc<Self>) -> ObjectRef {
        job.clone()
    }
}

impl KernelObject for Job {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::Job
    }

    fn allowed_rights(&self, role: HandleRole) -> Option<Rights> {
        (role == HandleRole::JobControl).then_some(
            Rights::CREATE
                | Rights::MANAGE
                | Rights::READ
                | Rights::WAIT
                | Rights::DUPLICATE
                | Rights::TRANSIT
                | Rights::GRANT,
        )
    }

    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals> {
        (role == HandleRole::JobControl).then_some(ObjectSignals::CLOSED)
    }

    fn signals(&self) -> ObjectSignals {
        self.wait.lock().signals()
    }

    fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
        self.wait.lock().subscribe(subscription)
    }

    fn unsubscribe(&self, id: u64) {
        self.wait.lock().unsubscribe(id);
    }

    fn close_handle(&self, role: HandleRole, _owner: &Process, _exiting: bool) {
        debug_assert!(role == HandleRole::JobControl);
        // JobControl 丢失只消散 authority，不隐式终止成员。
    }

    fn close_transit(&self, role: HandleRole) {
        debug_assert!(role == HandleRole::JobControl);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn create(
    thread: &Thread,
    parent: Handle,
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    let parent = {
        let table = thread.process.handles.lock();
        let entry = table.get(parent, Rights::CREATE).map_err(super::handle::map_error)?;
        if *entry.role() != HandleRole::JobControl || entry.object().kind() != ObjectKind::Job {
            return Err(SystemCallError::WrongObjectType);
        }
        if !rights.is_subset_of(entry.rights()) {
            return Err(SystemCallError::RightsDenied);
        }
        entry.object().clone()
    };
    let child = Job::child(parent)?;
    let entry = super::handle::entry(Job::object_ref(&child), HandleRole::JobControl, rights)
        .map_err(super::handle::map_error)?;
    install_one(thread, entry, output)
}

pub fn resolve(
    thread: &Thread,
    handle: Handle,
    rights: Rights,
) -> Result<ObjectRef, SystemCallError> {
    let table = thread.process.handles.lock();
    let entry = table.get(handle, rights).map_err(super::handle::map_error)?;
    if *entry.role() != HandleRole::JobControl || entry.object().kind() != ObjectKind::Job {
        return Err(SystemCallError::WrongObjectType);
    }
    Ok(entry.object().clone())
}

fn install_one(
    thread: &Thread,
    entry: super::handle::ProcessHandleEntry,
    output: usize,
) -> Result<(), SystemCallError> {
    let mut entries = alloc::vec::Vec::new();
    entries.try_reserve(1).map_err(|_| SystemCallError::OutOfMemory)?;
    entries.push(entry);
    let token = super::handle::transaction_token();
    let mut table = thread.process.handles.lock();
    let reservation = table.reserve(1, token).map_err(super::handle::map_error)?;
    let handle = reservation.handles()[0];
    let mut space = thread.process.space.lock();
    if let Err(error) = space.check_range(output, core::mem::size_of::<Handle>(), true) {
        drop(space);
        table.rollback(reservation).expect("JobCreate reservation must remain owned");
        return Err(error.into());
    }
    // SAFETY: Handle 无 padding，输出已在同一 space 锁下验证。
    unsafe { crate::uaccess::write_user_value(&mut space, output, &handle) }
        .expect("validated JobCreate output must remain writable");
    drop(space);
    table.commit(reservation, entries)
        .expect("JobCreate reservation count matches entry");
    Ok(())
}
