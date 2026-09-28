//! 线程公开面：waitable ThreadControl 壳与 Running ThreadCreate 事务。

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    any::Any,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use erhino_shared::{
    call::SystemCallError,
    object::{ObjectSignals, Rights},
    proc::{ThreadCreateResult, ThreadStartContext},
};

use super::{
    object::{
        HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState,
        SubscribeResult,
    },
    proc::Process,
    wait::{Subscription, schedule_waiters},
};

pub(crate) const CONTROL_MAX_RIGHTS: Rights = Rights::from_raw(
    Rights::WAIT.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw()
        | Rights::MANAGE.raw(),
);
static RESULT_OBLIGATION_DELAY_OBSERVED: AtomicBool = AtomicBool::new(false);

/// 线程离场的独立观察壳。Handle close 只消散观察权，不影响执行线程。
pub(crate) struct ThreadControl {
    header: ObjectHeader,
    wait: crate::sync::Spinlock<ObjectWaitState>,
    thread: crate::sync::Spinlock<Option<Weak<super::proc::Thread>>>,
    capabilities: AtomicUsize,
}

impl ThreadControl {
    pub(crate) fn new() -> Result<Arc<Self>, erhino_shared::call::SystemCallError> {
        Arc::try_new(Self {
            header: ObjectHeader::try_new()
                .ok_or(erhino_shared::call::SystemCallError::ReachLimit)?,
            wait: crate::sync::Spinlock::new(
                crate::sync::ranks::OBJECT_WAIT,
                ObjectWaitState::new(ObjectSignals::NONE),
            ),
            thread: crate::sync::Spinlock::new(crate::sync::ranks::LEAF, None),
            capabilities: AtomicUsize::new(0),
        })
        .map_err(|_| erhino_shared::call::SystemCallError::OutOfMemory)
    }

    pub(crate) fn object_ref(control: &Arc<Self>) -> ObjectRef {
        control.clone()
    }

    /// 成员摘除及线程级结果义务完成后发布持续 DONE 电平。
    pub(crate) fn publish_done(&self) {
        self.wait
            .lock()
            .update(ObjectSignals::NONE, ObjectSignals::DONE);
        schedule_waiters(&self.wait);
    }

    pub(crate) fn bind_thread(&self, thread: &Arc<super::proc::Thread>) {
        *self.thread.lock() = Some(Arc::downgrade(thread));
    }

    pub(crate) fn acquire_capability(&self) {
        self.capabilities
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |count| count.checked_add(1))
            .expect("ThreadControl capability count exhausted");
    }

    pub(crate) fn release_capability(&self) {
        let old = self.capabilities.fetch_sub(1, Ordering::AcqRel);
        assert!(old != 0, "ThreadControl capability released twice");
        if old == 1 {
            let thread = self.thread.lock().as_ref().and_then(Weak::upgrade);
            if let Some(thread) = thread {
                thread.abandon_created();
            }
        }
    }

    pub(crate) fn start(&self, caller: &Arc<Process>) -> Result<(), SystemCallError> {
        let thread = self
            .thread
            .lock()
            .as_ref()
            .and_then(Weak::upgrade)
            .ok_or(SystemCallError::ObjectClosed)?;
        if !Arc::ptr_eq(&thread.process, caller) {
            return Err(SystemCallError::RightsDenied);
        }
        thread.start()
    }
}

struct DepartureState {
    result_obligations: usize,
    requested: bool,
    finalizing: bool,
}

/// 执行 Thread 强引用消散后仍可由 committed Map 结果义务持有的离场状态。
pub(crate) struct ThreadDeparture {
    process: Weak<Process>,
    member: super::lifecycle::MemberKey,
    control: Option<Weak<ThreadControl>>,
    state: crate::sync::Spinlock<DepartureState>,
}

impl ThreadDeparture {
    pub(crate) fn new(
        process: &Arc<Process>,
        member: super::lifecycle::MemberKey,
        control: Option<&Arc<ThreadControl>>,
    ) -> Result<Arc<Self>, ()> {
        Arc::try_new(Self {
            process: Arc::downgrade(process),
            member,
            control: control.map(Arc::downgrade),
            state: crate::sync::Spinlock::new(
                crate::sync::ranks::LEAF,
                DepartureState {
                    result_obligations: 0,
                    requested: false,
                    finalizing: false,
                },
            ),
        })
        .map_err(|_| ())
    }

    pub(crate) fn acquire_result(self: &Arc<Self>) -> ThreadResultObligation {
        let mut state = self.state.lock();
        state.result_obligations = state
            .result_obligations
            .checked_add(1)
            .expect("thread result obligation count exhausted");
        drop(state);
        ThreadResultObligation {
            departure: self.clone(),
        }
    }

