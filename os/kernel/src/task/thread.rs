//! 线程公开面：waitable ThreadControl 壳与 Running ThreadSpawn 事务。

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use core::{
    any::Any,
    sync::atomic::{AtomicBool, Ordering},
};

use erhino_shared::{
    call::SystemCallError,
    object::{ObjectSignals, Rights},
    proc::{ProcessExitReason, ProcessFaultCode, ThreadSpawnResult, ThreadStartContext},
};

use super::{
    object::{
        HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState,
        SubscribeResult,
    },
    proc::Process,
    wait::{Subscription, finish_offered},
};

const CONTROL_MAX_RIGHTS: Rights = Rights::from_raw(
    Rights::WAIT.raw() | Rights::DUPLICATE.raw() | Rights::TRANSIT.raw() | Rights::GRANT.raw(),
);
static RESULT_OBLIGATION_DELAY_OBSERVED: AtomicBool = AtomicBool::new(false);

/// 线程离场的独立观察壳。Handle close 只消散观察权，不影响执行线程。
pub(crate) struct ThreadControl {
    header: ObjectHeader,
    wait: crate::sync::Spinlock<ObjectWaitState>,
}

impl ThreadControl {
    pub(crate) fn new() -> Result<Arc<Self>, erhino_shared::call::SystemCallError> {
        Arc::try_new(Self {
            header: ObjectHeader::new(),
            wait: crate::sync::Spinlock::new(
                crate::sync::ranks::OBJECT_WAIT,
                ObjectWaitState::new(ObjectSignals::NONE),
            ),
        })
        .map_err(|_| erhino_shared::call::SystemCallError::OutOfMemory)
    }

    pub(crate) fn object_ref(control: &Arc<Self>) -> ObjectRef {
        control.clone()
    }

