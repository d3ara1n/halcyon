use core::arch::asm;

use erhino_shared::{
    call::{SystemCall, SystemCallError},
    fal::{DentryAttribute, DentryType},
    mem::Address,
    message::MessageDigest,
    proc::{ExitCode, Pid, SignalMap, Tid},
    signal::SignalItem,
};
use flagset::FlagSet;
use num_traits::FromPrimitive;

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
    unsafe {
        asm!("ecall", in("x17") id, inlateout("x10") arg0 => error_code, inlateout("x11") arg1 => result, in("x12") arg2, in("x13") arg3);
    }
    (error_code, result)
}

fn sys_call(
    call: SystemCall,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
) -> SystemCallResult<usize> {
    // SAFETY: ecall 是唯一内核入口，参数按 ABI 传寄存器。
    let (error, ret) = unsafe { raw_call(call as usize, arg0, arg1, arg2, arg3) };
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

/// 非阻塞检查邮箱队头：有则填充 digest 并返回负载长度，空箱返回
/// ObjectNotAvailable。
pub unsafe fn sys_peek(digest: *mut MessageDigest) -> SystemCallResult<usize> {
    sys_call(
        SystemCall::Peek,
        digest as usize,
        0,
        0,
        0,
    )
}

/// 取队头消息负载到 buffer（长度须经 Peek 预知）。空箱时**阻塞**：
/// 线程转 Waiting，消息到达即唤醒，返回负载长度。
pub unsafe fn sys_receive(buffer: &mut [u8]) -> SystemCallResult<usize> {
    sys_call(
        SystemCall::Receive,
        buffer.as_ptr() as usize,
        buffer.len(),
        0,
        0,
    )
}

pub unsafe fn sys_discard() -> SystemCallResult<()> {
    sys_call(SystemCall::Discard, 0, 0, 0, 0).map(|_| ())
}

/// 向目标进程提交信号位。返回是否移交唤醒了等待者（false = 已并入粘滞余量）。
pub unsafe fn sys_signal_send(pid: Pid, mask: SignalMap) -> SystemCallResult<bool> {
    sys_call(SystemCall::SignalSend, pid as usize, mask as usize, 0, 0).map(|w| w != 0)
}

/// 阻塞等待任一对象的关注位命中。items 见 [`SignalItem`]；返回
/// `(命中项下标, 命中位)`。
pub unsafe fn sys_signal_wait(items: &[SignalItem]) -> SystemCallResult<(usize, SignalMap)> {
    let packed = sys_call(
        SystemCall::SignalWait,
        items.as_ptr() as usize,
        items.len(),
        0,
        0,
    )?;
    Ok((packed >> 56, (packed & 0x00FF_FFFF_FFFF_FFFF) as SignalMap))
}

// 当前线程睡眠指定毫秒（异步 syscall：内核登记期限，到期唤醒）
pub unsafe fn sys_sleep(ms: u64) -> SystemCallResult<()> {
    sys_call(SystemCall::Sleep, ms as usize, 0, 0, 0).map(|_| ())
}
