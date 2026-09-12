//! 进程本地 Handle 的基础操作。

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, Rights},
};

use crate::call::{sys_handle_close, sys_handle_duplicate};

/// # Safety
/// 调用者须独占 entry 的关闭责任，不能撤销仍被安全 owner 或引用使用的映射。
///
/// ```compile_fail
/// rinlib::ipc::object::close(rinlib::shared::object::Handle::INVALID);
/// ```
pub unsafe fn close(handle: Handle) -> Result<(), SystemCallError> {
    // SAFETY: entry 关闭责任由调用者保证。
    unsafe { sys_handle_close(handle) }
}

/// 关闭由安全 typed owner 唯一持有的非 Tunnel 叶 Handle。
///
/// 此边界不可用于任意 raw Handle：合法 leaf owner 的表项仍在本进程且内核关闭
/// 路径无异步阶段，因此错误只表示 unsafe owner 构造契约或内核不变量被破坏。
pub(crate) fn close_leaf_owner(handle: Handle) {
    // SAFETY: 私有调用者持有真实且独占的非映射 leaf owner。
    unsafe { close(handle) }
        .unwrap_or_else(|error| panic!("typed leaf Handle close invariant violated: {error:?}"));
}

pub fn duplicate(handle: Handle, rights: Rights) -> Result<Handle, SystemCallError> {
    let mut output = Handle::INVALID;
    // SAFETY: output 在 ecall 期间有效且可写。
    unsafe { sys_handle_duplicate(handle, rights, &mut output)? };
    Ok(output)
}
