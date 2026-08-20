//! trap 路径：用户现场进出内核的唯一通路（见 notes/internals.md「trap 帧与上下文」）。
//!
//! 汇编契约（`assembly.asm` `_user_trap` / `_ret_to_user`）：
//! - 用户态 stvec → `_user_trap`，sscratch 恒指本 hart 的 HartLocal（trap 锚）；
//! - 内核态 stvec → `_kernel_trap`（协作式定性下内核态中断不可能发生，一律致命）；
//! - trap 全程不切 satp（用户表含内核高半区），handler 栈从锚的 sched_sp 向下生长；
//! - 出口经返回值编码：Resume 装帧 sret 直接回用户态；其余恢复调度循环现场返回，
//!   由调度循环处置当前线程（Requeue / Drop / Park）。
//!
//! TrapFrame 布局与汇编偏移由 `OFFSETS` 断言表双向绑定。

use crate::{hart, sbi, sched, syscall};

/// 用户现场（`x[32] + f[32] + sepc`）。
///
/// 访问纪律：帧只在所属线程的执行 hart 上被访问——汇编存取发生在
/// 线程休眠于本 hart 期间，Rust 侧（syscall 写响应）同样处于该区间。
#[repr(C)]
pub struct TrapFrame {
    pub x: [u64; 32],
    pub f: [u64; 32],
    pub sepc: u64,
}

/// 帧内偏移（字节），汇编侧以字面量 + 同名注释访问（hart::off 同理，
/// 见该表说明）。
pub mod frame_off {
    pub const SEPC: usize = 512;
}

const _: () = assert!(core::mem::size_of::<TrapFrame>() == 520);
const _: () = assert!(core::mem::offset_of!(TrapFrame, sepc) == frame_off::SEPC);
// HartLocal 槽位偏移与汇编字面量的关键绑定（全表见 hart::off）。
const _: () = assert!(hart::off::FRAME_PTR == 24 && hart::off::SCHED_SP == 16);

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
/// 调用前须已 `set_context` 装好执行点（帧 / satp / 线程），且 tp 不变量成立。
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
        _ => unreachable!("trap 汇编返回了非法出口编码"),
    }
}

/// scause 编号：用户态软件中断（IPI 门铃）。
const SSIP: usize = 1;
/// scause：时钟中断。
const STIP: usize = 5;
/// scause：用户态 ecall。
const U_ECALL: usize = 8;
/// scause：用户态异常（非法指令 / 断点 / 各类页故障）——一律程序缺陷。
fn is_user_exception(scause: usize) -> bool {
    matches!(scause, 2 | 3 | 4 | 12 | 13 | 15 | 17 | 18 | 19 | 20 | 21 | 22 | 23 | 24)
}

/// 汇编调用入口：处理一次用户 trap，返回出口编码。
///
/// # Safety
/// 仅由 `_user_trap` 汇编按锚契约调用；frame 指向当前线程帧。
#[unsafe(no_mangle)]
unsafe extern "C" fn handle_user_trap(scause: usize, stval: usize, frame: *mut TrapFrame) -> usize {
    // SAFETY: 锚指向当前线程帧，本 hart 独占（模块契约）。
    let frame = unsafe { &mut *frame };
    let thread = hart::current().current_thread();
    // scause 最高位为中断标志，掩出编号统一分发。
    let code = scause & !(1 << 63);

    match code {
        U_ECALL => {
            let Some(t) = thread else { unreachable!("ecall 无当前线程") };
            match syscall::dispatch(frame, t) {
                syscall::Outcome::Completed => Outcome::Resume as usize,
                syscall::Outcome::Wait => Outcome::Park as usize,
                syscall::Outcome::Killed(code) => {
                    sched::report_exit(t, code);
                    Outcome::Killed as usize
                }
            }
        }
        STIP => {
            // 量子耗尽或 sleep 期限到达：卸载定时器 → 唤醒到期 → 轮转。
            sched::on_timer();
            Outcome::Requeue as usize
        }
        SSIP => {
            // 门铃：清中断源；有新待办才值得切走，否则原地继续。
            sbi::clear_ssip();
            if sched::domain_has_ready() {
                Outcome::Requeue as usize
            } else {
                Outcome::Resume as usize
            }
        }
        other if is_user_exception(other) => {
            // 用户态异常一律杀进程（notes/task.md「生命周期」）。
            let pid = thread.map(|t| t.process.pid).unwrap_or(0);
            crate::log!(
                Task,
                "pid {} 异常终止: scause={} stval={:#x} sepc={:#x}",
                pid,
                code,
                stval,
                frame.sepc
            );
            if let Some(t) = thread {
                sched::report_exit(t, -(code as i64));
            }
            Outcome::Killed as usize
        }
        other => {
            // 未知 trap 属内核 bug，致命。
            panic!("unexpected user trap: scause={:#x} stval={:#x}", other, stval);
        }
    }
}
