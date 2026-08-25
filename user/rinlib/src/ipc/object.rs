//! 进程本地 Handle 的基础操作。

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, Rights},
};

use crate::call::{sys_handle_close, sys_handle_duplicate};

pub fn close(handle: Handle) -> Result<(), SystemCallError> {
    // SAFETY: Handle 是值参数。
    unsafe { sys_handle_close(handle) }
}

pub fn duplicate(handle: Handle, rights: Rights) -> Result<Handle, SystemCallError> {
    let mut output = Handle::INVALID;
    // SAFETY: output 在 ecall 期间有效且可写。
    unsafe { sys_handle_duplicate(handle, rights, &mut output)? };
    Ok(output)
}
