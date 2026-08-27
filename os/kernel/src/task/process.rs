//! 用户态 process builder 与贯穿全生命周期的 ProcessControl 对象。

use alloc::{sync::{Arc, Weak}, vec::Vec};
use core::any::Any;

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    proc::{
        ExecutionProfile, HandleGrant, Pid, ProcessCreateResult, ProcessDrainResult, ProcessDrainStatus,
        PROCESS_DRAIN_MAX, ProcessExitReason, ProcessMapFlags, ProcessSnapshot,
        ProcessStartDescriptor, ProcessState, Tid,
    },
};

use super::{
    Thread,
    job::Job,
    lifecycle::TerminationTodo,
    object::{HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState, SubscribeResult},
    proc::{Process, SpaceError},
    wait::{Subscription, WaitOutcome, finish_offered},
};
use wait_context::OfferResult;

const MAX_MAP_PAGES: usize = 256;
const MAX_START_PAYLOAD: usize = 64 << 10;
const MAX_START_GRANTS: usize = 64;

/// ProcessControl 最大 rights（control_rights 请求的校验基准）。
const CONTROL_MAX_RIGHTS: Rights = Rights::from_raw(
    Rights::READ.raw()
        | Rights::WAIT.raw()
        | Rights::MANAGE.raw()
        | Rights::DUPLICATE.raw()
        | Rights::TRANSIT.raw()
        | Rights::GRANT.raw(),
);

struct BuilderState {
    /// weak：builder 不是 core 的生命周期根（Job 成员表才是）；
    /// Dead 后 core 释放即失效。
    process: Option<alloc::sync::Weak<Process>>,
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
            state: crate::sync::Spinlock::new(
                crate::sync::ranks::OBJECT_WAIT,
                BuilderState { process: Some(Arc::downgrade(&process)) },
            ),
            wait: crate::sync::Spinlock::new(
                crate::sync::ranks::OBJECT_WAIT,
                ObjectWaitState::new(ObjectSignals::NONE),
            ),
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    fn object_ref(builder: &Arc<Self>) -> ObjectRef {
        builder.clone()
    }

    /// weak 升级；目标已 Dead（core 释放）时返回 ObjectClosed。
    fn process(&self) -> Result<Arc<Process>, SystemCallError> {
        self.state
            .lock()
            .process
            .as_ref()
            .and_then(alloc::sync::Weak::upgrade)
            .ok_or(SystemCallError::ObjectClosed)
    }

    fn consume(&self) {
        self.state
            .lock()
            .process
            .take()
            .expect("ProcessBuilder consumed twice");
    }

