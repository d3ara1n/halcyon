//! 有线 IRQ 的受限派生、提示领取与确认；关闭 capability 会提交实际撤销。
use core::mem::ManuallyDrop;
use erhino_shared::{
    call::SystemCallError,
    irq::{IrqScope, IrqSnapshot, IrqToken},
    object::{Handle, Rights},
};
pub struct IrqControl {
    handle: Handle,
}
pub struct IrqUse {
    handle: Handle,
}
macro_rules! owner {
    ($ty:ident) => {
        impl $ty {
            /// # Safety
            /// handle 已核定为本实际角色；唯一关闭责任移交给返回值。
            pub const unsafe fn from_handle(handle: Handle) -> Self {
                Self { handle }
            }
            pub const fn handle(&self) -> Handle {
                self.handle
            }
            pub fn into_handle(self) -> Handle {
                ManuallyDrop::new(self).handle
            }
            pub fn query(&self) -> Result<IrqSnapshot, SystemCallError> {
                let mut snapshot = IrqSnapshot::default();
                crate::call::sys_irq_query(self.handle, &mut snapshot)?;
                Ok(snapshot)
            }
            /// 只提交撤销；完成另等 DONE，不能把调用返回当设备静止。
            pub fn revoke(&self) -> Result<(), SystemCallError> {
                crate::call::sys_irq_revoke(self.handle)
            }
        }
        impl Drop for $ty {
            fn drop(&mut self) {
                crate::ipc::object::close_object_owner(self.handle);
            }
        }
    };
}
owner!(IrqControl);
owner!(IrqUse);
impl IrqControl {
    pub fn create_control(&self, scope: IrqScope, rights: Rights) -> Result<Self, SystemCallError> {
        crate::call::sys_irq_create(self.handle, &scope, rights, true).map(|handle| Self { handle })
    }
    /// target 是 Query.scope.targets 内的 bit 下标，不是原始 hartid。
    pub fn create_use(
        &self,
        source: u32,
        target: u32,
        rights: Rights,
    ) -> Result<IrqUse, SystemCallError> {
        let targets = 1u64
            .checked_shl(target)
            .ok_or(SystemCallError::IllegalArgument)?;
        crate::call::sys_irq_create(
            self.handle,
            &IrqScope {
                first: source,
                count: 1,
                targets,
            },
            rights,
            false,
        )
        .map(|handle| IrqUse { handle })
    }
    pub fn seal(&self) -> Result<(), SystemCallError> {
        crate::call::sys_irq_seal(self.handle)
    }
}
impl IrqUse {
    /// 非阻塞领取提示；Empty 为 None。驱动须检查实际硬件，处理后才 Ack。
    pub fn take(&self) -> Result<Option<IrqToken>, SystemCallError> {
        let generation = self.query()?.generation;
        match crate::call::sys_irq_take(self.handle) {
            Ok(sequence) => Ok(Some(IrqToken {
                generation,
                sequence,
            })),
            Err(SystemCallError::ObjectNotAvailable) => Ok(None),
            Err(error) => Err(error),
        }
    }
    pub fn ack(&self, token: IrqToken) -> Result<(), SystemCallError> {
        crate::call::sys_irq_ack(self.handle, token)
    }
}
