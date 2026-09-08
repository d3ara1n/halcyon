//! 进程本地 Handle 的基础操作。

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, Rights},
};

use crate::call::{sys_handle_close, sys_handle_duplicate};

pub fn close(handle: Handle) -> Result<(), SystemCallError> {
    // SAFETY: Handle 是值参数。
    unsafe { sys_handle_close(handle) }
}

/// 关闭由安全 typed owner 唯一持有的非 Tunnel 叶 Handle。
///
/// 此边界不可用于任意 raw Handle：合法 leaf owner 的表项仍在本进程且内核关闭
/// 路径无异步阶段，因此错误只表示 unsafe owner 构造契约或内核不变量被破坏。
pub(crate) fn close_leaf_owner(handle: Handle) {
    close(handle)
        .unwrap_or_else(|error| panic!("typed leaf Handle close invariant violated: {error:?}"));
}

pub fn duplicate(handle: Handle, rights: Rights) -> Result<Handle, SystemCallError> {
    let mut output = Handle::INVALID;
    // SAFETY: output 在 ecall 期间有效且可写。
    unsafe { sys_handle_duplicate(handle, rights, &mut output)? };
    Ok(output)
}
