//! syscall 分发（notes/impls/call.md）：a7 调用号、a0–a5 参数；
//! 返回 a0 = 错误码（0 = NoError）、a1 = 返回值，sepc 前进 4。
//!
//! 出口三值 `Outcome`——同步调用 Completed；异步调用登记内核请求后 Wait
//! （完成时 wake 回 Ready）；Killed 终止进程。未知调用号返回错误，绝不 panic。

use num_traits::{FromPrimitive, ToPrimitive};
use erhino_shared::call::{SystemCall, SystemCallError};

use crate::{context::UserContext, sched, task::ipc, task::Thread, uaccess};

use alloc::boxed::Box;

/// syscall 处理出口（见 notes/impls/call.md「异步调用」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 结果已写入 UserContext，Resume 回用户态。
    Completed,
    /// 已登记内核请求，线程转 Waiting，Switch 回调度循环。
    Wait,
    /// 进程终止（Exit / 致命错误），Switch 后回收。
    Killed(i64),
}

/// 分发一次系统调用（trap handler 调用）。
pub fn dispatch(frame: &mut UserContext, thread: &Thread) -> Outcome {
    let Some(call) = SystemCall::from_usize(frame.x[17] as usize) else {
        respond_error(frame, SystemCallError::Unknown);
        return Outcome::Completed;
    };
    let a0 = frame.x[10] as usize;

    match call {
        SystemCall::Debug => {
            debug_print(frame, thread);
            Outcome::Completed
        }
        SystemCall::Exit => Outcome::Killed(a0 as i64),
        SystemCall::Extend => {
            extend_heap(frame, thread, a0);
            Outcome::Completed
        }
        SystemCall::Sleep => {
            let ms = a0 as u64;
            if ms == 0 {
                respond_ok(frame, 0);
            } else {
                // 只登记本 hart 意图槽；全局发布由调度循环在线程离开
                // 执行点后完成（sched::park_publish，唤醒所有权随迁）。
                sched::park_request_sleep(ms);
                return Outcome::Wait; // 不前进 sepc，完成唤醒后由帧携带结果
            }
            Outcome::Completed
        }
        SystemCall::Send => {
            // Send(target, kind, buf, len)：永不阻塞，满箱 MailboxFull。
            match ipc::send(thread, frame.x[12] as usize, frame.x[13] as usize, a0 as u32, frame.x[11] as usize) {
                Ok(len) => respond_ok(frame, len),
                Err(e) => respond_error(frame, e),
            }
            Outcome::Completed
        }
        SystemCall::Peek => {
            match ipc::peek(thread, a0) {
                Ok(len) => respond_ok(frame, len),
                Err(e) => respond_error(frame, e),
            }
            Outcome::Completed
        }
        SystemCall::Discard => {
            match ipc::discard(thread) {
                Ok(()) => respond_ok(frame, 0),
                Err(e) => respond_error(frame, e),
            }
            Outcome::Completed
        }
        SystemCall::Receive => {
            match ipc::receive(thread, a0, frame.x[11] as usize) {
                ipc::RecvOutcome::Done(result) => {
                    match result {
                        Ok(len) => respond_ok(frame, len),
                        Err(e) => respond_error(frame, e),
                    }
                    Outcome::Completed
                }
                ipc::RecvOutcome::Block => {
                    // 空箱阻塞：登记意图后转 Waiting；完成时帧携带
                    // a1 = 负载长度，sepc 由完成方前进。
                    sched::park_request_recv(a0, frame.x[11] as usize);
                    Outcome::Wait
                }
            }
        }
        SystemCall::SignalSend => {
            match ipc::signal_send(thread, a0 as u32, frame.x[11]) {
                Ok(woken) => respond_ok(frame, woken as usize),
                Err(e) => respond_error(frame, e),
            }
            Outcome::Completed
        }
        SystemCall::SignalWait => {
            match ipc::signal_wait(thread, a0, frame.x[11] as usize) {
                Ok(ipc::WaitPlan::Now { index, bits }) => {
                    respond_ok(frame, ((index as u64) << 56 | bits) as usize);
                    Outcome::Completed
                }
                Ok(ipc::WaitPlan::Park(targets)) => {
                    sched::park_request_signal(Box::into_raw(Box::new(targets)));
                    Outcome::Wait
                }
                Err(e) => {
                    respond_error(frame, e);
                    Outcome::Completed
                }
            }
        }
        SystemCall::TunnelCreate => {
            match crate::task::tunnel::create(thread, a0) {
                Ok(id) => respond_ok(frame, id as usize),
                Err(e) => respond_error(frame, e),
            }
            Outcome::Completed
        }
        SystemCall::TunnelAttach => {
            match crate::task::tunnel::attach(thread, a0 as u64, frame.x[11] as usize) {
                Ok(()) => respond_ok(frame, 0),
                Err(e) => respond_error(frame, e),
            }
            Outcome::Completed
        }
        SystemCall::TunnelDispose => {
            match crate::task::tunnel::dispose(thread, a0 as u64) {
                Ok(()) => respond_ok(frame, 0),
                Err(e) => respond_error(frame, e),
            }
            Outcome::Completed
        }
        SystemCall::TunnelNotify => {
            match crate::task::tunnel::notify(thread, a0 as u64) {
                Ok(()) => respond_ok(frame, 0),
                Err(e) => respond_error(frame, e),
            }
            Outcome::Completed
        }
        // 未实现面：一律返回错误（内核不可被用户调用 panic）。
        _ => {
            respond_error(frame, SystemCallError::FunctionNotAvailable);
            Outcome::Completed
        }
    }
}

