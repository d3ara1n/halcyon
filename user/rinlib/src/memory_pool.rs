//! MemoryPool typed Handle owner。

use core::mem::ManuallyDrop;
use erhino_shared::{
    call::SystemCallError,
    memory_pool::{MEMORY_POOL_MAX_DEPTH, MemoryPoolSnapshot},
    object::{Handle, Rights},
};

/// 一个进程本地 MemoryPool Handle 的唯一用户态 owner。
///
/// 类型不可复制；转入通用消息或 Building grant 时必须显式消费为 raw Handle。
pub struct MemoryPool {
    handle: Handle,
}

impl MemoryPool {
    /// 从调用者唯一拥有的 raw Handle 建立 typed owner。
    ///
    /// # Safety
    /// 调用者把该值的唯一使用权移入 owner；调用后不得再使用任何 raw alias，
    /// 也不得存在其它 typed owner。
    pub const unsafe fn from_handle(handle: Handle) -> Self {
        Self { handle }
    }

    pub fn into_handle(self) -> Handle {
        ManuallyDrop::new(self).handle
    }

    pub fn query(&self) -> Result<MemoryPoolSnapshot, SystemCallError> {
        let mut snapshot = MemoryPoolSnapshot {
            identity: 0,
            parent_identity: 0,
            total: 0,
            available: 0,
            reserved: 0,
            allocated: 0,
            delegated: 0,
            depth: 0,
            reserved0: 0,
        };
        // SAFETY: snapshot 在 syscall 期间有效且可写。
        unsafe { crate::call::sys_memory_pool_query(self.handle, &mut snapshot)? };
        if snapshot.identity == 0
            || snapshot.depth == 0
            || snapshot.depth > MEMORY_POOL_MAX_DEPTH
            || snapshot.reserved0 != 0
            || snapshot.depth == 1 && snapshot.parent_identity != 0
            || snapshot.depth > 1 && snapshot.parent_identity == 0
            || !snapshot.closes()
        {
            return Err(SystemCallError::InternalError);
        }
        Ok(snapshot)
    }

    pub fn derive(&self, pages: u64, rights: Rights) -> Result<Self, SystemCallError> {
        let mut child = Handle::INVALID;
        // SAFETY: child 在 syscall 期间有效且可写；内核完整校验值参数。
        unsafe {
            crate::call::sys_memory_pool_derive(self.handle, pages, rights, &mut child)?;
        }
        if !child.is_valid() {
            return Err(SystemCallError::InternalError);
        }
        Ok(Self { handle: child })
    }

    /// 关闭 Handle；若内核拒绝，返回仍可重试的 owner。
    pub fn close(self) -> Result<(), (Self, SystemCallError)> {
        let handle = self.into_handle();
        match crate::ipc::object::close(handle) {
            Ok(()) => Ok(()),
            Err(error) => Err((Self { handle }, error)),
        }
    }
}

impl Drop for MemoryPool {
    fn drop(&mut self) {
        crate::ipc::object::close(self.handle)
            .expect("MemoryPool owner failed to close its kernel Handle");
    }
}
