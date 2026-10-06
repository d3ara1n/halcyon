//! Job、affine ProcessBuilder 与 ProcessControl 的用户态基础封装。

use crate::call::{
    sys_job_create, sys_job_derive, sys_job_enumerate, sys_job_query, sys_job_seal,
    sys_process_bind_execution, sys_process_create, sys_process_grant, sys_process_kill,
    sys_process_map, sys_process_query, sys_process_start, sys_process_write, sys_thread_create,
};
use crate::ipc::object::close;
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, Rights},
    proc::{
        HandleGrant, JOB_ENUMERATE_MAX, JobEnumerateResult, JobMemberKind, JobSnapshot, JobState,
        ProcessCreateResult, ProcessMapFlags, ProcessSnapshot, TerminationDetail,
        ThreadCreateResult, ThreadStartContext,
    },
};

pub fn create_job(parent: Handle, pool: Handle, rights: Rights) -> Result<Handle, SystemCallError> {
    let mut output = Handle::INVALID;
    // SAFETY: output 在 syscall 期间有效且可写。
    unsafe { sys_job_create(parent, pool, rights, &mut output)? };
    Ok(output)
}

/// 放弃尚未 Start 的完整 Create 结果并推进至资源收束完成。清理步骤全部
/// 尝试执行，返回最先发生的错误；调用者无需重复维护 close/drain 顺序。
/// # Safety
/// created 必须是调用者独占的真实、尚未 Start 的 Create 结果，两枚 Handle 均未消费。
///
/// ```compile_fail
/// let created = libkernel::shared::proc::ProcessCreateResult {
///     builder: libkernel::shared::object::Handle::INVALID,
///     control: libkernel::shared::object::Handle::INVALID, pid: 0, reserved: 0,
/// };
/// libkernel::process::abandon_to_completion(created);
/// ```
pub unsafe fn abandon_to_completion(created: ProcessCreateResult) -> Result<(), SystemCallError> {
    let builder_result = unsafe { close(created.builder) };
    let done_result = wait_done(created.control);
    let control_result = unsafe { close(created.control) };
    builder_result?;
    done_result?;
    control_result
}

/// JobSeal：MANAGE；幂等封口——该 Job 及全部后代的创建/启动口永久
/// 关闭（后续创建/启动返回 ObjectClosed）。返回只证明封口已提交；
/// 等待 CLOSED 才证明本 Job 完成且从父表移除。
pub fn seal_job(control: Handle) -> Result<(), SystemCallError> {
    // SAFETY: 值参数由内核完整校验。
    unsafe { sys_job_seal(control) }
}

/// JobQuery：READ；固定宽快照。未知 state 判别值与非零 reserved 拒绝
/// （不降级解释）。
pub fn query_job(control: Handle) -> Result<JobSnapshot, SystemCallError> {
    let mut snapshot = JobSnapshot {
        jid: 0,
        parent_jid: 0,
        state: 0,
        live_processes: 0,
        live_children: 0,
        reserved: 0,
        reserved2: 0,
    };
    // SAFETY: snapshot 在 syscall 期间有效且可写。
    unsafe { sys_job_query(control, &mut snapshot)? };
    if snapshot.reserved != 0 || snapshot.reserved2 != 0 || snapshot.state > JobState::Dead as u32 {
        return Err(SystemCallError::InternalError);
    }
    Ok(snapshot)
}

/// JobEnumerate：READ；单调 ID 序游标分页。契约校验（违反即内核违约，
/// 拒绝不降级）：`more=1 ⇒ actual ≥ 1 ∨ next_cursor == 入参 cursor`
/// （后者是占位屏障零进展，调用方以原 cursor 重试）；actual 不超过
/// min(buf_len, JOB_ENUMERATE_MAX)；actual > 0 时 next_cursor 必为本批
/// 最后条目 ID（> 入参 cursor），actual == 0 时必等于入参 cursor。
pub fn enumerate_job(
    control: Handle,
    kind: JobMemberKind,
    cursor: u64,
    buf: &mut [u64],
) -> Result<JobEnumerateResult, SystemCallError> {
    let mut result = JobEnumerateResult {
        next_cursor: 0,
        actual: 0,
        more: 0,
    };
    // SAFETY: buf/result 在 syscall 期间有效且可写。
    unsafe {
        sys_job_enumerate(
            control,
            kind as u32,
            cursor,
            buf.as_mut_ptr(),
            buf.len(),
            &mut result,
        )?;
    }
    let cap = buf.len().min(JOB_ENUMERATE_MAX);
    if result.more > 1
        || result.actual as usize > cap
        || result.actual == 0 && result.next_cursor != cursor
        || result.actual > 0 && result.next_cursor <= cursor
    {
        return Err(SystemCallError::InternalError);
    }
    Ok(result)
}

