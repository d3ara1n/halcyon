//! syscall 分发（notes/call.md）：a7 调用号、a0–a5 参数；
//! 返回 a0 = 错误码（0 = NoError）、a1 = 返回值，sepc 前进 4。
//!
//! 出口三值 `Outcome`——同步调用 Completed；异步调用登记内核请求后 Wait
//! （完成时 wake 回 Ready）；Killed 终止进程。未知调用号返回错误，绝不 panic。

use num_traits::{FromPrimitive, ToPrimitive};
use erhino_shared::call::{SystemCall, SystemCallError};

use crate::{sched, task::Thread, trap::frame_off};
use crate::trap::TrapFrame;

/// syscall 处理出口（见 notes/call.md「异步调用」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 结果已写入 TrapFrame，Resume 回用户态。
    Completed,
    /// 已登记内核请求，线程转 Waiting，Switch 回调度循环。
    Wait,
    /// 进程终止（Exit / 致命错误），Switch 后回收。
    Killed(i64),
}

/// 分发一次系统调用（trap handler 调用）。
pub fn dispatch(frame: &mut TrapFrame, thread: &Thread) -> Outcome {
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
                // 登记期限（发起 hart 立即 arm 定时器，唤醒所有权）。
                sched::sleep_register(ms, thread.process.pid);
                return Outcome::Wait; // 不前进 sepc，完成唤醒后由帧携带结果
            }
            Outcome::Completed
        }
        SystemCall::SignalSet => {
            // 记录式实现：接受 mask/handler 配置，信号注入/返回语义随信号里程碑交付。
            let mut signal = thread.process.signal.lock();
            signal.mask = frame.x[10];
            signal.handler = frame.x[11] as usize;
            drop(signal);
            respond_ok(frame, 0);
            Outcome::Completed
        }
        // 未实现面：一律返回错误（内核不可被用户调用 panic）。
        _ => {
            respond_error(frame, SystemCallError::FunctionNotAvailable);
            Outcome::Completed
        }
    }
}

/// Debug(ptr, len)：读用户内存打印（rinlib debug!/println! 的观测通道）。
/// SUM 位常开（mm::init 置），前置逐页校验映射后直访用户 VA。
fn debug_print(frame: &mut TrapFrame, thread: &Thread) {
    let ptr = frame.x[10] as usize;
    let len = frame.x[11] as usize;
    let ok = thread
        .process
        .space
        .lock()
        .validate(ptr, len);
    if !ok {
        respond_error(frame, SystemCallError::MemoryNotAccessible);
        return;
    }
    // SAFETY: 区间已逐页校验映射且在用户半区内，SUM 下可读。
    let bytes = unsafe { core::slice::from_raw_parts(ptr as *const u8, len) };
    // 拷入内核堆再打印：console 的 SBI 通道只接受内核内存
    // （DBCN 需物理地址，用户指针不可跨内核边界）。
    let owned = bytes.to_vec();
    match core::str::from_utf8(&owned) {
        Ok(text) => crate::println!("{}", text),
        Err(_) => crate::println!("[Task    ] debug: {} 字节非 UTF-8，拒印", len),
    }
    respond_ok(frame, len);
}

/// Extend(pages)：从 brk 起映射页（不要求物理连续），返回新 brk。
/// 语义沿用旧 ABI：size 参数为页数、返回值为新堆末尾字节地址（rinlib 契约）。
fn extend_heap(frame: &mut TrapFrame, thread: &Thread, pages: usize) {
    match thread.process.space.lock().extend_heap(pages) {
        Ok(brk) => respond_ok(frame, brk),
        Err(_) => respond_error(frame, SystemCallError::OutOfMemory),
    }
}

fn respond_ok(frame: &mut TrapFrame, ret: usize) {
    frame.x[10] = SystemCallError::NoError as u64;
    frame.x[11] = ret as u64;
    frame.sepc += 4;
}

fn respond_error(frame: &mut TrapFrame, err: SystemCallError) {
    frame.x[10] = err.to_usize().unwrap_or(1) as u64;
    frame.sepc += 4;
}

/// TrapFrame sepc 偏移的存在性绑定（respond 路径依赖）。
const _: () = assert!(frame_off::SEPC == 512);
