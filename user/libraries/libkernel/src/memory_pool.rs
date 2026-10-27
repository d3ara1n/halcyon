//! MemoryPool typed Handle owner。

use core::mem::ManuallyDrop;
use erhino_shared::{
    call::SystemCallError,
    memory_pool::{
        MEMORY_POOL_MAX_DEPTH, MemoryPoolBudget, MemoryPoolDeriveRequest, MemoryPoolSnapshot,
    },
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
    /// `handle` 必须是当前进程中已安装的 MemoryPool role，调用者把唯一使用权移入
    /// owner；调用后不得再使用任何 raw alias，也不得存在其它 typed owner。
    pub const unsafe fn from_handle(handle: Handle) -> Self {
        Self { handle }
    }

    pub fn into_handle(self) -> Handle {
        ManuallyDrop::new(self).handle
    }
    pub fn as_handle(&self) -> Handle {
        self.handle
    }

    pub fn query(&self) -> Result<MemoryPoolSnapshot, SystemCallError> {
        query(self.handle)
    }

    pub fn derive(
        &self,
        budget: MemoryPoolBudget,
        rights: Rights,
    ) -> Result<Self, SystemCallError> {
        derive(self.handle, budget, rights)
    }

    /// 关闭唯一持有的 MemoryPool leaf Handle；合法 typed owner 不存在可恢复失败。
    pub fn close(self) {
        crate::ipc::object::close_object_owner(self.into_handle());
    }
}

/// 借用其他实际owner持有的Pool entry观察余额，不创建别名或转移关闭责任。
pub fn query(handle: Handle) -> Result<MemoryPoolSnapshot, SystemCallError> {
    let mut snapshot = MemoryPoolSnapshot::default();
    // SAFETY: snapshot 在同步调用期间有效且可写，内核验证原entry的类型/READ权限。
    unsafe { crate::call::sys_memory_pool_query(handle, &mut snapshot)? };
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

/// 从宿主持有的通用 Pool entry 借用派生权，输出有唯一关闭责任的新 Pool。
/// 输入身份、类型和 CREATE 权由内核核验；不构造第二份父 owner。
pub fn derive(
    parent: Handle,
    budget: MemoryPoolBudget,
    rights: Rights,
) -> Result<MemoryPool, SystemCallError> {
    let mut child = Handle::INVALID;
    let request = MemoryPoolDeriveRequest {
        budget,
        rights,
        reserved: 0,
    };
    // SAFETY: request/child 在调用期间有效，输入句柄及预算由内核完整校验。
    unsafe {
        crate::call::sys_memory_pool_derive(parent, &request, &mut child)?;
    }
    if !child.is_valid() {
        return Err(SystemCallError::InternalError);
    }
    Ok(MemoryPool { handle: child })
}

impl Drop for MemoryPool {
    fn drop(&mut self) {
        crate::ipc::object::close_object_owner(self.handle);
    }
}
