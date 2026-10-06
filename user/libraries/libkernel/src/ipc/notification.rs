//! Notification：显式消费的 OR 累积事件对象。

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, HandlePair, Rights},
};

use crate::call::{sys_notification_create, sys_notification_signal, sys_notification_take};

pub fn create(
    owner_rights: Rights,
    signaler_rights: Rights,
) -> Result<HandlePair, SystemCallError> {
    let mut output = HandlePair::new(Handle::INVALID, Handle::INVALID);
    // SAFETY: output 在 ecall 期间有效且可写。
    unsafe { sys_notification_create(owner_rights, signaler_rights, &mut output)? };
    Ok(output)
}

pub fn signal(handle: Handle, bits: u64) -> Result<(), SystemCallError> {
    // SAFETY: 参数均为值。
    unsafe { sys_notification_signal(handle, bits) }
}

pub fn take(handle: Handle, mask: u64) -> Result<u64, SystemCallError> {
    let mut output = 0;
    // SAFETY: output 在 ecall 期间有效且可写。
    unsafe { sys_notification_take(handle, mask, &mut output)? };
    Ok(output)
}

/// 独立只读关闭观察权；派生与丢弃都不改变 Notification 业务寿命。
pub fn observe(source: Handle) -> Result<super::capability::Capability, SystemCallError> {
    let mut output = Handle::INVALID;
    // SAFETY: 固定宽输出在调用期间有效；成功后交付新观察表项的唯一 owner。
    unsafe {
        crate::call::sys_notification_observe(source, &mut output)?;
    }
    Ok(super::capability::Capability::owned(output))
}