    pub(crate) fn request(&self) {
        let (finalize, delayed) = {
            let mut state = self.state.lock();
            if core::mem::replace(&mut state.requested, true) {
                return;
            }
            let delayed = state.result_obligations != 0;
            (Self::claim_finalize(&mut state), delayed)
        };
        if delayed && !RESULT_OBLIGATION_DELAY_OBSERVED.swap(true, Ordering::AcqRel) {
            log!(
                Memory,
                "thread departure delayed by a committed Map result obligation"
            );
        }
        if finalize {
            self.finalize();
        }
    }

    fn complete_result(&self) {
        let finalize = {
            let mut state = self.state.lock();
            state.result_obligations = state
                .result_obligations
                .checked_sub(1)
                .expect("thread result obligation completed without registration");
            Self::claim_finalize(&mut state)
        };
        if finalize {
            self.finalize();
        }
    }

    fn claim_finalize(state: &mut DepartureState) -> bool {
        if state.requested && state.result_obligations == 0 && !state.finalizing {
            state.finalizing = true;
            true
        } else {
            false
        }
    }

    fn finalize(&self) {
        let process = self
            .process
            .upgrade()
            .expect("process core vanished before thread departure");
        super::process::confirm_departure(&process, self.member);
        if let Some(control) = self.control.as_ref().and_then(Weak::upgrade) {
            control.publish_done();
        }
    }
}

/// committed MemoryMap 对线程结果记录的 affine 引用。
pub(crate) struct ThreadResultObligation {
    departure: Arc<ThreadDeparture>,
}

impl Drop for ThreadResultObligation {
    fn drop(&mut self) {
        self.departure.complete_result();
    }
}

impl KernelObject for ThreadControl {
    fn complete_waiter_drain(
        &self,
        reservation: super::notify_work::Reservation,
    ) -> super::notify_work::Completion {
        self.wait.lock().complete_notification(reservation)
    }