    /// 最后 builder authority 消散（close_handle/close_transit）：Building →
    /// Terminating(Abandoned, 0)，幂等竞争首因。
    fn abort(&self) {
        let Some(process) = self.state.lock().process.take().and_then(|weak| weak.upgrade()) else {
            return;
        };
        let todo = process
            .lifecycle
            .request_termination(ProcessExitReason::Abandoned, 0, None);
        run_termination_todo(&process, todo);
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

/// Dead 冻结快照：core 释放后 shell 仍可应答 Query。
struct DeadSnapshot {
    pid: u64,
    parent_pid: u64,
    reason: ProcessExitReason,
    code: i64,
}

struct ControlState {
    wait: ObjectWaitState,
    core: Weak<Process>,
    dead: Option<DeadSnapshot>,
}

pub struct ProcessControl {
    header: ObjectHeader,
    state: crate::sync::Spinlock<ControlState>,
}

impl ProcessControl {
    pub(crate) fn new(core: &Arc<Process>) -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            header: ObjectHeader::new(),
            state: crate::sync::Spinlock::new(crate::sync::ranks::OBJECT_WAIT, ControlState {
                wait: ObjectWaitState::new(ObjectSignals::NONE),
                core: Arc::downgrade(core),
                dead: None,
            }),
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub(crate) fn object_ref(control: &Arc<Self>) -> ObjectRef {
        control.clone()
    }

    fn core(&self) -> Weak<Process> {
        self.state.lock().core.clone()
    }

    /// 固定宽快照。dead 未冻结时 core 必仍被 Job 成员表强持
    /// （Dead 发布先冻结快照再摘成员），upgrade 必须成功。
    fn snapshot(&self) -> ProcessSnapshot {
        let state = self.state.lock();
        if let Some(dead) = &state.dead {
            return ProcessSnapshot {
                pid: dead.pid,
                parent_pid: dead.parent_pid,
                state: ProcessState::Dead as u32,
                reason: dead.reason as u32,
                code: dead.code,
                reserved: 0,
            };
        }
        let core = state
            .core
            .upgrade()
            .expect("live control core must be held by its job");
        let (state_, reason, code) = core.lifecycle.snapshot();
        ProcessSnapshot {
            pid: core.pid,
            parent_pid: core.parent,
            state: state_ as u32,
            reason: reason as u32,
            code,
            reserved: 0,
        }
    }

    /// REAPABLE 电平：线程与 active hart 均已离场（core 侧锁外调用）。
    /// 持续电平——直至最终批次 publish_dead 清除。完成者逐个经
    /// 「锁内 take 一个 → 锁外 finish」循环交付，不分配（OOM 安全）。
    pub fn publish_reapable(&self) {
        loop {
            let context = {
                let mut state = self.state.lock();
                if state.dead.is_none() {
                    state.wait.update(ObjectSignals::NONE, ObjectSignals::REAPABLE);
                }
                state.wait.take_completer()
            };
            let Some(context) = context else { break };
            finish_offered(context);
        }
    }

    /// Dead 发布：冻结终态快照、清 REAPABLE、置 CLOSED（收束完成点调用）。
    /// 同样不分配。
    pub fn publish_dead(&self, pid: u64, parent_pid: u64, reason: ProcessExitReason, code: i64) {
        loop {
            let context = {
                let mut state = self.state.lock();
                if state.dead.is_none() {
                    state.dead = Some(DeadSnapshot { pid, parent_pid, reason, code });
                    state
                        .wait
                        .update(ObjectSignals::REAPABLE, ObjectSignals::CLOSED);
                }
                state.wait.take_completer()
            };
            let Some(context) = context else { break };
            finish_offered(context);
        }
    }

    /// shell 终态是否已冻结。
    pub fn is_dead(&self) -> bool {
        self.state.lock().dead.is_some()
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
        (role == HandleRole::ProcessControl).then_some(CONTROL_MAX_RIGHTS)
    }

    fn allowed_signals(&self, role: HandleRole) -> Option<ObjectSignals> {
        (role == HandleRole::ProcessControl)
            .then_some(ObjectSignals::REAPABLE | ObjectSignals::CLOSED)
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
    control_rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    if !control_rights.is_known() || !control_rights.is_subset_of(CONTROL_MAX_RIGHTS) {
        return Err(SystemCallError::RightsDenied);
    }
    let job = {
        let table = thread.process.handles.lock();
        let entry = table.get(job, Rights::CREATE).map_err(super::handle::map_error)?;
        if *entry.role() != HandleRole::JobControl || entry.object().kind() != ObjectKind::Job {
            return Err(SystemCallError::WrongObjectType);
        }
        Job::concrete(entry.object())?
    };
    // 创建口闸门（链锁线性化）：链锁内上行检查祖先 seal、锁内分配 Pid
    // 并插入成员占位；后续可失败构造统一由外层回滚占位。
    let (pid, member_reservation) = job.gate_reserve_member()?;
    let staged = create_staged(thread, &job, pid, control_rights, output, member_reservation);
    if staged.is_err() {
        job.rollback_member(member_reservation);
    }
    staged
}

fn create_staged(
    thread: &Thread,
    job: &Arc<Job>,
    pid: Pid,
    control_rights: Rights,
    output: usize,
    member_reservation: super::job::MemberReservation,
) -> Result<(), SystemCallError> {
    let process = Arc::try_new(Process::new(pid, thread.process.pid, Arc::downgrade(&job)).map_err(map_space_error)?)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    let builder = ProcessBuilder::new(process.clone())?;
    let control = ProcessControl::new(&process)?;
    process.set_control(Arc::downgrade(&control));
    let builder_entry = super::handle::entry(
        ProcessBuilder::object_ref(&builder),
        HandleRole::ProcessBuilder,
        Rights::MAP | Rights::WRITE | Rights::MANAGE | Rights::TRANSIT | Rights::GRANT,
    )
    .map_err(super::handle::map_error)?;
    let control_entry = super::handle::entry(
        ProcessControl::object_ref(&control),
        HandleRole::ProcessControl,
        control_rights,
    )
    .map_err(super::handle::map_error)?;

    let mut entries = Vec::new();
    entries.try_reserve(2).map_err(|_| SystemCallError::OutOfMemory)?;
    entries.push(builder_entry);
    entries.push(control_entry);
    let token = super::handle::transaction_token();
    let mut table = thread.process.handles.lock();
    let reservation = table.reserve(2, token).map_err(super::handle::map_error)?;
    let result = ProcessCreateResult {
        builder: reservation.handles()[0],
        control: reservation.handles()[1],
        pid,
        reserved: 0,
    };
    let mut space = thread.process.space.lock();
    if let Err(error) = space.check_range(output, core::mem::size_of::<ProcessCreateResult>(), true) {
        drop(space);
        table.rollback(reservation).expect("ProcessCreate reservation must remain owned");
        return Err(error.into());
    }
    // SAFETY: ProcessCreateResult 无 padding；复检失败即杀本进程
    // （deliver_output），未提交的预留随进程消亡。
    unsafe { crate::uaccess::deliver_output(thread, &mut space, output, &result) }?;
    drop(space);
    // 提交序（F4）：capability 对其他线程可见前先完成不可失败的成员
    // 提交——早干活（kill/drain）必命中已提交成员，不会把 Dead core
    // 提交成永久成员。输出值此刻仍是 Reserved 槽号（不可用），
    // table.commit 后才成为有效 Handle。
    job.commit_member(member_reservation, process);
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
    // Building 操作准入：终止后拒绝新映射（REAPABLE 屏障前提）。
    if !process.lifecycle.enter_building_op() {
        return Err(SystemCallError::ObjectClosed);
    }
    let result = process
        .space
        .lock()
        .map_anonymous(target, len, permissions)
        .map_err(map_space_error);
    leave_building_op(&process);
    result
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
    if !process.lifecycle.enter_building_op() {
        return Err(SystemCallError::ObjectClosed);
    }
    let result = (|| {
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len).map_err(|_| SystemCallError::OutOfMemory)?;
        bytes.resize(len, 0);
        crate::uaccess::copy_from_user(&mut thread.process.space.lock(), &mut bytes, source)?;
        process
            .space
            .lock()
            .write_building(target, &bytes)
            .map_err(map_space_error)
    })();
    leave_building_op(&process);
    result
}

pub fn start(
    thread: &Thread,
    builder_handle: Handle,
    descriptor_ptr: usize,
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
    let builder = &concrete_builder(&builder_object)?;
    let process = builder.process()?;
    // Building 操作准入；此后每个失败路径离开前必须 leave（可能触发
    // REAPABLE）。begin_running 成功即消费登记，其后的防御分支不再 leave。
    if !process.lifecycle.enter_building_op() {
        return Err(SystemCallError::ObjectClosed);
    }
    let result = start_staged(thread, builder_handle, builder, &process, &descriptor, requirement, &payload, &grant_pairs);
    if matches!(result, Err(StartFault::PreCommit(_))) {
        leave_building_op(&process);
    }
    match result {
        Ok(()) => Ok(()),
        Err(StartFault::PreCommit(error)) => Err(error),
    }
}

/// Start 提交阶段失败分类：PreCommit 表示 Building 操作登记尚未消费
/// （外层 leave 配平）；生命周期线性化成功后无失败路径。
enum StartFault {
    PreCommit(SystemCallError),
}

fn rollback_from_block(
    process: &Arc<Process>,
    child_reservation: handle_table::Reservation,
    block_va: usize,
    block_len: usize,
) {
    process.space.lock().rollback_startup_block(block_va, block_len);
    process
        .handles
        .lock()
        .rollback(child_reservation)
        .expect("ProcessStart child reservation must remain owned");
}

#[allow(clippy::too_many_arguments)]
fn start_staged(
    thread: &Thread,
    builder_handle: Handle,
    builder: &ProcessBuilder,
    process: &Arc<Process>,
    descriptor: &ProcessStartDescriptor,
    requirement: elf::IsaRequirement,
    payload: &[u8],
    grant_pairs: &[(Handle, Rights)],
) -> Result<(), StartFault> {
    use StartFault::PreCommit as pre;
    process
        .space
        .lock()
        .validate_initial_context(descriptor.entry as usize, descriptor.stack_pointer as usize)
        .map_err(map_space_error)
        .map_err(pre)?;

    let child_token = super::handle::transaction_token();
    let child_reservation = process
        .handles
        .lock()
        .reserve(grant_pairs.len(), child_token)
        .map_err(super::handle::map_error)
        .map_err(pre)?;
    let block = match erhino_shared::startup::build_startup_block(
        process.pid,
        process.parent,
        child_reservation.handles(),
        payload,
    ) {
        Ok(block) => block,
        Err(error) => {
            process
                .handles
                .lock()
                .rollback(child_reservation)
                .expect("ProcessStart child reservation must remain owned");
            return Err(pre(match error {
                erhino_shared::startup::StartupBuildError::Overflow => SystemCallError::IllegalArgument,
                erhino_shared::startup::StartupBuildError::AllocationFailed => SystemCallError::OutOfMemory,
            }));
        }
    };
    let block_va = match process.space.lock().map_startup_block(&block) {
        Ok(base) => base,
        Err(error) => {
            // 映射失败：地址空间无已提交资源，只回滚 child 预留。
            process
                .handles
                .lock()
                .rollback(child_reservation)
                .expect("ProcessStart child reservation must remain owned");
            return Err(pre(map_space_error(error)));
        }
    };

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
            rollback_from_block(process, child_reservation, block_va, block.len());
            return Err(pre(map_space_error(error)));
        }
    };
    let ready_reservation = match crate::sched::reserve_ready() {
        Ok(reservation) => reservation,
        Err(()) => {
            rollback_from_block(process, child_reservation, block_va, block.len());
            return Err(pre(SystemCallError::OutOfMemory));
        }
    };

