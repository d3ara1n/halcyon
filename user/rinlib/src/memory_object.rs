//! MemoryObject typed Handle owner。

use core::mem::ManuallyDrop;
use erhino_shared::{
    call::SystemCallError,
    memory_object::{MemoryObjectCreateRequest, MemoryObjectSnapshot},
    object::Handle,
};

/// 一个进程本地 MemoryObject Handle 的唯一用户态 owner。
///
/// 类型不可复制；转入通用消息或 Building grant 时必须显式消费为 raw Handle。
/// 关闭 Handle 不撤销已建立的 view——view 由对象强引用独立保活。
pub struct MemoryObject {
    handle: Handle,
}

impl MemoryObject {
    /// 从调用者唯一拥有的 raw Handle 建立 typed owner。
    ///
    /// # Safety
    /// `handle` 必须是当前进程中已安装的 MemoryObject role，调用者把唯一使用权移入
    /// owner；调用后不得再使用任何 raw alias，也不得存在其它 typed owner。
    pub const unsafe fn from_handle(handle: Handle) -> Self {
        Self { handle }
    }

    pub fn into_handle(self) -> Handle {
        ManuallyDrop::new(self).handle
    }

    /// 借用 Handle 值。只用于把对象作为 Map 来源声明，不转移所有权。
    pub const fn handle(&self) -> Handle {
        self.handle
    }

    /// 从当前进程绑定池创建新对象，并写出完整 rights 的 Handle。
    pub fn create(bytes: usize) -> Result<Self, SystemCallError> {
        let mut handle = Handle::INVALID;
        let request = MemoryObjectCreateRequest::new(
            u64::try_from(bytes).map_err(|_| SystemCallError::IllegalArgument)?,
            core::ptr::addr_of_mut!(handle) as usize as u64,
        );
        // SAFETY: handle 在 syscall 期间有效且可写；内核完整校验值参数。
        unsafe { crate::call::sys_memory_object_create(&request) }?;
        if !handle.is_valid() {
            return Err(SystemCallError::InternalError);
        }
        Ok(Self { handle })
    }

    pub fn query(&self) -> Result<MemoryObjectSnapshot, SystemCallError> {
        let mut snapshot = MemoryObjectSnapshot {
            identity: 0,
            bytes: 0,
            write_views: 0,
            state: 0,
            reserved0: 0,
            reserved: [0; 4],
        };
        // SAFETY: snapshot 在 syscall 期间有效且可写。
        unsafe { crate::call::sys_memory_object_query(self.handle, &mut snapshot) }?;
        if snapshot.identity == 0 || snapshot.reserved0 != 0 || snapshot.reserved != [0; 4] {
            return Err(SystemCallError::InternalError);
        }
        Ok(snapshot)
    }

    /// 单向发布对象为可执行。幂等；完成经 WaitMany 观察 `EXECUTABLE` 电平。
    pub fn seal(&self) -> Result<(), SystemCallError> {
        // SAFETY: 值参数由内核完整校验。
        unsafe { crate::call::sys_memory_object_seal(self.handle) }
    }

    /// 关闭唯一持有的 MemoryObject leaf Handle；合法 typed owner 不存在可恢复失败。
    pub fn close(self) {
        crate::ipc::object::close_leaf_owner(self.into_handle());
    }
}

impl Drop for MemoryObject {
    fn drop(&mut self) {
        crate::ipc::object::close_leaf_owner(self.handle);
    }
}
