//! WaitMany 的安全封装。

use erhino_shared::{
    call::SystemCallError,
    object::ObjectSignals,
    wait::{WaitItem, WaitReason, WaitResult},
};

use crate::call::sys_wait_many;

/// 等待观察项完成，`deadline_ms` 为相对毫秒期限，`0` 表示无限等待。
/// 期限到达时返回 `reason == Deadline` 的结果，无任何观察项被消费。
pub fn wait_many(items: &[WaitItem], deadline_ms: u64) -> Result<WaitResult, SystemCallError> {
    let mut output = WaitResult::new(0, ObjectSignals::NONE, 0, WaitReason::Signaled);
    // SAFETY: items 与 output 在阻塞 syscall 完成前持续有效。
    unsafe { sys_wait_many(items, &mut output, deadline_ms)? };
    Ok(output)
}