    // 提交前最后的可失败步骤（无目标侧副作用）：pin 输出容量预留 +
    // HandleTable 事务 pin（验证 builder MANAGE 与全部 grants 的
    // GRANT/子集，翻转 Pinned——多线程调用方下其他线程不可见、不可关
    // 闭）。不持表锁触碰 lifecycle。
    let mut pinned = Vec::new();
    pinned
        .try_reserve(grant_pairs.len() + 1)
        .map_err(|_| pre(SystemCallError::OutOfMemory))?;
    let pin_token = super::handle::transaction_token();
    {
        let mut caller_table = thread.process.handles.lock();
        if let Err(error) =
            caller_table.pin_for_start(builder_handle, Rights::MANAGE, grant_pairs, pin_token)
        {
            drop(caller_table);
            crate::sched::rollback_ready(ready_reservation);
            rollback_from_block(process, child_reservation, block_va, block.len());
            return Err(pre(super::handle::map_error(error)));
        }
    }

    // 提交线性化点：链锁内「上行检查祖先 seal + Building → Running」
    // （member=Staging，操作登记被消费；lifecycle 锁嵌套于链锁内，锁序
    // Job 链锁 → lifecycle 锁）。失败则无损 unpin 后整体回滚。
    if let Err(error) = super::job::Job::start_commit_gate(process) {
        thread.process.handles.lock().unpin(pin_token);
        crate::sched::rollback_ready(ready_reservation);
        rollback_from_block(process, child_reservation, block_va, block.len());
        return Err(pre(error));
    }

