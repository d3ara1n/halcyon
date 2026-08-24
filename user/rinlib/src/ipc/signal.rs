//! 信号封装（契约见 notes/ideas/signal.md）：对象切面状态位 +
//! 等待者队列。分发到用户代码由 rt 层的监听机制承担，本模块只暴露
//! 机制级原语。

use erhino_shared::{
    call::SystemCallError,
    proc::{Pid, SignalMap},
};

use crate::call::{sys_signal_send, sys_signal_wait};

/// 向目标进程提交信号位（置位 + 唤醒等待者，永不阻塞）。返回是否移交
/// 唤醒了等待者（false = 已并入粘滞余量）。
pub fn send(pid: Pid, mask: SignalMap) -> Result<bool, SystemCallError> {
    unsafe { sys_signal_send(pid, mask) }
}

/// 阻塞等待任一对象的关注位命中，返回 `(命中项下标, 命中位)`。
/// 注意：进程级信号位是消费式清除（命中即清）；邮箱 NONEMPTY 为内核
/// 托管位，命中不清除。
pub fn wait(items: &[erhino_shared::signal::SignalItem]) -> Result<(usize, SignalMap), SystemCallError> {
    unsafe { sys_signal_wait(items) }
}