/// Debug(ptr, len)：读用户内存打印（rinlib debug! 的观测通道，测试用）。
/// 纯透传零策略：拷入内核堆后以 `[pid N]` 话题、debug 等级色输出；
/// 要自定义格式/颜色的用户态自己拼进消息字符串。正式的用户态输出
/// 是未来的 console 服务（设备租借 + IPC），不经过内核日志。
fn debug_print(frame: &mut UserContext, thread: &Thread) {
    let ptr = frame.x[10] as usize;
    let len = frame.x[11] as usize;
    // 限长先行（防恶意长度），再分配缓冲并拷入。
    if len > uaccess::MAX_USER_ACCESS {
        respond_error(frame, SystemCallError::MemoryNotAccessible);
        return;
    }
    let mut space = thread.process.space.lock();
    let mut buf = alloc::vec![0u8; len];
    if let Err(e) = uaccess::copy_from_user(&mut space, &mut buf, ptr) {
        drop(space);
        respond_error(frame, e.into());
        return;
    }
    drop(space);
    let tag = alloc::format!("pid {}", thread.process.pid);
    match core::str::from_utf8(&buf) {
        Ok(msg) => crate::console::log_tagged(&tag, crate::console::COLOR_DEBUG, format_args!("{}", msg)),
        Err(_) => crate::console::log_tagged(&tag, crate::console::COLOR_DEBUG, format_args!("non-UTF-8 message, {} bytes", len)),
    }
    respond_ok(frame, len);
}

/// Extend(bytes)：sbrk 语义——申请 bytes 字节，内核取整到页粒度，返回
/// 新堆顶；bytes = 0 查询当前堆顶。页大小是实现细节，不经 ABI 泄漏。
fn extend_heap(frame: &mut UserContext, thread: &Thread, bytes: usize) {
    match thread.process.space.lock().extend_heap(bytes) {
        Ok(brk) => respond_ok(frame, brk),
        Err(_) => respond_error(frame, SystemCallError::OutOfMemory),
    }
}

fn respond_ok(frame: &mut UserContext, ret: usize) {
    frame.x[10] = SystemCallError::NoError as u64;
    frame.x[11] = ret as u64;
    frame.sepc += 4;
}

fn respond_error(frame: &mut UserContext, err: SystemCallError) {
    frame.x[10] = err.to_usize().unwrap_or(1) as u64;
    frame.sepc += 4;
}
