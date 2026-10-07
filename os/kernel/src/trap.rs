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
//! 进入 SSIP/STIP，异常8为U ecall，12/13/15按VM发布依赖判定；其余同步异常终止进程。

use crate::{context::UserContext, hart, sbi, sched, syscall};

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
unsafe extern "C" fn handle_user_trap(
    scause: usize,
    stval: usize,
    frame: *mut UserContext,
) -> usize {
    crate::runtime_stop::check();
    // SAFETY: 锚指向当前线程现场，本 hart 独占（模块契约）。
    let frame = unsafe { &mut *frame };
    let thread = hart::current().current_thread();
    crate::maintenance::end_user();
    // Pending 电平是 Remote Call 真值；任意用户 trap 都是有界消费安全点。
    crate::vm::remote::drain_current(crate::vm::remote::SAFE_POINT_BATCH);

    // 终止吸收：kill 先行冻结终因后，目标线程在任何 trap 入口都不再
    // 返回用户态（IPI 到达、量子耗尽、异常均在此汇合）。
    if let Some(t) = thread
        && t.process.is_frozen()
    {
        // remote 已在计时区间外完成；早退同样把 Switch 离场尾段归 U。
        crate::maintenance::begin_user();
        return Outcome::Killed as usize;
    }

    let is_interrupt = scause >> 63 == 1;
    let code = scause & !(1 << 63);
    // syscall 与用户 fault 的业务处理归 U；timer/remote 紧急服务不归 U/M。
    if !is_interrupt {
        crate::maintenance::begin_user();
    }

    let outcome = match (is_interrupt, code) {
        (false, U_ECALL) => {
            if let Some(thread) = thread {
                // SAFETY: 当前trap执行owner独占现场，成功进入另一指令清旧修复续体。
                unsafe { thread.clear_fault_repair() };
            }
            let Some(t) = thread else {
                unreachable!("no current thread on ecall")
            };
            match syscall::dispatch(frame, t) {
                syscall::Outcome::Completed => Outcome::Resume as usize,
                syscall::Outcome::Requeue => Outcome::Requeue as usize,
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
            if let Some(thread) = thread {
                // SAFETY: 当前trap持该Thread的唯一执行现场；非page-fault不进入修复。
                match unsafe { crate::vm::fault::handle(thread, frame.sepc, code, stval) } {
                    crate::vm::fault::Action::Retry => {
                        return finish(Some(thread), Outcome::Resume as usize);
                    }
                    crate::vm::fault::Action::Wait(plan) => {
                        sched::park_request_wait(plan);
                        return finish(Some(thread), Outcome::Park as usize);
                    }
                    crate::vm::fault::Action::Terminate => {}
                }
            }
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
                t.process.freeze_and_seal(
                    erhino_shared::proc::TerminationCause::Faulted,
                    erhino_shared::proc::TerminationDetail::fault(
                        t.member().tid(),
                        fault,
                        stval as u64,
                    ),
                );
            }
            Outcome::Killed as usize
        }
        (true, other) => {
            // 未接入的中断来源进入用户 trap 路径属内核 bug，致命。
            panic!("unexpected interrupt in user trap: code={other:#x} stval={stval:#x}");
        }
    };
    finish(thread, outcome)
}

/// 所有trap出口共享业务锁外的两段通知、费用关闭与量子复检。
fn finish(thread: Option<&crate::task::Thread>, outcome: usize) -> usize {
    crate::runtime_stop::check();
    // handler 的业务 guard 已全部释放；统一交付输出复检失败的终止待办。
    if let Some(t) = thread {
        t.publish_output_fault();
    }
    crate::maintenance::end_user();
    // 业务 guard 已释放，统一推进两段式通知；剩余 pending 再发布门铃。
    crate::vm::remote::drain_current(crate::vm::remote::SAFE_POINT_BATCH);
    crate::heap::sample();
    let outcome = if thread.is_some_and(|t| t.process.is_frozen()) {
        Outcome::Killed as usize
    } else if outcome == Outcome::Resume as usize {
        if sched::quantum_expired() || !crate::maintenance::resume_user() {
            Outcome::Requeue as usize
        } else {
            // 原轮费用/机会均未重发；重装也保留 dispatch 的线程绝对期限。
            crate::maintenance::begin_user();
            sched::rearm_quantum(crate::maintenance::user_deadline());
            outcome
        }
    } else {
        outcome
    };
    // Resume 延续原类别余额；Switch 的 VM/成员离场尾段由 run 关闭此区间。
    if outcome != Outcome::Resume as usize {
        crate::maintenance::begin_user();
    }
    outcome
}
