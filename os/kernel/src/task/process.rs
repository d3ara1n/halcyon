//! 进程创建与组装：Shell 持生命周期，Body 只通过短租约访问，Builder 持构造授权。

pub(crate) mod body;
pub(crate) mod builder;
pub(crate) mod departure;
pub(crate) mod members;
pub(crate) mod reclaim;
pub(crate) mod shell;
pub mod thread;
pub(crate) mod ticket;

use super::resources::{MetadataBuffer, MetadataFunding, map_storage_error};
use super::{
    Thread,
    handle::{self, Authorization, Invocation},
    job::Job,
    memory_pool::MemoryPool,
};
use alloc::sync::Arc;
use builder::ProcessBuilder;
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, Rights},
    proc::{
        ExecutionProfile, HandleGrant, PROCESS_MAX_GRANTS, ProcessCreateResult, ProcessMapFlags,
        TerminationCause, TerminationDetail, ThreadStartContext,
    },
};
use shell::ProcessShell;

/// Shell 骨架先进入 Job；此后的任何失败均从 Frozen 进入同一回收记录。
pub(crate) fn construct(
    job: &Arc<Job>,
    pool: &Arc<MemoryPool>,
    parent: u64,
) -> Result<Arc<ProcessShell>, SystemCallError> {
    let (pid, reservation) = job.gate_reserve_member()?;
    let (shell, construction) = match ProcessShell::try_new(pid, parent, Arc::downgrade(job), pool)
    {
        Ok(value) => value,
        Err(error) => {
            job.rollback_member(reservation);
            return Err(error);
        }
    };
    job.commit_member(reservation, shell.clone());
    let result = (|| {
        let space = crate::vm::space::AnySpace::empty(pool.clone())?;
        let handles = handle::ProcessCapTable::try_new(&super::resources::MetadataFunding(pool))
            .map_err(handle::map_error)?;
        let body = body::ProcessBody::try_new(space, pool, handles, shell.source.clone())?;
        let body =
            funded_storage::OwnedBox::try_new(body, &super::resources::MetadataFunding(pool))
                .map_err(|(error, body)| {
                    drop(body);
                    super::resources::map_storage_error(error)
                })?;
        construction.install_body(body).map_err(|body| {
            // 尚未安装的 Body 必为空；唯一 root 表及准入存储可有界局部退款。
            drop(body);
            SystemCallError::ObjectClosed
        })
    })();
    if result.is_err() {
        shell.freeze_and_seal(TerminationCause::Aborted, TerminationDetail::NONE);
    }
    drop(construction);
    result.map(|()| shell)
}

pub fn create(
    caller: &Thread,
    job_handle: Handle,
    pool_handle: Handle,
    rights: Rights,
    output: usize,
) -> Result<(), SystemCallError> {
    if !rights.is_known() || !rights.is_subset_of(shell::CONTROL_RIGHTS) {
        return Err(SystemCallError::RightsDenied);
    }
    let job_authority = Authorization::accept(caller, job_handle, Rights::CREATE)?;
    let job = Job::concrete(job_authority.object())?;
    let pool_authority = Authorization::accept(caller, pool_handle, Rights::CREATE)?;
    let pool = MemoryPool::concrete(pool_authority.object())?;
    let shell = construct(&job, &pool, caller.process.pid)?;
    let result = (|| {
        let builder = ProcessBuilder::try_new(shell.clone())?;
        let builder_cap = handle::mint(ProcessBuilder::object_ref(&builder), builder::RIGHTS)?;
        let control_cap = match handle::mint(ProcessShell::object_ref(&shell), rights) {
            Ok(cap) => cap,
            Err(error) => {
                handle::release_all([builder_cap]);
                return Err(error);
            }
        };
        let caps = [builder_cap, control_cap];
        handle::install(
            caller,
            caps,
            |handles| {
                crate::uaccess::write_user(
                    caller,
                    output,
                    &ProcessCreateResult {
                        builder: handles[0],
                        control: handles[1],
                        pid: shell.pid,
                        reserved: 0,
                    },
                )
            },
            || builder.arm(),
        )
        .map(|_| ())
    })();
    if result.is_err() {
        shell.freeze_and_seal(TerminationCause::Aborted, TerminationDetail::NONE);
    }
    result
}

