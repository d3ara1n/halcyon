//! 用户态 process builder 与运行后 ProcessControl 对象。

use alloc::{sync::Arc, vec::Vec};
use core::any::Any;

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    proc::{ExecutionProfile, HandleGrant, ProcessCreateResult, ProcessMapFlags, ProcessStartDescriptor},
};

use super::{
    Thread,
    object::{HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState, SubscribeResult},
    proc::{Process, SpaceError},
    wait::{Subscription, finish_offered},
};

const MAX_MAP_PAGES: usize = 256;
const MAX_START_PAYLOAD: usize = 64 << 10;
const MAX_START_GRANTS: usize = 64;

struct BuilderState {
    process: Option<Arc<Process>>,
}

pub struct ProcessBuilder {
    header: ObjectHeader,
    state: crate::sync::Spinlock<BuilderState>,
    wait: crate::sync::Spinlock<ObjectWaitState>,
}

impl ProcessBuilder {
    fn new(process: Arc<Process>) -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            header: ObjectHeader::new(),
            state: crate::sync::Spinlock::new(BuilderState { process: Some(process) }),
            wait: crate::sync::Spinlock::new(ObjectWaitState::new(ObjectSignals::NONE)),
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    fn object_ref(builder: &Arc<Self>) -> ObjectRef {
        builder.clone()
    }

    fn process(&self) -> Result<Arc<Process>, SystemCallError> {
        self.state
            .lock()
            .process
            .clone()
            .ok_or(SystemCallError::ObjectNotAvailable)
    }

    fn consume(&self, expected: &Arc<Process>) {
        let actual = self
            .state
            .lock()
            .process
            .take()
            .expect("ProcessBuilder consumed twice");
        assert!(Arc::ptr_eq(&actual, expected), "ProcessBuilder target changed");
    }

    fn abort(&self) {
        self.state.lock().process.take();
    }
}

