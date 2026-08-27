//! trap 路径：用户现场进出内核的唯一通路。
//!
//! 汇编契约（`assembly.asm` `_trap_entry` / `_resume_user`）：
//! - stvec 恒指共同 direct-mode 入口，sscratch 恒指 HartLocal；
//! - 硬件 SPP 是来源唯一真值：SPP=0 保存 UserContext 进入本模块；
//!   SPP=1 保存 FatalFrame 进入 emergency fatal（见 rt::handle_fatal）；
//! - trap 全程不切 satp（用户表含内核高半区），handler 栈为调度循环栈；
//! - 出口经返回值编码：Resume 装帧 sret 直接回用户态；其余恢复
//!   SchedulerFrame 返回调度循环，由其处置当前线程。
//!
//! scause 按 `(is_interrupt, code)` 分发（TRAP-004 收口）：只有中断 1/5
//! 进入 SSIP/STIP，只有异常 8 是 U ecall；其余用户同步异常一律终止进程。

use crate::{context::UserContext, hart, sched, sbi, syscall};

/// trap 处理出口，汇编经 a0 返回给调度循环（0 保留给 Resume）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum Outcome {
    /// 装帧 sret，直接回用户态（不经过调度循环）。
    Resume = 0,
    /// Switch 回调度循环；当前线程重新入队（量子耗尽 / 让出 / IPI 查待办）。
    Requeue = 1,
    /// Switch 回调度循环；当前线程进程终止，调度循环执行回收。
    Killed = 2,
    /// Switch 回调度循环；当前线程已登记内核请求，转 Waiting，不做处置。
    Park = 3,
}

/// 进入用户态（调度循环调用）。Switch 发生时返回，返回值即 [`Outcome`]
/// 的非 Resume 分支；Resume 分支不返回（汇编直接 sret）。
///
/// # Safety
/// 调用前须已 `set_context` 装好执行点（帧 / satp / 线程 / FP 档位），
/// 且 tp 不变量成立。
#[inline]
pub unsafe fn ret_to_user() -> Outcome {
    unsafe extern "C" {
        #[link_name = "_ret_to_user"]
        fn shim() -> usize;
    }
    // SAFETY: 汇编按锚装帧 sret；Switch 路径按调用约定正常返回。
    let raw = unsafe { shim() };
    match raw {
        1 => Outcome::Requeue,
        2 => Outcome::Killed,
        3 => Outcome::Park,
        _ => unreachable!("trap assembly returned an invalid outcome encoding"),
    }
}

// scause (is_interrupt, code)：中断来源（supervisor.adoc「Supervisor Cause」）。
const SSIP: usize = 1;
const STIP: usize = 5;
/// U ecall 是 exception 8。
const U_ECALL: usize = 8;

/// 汇编调用入口：处理一次用户 trap，返回出口编码。
///
/// # Safety
/// 仅由 `_trap_entry` 汇编按锚契约调用；frame 指向当前线程的 UserContext。
#[unsafe(no_mangle)]
unsafe extern "C" fn handle_user_trap(scause: usize, stval: usize, frame: *mut UserContext) -> usize {
    // SAFETY: 锚指向当前线程现场，本 hart 独占（模块契约）。
    let frame = unsafe { &mut *frame };
    let thread = hart::current().current_thread();

    // 终止吸收：kill 先行冻结终因后，目标线程在任何 trap 入口都不再
    // 返回用户态（IPI 到达、量子耗尽、异常均在此汇合）。
    if let Some(t) = thread {
        if t.process.lifecycle.is_terminating() {
            return Outcome::Killed as usize;
        }
    }

    let is_interrupt = scause >> 63 == 1;
    let code = scause & !(1 << 63);

    match (is_interrupt, code) {
        (false, U_ECALL) => {
            let Some(t) = thread else { unreachable!("no current thread on ecall") };
            match syscall::dispatch(frame, t) {
                syscall::Outcome::Completed => Outcome::Resume as usize,
                syscall::Outcome::Wait => Outcome::Park as usize,
                syscall::Outcome::Killed => Outcome::Killed as usize,
            }
        }
        (true, STIP) => {
            // 量子耗尽或 sleep 期限到达：卸载定时器 → 唤醒到期 → 轮转。
            sched::on_timer();
            Outcome::Requeue as usize
        }
        (true, SSIP) => {
            // 门铃：清中断源；有新待办才值得切走，否则原地继续。
            sbi::clear_ssip();
            if sched::domain_has_ready() {
                Outcome::Requeue as usize
            } else {
                Outcome::Resume as usize
            }
        }
        (false, _) => {
            // 用户态同步异常一律终止进程（notes/impls/task.md「生命周期」）；
            // 不以裸编号匹配中断语义（TRAP-004）。终因经稳定编码冻结。
            let pid = thread.map(|t| t.process.pid).unwrap_or(0);
            warn!(
                Task,
                "pid {} aborted: scause={:#x} stval={:#x} sepc={:#x}",
                pid,
                scause,
                stval,
                frame.sepc
            );
            if let Some(t) = thread {
                let fault = erhino_shared::proc::ProcessFaultCode::from_scause(code);
                let todo = t.process.lifecycle.request_termination(
                    erhino_shared::proc::ProcessExitReason::Fault,
                    fault as i64,
                    Some(t.tid),
                );
                let process = t.process.clone();
                crate::task::process::run_termination_todo(&process, todo);
            }
            Outcome::Killed as usize
        }
        (true, other) => {
            // 未接入的中断来源进入用户 trap 路径属内核 bug，致命。
            panic!("unexpected interrupt in user trap: code={other:#x} stval={stval:#x}");
        }
    }
}
