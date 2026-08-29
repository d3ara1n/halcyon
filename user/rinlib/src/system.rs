//! 系统级终局操作。

use core::convert::Infallible;

use erhino_shared::{
    call::SystemCallError,
    object::Handle,
    reset::{ResetAction, ResetReason},
};

/// 提交系统复位。成功时系统终止，因此只可能以错误返回。
pub fn reset(
    authority: Handle,
    action: ResetAction,
    reason: ResetReason,
) -> Result<Infallible, SystemCallError> {
    match unsafe { crate::call::sys_system_reset(authority, action, reason) } {
        Err(error) => Err(error),
        Ok(()) => Err(SystemCallError::InternalError),
    }
}