fn builder(
    caller: &Thread,
    handle: Handle,
) -> Result<(Authorization, Arc<ProcessBuilder>), SystemCallError> {
    let authority = Authorization::accept(caller, handle, Rights::MANAGE)?;
    let builder = ProcessBuilder::concrete(authority.object())?;
    Ok((authority, builder))
}

pub fn map(
    caller: &Thread,
    handle: Handle,
    address: usize,
    bytes: usize,
    flags: ProcessMapFlags,
) -> Result<super::wait::WaitPlan, SystemCallError> {
    if bytes == 0 || bytes > erhino_shared::proc::PROCESS_MAP_MAX_BYTES || !flags.is_known() {
        return Err(SystemCallError::IllegalArgument);
    }
    let protection = match flags.raw() {
        1 => memory_space::Protection::ReadOnly,
        3 => memory_space::Protection::ReadWrite,
        5 => memory_space::Protection::ReadExecute,
        _ => return Err(SystemCallError::IllegalArgument),
    };
    let (_authority, builder) = builder(caller, handle)?;
    let target = builder.process()?;
    let body = target
        .lease_building()
        .ok_or(SystemCallError::ObjectClosed)?;
    crate::vm::operation::initial_map(&body, address, bytes, protection, None)
}

pub fn write(
    caller: &Thread,
    handle: Handle,
    target: usize,
    source: usize,
    len: usize,
) -> Result<(), SystemCallError> {
    if len > crate::uaccess::MAX_USER_ACCESS {
        return Err(SystemCallError::IllegalArgument);
    }
    let (_authority, builder) = builder(caller, handle)?;
    let target_process = builder.process()?;
    let body = target_process
        .lease_building()
        .ok_or(SystemCallError::ObjectClosed)?;
    let pool = caller.process.funding_pool();
    let mut bytes =
        MetadataBuffer::try_empty(len, &MetadataFunding(&pool)).map_err(map_storage_error)?;
    for _ in 0..len {
        bytes.push(0);
    }
    {
        let caller_body = caller
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let mut vm = caller_body.vm.space.lock();
        crate::uaccess::copy_from_user(
            vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?,
            &mut bytes,
            source,
        )?;
    }
    crate::vm::operation::initial_write(&body, target, &bytes)
}

pub fn create_thread(
    caller: &Thread,
    handle: Handle,
    input: usize,
    output: usize,
) -> Result<(), SystemCallError> {
    let context: ThreadStartContext = {
        let body = caller
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let mut vm = body.vm.space.lock();
        // SAFETY: 四个 u64，任意位型有效；目标空间随后验证 entry/sp。
        unsafe {
            crate::uaccess::read_user_value(
                vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?,
                input,
            )
        }?
    };
    let (_authority, builder) = builder(caller, handle)?;
    super::thread::create_in(caller, &builder.process()?, context, output, true)
}

