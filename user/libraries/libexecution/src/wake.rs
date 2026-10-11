//! 用户态债务发布接口；实际唤醒源在任务公开前建立，不依赖下一次业务请求。

pub trait Wake {
    fn publish(&self);
}

#[cfg(target_arch = "riscv64")]
pub struct NotificationWake {
    signaler: libkernel::ipc::notification::NotificationSignaler,
    bit: u64,
}

#[cfg(target_arch = "riscv64")]
impl NotificationWake {
    pub fn new(
        signaler: libkernel::ipc::capability::Capability,
        bit: u64,
    ) -> Result<
        Self,
        (
            libkernel::ipc::capability::Capability,
            erhino_shared::call::SystemCallError,
        ),
    > {
        use erhino_shared::{call::SystemCallError, object::Rights};
        let (signaler, description) =
            libkernel::ipc::notification::NotificationSignaler::from_capability(signaler)
                .map_err(|failure| (failure.owner, failure.error))?;
        if bit == 0 {
            return Err((signaler.into_capability(), SystemCallError::IllegalArgument));
        }
        if !description.rights.contains(Rights::SIGNAL) {
            return Err((signaler.into_capability(), SystemCallError::RightsDenied));
        }
        Ok(Self { signaler, bit })
    }
}

#[cfg(target_arch = "riscv64")]
impl Wake for NotificationWake {
    fn publish(&self) {
        // 接收 owner 先关闭只会终止执行域；不能在析构中重建已经停止的任务。
        match self.signaler.signal(self.bit) {
            Ok(()) | Err(erhino_shared::call::SystemCallError::ObjectClosed) => (),
            Err(error) => panic!("retirement notification invariant violated: {error:?}"),
        }
    }
}
