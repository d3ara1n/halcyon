//! 受信物理 DMA：Drop 只关闭角色，不证明设备完成，也不自动退休已暴露 mapping。
use crate::memory_object::MemoryObject;
use core::mem::ManuallyDrop;
use erhino_shared::{
    call::SystemCallError,
    dma::{DomainSnapshot, MappingSnapshot, PinRequest, ReadRequest, ReadResult, Segment},
    object::{Handle, Rights},
};
pub struct DmaDomainControl {
    handle: Handle,
}
pub struct DmaUse {
    handle: Handle,
}
pub struct DmaMapping {
    handle: Handle,
}
macro_rules! owner {
    ($ty:ident) => {
        impl $ty {
            /// # Safety
            /// handle 已核实际对象类型，唯一关闭责任移交给此 owner。
            pub const unsafe fn from_handle(handle: Handle) -> Self {
                Self { handle }
            }
            pub const fn handle(&self) -> Handle {
                self.handle
            }
            pub fn into_handle(self) -> Handle {
                ManuallyDrop::new(self).handle
            }
        }
        impl Drop for $ty {
            fn drop(&mut self) {
                crate::ipc::object::close_object_owner(self.handle);
            }
        }
    };
}
owner!(DmaDomainControl);
owner!(DmaUse);
owner!(DmaMapping);
macro_rules! domain {
    ($ty:ident) => {
        impl $ty {
            pub fn query(&self) -> Result<DomainSnapshot, SystemCallError> {
                let mut result = DomainSnapshot::default();
                crate::call::sys_dma_domain_query(self.handle, &mut result)?;
                Ok(result)
            }
            pub fn seal(&self) -> Result<(), SystemCallError> {
                crate::call::sys_dma_seal(self.handle)
            }
            /// # Safety
            /// 全部设备访问已经结束，缓存的地址也不会再次提交给硬件。CPU fence、
            /// 进程退出、IRQ/MMIO 撤销都不能单独证明这一点。完成另等 mapping DONE。
            pub unsafe fn retire(&self, mapping: &DmaMapping) -> Result<(), SystemCallError> {
                crate::call::sys_dma_retire(self.handle, mapping.handle)
            }
        }
    };
}
domain!(DmaDomainControl);
domain!(DmaUse);
impl DmaDomainControl {
    pub fn create_control(&self, rights: Rights) -> Result<Self, SystemCallError> {
        crate::call::sys_dma_create(self.handle, rights, true).map(|handle| Self { handle })
    }
    pub fn create_use(&self, rights: Rights) -> Result<DmaUse, SystemCallError> {
        crate::call::sys_dma_create(self.handle, rights, false).map(|handle| DmaUse { handle })
    }
}
impl DmaUse {
    /// 方向以设备为主体，取 shared::dma 的 DEVICE_READS/DEVICE_WRITES/BIDIRECTIONAL。
    /// offset/bytes 按页对齐；只交 mapping 角色，不交地址，也不产生共享页 Rust 引用。
    pub fn pin(
        &self,
        memory: &MemoryObject,
        offset: usize,
        bytes: usize,
        direction: u32,
        rights: Rights,
    ) -> Result<DmaMapping, SystemCallError> {
        let request = PinRequest {
            memory: memory.handle(),
            offset: offset as u64,
            bytes: bytes as u64,
            direction,
            reserved: 0,
            rights,
        };
        crate::call::sys_dma_pin(self.handle, &request).map(|handle| DmaMapping { handle })
    }
}
impl DmaMapping {
    pub fn query(&self) -> Result<MappingSnapshot, SystemCallError> {
        let mut result = MappingSnapshot::default();
        crate::call::sys_dma_mapping_query(self.handle, &mut result)?;
        Ok(result)
    }
    /// 一次最多交付一页 SG 描述。next_offset 是 mapping 内的字节偏移，达到 bytes 即结束。
    /// 调用可能暴露地址；失败也不能据此取消 pin，仍需受信退休。
    pub fn read_segments(
        &self,
        offset: usize,
        segments: &mut [Segment],
    ) -> Result<ReadResult, SystemCallError> {
        if segments.is_empty() {
            return Err(SystemCallError::IllegalArgument);
        }
        let capacity = segments.len().min(erhino_shared::dma::SEGMENTS_PER_READ);
        let mut result = ReadResult::default();
        let request = ReadRequest {
            offset: offset as u64,
            segments: segments.as_mut_ptr() as u64,
            capacity: capacity as u32,
            reserved: 0,
            result: core::ptr::addr_of_mut!(result) as u64,
        };
        // SAFETY: 独占输出切片及结果在同步调用期间有效。
        unsafe { crate::call::sys_dma_read(self.handle, &request) }?;
        if result.count == 0
            || result.count as usize > capacity
            || result.next_offset <= offset as u64
        {
            return Err(SystemCallError::InternalError);
        }
        Ok(result)
    }
}
