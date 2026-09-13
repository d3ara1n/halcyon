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

/// 关闭由安全 typed owner 唯一持有的非映射对象 Handle。
///
/// 此边界不可用于任意 raw Handle：合法 owner 的表项仍在本进程，关闭的必成资源
/// 已在构造时预付；非空 WaitSet 由 syscall 等待内核退休。Drop 时不能返还 owner，
/// 返回错误表示 unsafe 构造/撤销契约或内核不变量被破坏，不能静默遗弃该责任。
pub(crate) fn close_object_owner(handle: Handle) {
    // SAFETY: 私有调用者持有合法非映射对象 entry 的唯一关闭责任。
    unsafe { close(handle) }
        .unwrap_or_else(|error| panic!("typed object Handle close invariant violated: {error:?}"));
}

pub fn query(handle: Handle) -> Result<erhino_shared::object::HandleDescription, SystemCallError> {
    let mut output = erhino_shared::object::HandleDescription {
        object_id: 0,
        related_object_id: 0,
        kind: 0,
        role: 0,
        rights: Rights::NONE,
        badge: 0,
        reserved: 0,
    };
    // SAFETY: 查询只读，output 在 ecall 期间有效。
    unsafe { crate::call::sys_handle_query(handle, &mut output) }?;
    Ok(output)
}

pub fn duplicate(handle: Handle, rights: Rights) -> Result<Handle, SystemCallError> {
    let mut output = Handle::INVALID;
    // SAFETY: output 在 ecall 期间有效且可写。
    unsafe { sys_handle_duplicate(handle, rights, &mut output)? };
    Ok(output)
}