impl KernelObject for ProcessBuilder {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::ProcessBuilder
    }

    fn allowed_rights(&self, role: HandleRole) -> Option<Rights> {
        (role == HandleRole::ProcessBuilder).then_some(
            Rights::MAP | Rights::WRITE | Rights::MANAGE | Rights::TRANSIT | Rights::GRANT,
        )
    }

    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals> {
        (role == HandleRole::ProcessBuilder).then_some(ObjectSignals::CLOSED)
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
        debug_assert!(role == HandleRole::ProcessBuilder);
        self.abort();
    }

    fn close_transit(&self, role: HandleRole) {
        debug_assert!(role == HandleRole::ProcessBuilder);
        self.abort();
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

struct ControlState {
    wait: ObjectWaitState,
    exit_code: Option<i64>,
}

pub struct ProcessControl {
    header: ObjectHeader,
    state: crate::sync::Spinlock<ControlState>,
}

impl ProcessControl {
    fn new() -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            header: ObjectHeader::new(),
            state: crate::sync::Spinlock::new(ControlState {
                wait: ObjectWaitState::new(ObjectSignals::NONE),
                exit_code: None,
            }),
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    fn object_ref(control: &Arc<Self>) -> ObjectRef {
        control.clone()
    }

    pub fn finish(&self, code: i64) {
        {
            let mut state = self.state.lock();
            if state.exit_code.is_some() {
                return;
            }
            state.exit_code = Some(code);
            state.wait.update(ObjectSignals::NONE, ObjectSignals::CLOSED);
        }
        loop {
            let context = self.state.lock().wait.take_completer();
            let Some(context) = context else { break };
            finish_offered(context);
        }
    }
}

impl KernelObject for ProcessControl {
    fn header(&self) -> &ObjectHeader {
        &self.header
    }

    fn kind(&self) -> ObjectKind {
        ObjectKind::ProcessControl
    }

    fn allowed_rights(&self, role: HandleRole) -> Option<Rights> {
        (role == HandleRole::ProcessControl).then_some(
            Rights::READ
                | Rights::WAIT
                | Rights::MANAGE
                | Rights::DUPLICATE
                | Rights::TRANSIT
                | Rights::GRANT,
        )
    }

    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals> {
        (role == HandleRole::ProcessControl).then_some(ObjectSignals::CLOSED)
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
        debug_assert!(role == HandleRole::ProcessControl);
        // 关闭 control 只消散管理 authority，不终止目标进程。
    }

    fn close_transit(&self, role: HandleRole) {
        debug_assert!(role == HandleRole::ProcessControl);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

pub fn create(
    thread: &Thread,
    job: Handle,
    output: usize,
) -> Result<(), SystemCallError> {
    let job = super::job::resolve(thread, job, Rights::CREATE)?;
    let pid = super::table::alloc_pid();
    let process = Arc::try_new(Process::new(pid, thread.process.pid, job).map_err(map_space_error)?)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    let builder = ProcessBuilder::new(process)?;
    let entry = super::handle::entry(
        ProcessBuilder::object_ref(&builder),
        HandleRole::ProcessBuilder,
        Rights::MAP | Rights::WRITE | Rights::MANAGE | Rights::TRANSIT | Rights::GRANT,
    )
    .map_err(super::handle::map_error)?;

    let mut entries = Vec::new();
    entries.try_reserve(1).map_err(|_| SystemCallError::OutOfMemory)?;
    entries.push(entry);
    let token = super::handle::transaction_token();
    let mut table = thread.process.handles.lock();
    let reservation = table.reserve(1, token).map_err(super::handle::map_error)?;
    let result = ProcessCreateResult {
        builder: reservation.handles()[0],
        pid,
        reserved: 0,
    };
    let mut space = thread.process.space.lock();
    if let Err(error) = space.check_range(output, core::mem::size_of::<ProcessCreateResult>(), true) {
        drop(space);
        table.rollback(reservation).expect("ProcessCreate reservation must remain owned");
        return Err(error.into());
    }
    // SAFETY: ProcessCreateResult 无 padding，输出在同一 space 锁下验证。
    unsafe { crate::uaccess::write_user_value(&mut space, output, &result) }
        .expect("validated ProcessCreate output must remain writable");
    drop(space);
    table.commit(reservation, entries).expect("ProcessCreate reservation count matches entry");
    Ok(())
}

pub fn map(
    thread: &Thread,
    builder_handle: Handle,
    target: usize,
    len: usize,
    permissions: ProcessMapFlags,
) -> Result<(), SystemCallError> {
    if len == 0 || len / super::proc::PAGE_SIZE > MAX_MAP_PAGES {
        return Err(SystemCallError::IllegalArgument);
    }
    let builder_object = resolve_builder(thread, builder_handle, Rights::MAP)?;
    let process = concrete_builder(&builder_object)?.process()?;
    process
        .space
        .lock()
        .map_anonymous(target, len, permissions)
        .map_err(map_space_error)
}

pub fn write(
    thread: &Thread,
    builder_handle: Handle,
    target: usize,
    source: usize,
    len: usize,
) -> Result<(), SystemCallError> {
    if len > crate::uaccess::MAX_USER_ACCESS {
        return Err(SystemCallError::IllegalArgument);
    }
    let builder_object = resolve_builder(thread, builder_handle, Rights::WRITE)?;
    let process = concrete_builder(&builder_object)?.process()?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(len).map_err(|_| SystemCallError::OutOfMemory)?;
    bytes.resize(len, 0);
    crate::uaccess::copy_from_user(&mut thread.process.space.lock(), &mut bytes, source)?;
    process.space.lock().write_building(target, &bytes).map_err(map_space_error)
}

pub fn start(
    thread: &Thread,
    builder_handle: Handle,
    descriptor_ptr: usize,
    output: usize,
) -> Result<(), SystemCallError> {
    let descriptor = unsafe {
        crate::uaccess::read_user_value::<ProcessStartDescriptor>(
            &mut thread.process.space.lock(),
            descriptor_ptr,
        )?
    };
    if descriptor.reserved != 0
        || descriptor.payload_len as usize > MAX_START_PAYLOAD
        || descriptor.grant_count as usize > MAX_START_GRANTS
        || !descriptor.control_rights.is_known()
    {
        return Err(SystemCallError::IllegalArgument);
    }
    let requirement = match descriptor.profile {
        value if value == ExecutionProfile::Base64 as u32 => elf::IsaRequirement::Base64,
        value if value == ExecutionProfile::D64 as u32 => {
            // 多域 eligibility 尚未接线；接受后可能在无 D hart 的 S 态恢复
            // FP 并 fatal。结构就绪前明确拒绝，不把错误声明变成内核异常。
            return Err(SystemCallError::NotSupported);
        }
        _ => return Err(SystemCallError::IllegalArgument),
    };

    let mut payload = Vec::new();
    payload
        .try_reserve_exact(descriptor.payload_len as usize)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    payload.resize(descriptor.payload_len as usize, 0);
    let mut grants = Vec::new();
    grants
        .try_reserve_exact(descriptor.grant_count as usize)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    grants.resize(
        descriptor.grant_count as usize,
        HandleGrant { handle: Handle::INVALID, rights: Rights::NONE },
    );
    {
        let mut caller_space = thread.process.space.lock();
        crate::uaccess::copy_from_user(&mut caller_space, &mut payload, descriptor.payload_ptr as usize)?;
        let grant_bytes = unsafe {
            core::slice::from_raw_parts_mut(
                grants.as_mut_ptr().cast::<u8>(),
                core::mem::size_of_val(grants.as_slice()),
            )
        };
        crate::uaccess::copy_from_user(&mut caller_space, grant_bytes, descriptor.grants_ptr as usize)?;
        caller_space.check_range(output, core::mem::size_of::<Handle>(), true)?;
    }
    if grants.iter().any(|grant| grant.handle == builder_handle) {
        return Err(SystemCallError::IllegalArgument);
    }
    let mut grant_pairs = Vec::new();
    grant_pairs
        .try_reserve_exact(grants.len())
        .map_err(|_| SystemCallError::OutOfMemory)?;
    grant_pairs.extend(grants.iter().map(|grant| (grant.handle, grant.rights)));

    let builder_object = resolve_builder(thread, builder_handle, Rights::MANAGE)?;
    let builder = concrete_builder(&builder_object)?;
    let process = builder.process()?;
    process
        .space
        .lock()
        .validate_initial_context(descriptor.entry as usize, descriptor.stack_pointer as usize)
        .map_err(map_space_error)?;

    let child_token = super::handle::transaction_token();
    let child_reservation = process
        .handles
        .lock()
        .reserve(grants.len(), child_token)
        .map_err(super::handle::map_error)?;
    let block = match erhino_shared::startup::build_startup_block(
        process.pid,
        process.parent,
        child_reservation.handles(),
        &payload,
    ) {
        Ok(block) => block,
        Err(error) => {
            process
                .handles
                .lock()
                .rollback(child_reservation)
                .expect("ProcessStart child reservation must remain owned");
            return Err(match error {
                erhino_shared::startup::StartupBuildError::Overflow => SystemCallError::IllegalArgument,
                erhino_shared::startup::StartupBuildError::AllocationFailed => SystemCallError::OutOfMemory,
            });
        }
    };
    let block_va = match process.space.lock().map_startup_block(&block) {
        Ok(base) => base,
        Err(error) => {
            process
                .handles
                .lock()
                .rollback(child_reservation)
                .expect("ProcessStart child reservation must remain owned");
            return Err(map_space_error(error));
        }
    };

    let control = match ProcessControl::new() {
        Ok(control) => control,
        Err(error) => {
            process.space.lock().rollback_startup_block(block_va, block.len());
            process
                .handles
                .lock()
                .rollback(child_reservation)
                .expect("ProcessStart child reservation must remain owned");
            return Err(error);
        }
    };
    let control_entry = match super::handle::entry(
        ProcessControl::object_ref(&control),
        HandleRole::ProcessControl,
        descriptor.control_rights,
    ) {
        Ok(entry) => entry,
        Err(error) => {
            process.space.lock().rollback_startup_block(block_va, block.len());
            process
                .handles
                .lock()
                .rollback(child_reservation)
                .expect("ProcessStart child reservation must remain owned");
            return Err(super::handle::map_error(error));
        }
    };
    let mut control_entries = Vec::new();
    if control_entries.try_reserve(1).is_err() {
        process.space.lock().rollback_startup_block(block_va, block.len());
        process
            .handles
            .lock()
            .rollback(child_reservation)
            .expect("ProcessStart child reservation must remain owned");
        return Err(SystemCallError::OutOfMemory);
    }
    control_entries.push(control_entry);

    let child_thread = match super::proc::prepare_main_thread(
        process.clone(),
        descriptor.entry as usize,
        requirement,
        descriptor.stack_pointer as usize,
        block_va,
        block.len(),
    ) {
        Ok(thread) => thread,
        Err(error) => {
            process.space.lock().rollback_startup_block(block_va, block.len());
            process
                .handles
                .lock()
                .rollback(child_reservation)
                .expect("ProcessStart child reservation must remain owned");
            return Err(map_space_error(error));
        }
    };
    let table_reservation = match super::table::reserve_insert(process.pid) {
        Ok(reservation) => reservation,
        Err(()) => {
            process.space.lock().rollback_startup_block(block_va, block.len());
            process
                .handles
                .lock()
                .rollback(child_reservation)
                .expect("ProcessStart child reservation must remain owned");
            return Err(SystemCallError::OutOfMemory);
        }
    };
    let ready_reservation = match crate::sched::reserve_ready() {
        Ok(reservation) => reservation,
        Err(()) => {
            super::table::rollback_insert(table_reservation);
            process.space.lock().rollback_startup_block(block_va, block.len());
            process
                .handles
                .lock()
                .rollback(child_reservation)
                .expect("ProcessStart child reservation must remain owned");
            return Err(SystemCallError::OutOfMemory);
        }
    };

    let output_token = super::handle::transaction_token();
    let mut caller_table = thread.process.handles.lock();
    let output_reservation = match caller_table.reserve(1, output_token) {
        Ok(reservation) => reservation,
        Err(error) => {
            drop(caller_table);
            crate::sched::rollback_ready(ready_reservation);
            super::table::rollback_insert(table_reservation);
            process.space.lock().rollback_startup_block(block_va, block.len());
            process
                .handles
                .lock()
                .rollback(child_reservation)
                .expect("ProcessStart child reservation must remain owned");
            return Err(super::handle::map_error(error));
        }
    };
    let control_handle = output_reservation.handles()[0];
    let mut caller_space = thread.process.space.lock();
    if let Err(error) = caller_space.check_range(output, core::mem::size_of::<Handle>(), true) {
        drop(caller_space);
        caller_table
            .rollback(output_reservation)
            .expect("ProcessStart output reservation must remain owned");
        drop(caller_table);
        crate::sched::rollback_ready(ready_reservation);
        super::table::rollback_insert(table_reservation);
        process.space.lock().rollback_startup_block(block_va, block.len());
        process
            .handles
            .lock()
            .rollback(child_reservation)
            .expect("ProcessStart child reservation must remain owned");
        return Err(error.into());
    }
    let moved = match caller_table.extract_grants(&grant_pairs) {
        Ok(entries) => entries,
        Err(error) => {
            drop(caller_space);
            caller_table
                .rollback(output_reservation)
                .expect("ProcessStart output reservation must remain owned");
            drop(caller_table);
            crate::sched::rollback_ready(ready_reservation);
            super::table::rollback_insert(table_reservation);
            process.space.lock().rollback_startup_block(block_va, block.len());
            process
                .handles
                .lock()
                .rollback(child_reservation)
                .expect("ProcessStart child reservation must remain owned");
            return Err(super::handle::map_error(error));
        }
    };

    // 提交区：此前全部可恢复失败已经排除。
    process
        .handles
        .lock()
        .commit(child_reservation, moved)
        .expect("ProcessStart child reservation count matches grants");
    builder.consume(&process);
    let builder_entry = caller_table
        .remove(builder_handle)
        .expect("ProcessStart builder must remain in caller table");
    caller_table
        .commit(output_reservation, control_entries)
        .expect("ProcessStart output reservation count matches control");
    // SAFETY: Handle 无 padding；输出在同一 caller space 锁下验证。
    unsafe { crate::uaccess::write_user_value(&mut caller_space, output, &control_handle) }
        .expect("validated ProcessStart output must remain writable");
    drop(caller_space);
    drop(caller_table);
    super::handle::close_entry(builder_entry, &thread.process, false);

    process.attach_control(control);
    super::table::commit_insert(table_reservation, process);
    crate::sched::commit_ready(ready_reservation, child_thread);
    Ok(())
}

fn resolve_builder(
    thread: &Thread,
    handle: Handle,
    rights: Rights,
) -> Result<ObjectRef, SystemCallError> {
    let table = thread.process.handles.lock();
    let entry = table.get(handle, rights).map_err(super::handle::map_error)?;
    if *entry.role() != HandleRole::ProcessBuilder || entry.object().kind() != ObjectKind::ProcessBuilder {
        return Err(SystemCallError::WrongObjectType);
    }
    Ok(entry.object().clone())
}

fn concrete_builder(object: &ObjectRef) -> Result<&ProcessBuilder, SystemCallError> {
    object
        .as_any()
        .downcast_ref::<ProcessBuilder>()
        .ok_or(SystemCallError::WrongObjectType)
}

fn map_space_error(error: SpaceError) -> SystemCallError {
    match error {
        SpaceError::NoFrame => SystemCallError::OutOfMemory,
        SpaceError::BadSegment => SystemCallError::IllegalArgument,
        SpaceError::Conflict => SystemCallError::InvalidAddress,
    }
}
