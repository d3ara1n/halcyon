//! Low-level thread syscalls. Higher-level ownership belongs to `libthread`.

use erhino_shared::{
    call::SystemCallError,
    object::Handle,
    proc::{ThreadCreateResult, ThreadStartContext},
};

/// Create a suspended thread in the current process or a Building target.
///
/// # Safety
///
/// The caller must keep the context and result writable for the syscall and
/// must establish ownership for the returned control.
pub unsafe fn create(
    target: Handle,
    context: &ThreadStartContext,
    result: &mut ThreadCreateResult,
) -> Result<(), SystemCallError> {
    // SAFETY: forwarded to the syscall wrapper with the caller's validity proof.
    unsafe { super::call::sys_thread_create(target, context, result) }
}

/// Start a suspended thread.
///
/// # Safety
///
/// `control` must be a MANAGE-capable ThreadControl entry owned by the caller.
pub unsafe fn start(control: Handle) -> Result<(), SystemCallError> {
    // SAFETY: forwarded to the syscall wrapper.
    unsafe { super::call::sys_thread_start(control) }
}

pub fn yield_now() -> Result<(), SystemCallError> {
    // SAFETY: ThreadYield has no borrowed user memory.
    unsafe { super::call::sys_thread_yield() }
}

/// Leave the current thread without running user-space destructors.
///
/// # Safety
///
/// The caller must have published all user-space result and environment
/// responsibilities before invoking this primitive.
pub unsafe fn exit() -> ! {
    // SAFETY: caller satisfies the contract above.
    unsafe { super::call::sys_thread_exit() }
}
