//! Notification 的消费、发送与只读观察能力；等待仍使用共同 handle 借用接口。

use super::capability::{Capability, capability_role};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, HandlePair, Rights},
};

capability_role!(NotificationOwner, NotificationOwner, Rights::NONE);
capability_role!(NotificationSignaler, NotificationSignaler, Rights::NONE);
capability_role!(NotificationObserver, NotificationObserver, Rights::WAIT);

#[derive(Debug)]
pub struct NotificationPair {
    pub owner: NotificationOwner,
    pub peer: NotificationSignaler,
}

pub fn create(
    owner_rights: Rights,
    signaler_rights: Rights,
) -> Result<NotificationPair, SystemCallError> {
    let mut output = HandlePair::new(Handle::INVALID, Handle::INVALID);
    // SAFETY: 固定两项输出有效；成功时唯一关闭责任移交两个 typed owner。
    unsafe { crate::call::sys_notification_create(owner_rights, signaler_rights, &mut output)? };
    Ok(NotificationPair {
        owner: NotificationOwner::from_minted(Capability::owned(output.owner)),
        peer: NotificationSignaler::from_minted(Capability::owned(output.peer)),
    })
}

impl NotificationOwner {
    pub fn take(&self, mask: u64) -> Result<u64, SystemCallError> {
        let mut output = 0;
        // SAFETY: output 同步调用期间有效，消费权限由内核检查。
        unsafe { crate::call::sys_notification_take(self.as_handle(), mask, &mut output)? };
        Ok(output)
    }
    pub fn mint_observer(&self, rights: Rights) -> Result<NotificationObserver, SystemCallError> {
        let mut output = Handle::INVALID;
        // SAFETY: 有效的单项输出；成功工厂保证角色和 WAIT，直接移交新 owner。
        unsafe {
            crate::call::sys_notification_mint_observer(self.as_handle(), rights, &mut output)?
        };
        Ok(NotificationObserver::from_minted(Capability::owned(output)))
    }
}

impl NotificationSignaler {
    pub fn signal(&self, bits: u64) -> Result<(), SystemCallError> {
        // SAFETY: 参数为值，实际 SIGNAL 授权由内核检查。
        unsafe { crate::call::sys_notification_signal(self.as_handle(), bits) }
    }
}
