//! 用户态 process builder 与贯穿全生命周期的 ProcessControl 对象。

use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use core::any::Any;

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    proc::{
        ExecutionProfile, HandleGrant, PROCESS_DRAIN_MAX, PROCESS_MAX_GRANTS, Pid,
        ProcessCreateResult, ProcessDrainResult, ProcessDrainStatus, ProcessExitReason,
        ProcessMapFlags, ProcessSnapshot, ProcessState, ThreadStartContext, Tid,
    },
};

use super::{
    Thread,
    job::Job,
    lifecycle::TerminationTodo,
    object::{
        HandleRole, KernelObject, ObjectHeader, ObjectKind, ObjectRef, ObjectWaitState,
        SubscribeResult,
    },
    proc::{Process, SpaceError, ThreadAttachError},
    wait::{Subscription, WaitOutcome, finish_offered},
};
use wait_context::OfferResult;

const MAX_MAP_PAGES: usize = 256;

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
    _metadata: super::resources::BuilderPermit,
    state: crate::sync::Spinlock<BuilderState>,
    wait: crate::sync::Spinlock<ObjectWaitState>,
}

impl ProcessBuilder {
    fn new(process: Arc<Process>) -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            _metadata: super::resources::MetadataSponsor::reserve_builder(
                process.resources.metadata(),
            )?,
            header: ObjectHeader::new(),
            state: crate::sync::Spinlock::new(
                crate::sync::ranks::OBJECT_WAIT,
                BuilderState {
                    process: Some(Arc::downgrade(&process)),
                },
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
        let Some(process) = self
            .state
            .lock()
            .process
            .take()
            .and_then(|weak| weak.upgrade())
        else {
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
    _metadata: super::resources::ControlPermit,
    state: crate::sync::Spinlock<ControlState>,
}

impl ProcessControl {
    pub(crate) fn new(core: &Arc<Process>) -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            _metadata: super::resources::MetadataSponsor::reserve_control(
                core.resources.metadata(),
            )?,
            header: ObjectHeader::new(),
            state: crate::sync::Spinlock::new(
                crate::sync::ranks::OBJECT_WAIT,
                ControlState {
                    wait: ObjectWaitState::new(ObjectSignals::NONE),
                    core: Arc::downgrade(core),
                    dead: None,
                },
            ),
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
                    state
                        .wait
                        .update(ObjectSignals::NONE, ObjectSignals::REAPABLE);
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
                    state.dead = Some(DeadSnapshot {
                        pid,
                        parent_pid,
                        reason,
                        code,
                    });
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
        let entry = table
            .get(job, Rights::CREATE)
            .map_err(super::handle::map_error)?;
        if *entry.role() != HandleRole::JobControl || entry.object().kind() != ObjectKind::Job {
            return Err(SystemCallError::WrongObjectType);
        }
        Job::concrete(entry.object())?
    };
    // 创建口闸门（链锁线性化）：链锁内上行检查祖先 seal、锁内分配 Pid
    // 并插入成员占位；后续可失败构造统一由外层回滚占位。
    let (pid, member_reservation) = job.gate_reserve_member()?;
    let staged = create_staged(
        thread,
        &job,
        pid,
        control_rights,
        output,
        member_reservation,
    );
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
    let resources = super::resources::ProcessResources::try_new()?;
    let process = Arc::try_new(
        Process::new(pid, thread.process.pid, Arc::downgrade(&job), resources)
            .map_err(map_space_error)?,
    )
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
    entries
        .try_reserve(2)
        .map_err(|_| SystemCallError::OutOfMemory)?;
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
    if let Err(error) = space.check_range(output, core::mem::size_of::<ProcessCreateResult>(), true)
    {
        drop(space);
        table
            .rollback(reservation)
            .expect("ProcessCreate reservation must remain owned");
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
    table
        .commit(reservation, entries)
        .expect("ProcessCreate reservation count matches entry");
    Ok(())
}

/// 为 Unbound shell 准备完整 BoundAddressSpace。全部可失败工作发生在发布前；
/// owner 析构同时回滚 metadata permit、root frame 与 Pool charge。
fn prepare_memory_binding(
    process: &Arc<Process>,
    pool: Arc<super::memory_pool::MemoryPool>,
) -> Result<alloc::boxed::Box<super::proc::BoundAddressSpace>, SystemCallError> {
    let binding = super::resources::PoolBinding::prepare(pool, process.resources.metadata())?;
    super::proc::BoundAddressSpace::new(binding).map_err(map_space_error)
}

/// Bootstrap 复用普通 Bind 的资源准备与 Unbound→Bound 提交语义；唯一差异是 root
/// Pool authority 来自 primordial owner，不经过用户 Handle pin。
pub(crate) fn bind_memory_internal(
    process: &Arc<Process>,
    pool: Arc<super::memory_pool::MemoryPool>,
) -> Result<(), SystemCallError> {
    let bound = prepare_memory_binding(process, pool)?;
    process
        .space
        .lock()
        .bind(bound)
        .map_err(|_| SystemCallError::ObjectNotAvailable)
}

/// ProcessBindMemory(builder, pool)：登记获胜的 Building operation。builder 只作受保护
/// authority，pool entry 成功时被消费；Bound 发布与逻辑消费在同一双锁提交段完成。
pub fn bind_memory(
    thread: &Thread,
    builder_handle: Handle,
    pool_handle: Handle,
) -> Result<(), SystemCallError> {
    if builder_handle == pool_handle {
        return Err(SystemCallError::IllegalArgument);
    }
    let builder_object = resolve_builder(thread, builder_handle, Rights::MANAGE)?;
    let process = concrete_builder(&builder_object)?.process()?;
    let _lease = BuildingLease::begin(process.clone())?;
    if process.space.lock().is_bound() {
        return Err(SystemCallError::ObjectNotAvailable);
    }
    let _bind_reservation = process.resources.try_reserve_binding()?;
    // 与前一竞争者释放 reservation 后再判一次，保证串行重复 Bind 不消耗额度。
    if process.space.lock().is_bound() {
        return Err(SystemCallError::ObjectNotAvailable);
    }

    let pin_token = super::handle::transaction_token();
    let (pool, pool_rights) = {
        let mut table = thread.process.handles.lock();
        let entry = table
            .get(pool_handle, Rights::GRANT)
            .map_err(super::handle::map_error)?;
        if *entry.role() != HandleRole::MemoryPool
            || entry.object().kind() != ObjectKind::MemoryPool
        {
            return Err(SystemCallError::WrongObjectType);
        }
        let pool = super::memory_pool::MemoryPool::concrete(entry.object())?;
        let rights = entry.rights();
        table
            .pin_transfer(
                builder_handle,
                Rights::MANAGE,
                &[(pool_handle, rights)],
                pin_token,
            )
            .map_err(super::handle::map_error)?;
        (pool, rights)
    };

    let mut moved = Vec::new();
    if moved.try_reserve_exact(1).is_err() {
        thread.process.handles.lock().unpin(pin_token);
        return Err(SystemCallError::OutOfMemory);
    }
    let bound = match prepare_memory_binding(&process, pool) {
        Ok(bound) => bound,
        Err(error) => {
            thread.process.handles.lock().unpin(pin_token);
            return Err(error);
        }
    };

    {
        // HANDLE_TABLE → ADDRESS_SPACE 符合 Lock Ladder。两 entry 均处于 Pinned，
        // 因而其它线程只能观察 ObjectBusy，不能越过提交点消费或关闭。
        let mut table = thread.process.handles.lock();
        let mut space = process.space.lock();
        if space.is_bound() {
            drop(space);
            table.unpin(pin_token);
            return Err(SystemCallError::ObjectNotAvailable);
        }
        space
            .bind(bound)
            .unwrap_or_else(|_| unreachable!("Unbound bind precheck changed under one lock"));
        table.commit_pinned_transfer(
            pin_token,
            builder_handle,
            &[(pool_handle, pool_rights)],
            &mut moved,
        );
    }
    let pool_entry = moved
        .pop()
        .expect("BindMemory committed Pool entry missing");
    super::handle::close_entry_infallible(pool_entry, &thread.process, false);
    Ok(())
}

struct BuildingLease {
    process: Arc<Process>,
    active: bool,
}

impl BuildingLease {
    fn begin(process: Arc<Process>) -> Result<Self, SystemCallError> {
        if !process.lifecycle.enter_building_op() {
            return Err(SystemCallError::ObjectClosed);
        }
        Ok(Self {
            process,
            active: true,
        })
    }

    /// 只有持有已登记 lease 才能调用终止截止后仍具提交资格的 Attach seam。
    fn attach_thread(&self, context: ThreadStartContext) -> Result<Tid, ThreadAttachError> {
        self.process.attach_thread_registered(context)
    }

    /// begin_running 已在 lifecycle 线性化点消费 Building 操作登记。
    fn commit_running(mut self) {
        self.active = false;
    }
}

impl Drop for BuildingLease {
    fn drop(&mut self) {
        if self.active && self.process.lifecycle.leave_building_op() {
            control_publish_reapable(&self.process.control());
        }
    }
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
    let _lease = BuildingLease::begin(process.clone())?;
    let (plan, pool) = {
        let mut space = process.space.lock();
        let plan = space
            .plan_anonymous_mapping(target, len, permissions)
            .map_err(map_space_error)?;
        let pool = Arc::clone(space.pool());
        (plan, pool)
    };
    let funded = match super::proc::fund_owned_mapping(&pool, &plan) {
        Ok(funded) => funded,
        Err(_) => {
            process.space.lock().rollback_owned_mapping_plan(plan);
            return Err(SystemCallError::OutOfMemory);
        }
    };
    let released = process
        .space
        .lock()
        .complete_anonymous_mapping(plan, funded)
        .map_err(map_space_error)?;
    drop(released);
    Ok(())
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
    let _lease = BuildingLease::begin(process.clone())?;
    (|| {
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(len)
            .map_err(|_| SystemCallError::OutOfMemory)?;
        bytes.resize(len, 0);
        crate::uaccess::copy_from_user(&mut thread.process.space.lock(), &mut bytes, source)?;
        process
            .space
            .lock()
            .write_building(target, &bytes)
            .map_err(map_space_error)
    })()
}

/// ProcessAttach(builder, descriptor)：组装者向 Building process 附入
/// 线程（线程是组装资源）。内核零资源分配——栈与出生块由组装者经
/// Map/Write 供给；本调用只创建执行基底并插入预育表（Staging）。
/// 无观察壳：组装不是协作，组装者在 Start 之后不再观察内部状态。
pub fn attach(
    thread: &Thread,
    builder_handle: Handle,
    descriptor_ptr: usize,
) -> Result<Tid, SystemCallError> {
    let descriptor = unsafe {
        crate::uaccess::read_user_value::<ThreadStartContext>(
            &mut thread.process.space.lock(),
            descriptor_ptr,
        )?
    };
    let builder_object = resolve_builder(thread, builder_handle, Rights::MANAGE)?;
    let builder = &concrete_builder(&builder_object)?;
    let process = builder.process()?;
    let lease = BuildingLease::begin(process)?;
    lease
        .attach_thread(descriptor)
        .map_err(|error| match error {
            ThreadAttachError::Context(error) => map_space_error(error),
            ThreadAttachError::Closed => SystemCallError::ObjectClosed,
            ThreadAttachError::Limit => SystemCallError::ReachLimit,
            ThreadAttachError::Oom => SystemCallError::OutOfMemory,
        })
}

/// ProcessGrant(builder, grants_ptr, count, out_values)：组装者把 grants
/// 从本表移入目标 Building process 的 HandleTable 并输出目标侧句柄值
/// （组装者将其写入出生块后经 ProcessWrite 交付）。pin 事务保证输出
/// 失败无损还原；成功即目标表可见。
pub fn grant(
    thread: &Thread,
    builder_handle: Handle,
    grants_ptr: usize,
    count: usize,
    out_values: usize,
) -> Result<(), SystemCallError> {
    if count == 0 || count > PROCESS_MAX_GRANTS {
        return Err(SystemCallError::IllegalArgument);
    }
    let mut grants = Vec::new();
    grants
        .try_reserve_exact(count)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    grants.resize(
        count,
        HandleGrant {
            handle: Handle::INVALID,
            rights: Rights::NONE,
        },
    );
    {
        let mut caller_space = thread.process.space.lock();
        let grant_bytes = unsafe {
            core::slice::from_raw_parts_mut(
                grants.as_mut_ptr().cast::<u8>(),
                core::mem::size_of_val(grants.as_slice()),
            )
        };
        crate::uaccess::copy_from_user(&mut caller_space, grant_bytes, grants_ptr)?;
    }
    let grant_pairs: Vec<(Handle, Rights)> = grants
        .iter()
        .map(|grant| (grant.handle, grant.rights))
        .collect();

    let builder_object = resolve_builder(thread, builder_handle, Rights::MANAGE)?;
    let builder = &concrete_builder(&builder_object)?;
    let process = builder.process()?;
    let _lease = BuildingLease::begin(process.clone())?;
    (|| {
        // 调用者表 pin：原子验证并翻转（失败零副作用）。
        let pin_token = super::handle::transaction_token();
        {
            let mut caller_table = thread.process.handles.lock();
            caller_table
                .pin_transfer(builder_handle, Rights::MANAGE, &grant_pairs, pin_token)
                .map_err(super::handle::map_error)?;
        }
        // 提取前完成全部可失败步骤：目标预留 + 提交缓冲预留——此后
        // 的失败路径（交付失败）只需无损还原 pin 与预留，不存在
        // 「条目已离开调用者表」的中间态。
        let target_token = super::handle::transaction_token();
        let reservation = match process
            .handles
            .lock()
            .reserve(grant_pairs.len(), target_token)
        {
            Ok(reservation) => reservation,
            Err(error) => {
                thread.process.handles.lock().unpin(pin_token);
                return Err(super::handle::map_error(error));
            }
        };
        let mut moved = Vec::new();
        if moved.try_reserve_exact(grant_pairs.len()).is_err() {
            process
                .handles
                .lock()
                .rollback(reservation)
                .expect("ProcessGrant reservation must remain owned");
            thread.process.handles.lock().unpin(pin_token);
            return Err(SystemCallError::OutOfMemory);
        }
        // 句柄值交付：复检失败即杀调用进程（deliver_output 已冻结终因
        // 并组装 todo，分发出口终止检查收束），此处只做无损还原。
        {
            let mut caller_space = thread.process.space.lock();
            for (index, value) in reservation.handles().iter().enumerate() {
                let dst = out_values + index * core::mem::size_of::<Handle>();
                // SAFETY: Handle 为无 padding 的 u64 newtype。
                let deliver = unsafe {
                    crate::uaccess::deliver_output(thread, &mut caller_space, dst, value)
                };
                if let Err(error) = deliver {
                    drop(caller_space);
                    process
                        .handles
                        .lock()
                        .rollback(reservation)
                        .expect("ProcessGrant reservation must remain owned");
                    thread.process.handles.lock().unpin(pin_token);
                    return Err(error);
                }
            }
        }
        // 提交区：pinned 已验证、容量已预留、值已交付——以下不可失败。
        {
            let mut caller_table = thread.process.handles.lock();
            caller_table.commit_pinned_transfer(
                pin_token,
                builder_handle,
                &grant_pairs,
                &mut moved,
            );
        }
        process
            .handles
            .lock()
            .commit(reservation, moved)
            .expect("ProcessGrant reservation count matches entries");
        Ok(())
    })()
}

/// ProcessStart(builder, profile)：活体门（已附线程 ≥1）→ Building →
/// Running → execution binding 冻结 → 预育线程整体入册（转 Ready）。
/// 唯一首次发布 runnable 的提交点；builder 在此消费。
pub fn start(
    thread: &Thread,
    builder_handle: Handle,
    profile: usize,
) -> Result<(), SystemCallError> {
    let requirement = match profile {
        value if value == ExecutionProfile::Base64 as usize => elf::IsaRequirement::Base64,
        value if value == ExecutionProfile::D64 as usize => elf::IsaRequirement::D64,
        _ => return Err(SystemCallError::IllegalArgument),
    };

    let builder_object = resolve_builder(thread, builder_handle, Rights::MANAGE)?;
    let builder = &concrete_builder(&builder_object)?;
    let process = builder.process()?;
    let lease = BuildingLease::begin(process.clone())?;
    if !process.space.lock().is_bound() {
        return Err(SystemCallError::ObjectNotAvailable);
    }
    match start_staged(thread, builder_handle, builder, &process, requirement) {
        Ok(()) => {
            lease.commit_running();
            Ok(())
        }
        Err(error) => Err(error),
    }
}

fn start_staged(
    thread: &Thread,
    builder_handle: Handle,
    builder: &ProcessBuilder,
    process: &Arc<Process>,
    requirement: elf::IsaRequirement,
) -> Result<(), SystemCallError> {
    // eligibility：执行需求 → 兼容域中最弱者（平台无兼容 hart 是
    // NotSupported 的平台事实语义；域绑定在提交点冻结）。
    let domain = crate::sched::resolve_domain(requirement).ok_or(SystemCallError::NotSupported)?;

    // 可失败段：按预育成员数预留就绪批次与提交缓冲。计数读点与 gate
    // 判点之间存在并发 attach 窗口——begin_running 以 expected 拒绝
    // 插队（ObjectBusy），组装者以新计数重试。
    let count = process.lifecycle.member_count();
    if count == 0 {
        return Err(SystemCallError::ObjectNotAvailable); // 活体门：从未活过
    }
    let mut staged = Vec::new();
    staged
        .try_reserve_exact(count)
        .map_err(|_| SystemCallError::OutOfMemory)?;

    // 提交前最后的可失败步骤：调用者表内原子 pin builder。
    let pin_token = super::handle::transaction_token();
    {
        let mut caller_table = thread.process.handles.lock();
        if let Err(error) = caller_table.pin_consume(builder_handle, Rights::MANAGE, pin_token) {
            return Err(super::handle::map_error(error));
        }
    }
    // 就绪容量整批原子预留，失败不留下部分 marker。
    let ready_batch = match crate::sched::reserve_ready_batch(domain, count) {
        Ok(batch) => batch,
        Err(()) => {
            thread.process.handles.lock().unpin(pin_token);
            return Err(SystemCallError::OutOfMemory);
        }
    };

    // 提交线性化点：链锁内「上行检查祖先 seal + Building → Running
    // （含预育提取）」——活体门、计数一致性与 Staging 强引用交出在
    // 同一 lifecycle 锁内原子完成，消除 gate 后、入队前的 kill 游标
    // 窗口。失败则无损 unpin 后回滚就绪预留。
    if let Err(error) = super::job::Job::start_commit_gate(process, count, &mut staged) {
        thread.process.handles.lock().unpin(pin_token);
        crate::sched::rollback_ready_batch(ready_batch);
        return Err(error);
    }

    // 提交区：容量已预留、槽位已 pin、线程已交出——以下全部不可失败。
    // requirement 与兼容域在此合为单一 execution binding；线程随后只经
    // 所属域的类队列出现。
    process.bind_execution(requirement, domain);
    let builder_entry = thread
        .process
        .handles
        .lock()
        .commit_pinned_consume(pin_token, builder_handle);
    builder.consume();
    super::handle::close_entry_infallible(builder_entry, &thread.process, false);
    crate::sched::commit_ready_batch(ready_batch, staged);
    Ok(())
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
    // 预育成员游标：预育线程从未进入容器，摘除即完成（强引用锁外释放，
    // 打破 Building 期的 Process↔Thread 引用环）。
    while let Some(thread) = process.lifecycle.take_first_staging() {
        drop(thread);
    }
    if todo.ipi_slots != 0 {
        crate::registry::ipi_slots(todo.ipi_slots);
    }
    // Staging 摘除可能使 REAPABLE 条件达成（todo 组装时成员尚在）。
    if todo.reapable || process.lifecycle.is_reapable() {
        control_publish_reapable(&process.control());
    }
}

fn control_publish_reapable(control: &Option<Arc<ProcessControl>>) {
    if let Some(control) = control {
        control.publish_reapable();
    }
}

/// 线程级结果义务归零后的离场确认。正常末线程在 lifecycle 线性化点铸造
/// 进程终局；已有进程级终止则只协助 REAPABLE 发布。
pub fn confirm_departure(process: &Arc<Process>, tid: Tid, normal_code: Option<i64>) {
    let (termination, reapable) = process.lifecycle.thread_departed(tid, normal_code);
    if let Some(todo) = termination {
        run_termination_todo(process, todo);
    } else if reapable {
        control_publish_reapable(&process.control());
    }
}

/// ProcessQuery(control) -> 固定宽快照。
pub fn query(thread: &Thread, control: Handle, output: usize) -> Result<(), SystemCallError> {
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

pub fn kill(thread: &Thread, control: Handle, code: i64) -> Result<KillOutcome, SystemCallError> {
    let object = resolve_control(thread, control, Rights::MANAGE)?;
    let control = concrete_control(&object)?;
    let Some(process) = control.core().upgrade() else {
        return Ok(KillOutcome::Accepted); // 已 Dead：幂等成功
    };
    if process.pid == thread.process.pid {
        let todo = process.lifecycle.request_termination(
            ProcessExitReason::Killed,
            code,
            Some(thread.tid),
        );
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
    debug_assert!(work <= budget, "drain over budget: {} > {}", work, budget);
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
    let entry = table
        .get(handle, rights)
        .map_err(super::handle::map_error)?;
    if *entry.role() != HandleRole::ProcessBuilder
        || entry.object().kind() != ObjectKind::ProcessBuilder
    {
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
    let entry = table
        .get(handle, rights)
        .map_err(super::handle::map_error)?;
    if *entry.role() != HandleRole::ProcessControl
        || entry.object().kind() != ObjectKind::ProcessControl
    {
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
        SpaceError::Busy => SystemCallError::ObjectBusy,
        SpaceError::Unbound => SystemCallError::ObjectNotAvailable,
    }
}
