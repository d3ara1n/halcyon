//! WaitMany 的安全封装；相对便利入口归到唯一绝对核心。

use crate::{call::sys_wait_many, time::Deadline};
use erhino_shared::{
    call::SystemCallError,
    object::ObjectSignals,
    wait::{WaitItem, WaitReason, WaitResult},
};

/// 相对毫秒便利入口，零表示无限；只在入口转换一次。
pub fn wait_many(items: &[WaitItem], timeout_ms: u64) -> Result<WaitResult, SystemCallError> {
    wait_until(items, crate::time::timeout_millis(timeout_ms)?)
}

/// 多阶段操作共享同一绝对期限，不从本次等待时刻重新计时。
pub fn wait_until(items: &[WaitItem], deadline: Deadline) -> Result<WaitResult, SystemCallError> {
    let mut output = WaitResult::new(0, ObjectSignals::NONE, 0, WaitReason::Signaled);
    // SAFETY: 输入及输出在阻塞 syscall 完成前保持有效。
    unsafe { sys_wait_many(items, &mut output, deadline)? };
    Ok(output)
}