/// JobDerive：MANAGE；在直接成员域内按 ID 派生 child JobControl /
/// member ProcessControl。请求 rights 必须是源 Handle rights 与目标
/// 角色 allowed_rights 交集的子集；目标已完成返回 ObjectNotFound。
pub fn derive_job(
    control: Handle,
    kind: JobMemberKind,
    id: u64,
    rights: Rights,
) -> Result<Handle, SystemCallError> {
    let mut output = Handle::INVALID;
    // SAFETY: output 在 syscall 期间有效且可写。
    unsafe { sys_job_derive(control, kind as u32, id, rights, &mut output)? };
    Ok(output)
}

/// ProcessCreate：Building process + affine ProcessBuilder + ProcessControl
/// 同一事务交付；control_rights 显式请求并被校验为最大 rights 子集。
/// 返回唯一Builder（MANAGE|GRANT|TRANSIT）及按显式rights铸造的Control。
/// Builder运输/授予失败返原owner；Start成功消费启动权，随后由Kernel持责任。
pub fn create(
    job: Handle,
    pool: Handle,
    control_rights: Rights,
) -> Result<ProcessCreateResult, SystemCallError> {
    let mut output = ProcessCreateResult {
        builder: Handle::INVALID,
        control: Handle::INVALID,
        pid: 0,
        reserved: 0,
    };
    // SAFETY: output 在 syscall 期间有效且可写。
    unsafe { sys_process_create(job, pool, control_rights, &mut output)? };
    Ok(output)
}

pub fn map(
    builder: Handle,
    target: usize,
    len: usize,
    permissions: ProcessMapFlags,
) -> Result<(), SystemCallError> {
    // SAFETY: 值参数由内核完整校验。
    unsafe { sys_process_map(builder, target, len, permissions) }
}

pub fn write(builder: Handle, target: usize, source: &[u8]) -> Result<(), SystemCallError> {
    // SAFETY: source 在 syscall 期间保持有效。
    unsafe { sys_process_write(builder, target, source) }
}

/// ThreadCreate：组装者向 Building process 附入线程（外部通道；线程是
/// 组装资源）。栈与出生参数由组装者经 Map/Write 预先供给。
pub fn thread_create(
    builder: Handle,
    descriptor: &ThreadStartContext,
    result: &mut ThreadCreateResult,
) -> Result<(), SystemCallError> {
    // SAFETY: descriptor/result 在 syscall 期间保持有效。
    unsafe { sys_thread_create(builder, descriptor, result) }
}

/// ProcessGrant：组装者把 grants 装入目标 Building process 的 HandleTable
/// 并输出目标侧句柄值（写入出生块后经 Write 交付）。输出切片必须与
/// grants 等长，安全封装在进入裸 syscall 前拒绝不一致容量。
pub fn grant(
    builder: Handle,
    grants: &[HandleGrant],
    out_values: &mut [Handle],
) -> Result<(), SystemCallError> {
    if grants.len() != out_values.len() {
        return Err(SystemCallError::IllegalArgument);
    }
    // SAFETY: 两个切片在 syscall 期间保持有效，且输出容量等于内核写入数量。
    unsafe { sys_process_grant(builder, grants, out_values) }
}

/// ProcessStart：活体门（已附线程 ≥1）检查后首次发布进程；builder 消费。
pub fn start(builder: Handle) -> Result<(), SystemCallError> {
    // SAFETY: 值参数由内核完整校验。
    unsafe { sys_process_start(builder) }
}

