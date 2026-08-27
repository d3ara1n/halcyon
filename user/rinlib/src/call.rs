#[cfg(target_arch = "riscv64")]
use core::arch::asm;

use erhino_shared::{
    call::{SystemCall, SystemCallError},
    mem::Address,
    message::{HandleMove, MailboxBadge, MessageHeader, SendHeader},
    object::{Handle, HandlePair, Rights},
    proc::{
        ExitCode, JobEnumerateResult, JobSnapshot, ProcessCreateResult, ProcessDrainResult,
        ProcessMapFlags, ProcessSnapshot, ProcessStartDescriptor, Tid,
    },
    wait::{WaitItem, WaitResult},
};
use num_traits::FromPrimitive;

type SystemCallResult<T> = Result<T, SystemCallError>;

fn to_error(error: usize) -> SystemCallError {
    if let Some(ret) = SystemCallError::from_usize(error) {
        ret
    } else {
        SystemCallError::Unknown
    }
}

#[cfg(target_arch = "riscv64")]
unsafe fn raw_call(
    id: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> (usize, usize) {
    let mut error_code;
    let mut result;
    unsafe {
        asm!("ecall", in("x17") id, inlateout("x10") arg0 => error_code, inlateout("x11") arg1 => result, in("x12") arg2, in("x13") arg3, in("x14") arg4, in("x15") arg5);
    }
    (error_code, result)
}

#[cfg(not(target_arch = "riscv64"))]
unsafe fn raw_call(
    _id: usize,
    _arg0: usize,
    _arg1: usize,
    _arg2: usize,
    _arg3: usize,
    _arg4: usize,
    _arg5: usize,
) -> (usize, usize) {
    panic!("system calls are unavailable on the host target")
}

fn sys_call(
    call: SystemCall,
    arg0: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
) -> SystemCallResult<usize> {
    // SAFETY: ecall 是唯一内核入口，参数按 ABI 传寄存器。
    let (error, ret) = unsafe { raw_call(call as usize, arg0, arg1, arg2, arg3, 0, 0) };
    if error == 0 {
        Ok(ret)
    } else {
        Err(to_error(error))
    }
}

fn sys_call6(
    call: SystemCall,
    args: [usize; 6],
) -> SystemCallResult<usize> {
    // SAFETY: ecall 是唯一内核入口，参数按 ABI 传寄存器。
    let (error, ret) = unsafe {
        raw_call(call as usize, args[0], args[1], args[2], args[3], args[4], args[5])
    };
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

pub unsafe fn sys_job_create(
    parent: Handle,
    rights: Rights,
    output: &mut Handle,
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::JobCreate,
        parent.raw() as usize,
        rights.raw() as usize,
        output as *mut Handle as usize,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_job_seal(control: Handle) -> SystemCallResult<()> {
    sys_call(SystemCall::JobSeal, control.raw() as usize, 0, 0, 0).map(|_| ())
}

pub unsafe fn sys_job_query(control: Handle, output: &mut JobSnapshot) -> SystemCallResult<()> {
    sys_call(
        SystemCall::JobQuery,
        control.raw() as usize,
        output as *mut JobSnapshot as usize,
        0,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_job_enumerate(
    control: Handle,
    kind: u32,
    cursor: u64,
    buf: *mut u64,
    buf_len: usize,
    output: &mut JobEnumerateResult,
) -> SystemCallResult<()> {
    sys_call6(
        SystemCall::JobEnumerate,
        [
            control.raw() as usize,
            kind as usize,
            cursor as usize,
            buf as usize,
            buf_len,
            output as *mut JobEnumerateResult as usize,
        ],
    )
    .map(|_| ())
}

pub unsafe fn sys_job_derive(
    control: Handle,
    kind: u32,
    id: u64,
    rights: Rights,
    output: &mut Handle,
) -> SystemCallResult<()> {
    sys_call6(
        SystemCall::JobDerive,
        [
            control.raw() as usize,
            kind as usize,
            id as usize,
            rights.raw() as usize,
            output as *mut Handle as usize,
            0,
        ],
    )
    .map(|_| ())
}

pub unsafe fn sys_process_create(
    job: Handle,
    control_rights: Rights,
    output: &mut ProcessCreateResult,
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::ProcessCreate,
        job.raw() as usize,
        control_rights.raw() as usize,
        output as *mut ProcessCreateResult as usize,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_process_query(
    control: Handle,
    output: &mut ProcessSnapshot,
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::ProcessQuery,
        control.raw() as usize,
        output as *mut ProcessSnapshot as usize,
        0,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_process_kill(control: Handle, code: i64) -> SystemCallResult<()> {
    sys_call(
        SystemCall::ProcessKill,
        control.raw() as usize,
        code as usize,
        0,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_process_drain(
    control: Handle,
    max_work: u32,
    output: &mut ProcessDrainResult,
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::ProcessDrain,
        control.raw() as usize,
        max_work as usize,
        output as *mut ProcessDrainResult as usize,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_process_map(
    builder: Handle,
    target: usize,
    len: usize,
    permissions: ProcessMapFlags,
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::ProcessMap,
        builder.raw() as usize,
        target,
        len,
        permissions.raw() as usize,
    )
    .map(|_| ())
}

pub unsafe fn sys_process_write(
    builder: Handle,
    target: usize,
    source: &[u8],
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::ProcessWrite,
        builder.raw() as usize,
        target,
        source.as_ptr() as usize,
        source.len(),
    )
    .map(|_| ())
}

pub unsafe fn sys_process_start(
    builder: Handle,
    descriptor: &ProcessStartDescriptor,
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::ProcessStart,
        builder.raw() as usize,
        descriptor as *const ProcessStartDescriptor as usize,
        0,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_thread_spawn(func_point: Address) -> SystemCallResult<Tid> {
    sys_call(SystemCall::ThreadSpawn, func_point, 0, 0, 0).map(|t| t as Tid)
}

pub unsafe fn sys_tunnel_create(addr: usize, output: &mut HandlePair) -> SystemCallResult<()> {
    sys_call(
        SystemCall::TunnelCreate,
        addr,
        output as *mut HandlePair as usize,
        0,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_tunnel_attach(
    invitation: Handle,
    addr: usize,
    output: &mut Handle,
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::TunnelAttach,
        invitation.raw() as usize,
        addr,
        output as *mut Handle as usize,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_tunnel_notify(endpoint: Handle) -> SystemCallResult<()> {
    sys_call(SystemCall::TunnelNotify, endpoint.raw() as usize, 0, 0, 0).map(|_| ())
}

pub unsafe fn sys_tunnel_acknowledge_data(endpoint: Handle) -> SystemCallResult<()> {
    sys_call(
        SystemCall::TunnelAcknowledgeData,
        endpoint.raw() as usize,
        0,
        0,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_handle_close(handle: Handle) -> SystemCallResult<()> {
    sys_call(SystemCall::HandleClose, handle.raw() as usize, 0, 0, 0).map(|_| ())
}

pub unsafe fn sys_handle_duplicate(
    source: Handle,
    rights: Rights,
    output: &mut Handle,
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::HandleDuplicate,
        source.raw() as usize,
        rights.raw() as usize,
        output as *mut Handle as usize,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_mailbox_create(
    owner_rights: Rights,
    sender_rights: Rights,
    output: &mut HandlePair,
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::MailboxCreate,
        owner_rights.raw() as usize,
        sender_rights.raw() as usize,
        output as *mut HandlePair as usize,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_send(
    mailbox: Handle,
    kind: u64,
    payload: &[u8],
    moves: &[HandleMove],
) -> SystemCallResult<()> {
    let header = SendHeader::new(kind, payload.len() as u32, moves.len() as u32);
    sys_call6(
        SystemCall::Send,
        [
            mailbox.raw() as usize,
            &header as *const SendHeader as usize,
            payload.as_ptr() as usize,
            moves.as_ptr() as usize,
            moves.len(),
            payload.len(),
        ],
    )
    .map(|_| ())
}

pub unsafe fn sys_peek(mailbox: Handle, output: &mut MessageHeader) -> SystemCallResult<()> {
    sys_call(
        SystemCall::Peek,
        mailbox.raw() as usize,
        output as *mut MessageHeader as usize,
        0,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_receive(
    mailbox: Handle,
    header: &mut MessageHeader,
    payload: &mut [u8],
    handles: &mut [Handle],
) -> SystemCallResult<()> {
    sys_call6(
        SystemCall::Receive,
        [
            mailbox.raw() as usize,
            header as *mut MessageHeader as usize,
            payload.as_mut_ptr() as usize,
            payload.len(),
            handles.as_mut_ptr() as usize,
            handles.len(),
        ],
    )
    .map(|_| ())
}

pub unsafe fn sys_discard(mailbox: Handle) -> SystemCallResult<()> {
    sys_call(SystemCall::Discard, mailbox.raw() as usize, 0, 0, 0).map(|_| ())
}

pub unsafe fn sys_mailbox_make_send_once(
    source: Handle,
    rights: Rights,
    output: &mut Handle,
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::MailboxMakeSendOnce,
        source.raw() as usize,
        rights.raw() as usize,
        output as *mut Handle as usize,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_mailbox_mint_sender(
    owner: Handle,
    badge: MailboxBadge,
    rights: Rights,
    output: &mut Handle,
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::MailboxMintSender,
        owner.raw() as usize,
        badge as usize,
        rights.raw() as usize,
        output as *mut Handle as usize,
    )
    .map(|_| ())
}

pub unsafe fn sys_wait_many(
    items: &[WaitItem],
    result: &mut WaitResult,
    deadline_ms: u64,
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::WaitMany,
        items.as_ptr() as usize,
        items.len(),
        result as *mut WaitResult as usize,
        deadline_ms as usize,
    )
    .map(|_| ())
}

pub unsafe fn sys_notification_create(
    owner_rights: Rights,
    signaler_rights: Rights,
    output: &mut HandlePair,
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::NotificationCreate,
        owner_rights.raw() as usize,
        signaler_rights.raw() as usize,
        output as *mut HandlePair as usize,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_notification_signal(handle: Handle, bits: u64) -> SystemCallResult<()> {
    sys_call(
        SystemCall::NotificationSignal,
        handle.raw() as usize,
        bits as usize,
        0,
        0,
    )
    .map(|_| ())
}

pub unsafe fn sys_notification_take(
    handle: Handle,
    mask: u64,
    output: &mut u64,
) -> SystemCallResult<()> {
    sys_call(
        SystemCall::NotificationTake,
        handle.raw() as usize,
        mask as usize,
        output as *mut u64 as usize,
        0,
    )
    .map(|_| ())
}

// 当前线程睡眠指定毫秒（异步 syscall：内核登记期限，到期唤醒）
pub unsafe fn sys_sleep(ms: u64) -> SystemCallResult<()> {
    sys_call(SystemCall::Sleep, ms as usize, 0, 0, 0).map(|_| ())
}
