//! Job、affine ProcessBuilder 与 ProcessControl 的用户态基础封装。

use crate::call::{
    sys_job_create, sys_process_create, sys_process_drain, sys_process_kill, sys_process_map,
    sys_process_query, sys_process_start, sys_process_write,
};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, Rights},
    proc::{
        ProcessCreateResult, ProcessDrainResult, ProcessDrainStatus, ProcessMapFlags,
        ProcessSnapshot, ProcessStartDescriptor, PROCESS_DRAIN_MAX,
    },
};

pub fn create_job(parent: Handle, rights: Rights) -> Result<Handle, SystemCallError> {
    let mut output = Handle::INVALID;
    // SAFETY: output 在 syscall 期间有效且可写。
    unsafe { sys_job_create(parent, rights, &mut output)? };
    Ok(output)
}

/// ProcessCreate：Building process + affine ProcessBuilder + ProcessControl
/// 同一事务交付；control_rights 显式请求并被校验为最大 rights 子集。
pub fn create(
    job: Handle,
    control_rights: Rights,
) -> Result<ProcessCreateResult, SystemCallError> {
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

pub fn map(
    builder: Handle,
    target: usize,
    len: usize,
    permissions: ProcessMapFlags,
) -> Result<(), SystemCallError> {
    // SAFETY: 值参数由内核完整校验。
    unsafe { sys_process_map(builder, target, len, permissions) }
}

pub fn write(
    builder: Handle,
    target: usize,
    source: &[u8],
) -> Result<(), SystemCallError> {
    // SAFETY: source 在 syscall 期间保持有效。
    unsafe { sys_process_write(builder, target, source) }
}

/// ProcessStart：消费 builder 并首次发布进程；control 已在 Create 交付。
pub fn start(
    builder: Handle,
    descriptor: &ProcessStartDescriptor,
) -> Result<(), SystemCallError> {
    // SAFETY: descriptor 在 syscall 期间保持有效。
    unsafe { sys_process_start(builder, descriptor) }
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
    if reason == ProcessExitReason::Fault as u32
        && !(0..=8).contains(&snapshot.code)
    {
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
    let mut result = ProcessDrainResult { work_done: 0, status: 0, reserved: 0 };
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
