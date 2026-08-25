//! Tunnel Endpoint/Invitation 的机制级封装。

use erhino_shared::{
    call::SystemCallError,
    object::{Handle, HandlePair},
};

use crate::call::{
    sys_tunnel_acknowledge_data, sys_tunnel_attach, sys_tunnel_create, sys_tunnel_notify,
};

type SystemCallResult<T> = Result<T, SystemCallError>;

pub fn create(addr: usize) -> SystemCallResult<HandlePair> {
    let mut output = HandlePair::new(Handle::INVALID, Handle::INVALID);
    // SAFETY: output 在 ecall 期间有效且可写。
    unsafe { sys_tunnel_create(addr, &mut output)? };
    Ok(output)
}

pub fn attach(invitation: Handle, addr: usize) -> SystemCallResult<Handle> {
    let mut output = Handle::INVALID;
    // SAFETY: output 在 ecall 期间有效且可写；成功时 invitation 被原子消费。
    unsafe { sys_tunnel_attach(invitation, addr, &mut output)? };
    Ok(output)
}

pub fn notify(endpoint: Handle) -> SystemCallResult<()> {
    // SAFETY: Handle 是值参数。
    unsafe { sys_tunnel_notify(endpoint) }
}

pub fn acknowledge_data(endpoint: Handle) -> SystemCallResult<()> {
    // SAFETY: Handle 是值参数。
    unsafe { sys_tunnel_acknowledge_data(endpoint) }
}