    // 提交区：容量已预留、槽位已 pin——以下全部不可失败。
    thread.process.handles.lock().commit_pinned_into(pin_token, &mut pinned);
    let mut moved = Vec::new();
    let mut builder_entry = None;
    for entry in pinned {
        if *entry.role() == HandleRole::ProcessBuilder {
            debug_assert!(builder_entry.is_none(), "pinned set holds one builder");
            builder_entry = Some(entry);
        } else {
            moved.push(entry);
        }
    }
    let builder_entry = builder_entry.expect("pinned set holds the builder");
    process
        .handles
        .lock()
        .commit(child_reservation, moved)
        .expect("ProcessStart child reservation count matches grants");
    builder.consume();
    super::handle::close_entry(builder_entry, &thread.process, false);
    process.lifecycle.staging_ready(child_thread.tid);
    crate::sched::commit_ready(ready_reservation, child_thread);
    Ok(())
}

/// Building 操作退出（map/write/start 失败路径）：归零且已终止、无线程、
/// 无 active 时发布 REAPABLE（终末 Building 操作触发屏障）。
fn leave_building_op(process: &Arc<Process>) {
    if process.lifecycle.leave_building_op() {
        control_publish_reapable(&process.control());
    }
}

/// 终止待办的锁外执行（IPI、等待取消、REAPABLE 发布）。任何容器路径都
/// 只到达 REAPABLE；Dead 仅由 ProcessDrain 的 Complete 分支发布。
/// 等待取消走锁外游标：每次只持一个 weak context，零分配——摘取与
/// 自然完成的竞争由单 outcome 仲裁，胜者负责线程消散与离场确认。
pub(crate) fn run_termination_todo(process: &Arc<Process>, todo: TerminationTodo) {
    loop {
        let Some(weak) = process.lifecycle.take_first_waiting() else {
            break;
        };
        if let Some(context) = weak.upgrade() {
            if context.offer(WaitOutcome::Abandoned) == OfferResult::Complete {
                // 完成方负责收尾：线程 drop 与离场确认在 finish 内完成。
                finish_offered(context);
            }
        }
    }
    if todo.ipi_slots != 0 {
        crate::registry::ipi_slots(todo.ipi_slots);
    }
    if todo.reapable {
        control_publish_reapable(&process.control());
    }
}