    fn advance_waiter(&self) -> super::object::WaitAdvance {
        self.wait.lock().advance_waiter()
    }

    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::ThreadControl
    }

    fn allowed_rights(&self, role: HandleRole) -> Option<Rights> {
        (role == HandleRole::ThreadControl).then_some(CONTROL_MAX_RIGHTS)
    }

    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals> {
        (role == HandleRole::ThreadControl).then_some(ObjectSignals::DONE)
    }

    fn signals(&self) -> ObjectSignals {
        self.wait.lock().signals()
    }

    fn subscribe(&self, subscription: Subscription) -> SubscribeResult {
        self.wait.lock().subscribe(subscription)
    }

    fn rearm_observer(&self, id: u64) -> Result<super::object::ObserverRearm, SystemCallError> {
        self.wait.lock().rearm_observer(id)
    }

    fn cancel_observer(&self, id: u64) -> Option<super::object::CancelledObservation> {
        self.wait.lock().cancel_observer(id)
    }

    fn unsubscribe(&self, id: u64) {
        let retired = self.wait.lock().unsubscribe(id);
        drop(retired);
    }

    fn close_handle(&self, role: HandleRole, _owner: &Process, _exiting: bool) {
        debug_assert!(role == HandleRole::ThreadControl);
    }

    fn close_transit(&self, role: HandleRole) {
        debug_assert!(role == HandleRole::ThreadControl);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn map_attach_fault(error: super::lifecycle::AttachFault) -> SystemCallError {
    match error {
        super::lifecycle::AttachFault::Closed => SystemCallError::ObjectClosed,
        super::lifecycle::AttachFault::Limit => SystemCallError::ReachLimit,
        super::lifecycle::AttachFault::Oom => SystemCallError::OutOfMemory,
    }
}

fn map_context_fault(error: super::proc::SpaceError) -> SystemCallError {
    SystemCallError::from(error)
}

/// Running 期同步创建线程。固定宽结果写出成功后，handle/member/Ready
/// 三项按不可失败顺序发布。
pub(crate) fn create(
    caller: &super::Thread,
    context_address: usize,
    result_address: usize,
) -> Result<(), SystemCallError> {
    let context = {
        let mut space = caller.process.space.lock();
        // SAFETY: ThreadStartContext 只含 u64，任意位型均有效。
        unsafe {
            crate::uaccess::read_user_value::<ThreadStartContext>(&mut space, context_address)
        }?
    };
    let entry_address =
        usize::try_from(context.entry).map_err(|_| SystemCallError::IllegalArgument)?;
    let stack_pointer =
        usize::try_from(context.stack_pointer).map_err(|_| SystemCallError::IllegalArgument)?;

    let control = ThreadControl::new()?;
    let object = ThreadControl::object_ref(&control);
    let entry = super::handle::entry(object, HandleRole::ThreadControl, CONTROL_MAX_RIGHTS)
        .map_err(super::handle::map_error)?;
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(1)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    entries.push(entry);

    let ready_batch = caller
        .process
        .domain()
        .reserve_ready(1)
        .map_err(|_| SystemCallError::OutOfMemory)?;

    let token = super::handle::transaction_token()?;
    let mut table = caller.process.handles.lock();
    let reservation = match table.reserve(1, token) {
        Ok(reservation) => reservation,
        Err(error) => {
            return Err(super::handle::map_error(error));
        }
    };
    let result_handle = reservation.handles()[0];
    let mut space = caller.process.space.lock();
    if let Err(error) = space.check_range(
        result_address,
        core::mem::size_of::<ThreadCreateResult>(),
        true,
    ) {
        table
            .rollback(reservation)
            .expect("spawn reservation must remain owned");
        drop(space);
        return Err(error.into());
    }
    if let Err(error) = space.validate_initial_context(entry_address, stack_pointer) {
        table
            .rollback(reservation)
            .expect("spawn reservation must remain owned");
        drop(space);
        return Err(map_context_fault(error));
    }

    let (member, thread) = match caller.process.lifecycle.create_running(|_, member| {
        let thread = super::Thread::new_thread_with_control(
            member,
            &caller.process,
            context,
            Some(&control),
        )
        .map_err(|_| super::lifecycle::AttachFault::Oom)?;
        Arc::try_new(thread).map_err(|_| super::lifecycle::AttachFault::Oom)
    }) {
        Ok(staged) => staged,
        Err(error) => {
            table
                .rollback(reservation)
                .expect("spawn reservation must remain owned");
            drop(space);
            return Err(map_attach_fault(error));
        }
    };
    let result = ThreadCreateResult {
        tid: member.tid(),
        reserved: 0,
        control: result_handle,
    };
    // SAFETY: ThreadCreateResult 的 u32/u32/Handle 恰好覆盖 16 字节，无 padding。
    let output = unsafe { crate::uaccess::write_user_value(&mut space, result_address, &result) };
    if let Err(error) = output {
        let (staged, _reapable) = caller.process.lifecycle.rollback_created(member);
        drop(space);
        table
            .rollback(reservation)
            .expect("spawn reservation must remain owned");
        drop(table);
        drop(thread);
        drop(staged);
        let todo = crate::uaccess::fault_output(
            &caller.process,
            caller.member().tid(),
            result_address,
            Some(caller.member()),
        );
        super::process::run_termination_todo(&caller.process, todo);
        return Err(error.into());
    }
    drop(space);

    table
        .commit(reservation, entries)
        .expect("spawn reservation must remain owned");
    drop(table);
    control.bind_thread(&thread);
    thread.install_ready_batch(ready_batch);
    drop(thread);
    Ok(())
}

pub(crate) fn start(caller: &super::Thread, handle: erhino_shared::object::Handle) -> Result<(), SystemCallError> {
    let object = {
        let table = caller.process.handles.lock();
        let entry = table
            .get(handle, Rights::MANAGE)
            .map_err(super::handle::map_error)?;
        if *entry.role() != HandleRole::ThreadControl
            || entry.object().kind() != ObjectKind::ThreadControl
        {
            return Err(SystemCallError::WrongObjectType);
        }
        entry.object().as_object().clone()
    };
    let any: alloc::sync::Arc<dyn Any + Send + Sync> = object;
    let control = any
        .downcast::<ThreadControl>()
        .map_err(|_| SystemCallError::WrongObjectType)?;
    control.start(&caller.process)
}

/// 运输中的副本使最后一个表项关闭后 Created 线程仍保持可启动。
pub(crate) fn capability_lifecycle_selftest(root: &Arc<super::memory_pool::MemoryPool>) {
    for terminate_first in [false, true] {
        let process = super::selftest::bound(root);
        let control = ThreadControl::new().expect("fixture ThreadControl creation failed");
        let first = super::handle::entry(
            ThreadControl::object_ref(&control),
            HandleRole::ThreadControl,
            CONTROL_MAX_RIGHTS,
        )
        .expect("fixture ThreadControl capability failed");
        let transit = first.clone();
        let mut staged = None;
        process
            .lifecycle
            .attach_member(|_, member| {
                let thread = Arc::try_new(
                    super::Thread::new_thread_with_control(
                        member,
                        &process,
                        ThreadStartContext {
                            entry: 0,
                            stack_pointer: 0,
                            arg1: 0,
                            arg2: 0,
                        },
                        Some(&control),
                    )
                    .map_err(|_| super::lifecycle::AttachFault::Oom)?,
                )
                .map_err(|_| super::lifecycle::AttachFault::Oom)?;
                staged = Some(thread.clone());
                Ok(thread)
            })
            .expect("fixture Created thread failed");
        let created = staged.expect("fixture Created thread missing");
        control.bind_thread(&created);
        drop(created);
        drop(first);
        assert_eq!(control.capabilities.load(Ordering::Acquire), 1);
        assert_eq!(process.lifecycle.member_count(), 1);
        if terminate_first {
            super::selftest::terminate(&process);
            super::selftest::pump();
            assert_eq!(process.lifecycle.member_count(), 0);
        }
        drop(transit);
        assert_eq!(process.lifecycle.member_count(), 0);
        assert!(control.signals().contains(ObjectSignals::DONE));
        if !terminate_first {
            super::selftest::terminate(&process);
        }
        super::selftest::pump();
        super::selftest::collect(&process);
    }
}
