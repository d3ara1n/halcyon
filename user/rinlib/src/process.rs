//! Job 与 affine ProcessBuilder 的用户态基础封装。

use crate::call::{
    sys_job_create, sys_process_create, sys_process_map, sys_process_start, sys_process_write,
};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, Rights},
    proc::{ProcessCreateResult, ProcessMapFlags, ProcessStartDescriptor},
};

pub fn create_job(parent: Handle, rights: Rights) -> Result<Handle, SystemCallError> {
    let mut output = Handle::INVALID;
    // SAFETY: output 在 syscall 期间有效且可写。
    unsafe { sys_job_create(parent, rights, &mut output)? };
    Ok(output)
}

pub fn create(job: Handle) -> Result<ProcessCreateResult, SystemCallError> {
    let mut output = ProcessCreateResult {
        builder: Handle::INVALID,
        pid: 0,
        reserved: 0,
    };
    // SAFETY: output 在 syscall 期间有效且可写。
    unsafe { sys_process_create(job, &mut output)? };
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

pub fn start(
    builder: Handle,
    descriptor: &ProcessStartDescriptor,
) -> Result<Handle, SystemCallError> {
    let mut output = Handle::INVALID;
    // SAFETY: descriptor 与 output 在 syscall 期间保持有效。
    unsafe { sys_process_start(builder, descriptor, &mut output)? };
    Ok(output)
}
