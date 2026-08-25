//! WaitMany 的安全封装。

use erhino_shared::{
    call::SystemCallError,
    object::ObjectSignals,
    wait::{WaitItem, WaitReason, WaitResult},
};

use crate::call::sys_wait_many;

pub fn wait_many(items: &[WaitItem]) -> Result<WaitResult, SystemCallError> {
    let mut output = WaitResult::new(0, ObjectSignals::NONE, 0, WaitReason::Signaled);
    // SAFETY: items 与 output 在阻塞 syscall 完成前持续有效。
    unsafe { sys_wait_many(items, &mut output)? };
    Ok(output)
}
