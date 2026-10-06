//! ThreadControl 观察壳与线程 syscall；Pending 启动权由独立 ThreadStartTicket 拥有。

use super::{
    Thread, handle,
    job::Job,
    memory_pool::MemoryPool,
    object::{KernelObject, ObjectHeader, ObjectRef},
    process::{
        members::MemberState, shell::ProcessShell, thread::Thread as ExecutionThread,
        ticket::ThreadStartTicket,
    },
    wait_source::{ObservationSource, SignalSource},
};
use alloc::sync::Arc;
use core::any::Any;
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectKind, ObjectSignals, Rights},
    proc::{ThreadCreateResult, ThreadStartContext},
};

pub(crate) const CONTROL_RIGHTS: Rights = Rights::from_raw(
    Rights::WAIT.raw() | Rights::DUPLICATE.raw() | Rights::GRANT.raw() | Rights::TRANSIT.raw(),
);
const START_RIGHTS: Rights =
    Rights::from_raw(Rights::MANAGE.raw() | Rights::GRANT.raw() | Rights::TRANSIT.raw());

/// 只维护 DONE，既不强持 Thread 也不拥有 Pending 启动责任。
pub(crate) struct ThreadControl {
    header: ObjectHeader,
    holders: capability::Holders,
    process_id: u64,
    source: Arc<SignalSource>,
    _metadata: super::resources::MetadataCharge,
}

impl ThreadControl {
    pub(crate) fn done_source(&self) -> Arc<SignalSource> {
        self.source.clone()
    }
    pub(crate) fn try_new(
        process_id: u64,
        pool: Arc<MemoryPool>,
    ) -> Result<Arc<Self>, SystemCallError> {
        let metadata = super::resources::MetadataCharge::arc::<Self>(&pool)?;
        Arc::try_new(Self {
            header: ObjectHeader::try_new().ok_or(SystemCallError::ReachLimit)?,
            holders: capability::Holders::new(capability::ZeroPolicy::Remintable),
            process_id,
            source: SignalSource::try_new(pool)?,
            _metadata: metadata,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub(crate) fn object_ref(this: &Arc<Self>) -> ObjectRef {
        this.clone()
    }
}

impl KernelObject for ThreadControl {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }
    fn holders(&self) -> &capability::Holders {
        &self.holders
    }
    fn kind(&self) -> ObjectKind {
        ObjectKind::ThreadControl
    }
    fn related_id(&self) -> u64 {
        self.process_id
    }
    fn allowed_rights(&self) -> Option<Rights> {
        Some(CONTROL_RIGHTS)
    }
    fn allowed_signals(&self) -> Option<ObjectSignals> {
        Some(ObjectSignals::DONE)
    }

    fn wait_source(&self) -> Option<Arc<dyn ObservationSource>> {
        Some(self.source.clone())
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Self ThreadCreate：调用者在 Running 的自身空间创建可独立委托的 Pending 线程。
pub(crate) fn create(
    caller: &Thread,
    context_address: usize,
    result_address: usize,
) -> Result<(), SystemCallError> {
    let context = {
        let body = caller
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let mut vm = body.vm.space.lock();
        let space = vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?;
        // SAFETY: ThreadStartContext 由四个 u64 组成，任意位型有效。
        unsafe { crate::uaccess::read_user_value::<ThreadStartContext>(space, context_address) }?
    };
    create_in(caller, &caller.process, context, result_address, false)
}

/// Builder 与 self 共用的出生链；输出槽公开前 Job/Shell 仲裁成员提交。
pub(crate) fn create_in(
    caller: &Thread,
    target: &Arc<ProcessShell>,
    context: ThreadStartContext,
    result_address: usize,
    building: bool,
) -> Result<(), SystemCallError> {
    let body = target.lease_body().ok_or(SystemCallError::ObjectClosed)?;
    let reservation = Job::reserve_thread(target, &body)?;
    if reservation.building() != building {
        return Err(SystemCallError::ObjectClosed);
    }
    {
        let mut vm = body.vm.space.lock();
        vm.space
            .as_mut()
            .ok_or(SystemCallError::ObjectClosed)?
            .validate_initial_context(
                usize::try_from(context.entry).map_err(|_| SystemCallError::IllegalArgument)?,
                usize::try_from(context.stack_pointer)
                    .map_err(|_| SystemCallError::IllegalArgument)?,
            )?;
    }
    let domain = body
        .execution
        .lock()
        .ok_or(SystemCallError::ObjectNotAvailable)?;
    let requirement = body
        .requirement
        .lock()
        .ok_or(SystemCallError::ObjectNotAvailable)?;
    let batch = domain.reserve_ready(1)?;
    let control = ThreadControl::try_new(target.header.koid(), target.funding_pool())?;
    let thread = ExecutionThread::try_new(
        reservation.key(),
        target,
        context,
        domain,
        body.vm.satp(),
        requirement == elf::IsaRequirement::D64,
        Some(&control),
        batch,
    )?;
    let ticket = if building {
        None
    } else {
        Some(ThreadStartTicket::try_new(thread.clone())?)
    };
    let pool = caller.process.funding_pool();
    let mut caps = funded_storage::Buffer::try_empty(
        if building { 1 } else { 2 },
        &super::resources::MetadataFunding(&pool),
    )
    .map_err(super::resources::map_storage_error)?;
    caps.push(handle::mint(
        ThreadControl::object_ref(&control),
        CONTROL_RIGHTS,
    )?);
    if let Some(ticket) = &ticket {
        match handle::mint(ThreadStartTicket::object_ref(ticket), START_RIGHTS) {
            Ok(cap) => caps.push(cap),
            Err(error) => {
                handle::release_all(caps);
                return Err(error);
            }
        }
    }
    let tid = reservation.key().tid();
    handle::install(
        caller,
        caps,
        |handles| {
            crate::uaccess::write_user(
                caller,
                result_address,
                &ThreadCreateResult {
                    tid,
                    reserved: 0,
                    control: handles[0],
                    start: handles.get(1).copied().unwrap_or(Handle::INVALID),
                },
            )
        },
        || {
            reservation.commit(MemberState::Created(thread.clone()));
            if let Some(ticket) = &ticket {
                ticket.arm();
            }
        },
    )
    .map(|_| ())
}

pub(crate) fn start(caller: &Thread, ticket: Handle) -> Result<(), SystemCallError> {
    let body = caller
        .process
        .lease_body()
        .ok_or(SystemCallError::ObjectClosed)?;
    let (cap, claim) = {
        let mut table = body.handles.lock();
        let _ = handle::resolve(
            &table,
            ticket,
            Rights::MANAGE,
            ObjectKind::ThreadStartTicket,
        )?;
        table
            .claim(ticket, Rights::MANAGE)
            .map_err(handle::map_error)?
    };
    let result = ThreadStartTicket::concrete(&cap.object().0).and_then(|ticket| ticket.start());
    if result.is_ok() {
        body.handles.lock().commit_claim(claim);
        super::cap::deliver(cap.release());
    } else {
        let restored = body.handles.lock().restore(claim, cap);
        if let capability::Restore::Released(event) = restored {
            super::cap::deliver(event);
        }
    }
    result
}
