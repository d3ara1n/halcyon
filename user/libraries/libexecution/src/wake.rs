//! 用户态债务发布接口；实际唤醒源在任务公开前建立，不依赖下一次业务请求。

pub trait Wake {
    fn publish(&self);
}

#[cfg(target_arch = "riscv64")]
pub struct NotificationWake {
    signaler: rinlib::ipc::capability::Capability,
    bit: u64,
}

#[cfg(target_arch = "riscv64")]
impl NotificationWake {
    pub fn new(
        signaler: rinlib::ipc::capability::Capability,
        bit: u64,
    ) -> Result<
        Self,
        (
            rinlib::ipc::capability::Capability,
            erhino_shared::call::SystemCallError,
        ),
    > {
        use erhino_shared::{
            call::SystemCallError,
            object::{HandleRole, Rights},
        };
        let checked = signaler.description().and_then(|description| {
            if bit == 0 || description.role != HandleRole::NotificationSignaler as u32 {
                return Err(SystemCallError::WrongObjectType);
            }
            if !description.rights.contains(Rights::SIGNAL) {
                return Err(SystemCallError::RightsDenied);
            }
            Ok(())
        });
        match checked {
            Ok(()) => Ok(Self { signaler, bit }),
            Err(error) => Err((signaler, error)),
        }
    }
}

#[cfg(target_arch = "riscv64")]
impl Wake for NotificationWake {
    fn publish(&self) {
        // 接收 owner 先关闭只会终止执行域；不能在析构中重建已经停止的任务。
        match rinlib::ipc::notification::signal(self.signaler.as_handle(), self.bit) {
            Ok(()) | Err(erhino_shared::call::SystemCallError::ObjectClosed) => (),
            Err(error) => panic!("retirement notification invariant violated: {error:?}"),
        }
    }
}