    /// 成员摘除及线程级结果义务完成后发布持续 DONE 电平。
    pub(crate) fn publish_done(&self) {
        loop {
            let context = {
                let mut wait = self.wait.lock();
                wait.update(ObjectSignals::NONE, ObjectSignals::DONE);
                wait.take_completer()
            };
            let Some(context) = context else { break };
            finish_offered(context);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DepartureKind {
    Normal(i64),
    Terminated,
}

struct DepartureState {
    result_obligations: usize,
    requested: Option<DepartureKind>,
    finalizing: bool,
}

/// 执行 Thread 强引用消散后仍可由 committed Map 结果义务持有的离场状态。
pub(crate) struct ThreadDeparture {
    process: Weak<Process>,
    tid: erhino_shared::proc::Tid,
    control: Option<Weak<ThreadControl>>,
    state: crate::sync::Spinlock<DepartureState>,
}

impl ThreadDeparture {
    pub(crate) fn new(
        process: &Arc<Process>,
        tid: erhino_shared::proc::Tid,
        control: Option<&Arc<ThreadControl>>,
    ) -> Result<Arc<Self>, ()> {
        Arc::try_new(Self {
            process: Arc::downgrade(process),
            tid,
            control: control.map(Arc::downgrade),
            state: crate::sync::Spinlock::new(
                crate::sync::ranks::LEAF,
                DepartureState {
                    result_obligations: 0,
                    requested: None,
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

    pub(crate) fn request(&self, kind: DepartureKind) {
        let (finalize, delayed) = {
            let mut state = self.state.lock();
            assert!(
                state.requested.replace(kind).is_none(),
                "thread departure requested twice"
            );
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
            self.finalize(kind);
        }
    }

    fn complete_result(&self) {
        let (finalize, kind) = {
            let mut state = self.state.lock();
            state.result_obligations = state
                .result_obligations
                .checked_sub(1)
                .expect("thread result obligation completed without registration");
            let kind = state.requested;
            (Self::claim_finalize(&mut state), kind)
        };
        if finalize {
            self.finalize(kind.expect("claimed finalization must retain departure kind"));
        }
    }

    fn claim_finalize(state: &mut DepartureState) -> bool {
        if state.requested.is_some() && state.result_obligations == 0 && !state.finalizing {
            state.finalizing = true;
            true
        } else {
            false
        }
    }

    fn finalize(&self, kind: DepartureKind) {
        let process = self
            .process
            .upgrade()
            .expect("process core vanished before thread departure");
        let normal_code = match kind {
            DepartureKind::Normal(code) => Some(code),
            DepartureKind::Terminated => None,
        };
        super::process::confirm_departure(&process, self.tid, normal_code);
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
        (role == HandleRole::ThreadControl).then_some(ObjectSignals::DONE | ObjectSignals::CLOSED)
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
    match error {
        super::proc::SpaceError::NoFrame => SystemCallError::OutOfMemory,
        super::proc::SpaceError::BadSegment => SystemCallError::IllegalArgument,
        super::proc::SpaceError::Conflict => SystemCallError::InvalidAddress,
        super::proc::SpaceError::Busy => SystemCallError::ObjectBusy,
    }
}

/// Running 期同步创建线程。固定宽结果写出成功后，handle/member/Ready
/// 三项按不可失败顺序发布。
pub(crate) fn spawn(
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

    let mut ready = Vec::new();
    ready
        .try_reserve_exact(1)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    let ready_batch = crate::sched::reserve_ready_batch(caller.process.domain(), 1)
        .map_err(|_| SystemCallError::OutOfMemory)?;

    let token = super::handle::transaction_token();
    let mut table = caller.process.handles.lock();
    let reservation = match table.reserve(1, token) {
        Ok(reservation) => reservation,
        Err(error) => {
            crate::sched::rollback_ready_batch(ready_batch);
            return Err(super::handle::map_error(error));
        }
    };
    let result_handle = reservation.handles()[0];
    let mut space = caller.process.space.lock();
    if let Err(error) = space.check_range(
        result_address,
        core::mem::size_of::<ThreadSpawnResult>(),
        true,
    ) {
        table
            .rollback(reservation)
            .expect("spawn reservation must remain owned");
        drop(space);
        crate::sched::rollback_ready_batch(ready_batch);
        return Err(error.into());
    }
    if let Err(error) = space.validate_initial_context(entry_address, stack_pointer) {
        table
            .rollback(reservation)
            .expect("spawn reservation must remain owned");
        drop(space);
        crate::sched::rollback_ready_batch(ready_batch);
        return Err(map_context_fault(error));
    }

    let (member, thread) = match caller.process.lifecycle.begin_spawn(|tid| {
        let thread =
            super::Thread::new_thread_with_control(tid, &caller.process, context, Some(&control))
                .map_err(|_| super::lifecycle::AttachFault::Oom)?;
        Arc::try_new(thread).map_err(|_| super::lifecycle::AttachFault::Oom)
    }) {
        Ok(staged) => staged,
        Err(error) => {
            table
                .rollback(reservation)
                .expect("spawn reservation must remain owned");
            drop(space);
            crate::sched::rollback_ready_batch(ready_batch);
            return Err(map_attach_fault(error));
        }
    };
    let result = ThreadSpawnResult {
        tid: member.tid(),
        reserved: 0,
        control: result_handle,
    };
    // SAFETY: ThreadSpawnResult 的 u32/u32/Handle 恰好覆盖 16 字节，无 padding。
    let output = unsafe { crate::uaccess::write_user_value(&mut space, result_address, &result) };
    if let Err(error) = output {
        let (staged, _reapable) = caller.process.lifecycle.rollback_spawn(member);
        drop(space);
        table
            .rollback(reservation)
            .expect("spawn reservation must remain owned");
        drop(table);
        drop(thread);
        drop(staged);
        crate::sched::rollback_ready_batch(ready_batch);
        let todo = caller.process.lifecycle.request_termination(
            ProcessExitReason::Fault,
            ProcessFaultCode::StoreAccess as i64,
            Some(caller.tid),
        );
        super::process::run_termination_todo(&caller.process, todo);
        return Err(error.into());
    }
    drop(space);

    table
        .commit(reservation, entries)
        .expect("spawn reservation must remain owned");
    drop(table);
    let staged = caller.process.lifecycle.commit_spawn(member);
    debug_assert!(Arc::ptr_eq(&thread, &staged));
    drop(thread);
    ready.push(staged);
    crate::sched::commit_ready_batch(ready_batch, ready);
    Ok(())
}
