//! 隧道系统调用的 rinlib 封装。协议层体验在 librunnel，这里只暴露
//! 机制级原语。

use erhino_shared::{call::SystemCallError, mem::Address};

use crate::call::{
    sys_tunnel_attach, sys_tunnel_create, sys_tunnel_dispose, sys_tunnel_notify,
};

type SystemCallResult<T> = Result<T, SystemCallError>;

/// 创建隧道：零态页映射到 addr，返回隧道 id。
pub fn create(addr: usize) -> SystemCallResult<u64> {
    unsafe { sys_tunnel_create(addr) }
}

/// 凭 id 挂接本进程第二端点到 addr。
pub fn attach(id: u64, addr: usize) -> SystemCallResult<()> {
    unsafe { sys_tunnel_attach(id, addr) }
}

/// 拆除本端点；双端皆亡时内核归还帧。
pub fn dispose(id: u64) -> SystemCallResult<()> {
    unsafe { sys_tunnel_dispose(id as usize) }
}

/// 摇门铃：在对端信号状态提交 DATA 事件。
pub fn notify(id: u64) -> SystemCallResult<()> {
    unsafe { sys_tunnel_notify(id) }
}

/// 保留 Address 引用（attach 地址参数的类型化文档）。
const _: () = {
    let _ = core::mem::size_of::<Address>();
};
