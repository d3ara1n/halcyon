//! syscall 分发（notes/impls/call.md）：a7 调用号、a0–a5 参数；
//! 返回 a0 = 错误码（0 = NoError）、a1 = 返回值，sepc 前进 4。
//!
//! 出口四值 `Outcome`——同步调用 Completed；主动让出 Requeue；异步调用 Wait；
//! 终局调用 Killed。未知调用号返回错误，绝不 panic。

use erhino_shared::{
    call::{SystemCall, SystemCallError},
    object::{Handle, Rights},
    proc::{ProcessMapFlags, TerminationCause, TerminationDetail},
};
use num_traits::{FromPrimitive, ToPrimitive};

use crate::{
    context::UserContext,
    sched,
    task::{self, Thread, handle, mailbox, notification, thread as task_thread, wait},
    uaccess,
};

/// syscall 处理出口（见 notes/impls/call.md「异步调用」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 结果已写入 UserContext，Resume 回用户态。
    Completed,
    /// 当前线程主动让出，Switch 后重新入队。
    Requeue,
    /// 已登记内核请求，线程转 Waiting，Switch 回调度循环。
    Wait,
    /// 进程终止（终因已在 lifecycle 冻结），Switch 后回收。
    Killed,
}

/// 分发一次系统调用（trap handler 调用）。
pub fn dispatch(frame: &mut UserContext, thread: &Thread) -> Outcome {
    let Some(call) = SystemCall::from_usize(frame.x[17] as usize) else {
        respond_error(frame, SystemCallError::Unknown);
        return Outcome::Completed;
    };
    let a0 = frame.x[10] as usize;

    let outcome = match call {
        SystemCall::Debug => {
            debug_print(frame, thread);
            Outcome::Completed
        }
        SystemCall::SystemReset => {
            respond_result(
                frame,
                task::system_reset::request(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11],
                    frame.x[12],
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::ProcessTerminate => {
            thread
                .process
                .freeze_and_seal(TerminationCause::Terminated, TerminationDetail::NONE);
            Outcome::Killed
        }
        SystemCall::ProcessAbort => {
            thread
                .process
                .freeze_and_seal(TerminationCause::Aborted, TerminationDetail::NONE);
            Outcome::Killed
        }
        SystemCall::ThreadExit => {
            let _ = a0;
            Outcome::Killed
        }
        SystemCall::ThreadYield => {
            respond_ok(frame, 0);
            Outcome::Requeue
        }
        SystemCall::ThreadCreate => {
            let result = if frame.x[10] == Handle::INVALID.raw() {
                task_thread::create(thread, frame.x[11] as usize, frame.x[12] as usize)
            } else {
                task::process::create_thread(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11] as usize,
                    frame.x[12] as usize,
                )
                .map(|_| ())
            };
            respond_result(frame, result.map(|_| 0));
            Outcome::Completed
        }
        SystemCall::ThreadStart => {
            respond_result(
                frame,
                task_thread::start(thread, Handle::from_raw(frame.x[10])).map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::ProcessBindExecution => {
            respond_result(
                frame,
                task::process::bind_execution(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::JobCreate => {
            respond_result(
                frame,
                task::job::create(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    Handle::from_raw(frame.x[11]),
                    Rights::from_raw(frame.x[12]),
                    frame.x[13] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::ProcessCreate => {
            respond_result(
                frame,
                task::process::create(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    Handle::from_raw(frame.x[11]),
                    Rights::from_raw(frame.x[12]),
                    frame.x[13] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::ProcessMap => {
            match task::process::map(
                thread,
                Handle::from_raw(frame.x[10]),
                frame.x[11] as usize,
                frame.x[12] as usize,
                ProcessMapFlags::from_raw(frame.x[13] as u32),
            ) {
                Ok(plan) => {
                    sched::park_request_wait(plan);
                    Outcome::Wait
                }
                Err(error) => {
                    respond_error(frame, error);
                    Outcome::Completed
                }
            }
        }
        SystemCall::ProcessWrite => {
            respond_result(
                frame,
                task::process::write(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11] as usize,
                    frame.x[12] as usize,
                    frame.x[13] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::ProcessStart => {
            match task::process::start(thread, Handle::from_raw(frame.x[10])) {
                Ok(plan) => {
                    sched::park_request_wait(plan);
                    Outcome::Wait
                }
                Err(error) => {
                    respond_error(frame, error);
                    Outcome::Completed
                }
            }
        }
        SystemCall::ProcessGrant => {
            respond_result(
                frame,
                task::process::grant(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11] as usize,
                    frame.x[12] as usize,
                    frame.x[13] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::ProcessQuery => {
            respond_result(
                frame,
                task::process::query(thread, Handle::from_raw(frame.x[10]), frame.x[11] as usize)
                    .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::ProcessKill => {
            match task::process::kill(thread, Handle::from_raw(frame.x[10])) {
                Ok(task::process::KillOutcome::Accepted) => {
                    respond_ok(frame, 0);
                    Outcome::Completed
                }
                Ok(task::process::KillOutcome::AlreadyTerminating) => {
                    respond_ok(frame, 1);
                    Outcome::Completed
                }
                // 自杀式调用不返回用户态；终因已在 lifecycle 冻结。
                Ok(task::process::KillOutcome::TerminatedCaller) => Outcome::Killed,
                Err(error) => {
                    respond_error(frame, error);
                    Outcome::Completed
                }
            }
        }
        SystemCall::JobSeal => {
            respond_result(
                frame,
                task::job::seal(thread, Handle::from_raw(frame.x[10])).map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::JobQuery => {
            respond_result(
                frame,
                task::job::query(thread, Handle::from_raw(frame.x[10]), frame.x[11] as usize)
                    .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::JobEnumerate => {
            respond_result(
                frame,
                task::job::enumerate(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11],
                    frame.x[12],
                    frame.x[13] as usize,
                    frame.x[14] as usize,
                    frame.x[15] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::JobDerive => {
            respond_result(
                frame,
                task::job::derive(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11],
                    frame.x[12],
                    Rights::from_raw(frame.x[13]),
                    frame.x[14] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::MemoryMap => match crate::vm::operation::memory_map(thread, a0) {
            Ok(plan) => {
                sched::park_request_wait(plan);
                Outcome::Wait
            }
            Err(error) => {
                respond_error(frame, error);
                Outcome::Completed
            }
        },
        SystemCall::MemoryUnmap => {
            match crate::vm::operation::memory_unmap(
                thread,
                frame.x[10] as usize,
                frame.x[11] as usize,
            ) {
                Ok(plan) => {
                    sched::park_request_wait(plan);
                    Outcome::Wait
                }
                Err(error) => {
                    respond_error(frame, error);
                    Outcome::Completed
                }
            }
        }
        SystemCall::MemoryProtect => {
            match crate::vm::operation::memory_protect(
                thread,
                frame.x[10] as usize,
                frame.x[11] as usize,
                frame.x[12] as usize,
            ) {
                Ok(plan) => {
                    sched::park_request_wait(plan);
                    Outcome::Wait
                }
                Err(error) => {
                    respond_error(frame, error);
                    Outcome::Completed
                }
            }
        }
        SystemCall::MemoryPoolQuery => {
            respond_result(
                frame,
                task::memory_pool::query(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::MemoryObjectCreate => {
            respond_result(
                frame,
                task::memory_object::create(thread, frame.x[10] as usize).map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::MemoryObjectQuery => {
            respond_result(
                frame,
                task::memory_object::query(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::MemoryObjectSeal => {
            respond_result(
                frame,
                task::memory_object::seal(thread, Handle::from_raw(frame.x[10])).map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::MemoryPoolDerive => {
            respond_result(
                frame,
                task::memory_pool::derive(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11] as usize,
                    frame.x[12] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::Sleep => {
            let plan = (|| {
                let deadline = {
                    let body = thread
                        .process
                        .lease_body()
                        .ok_or(SystemCallError::ObjectClosed)?;
                    let mut vm = body.vm.space.lock();
                    let space = vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?;
                    // SAFETY: Deadline 仅含固定宽整数，sleep_plan 验证判别和值域。
                    unsafe { uaccess::read_user_value(space, a0) }?
                };
                wait::sleep_plan(deadline)
            })();
            match plan {
                Ok(plan) => {
                    sched::park_request_wait(plan);
                    Outcome::Wait
                }
                Err(error) => {
                    respond_error(frame, error);
                    Outcome::Completed
                }
            }
        }
        SystemCall::HandleClose => {
            respond_result(
                frame,
                handle::close(thread, Handle::from_raw(frame.x[10])).map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::HandleDuplicate => {
            respond_result(
                frame,
                handle::duplicate(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    Rights::from_raw(frame.x[11]),
                    frame.x[12] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::WaitOne => match wait::prepare(thread, a0) {
            Ok(wait::WaitStart::Ready(signals)) => {
                respond_ok(frame, signals.raw() as usize);
                Outcome::Completed
            }
            Ok(wait::WaitStart::Park(plan)) => {
                sched::park_request_wait(plan);
                Outcome::Wait
            }
            Err(error) => {
                respond_error(frame, error);
                Outcome::Completed
            }
        },
        SystemCall::NotificationCreate => {
            respond_result(
                frame,
                notification::create(
                    thread,
                    Rights::from_raw(frame.x[10]),
                    Rights::from_raw(frame.x[11]),
                    frame.x[12] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::NotificationSignal => {
            respond_result(
                frame,
                notification::signal(thread, Handle::from_raw(frame.x[10]), frame.x[11]).map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::NotificationMintObserver => {
            respond_result(
                frame,
                notification::mint_observer(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    Rights::from_raw(frame.x[11]),
                    frame.x[12] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::NotificationTake => {
            respond_result(
                frame,
                notification::take(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11],
                    frame.x[12] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::WaitSetCreate => {
            respond_result(
                frame,
                task::wait_set::create(
                    thread,
                    a0,
                    Rights::from_raw(frame.x[11]),
                    frame.x[12] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::WaitSetRegister => {
            respond_result(
                frame,
                task::wait_set::register(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11] as usize,
                    frame.x[12] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::WaitSetReceive => {
            respond_result(
                frame,
                task::wait_set::receive(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11] as usize,
                    frame.x[12] as usize,
                    frame.x[13] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::WaitSetRemove => {
            respond_result(
                frame,
                task::wait_set::remove(thread, Handle::from_raw(frame.x[10]), frame.x[11])
                    .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::HandleQuery => {
            respond_result(
                frame,
                handle::query(thread, Handle::from_raw(frame.x[10]), frame.x[11] as usize)
                    .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::MonotonicNow => {
            let result =
                crate::clock::now().and_then(|snapshot| uaccess::write_user(thread, a0, &snapshot));
            respond_result(frame, result.map(|_| 0));
            Outcome::Completed
        }
        SystemCall::MailboxCreate => {
            respond_result(
                frame,
                mailbox::create(thread, Rights::from_raw(frame.x[10]), frame.x[11] as usize)
                    .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::Send => {
            respond_result(
                frame,
                mailbox::send(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11] as usize,
                    frame.x[12] as usize,
                    frame.x[13] as usize,
                    frame.x[14] as usize,
                    frame.x[15] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::Peek => {
            respond_result(
                frame,
                mailbox::peek(thread, Handle::from_raw(frame.x[10]), frame.x[11] as usize)
                    .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::Receive => {
            respond_result(
                frame,
                mailbox::receive(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11] as usize,
                    frame.x[12] as usize,
                    frame.x[13] as usize,
                    frame.x[14] as usize,
                    frame.x[15] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::Discard => {
            respond_result(
                frame,
                mailbox::discard(thread, Handle::from_raw(frame.x[10])).map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::MailboxMakeSendOnce => {
            respond_result(
                frame,
                mailbox::make_send_once(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    Rights::from_raw(frame.x[11]),
                    frame.x[12] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::MailboxMintSender => {
            respond_result(
                frame,
                mailbox::mint_sender(
                    thread,
                    Handle::from_raw(frame.x[10]),
                    frame.x[11],
                    Rights::from_raw(frame.x[12]),
                    frame.x[13] as usize,
                )
                .map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::TunnelCreate => {
            respond_result(frame, task::tunnel::create(thread, a0).map(|_| 0));
            Outcome::Completed
        }
        SystemCall::TunnelAttach => {
            respond_result(
                frame,
                task::tunnel::attach(thread, Handle::from_raw(frame.x[10]))
                    .map(|handle| handle.raw() as usize),
            );
            Outcome::Completed
        }
        SystemCall::EndpointNotify => {
            respond_result(
                frame,
                crate::task::tunnel::notify(thread, Handle::from_raw(frame.x[10])).map(|_| 0),
            );
            Outcome::Completed
        }
        SystemCall::EndpointAcknowledgeData => {
            respond_result(
                frame,
                crate::task::tunnel::acknowledge_data(thread, Handle::from_raw(frame.x[10]))
                    .map(|_| 0),
            );
            Outcome::Completed
        }
    };
    // 分发出口终止检查：syscall 执行期间冻结了终因（写回复检失败自杀、
    // 异 hart kill）则线程不回用户态——收束确定性提前一个 syscall，
    // 不依赖 sret 边界的 IPI 吸收时序。Wait 在此保留；trap 尾段仍可将
    // 它吸收为 Killed。调度循环统一取走意图，Park 安装或 Killed 放弃。
    if outcome == Outcome::Completed && thread.process.is_frozen() {
        return Outcome::Killed;
    }
    outcome
}

/// Debug(ptr, len)：读用户内存打印（rinlib debug! 的观测通道，测试用）。
/// 纯透传零策略：拷入内核堆后以 Debug 话题、pid 与 debug 等级色输出；
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
    let pool = thread.process.funding_pool();
    let mut buf = match task::resources::MetadataBuffer::try_empty(
        len,
        &task::resources::MetadataFunding(&pool),
    ) {
        Ok(buf) => buf,
        Err(error) => {
            respond_error(frame, task::resources::map_storage_error(error));
            return;
        }
    };
    for _ in 0..len {
        buf.push(0);
    }
    let copied = (|| {
        let body = thread
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let mut vm = body.vm.space.lock();
        uaccess::copy_from_user(
            vm.space.as_mut().ok_or(SystemCallError::ObjectClosed)?,
            &mut buf,
            ptr,
        )
        .map_err(SystemCallError::from)
    })();
    if let Err(error) = copied {
        respond_error(frame, error);
        return;
    }
    match core::str::from_utf8(&buf) {
        Ok(msg) => crate::console::log_tagged(
            "Debug",
            crate::console::COLOR_DEBUG,
            format_args!("pid {}: {}", thread.process.pid, msg),
        ),
        Err(_) => crate::console::log_tagged(
            "Debug",
            crate::console::COLOR_DEBUG,
            format_args!(
                "pid {}: non-UTF-8 message, {} bytes",
                thread.process.pid, len
            ),
        ),
    }
    respond_ok(frame, len);
}

fn respond_result(frame: &mut UserContext, result: Result<usize, SystemCallError>) {
    match result {
        Ok(value) => respond_ok(frame, value),
        Err(error) => respond_error(frame, error),
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