fn control_publish_reapable(control: &Option<Arc<ProcessControl>>) {
    if let Some(control) = control {
        control.publish_reapable();
    }
}

/// 线程离场确认（调用方已 drop 线程强引用：reap / WaitContext 完成
/// 方 / Start 防御失败路径）：摘除成员；全部离场则发布 REAPABLE。
pub fn confirm_departure(process: &Arc<Process>, tid: Tid) {
    if process.lifecycle.thread_departed(tid) {
        control_publish_reapable(&process.control());
    }
}

/// ProcessQuery(control) -> 固定宽快照。
pub fn query(
    thread: &Thread,
    control: Handle,
    output: usize,
) -> Result<(), SystemCallError> {
    let object = resolve_control(thread, control, Rights::READ)?;
    let control = concrete_control(&object)?;
    let snapshot = control.snapshot();
    let mut space = thread.process.space.lock();
    space.check_range(output, core::mem::size_of::<ProcessSnapshot>(), true)?;
    // SAFETY: ProcessSnapshot 字段与 reserved 全部初始化，结构无 padding；
    // 复检失败即杀本进程（deliver_output）。
    unsafe { crate::uaccess::deliver_output(thread, &mut space, output, &snapshot) }
}

/// ProcessKill 出口：自杀式调用已冻结终因，不返回用户态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillOutcome {
    Accepted,
    TerminatedCaller,
}

pub fn kill(
    thread: &Thread,
    control: Handle,
    code: i64,
) -> Result<KillOutcome, SystemCallError> {
    let object = resolve_control(thread, control, Rights::MANAGE)?;
    let control = concrete_control(&object)?;
    let Some(process) = control.core().upgrade() else {
        return Ok(KillOutcome::Accepted); // 已 Dead：幂等成功
    };
    if process.pid == thread.process.pid {
        let todo = process
            .lifecycle
            .request_termination(ProcessExitReason::Killed, code, Some(thread.tid));
        run_termination_todo(&process, todo);
        return Ok(KillOutcome::TerminatedCaller);
    }
    let todo = process
        .lifecycle
        .request_termination(ProcessExitReason::Killed, code, None);
    run_termination_todo(&process, todo);
    Ok(KillOutcome::Accepted)
}

