//! Job、affine ProcessBuilder 与 ProcessControl 的用户态基础封装。

use crate::call::{
    sys_job_create, sys_job_derive, sys_job_enumerate, sys_job_query, sys_job_seal,
    sys_process_attach, sys_process_bind_memory, sys_process_create, sys_process_drain,
    sys_process_grant, sys_process_kill, sys_process_map, sys_process_query, sys_process_start,
    sys_process_write,
};
use crate::ipc::object::close;
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, Rights},
    proc::{
        HandleGrant, JOB_ENUMERATE_MAX, JobEnumerateResult, JobMemberKind, JobSnapshot, JobState,
        PROCESS_DRAIN_MAX, ProcessCreateResult, ProcessDrainResult, ProcessDrainStatus,
        ProcessMapFlags, ProcessSnapshot, ThreadStartContext, Tid,
    },
};

pub fn create_job(parent: Handle, rights: Rights) -> Result<Handle, SystemCallError> {
    let mut output = Handle::INVALID;
    // SAFETY: output 在 syscall 期间有效且可写。
    unsafe { sys_job_create(parent, rights, &mut output)? };
    Ok(output)
}

/// 放弃尚未 Start 的完整 Create 结果并推进至资源收束完成。清理步骤全部
/// 尝试执行，返回最先发生的错误；调用者无需重复维护 close/drain 顺序。
pub fn abandon_to_completion(created: ProcessCreateResult) -> Result<(), SystemCallError> {
    let builder_result = close(created.builder);
    let drain_result = drain_to_completion(created.control);
    let control_result = close(created.control);
    builder_result?;
    drain_result?;
    control_result
}

/// JobSeal：MANAGE；幂等封口——该 Job 及全部后代的创建/启动口永久
/// 关闭（后续创建/启动返回 ObjectClosed）。
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
pub fn create(job: Handle, control_rights: Rights) -> Result<ProcessCreateResult, SystemCallError> {
    let mut output = ProcessCreateResult {
        builder: Handle::INVALID,
        control: Handle::INVALID,
        pid: 0,
        reserved: 0,
    };
    // SAFETY: output 在 syscall 期间有效且可写。
    unsafe { sys_process_create(job, control_rights, &mut output)? };
    Ok(output)
}

/// 一次性把 MemoryPool authority 移交给 Building process。成功消费 pool Handle；
/// 失败保留 pool Handle，调用方仍负责其关闭或重试。
pub fn bind_memory(builder: Handle, pool: Handle) -> Result<(), SystemCallError> {
    // SAFETY: 两个 Handle 值由内核按 role/rights 与 affine 状态完整校验。
    unsafe { sys_process_bind_memory(builder, pool) }
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

/// ProcessAttach：组装者向 Building process 附入线程（外部通道；线程是
/// 组装资源）。栈与出生参数由组装者经 Map/Write 预先供给。
pub fn attach(builder: Handle, descriptor: &ThreadStartContext) -> Result<Tid, SystemCallError> {
    // SAFETY: descriptor 在 syscall 期间保持有效。
    unsafe { sys_process_attach(builder, descriptor) }
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
pub fn start(builder: Handle, profile: u32) -> Result<(), SystemCallError> {
    // SAFETY: 值参数由内核完整校验。
    unsafe { sys_process_start(builder, profile) }
}

/// ProcessQuery：固定宽生命周期快照。未知判别值与非法组合拒绝
/// （不降级解释；reserved 非零即内核 ABI 违约）。
pub fn query(control: Handle) -> Result<ProcessSnapshot, SystemCallError> {
    let mut snapshot = ProcessSnapshot {
        pid: 0,
        parent_pid: 0,
        state: 0,
        reason: 0,
        code: 0,
        reserved: 0,
    };
    // SAFETY: snapshot 在 syscall 期间有效且可写。
    unsafe { sys_process_query(control, &mut snapshot)? };
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}

fn validate_snapshot(snapshot: &ProcessSnapshot) -> Result<(), SystemCallError> {
    use erhino_shared::proc::{ProcessExitReason, ProcessState};
    if snapshot.reserved != 0 {
        return Err(SystemCallError::InternalError);
    }
    let state = snapshot.state;
    let reason = snapshot.reason;
    let ok_state = state <= ProcessState::Dead as u32;
    let ok_reason = reason <= ProcessExitReason::Abandoned as u32;
    if !ok_state || !ok_reason {
        return Err(SystemCallError::InternalError);
    }
    let live = state == ProcessState::Building as u32 || state == ProcessState::Running as u32;
    let terminal = state == ProcessState::Terminating as u32 || state == ProcessState::Dead as u32;
    // 终态/终止态必须携带终因；活态必须 reason=None 且 code=0。
    if terminal && reason == ProcessExitReason::None as u32 {
        return Err(SystemCallError::InternalError);
    }
    if reason == ProcessExitReason::None as u32 && snapshot.code != 0 {
        return Err(SystemCallError::InternalError);
    }
    if live && (reason != ProcessExitReason::None as u32 || snapshot.code != 0) {
        return Err(SystemCallError::InternalError);
    }
    if reason == ProcessExitReason::Abandoned as u32 && snapshot.code != 0 {
        return Err(SystemCallError::InternalError);
    }
    if reason == ProcessExitReason::Fault as u32 && !(0..=8).contains(&snapshot.code) {
        return Err(SystemCallError::InternalError);
    }
    Ok(())
}

/// ProcessKill：异步幂等终止请求（MANAGE）。成功仅表示请求被接受或
/// 目标已越过不可逆终止边界；不表示 teardown 完成。自杀式调用不返回。
pub fn kill(control: Handle, code: i64) -> Result<(), SystemCallError> {
    // SAFETY: 值参数由内核完整校验。
    unsafe { sys_process_kill(control, code) }
}

/// ProcessDrain：REAPABLE/Dead 上推进固定预算收束批次（MANAGE）。
/// 未知 status 判别值拒绝（不当作 More 无限循环）；More + 0 work
/// （无进展，内核违约）与超预算 work_done 同样拒绝。
pub fn drain(control: Handle, max_work: u32) -> Result<ProcessDrainResult, SystemCallError> {
    let mut result = ProcessDrainResult {
        work_done: 0,
        status: 0,
        reserved: 0,
    };
    // SAFETY: result 在 syscall 期间有效且可写。
    unsafe { sys_process_drain(control, max_work, &mut result)? };
    if result.reserved != 0 || result.status > ProcessDrainStatus::Complete as u32 {
        return Err(SystemCallError::InternalError);
    }
    let effective_max = max_work.min(PROCESS_DRAIN_MAX);
    if result.work_done > effective_max {
        return Err(SystemCallError::InternalError);
    }
    if result.status == ProcessDrainStatus::More as u32 && result.work_done == 0 {
        return Err(SystemCallError::InternalError);
    }
    Ok(result)
}

/// Drain 至 Complete（管理者循环用；步进用裸 [`drain`]）。
pub fn drain_to_completion(control: Handle) -> Result<u32, SystemCallError> {
    let mut total = 0u32;
    loop {
        let result = drain(control, erhino_shared::proc::PROCESS_DRAIN_MAX)?;
        total = total.saturating_add(result.work_done);
        if result.status == ProcessDrainStatus::Complete as u32 {
            return Ok(total);
        }
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