pub fn grant(
    caller: &Thread,
    handle: Handle,
    input: usize,
    count: usize,
    output: usize,
) -> Result<(), SystemCallError> {
    if count == 0 || count > PROCESS_MAX_GRANTS {
        return Err(SystemCallError::IllegalArgument);
    }
    output
        .checked_add(count * core::mem::size_of::<Handle>())
        .ok_or(SystemCallError::IllegalArgument)?;
    let (_authority, builder) = builder(caller, handle)?;
    let target = builder.process()?;
    let body = target
        .lease_building()
        .ok_or(SystemCallError::ObjectClosed)?;
    let pool = caller.process.funding_pool();
    let funding = MetadataFunding(&pool);
    let mut grants = MetadataBuffer::try_empty(count, &funding).map_err(map_storage_error)?;
    for _ in 0..count {
        grants.push(HandleGrant {
            handle: Handle::INVALID,
            rights: Rights::NONE,
        });
    }
    {
        let caller_body = caller
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let mut vm = caller_body.vm.space.lock();
        // SAFETY: HandleGrant 是两个整数 newtype，完整覆盖后再验证 rights。
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(
                grants.as_mut_ptr().cast::<u8>(),
                core::mem::size_of_val(&*grants),
            )
        };
        crate::uaccess::copy_from_user(
            vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?,
            bytes,
            input,
        )?;
    }
    let mut claimed = MetadataBuffer::try_empty(count, &funding).map_err(map_storage_error)?;
    let mut caps = MetadataBuffer::try_empty(count, &funding).map_err(map_storage_error)?;
    for grant in grants.iter() {
        let claim = Invocation::claim(caller, grant.handle, Rights::GRANT)?;
        if !grant.rights.is_known() || !grant.rights.is_subset_of(claim.cap().rights()) {
            return Err(SystemCallError::RightsDenied);
        }
        claimed.push(claim);
    }
    let reservation = body
        .handles
        .lock()
        .reserve(count, &super::resources::MetadataFunding(target.pool()))
        .map_err(handle::map_error)?;
    for (index, value) in reservation.handles().iter().enumerate() {
        if let Err(error) = crate::uaccess::write_user(
            caller,
            output + index * core::mem::size_of::<Handle>(),
            value,
        ) {
            body.handles.lock().rollback(reservation);
            return Err(error);
        }
    }
    // 输出完成后必成；失败前不收窄原 authority，逐个 claim 的 Drop 自动恢复。
    for (claim, grant) in claimed.iter_mut().zip(grants.iter()) {
        let mut cap = claim.take_for_transfer();
        cap.attenuate(grant.rights)
            .expect("validated grant rights changed");
        claim.commit_transfer();
        caps.push(cap);
    }
    let committed = body.handles.lock().commit(reservation, caps);
    match committed {
        Ok(capability::Commit::Installed(_)) => {}
        Ok(capability::Commit::Released { events, .. }) => {
            for event in events {
                super::cap::deliver(Some(event));
            }
        }
        Err((caps, _)) => {
            handle::release_all(caps);
            unreachable!("owned grant reservation changed");
        }
    }
    Ok(())
}

pub fn start(caller: &Thread, handle: Handle) -> Result<super::wait::WaitPlan, SystemCallError> {
    let mut authority = Invocation::claim(caller, handle, Rights::MANAGE)?;
    let builder = ProcessBuilder::concrete(authority.object())?;
    let plan = builder.start()?;
    authority.consume();
    Ok(plan)
}

pub fn bind_execution(
    caller: &Thread,
    handle: Handle,
    profile: usize,
) -> Result<(), SystemCallError> {
    let requirement = match profile {
        value if value == ExecutionProfile::Base64 as usize => elf::IsaRequirement::Base64,
        value if value == ExecutionProfile::D64 as usize => elf::IsaRequirement::D64,
        _ => return Err(SystemCallError::IllegalArgument),
    };
    let (_authority, builder) = builder(caller, handle)?;
    let target = builder.process()?;
    bind_initial_execution(&target, requirement)
}

pub(crate) fn bind_initial_execution(
    target: &Arc<ProcessShell>,
    requirement: elf::IsaRequirement,
) -> Result<(), SystemCallError> {
    let domain = crate::sched::resolve_domain(requirement).ok_or(SystemCallError::NotSupported)?;
    let body = target
        .lease_building()
        .ok_or(SystemCallError::ObjectClosed)?;
    let mut binding = body.execution.lock();
    if binding.is_some() {
        return Err(SystemCallError::ObjectNotAvailable);
    }
    *binding = Some(domain);
    drop(binding);
    *body.requirement.lock() = Some(requirement);
    Ok(())
}

pub fn query(caller: &Thread, handle: Handle, output: usize) -> Result<(), SystemCallError> {
    let authority = Authorization::accept(caller, handle, Rights::READ)?;
    let shell = ProcessShell::concrete(authority.object())?;
    crate::uaccess::write_user(caller, output, &shell.snapshot())
}

pub enum KillOutcome {
    Accepted,
    AlreadyTerminating,
    TerminatedCaller,
}
pub fn kill(caller: &Thread, handle: Handle) -> Result<KillOutcome, SystemCallError> {
    let authority = Authorization::accept(caller, handle, Rights::MANAGE)?;
    let shell = ProcessShell::concrete(authority.object())?;
    let first = shell.freeze_and_seal(
        TerminationCause::Killed,
        TerminationDetail::PROCESS_CONTROL_KILL,
    );
    Ok(if Arc::ptr_eq(&shell, &caller.process) {
        KillOutcome::TerminatedCaller
    } else if first {
        KillOutcome::Accepted
    } else {
        KillOutcome::AlreadyTerminating
    })
}