pub fn bind_execution(builder: Handle, profile: u32) -> Result<(), SystemCallError> {
    // SAFETY: profile is a scalar and builder is a caller-owned capability.
    unsafe { sys_process_bind_execution(builder, profile) }
}

/// ProcessQuery：固定宽生命周期快照。未知判别值与非法组合拒绝
/// （不降级解释；reserved 非零即内核 ABI 违约）。
pub fn query(control: Handle) -> Result<ProcessSnapshot, SystemCallError> {
    let mut snapshot = ProcessSnapshot {
        pid: 0,
        parent_pid: 0,
        state: 0,
        cause: 0,
        detail: TerminationDetail::NONE,
        reserved: 0,
    };
    // SAFETY: snapshot 在 syscall 期间有效且可写。
    unsafe { sys_process_query(control, &mut snapshot)? };
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}

fn validate_snapshot(snapshot: &ProcessSnapshot) -> Result<(), SystemCallError> {
    use erhino_shared::proc::{ProcessFaultCode, ProcessState, TerminationCause};
    if snapshot.reserved != 0 {
        return Err(SystemCallError::InternalError);
    }
    let state = snapshot.state;
    let cause = snapshot.cause;
    let ok_state = state <= ProcessState::Dead as u32;
    let ok_cause = cause <= TerminationCause::Abandoned as u32;
    if !ok_state || !ok_cause {
        return Err(SystemCallError::InternalError);
    }
    let live = state == ProcessState::Building as u32 || state == ProcessState::Running as u32;
    let terminal = state == ProcessState::Terminating as u32 || state == ProcessState::Dead as u32;
    if terminal && cause == TerminationCause::None as u32 {
        return Err(SystemCallError::InternalError);
    }
    let detail_valid = match cause {
        x if x == TerminationCause::None as u32
            || x == TerminationCause::Completed as u32
            || x == TerminationCause::Terminated as u32
            || x == TerminationCause::Aborted as u32
            || x == TerminationCause::Abandoned as u32 =>
        {
            snapshot.detail == TerminationDetail::NONE
        }
        x if x == TerminationCause::Killed as u32 => {
            snapshot.detail == TerminationDetail::PROCESS_CONTROL_KILL
        }
        x if x == TerminationCause::Faulted as u32 => {
            (snapshot.detail.word0 >> 32) <= ProcessFaultCode::UserAccess as u64
        }
        _ => false,
    };
    if !detail_valid {
        return Err(SystemCallError::InternalError);
    }
    if live && cause != TerminationCause::None as u32 {
        return Err(SystemCallError::InternalError);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KillOutcome {
    Accepted,
    AlreadyTerminating,
}

/// ProcessKill：异步幂等终止请求（MANAGE）。返回值只反映终止线性化，
/// 不表示 teardown 完成；自杀式调用不返回。
pub fn kill(control: Handle) -> Result<KillOutcome, SystemCallError> {
    // SAFETY: 值参数由内核完整校验。
    match unsafe { sys_process_kill(control)? } {
        0 => Ok(KillOutcome::Accepted),
        1 => Ok(KillOutcome::AlreadyTerminating),
        _ => Err(SystemCallError::InternalError),
    }
}

/// 显式等实际 Body 退款后的 DONE；进程回收由内核自主推进。
pub fn wait_done(control: Handle) -> Result<(), SystemCallError> {
    let result = crate::ipc::wait::wait_until(
        control,
        erhino_shared::object::ObjectSignals::DONE,
        crate::time::Deadline::INFINITE,
    )?;
    if result
        .observed
        .contains(erhino_shared::object::ObjectSignals::DONE)
    {
        Ok(())
    } else {
        Err(SystemCallError::ObjectClosed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_rejects_output_length_mismatch_before_syscall() {
        let grants = [HandleGrant {
            handle: Handle::INVALID,
            rights: Rights::NONE,
        }];
        assert_eq!(
            grant(Handle::INVALID, &grants, &mut []),
            Err(SystemCallError::IllegalArgument)
        );
    }
}