/// ProcessDrain(control, max_work)：REAPABLE/Dead 上推进固定预算收束。
/// 仅 Complete 批次发布 Dead/CLOSED 并从 Job 成员表摘除 core；此后
/// core 只剩空资源壳（root 帧已释放），最后 Arc 何时 drop 不影响
/// Dead 语义。
pub fn drain(
    thread: &Thread,
    control: Handle,
    max_work: u32,
    output: usize,
) -> Result<(), SystemCallError> {
    if max_work == 0 {
        return Err(SystemCallError::IllegalArgument);
    }
    // 输出先验证（P2-1）：坏指针在推进任何资源副作用之前拒绝，
    // 不消耗 drain_gate 预算，也不把批次做一半才报错。
    {
        let mut space = thread.process.space.lock();
        space.check_range(output, core::mem::size_of::<ProcessDrainResult>(), true)?;
    }
    let object = resolve_control(thread, control, Rights::MANAGE)?;
    let control = concrete_control(&object)?;
    let dead_result = || ProcessDrainResult {
        work_done: 0,
        status: ProcessDrainStatus::Complete as u32,
        reserved: 0,
    };
    let Some(process) = control.core().upgrade() else {
        return write_drain_result(thread, output, dead_result());
    };
    if control.is_dead() {
        return write_drain_result(thread, output, dead_result());
    }
    if !control.signals().contains(ObjectSignals::REAPABLE) {
        return Err(SystemCallError::ObjectNotAvailable);
    }
    let Some(_gate) = process.drain_gate.try_lock() else {
        return Err(SystemCallError::ObjectBusy);
    };
    let budget = (max_work as usize).min(PROCESS_DRAIN_MAX as usize);
    let (work, complete) = process.drain_batch(budget);
    if complete {
        // 发布序（外部真值先行）：shell 先冻结终态快照并置 CLOSED
        // （原子清 REAPABLE，外部观察不到 Dead+REAPABLE 混合）；随后
        // core 内部置 Dead；最后从 Job 成员表摘除（此后 core 仅剩空壳）。
        let (_state, reason, code) = process.lifecycle.snapshot();
        control.publish_dead(process.pid, process.parent, reason, code);
        process.lifecycle.mark_dead();
        process.job().remove_member(process.pid);
    }
    // drain_gate 必须先于 process 强引用释放：complete 分支后 core 可能
    // 只剩本局部强引用，Process::Drop 的 close 回调链不得发生在 gate
    // 持有之下（显式 drop，不依赖声明顺序的逆序巧合）。
    drop(_gate);
    write_drain_result(
        thread,
        output,
        ProcessDrainResult {
            work_done: work as u32,
            status: if complete {
                ProcessDrainStatus::Complete as u32
            } else {
                ProcessDrainStatus::More as u32
            },
            reserved: 0,
        },
    )
}

fn write_drain_result(
    thread: &Thread,
    output: usize,
    result: ProcessDrainResult,
) -> Result<(), SystemCallError> {
    let mut space = thread.process.space.lock();
    space.check_range(output, core::mem::size_of::<ProcessDrainResult>(), true)?;
    // SAFETY: ProcessDrainResult 字段与 reserved 全部初始化，无 padding；
    // 复检失败即杀本进程（deliver_output）。
    unsafe { crate::uaccess::deliver_output(thread, &mut space, output, &result) }
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

fn resolve_control(
    thread: &Thread,
    handle: Handle,
    rights: Rights,
) -> Result<ObjectRef, SystemCallError> {
    let table = thread.process.handles.lock();
    let entry = table.get(handle, rights).map_err(super::handle::map_error)?;
    if *entry.role() != HandleRole::ProcessControl || entry.object().kind() != ObjectKind::ProcessControl {
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

fn concrete_control(object: &ObjectRef) -> Result<Arc<ProcessControl>, SystemCallError> {
    let any: Arc<dyn Any + Send + Sync> = object.clone();
    any.downcast::<ProcessControl>()
        .map_err(|_| SystemCallError::WrongObjectType)
}

fn map_space_error(error: SpaceError) -> SystemCallError {
    match error {
        SpaceError::NoFrame => SystemCallError::OutOfMemory,
        SpaceError::BadSegment => SystemCallError::IllegalArgument,
        SpaceError::Conflict => SystemCallError::InvalidAddress,
    }
}
