use core::{arch::asm, mem::size_of};

use erhino_shared::{
    call::{SystemCall, SystemCallError},
    fal::{DentryAttribute, DentryType},
    mem::Address,
    message::MessageDigest,
    proc::{ExitCode, Pid, SystemSignal, Tid},
};
use flagset::FlagSet;
use num_traits::FromPrimitive;
use num_traits::ToPrimitive;

type SystemCallResult<T> = Result<T, SystemCallError>;

fn to_error(error: usize) -> SystemCallError {
    if let Some(ret) = SystemCallError::from_usize(error) {
        ret
    } else {
        SystemCallError::Unknown
    }
}

unsafe fn raw_call(
    id: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
) -> (usize, usize) {
    let mut error_code;
    let mut result;
    asm!("ecall", in("x17") id, inlateout("x10") arg0 => error_code, inlateout("x11") arg1 => result, in("x12") arg2, in("x13") arg3);
    (error_code, result)
}

unsafe fn sys_call(
    call: SystemCall,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
) -> SystemCallResult<usize> {
    let (error, ret) = raw_call(call as usize, arg0, arg1, arg2, arg3);
    if error == 0 {
        Ok(ret)
    } else {
        Err(to_error(error))
    }
}

// returns actual byte count sent to debug stream
pub unsafe fn sys_debug(msg: &str) -> SystemCallResult<usize> {
    sys_call(SystemCall::Debug, msg.as_ptr() as usize, msg.len(), 0, 0)
}

// returns the new heap top address, or the current when size is 0
pub unsafe fn sys_extend(size: usize) -> SystemCallResult<Address> {
    sys_call(SystemCall::Extend, size, 0, 0, 0)
}

// returns nothing
pub unsafe fn sys_exit(code: ExitCode) -> SystemCallResult<()> {
    sys_call(SystemCall::Exit, code as usize, 0, 0, 0).map(|_| ())
}

pub unsafe fn sys_thread_spawn(func_point: Address) -> SystemCallResult<Tid> {
    sys_call(SystemCall::ThreadSpawn, func_point, 0, 0, 0).map(|t| t as Tid)
}

pub unsafe fn sys_tunnel_build() -> SystemCallResult<usize> {
    sys_call(SystemCall::TunnelBuild, 0, 0, 0, 0)
}

pub unsafe fn sys_tunnel_link(key: usize) -> SystemCallResult<Address> {
    sys_call(SystemCall::TunnelLink, key, 0, 0, 0)
}

pub unsafe fn sys_tunnel_dispose(key: usize) -> SystemCallResult<()> {
    sys_call(SystemCall::TunnelDispose, key, 0, 0, 0).map(|_| {})
}

pub unsafe fn sys_signal_set(
    mask: FlagSet<SystemSignal>,
    handler: Address,
) -> SystemCallResult<()> {
    sys_call(SystemCall::SignalSet, mask.bits() as usize, handler, 0, 0).map(|_| ())
}

pub unsafe fn sys_signal_send(pid: Pid, signal: SystemSignal) -> SystemCallResult<bool> {
    sys_call(
        SystemCall::SignalSend,
        pid as usize,
        signal
            .to_u64()
            .expect("cast system signal to signal map wont failed") as usize,
        0,
        0,
    )
    .map(|f| f != 0)
}

pub unsafe fn sys_signal_return() -> SystemCallResult<()> {
    sys_call(SystemCall::SignalReturn, 0, 0, 0, 0).map(|_| ())
}

// 返回需要准备的 buffer 大小
pub unsafe fn sys_access(path: &str) -> SystemCallResult<usize> {
    sys_call(SystemCall::Access, path.as_ptr() as usize, path.len(), 0, 0)
}

// 返回在 buffer 中实际写入的 Dentry 数量
pub unsafe fn sys_inspect(path: &str, buffer: &[u8]) -> SystemCallResult<usize> {
    sys_call(
        SystemCall::Inspect,
        path.as_ptr() as usize,
        path.len(),
        buffer.as_ptr() as usize,
        buffer.len(),
    )
}

// 实际写入在 buffer 有效部分的长度
pub unsafe fn sys_read(path: &str, buffer: &[u8]) -> SystemCallResult<usize> {
    sys_call(
        SystemCall::Read,
        path.as_ptr() as usize,
        path.len(),
        buffer.as_ptr() as usize,
        buffer.len(),
    )
}

pub unsafe fn sys_write(path: &str, buffer: &[u8]) -> SystemCallResult<()> {
    sys_call(
        SystemCall::Write,
        path.as_ptr() as usize,
        path.len(),
        buffer.as_ptr() as usize,
        buffer.len(),
    )
    .map(|_| ())
}

pub unsafe fn sys_create(
    path: &str,
    kind: DentryType,
    attr: FlagSet<DentryAttribute>,
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::Create,
        path.as_ptr() as usize,
        path.len(),
        kind as u8 as usize,
        attr.bits() as usize,
    )
    .map(|_| ())
}

pub unsafe fn sys_send(target: Pid, kind: usize, buffer: &[u8]) -> SystemCallResult<()> {
    sys_call(
        SystemCall::Send,
        target as usize,
        kind,
        buffer.as_ptr() as usize,
        buffer.len(),
    )
    .map(|_| ())
}

pub unsafe fn sys_peek(digest_buffer: &[u8]) -> SystemCallResult<bool> {
    sys_call(
        SystemCall::Peek,
        digest_buffer.as_ptr() as usize,
        size_of::<MessageDigest>(),
        0,
        0,
    )
    .map(|b| b > 0)
}

pub unsafe fn sys_receive(buffer: &[u8]) -> SystemCallResult<usize> {
    sys_call(
        SystemCall::Receive,
        buffer.as_ptr() as usize,
        buffer.len(),
        0,
        0,
    )
}

// 当前线程睡眠指定毫秒（异步 syscall：内核登记期限，到期唤醒）
pub unsafe fn sys_sleep(ms: u64) -> SystemCallResult<()> {
    sys_call(SystemCall::Sleep, ms as usize, 0, 0, 0).map(|_| ())
}
