//! fp：D64 eligibility 验证负载。内核只在 FLEN 恰为 64 的域上调度本
//! profile；FP 状态（FPR/fcsr）跨 trap、时间片轮转与 hart 迁移的
//! 存活性由本验收进程以位型校验（验收行 "fp verification passed"）。
//!
//! 三类校验：
//! 1. 硬件 FP 计算位型（fsqrt/fmadd/fdiv）——证明运行于 D64 档位；
//! 2. FPR 惟一位型跨 syscall 往返保持——证明 trap 路径完整保存恢复；
//! 3. fcsr 舍入模式（RTZ）跨 syscall 保持——fcsr 同属 UserContext。

#![no_std]

use core::arch::asm;
use core::hint::black_box;

use rinlib::{debug, sys_sleep};

/// f64 位型常量（IEEE 754 binary64，LP64D 硬件结果）。
const SQRT2: u64 = 0x3FF6_A09E_667F_3BCD;
const THIRTEEN: u64 = 0x402A_0000_0000_0000;
/// FPR probe 位型（非正规化值，避开常用数值域）。
const PROBE_F30: u64 = 0x0000_0000_DEAD_BEEF;
const PROBE_F31: u64 = 0x7FF8_0000_C0FF_EE01;

fn main() {
    debug!("FP verification test started (D64 profile)");

    for round in 1..=4u64 {
        check_compute(round);
        check_fpr_across_trap(round);
        check_fcsr_across_trap(round);
        // 跨时间片轮转（多 hart FIFO 下自然迁移执行点）后复检。
        if round < 4 {
            unsafe { sys_sleep(5).expect("fp sleep") };
        }
    }
    debug!("fp verification passed");
}

/// 硬件 FP 指令位型（black_box 防常量折叠；no_std 无 f64 高级方法，
/// 直接内联汇编——验证目标本就是这些指令的执行档位）。
fn check_compute(round: u64) {
    let sqrt2 = fsqrt(black_box(2.0f64));
    expect(sqrt2.to_bits() == SQRT2, round, "fsqrt.d bit pattern");
    let fused = fmadd(black_box(3.0f64), black_box(4.0), black_box(1.0));
    expect(fused.to_bits() == THIRTEEN, round, "fmadd.d bit pattern");
}

/// `fsqrt.d`：双精度开方。
fn fsqrt(a: f64) -> f64 {
    let out: u64;
    // SAFETY: 纯计算指令序列（f0 中转），无内存访问。
    unsafe {
        asm!(
            "fmv.d.x f0, {a}",
            "fsqrt.d f0, f0",
            "fmv.x.d {out}, f0",
            a = in(reg) a.to_bits(),
            out = lateout(reg) out,
            options(pure, nomem, nostack)
        )
    };
    f64::from_bits(out)
}

/// `fmadd.d`：融合乘加（单舍入）。
fn fmadd(a: f64, b: f64, c: f64) -> f64 {
    let out: u64;
    // SAFETY: 纯计算指令序列（f0-f2 中转），无内存访问。
    unsafe {
        asm!(
            "fmv.d.x f0, {a}",
            "fmv.d.x f1, {b}",
            "fmv.d.x f2, {c}",
            "fmadd.d f0, f0, f1, f2",
            "fmv.x.d {out}, f0",
            a = in(reg) a.to_bits(),
            b = in(reg) b.to_bits(),
            c = in(reg) c.to_bits(),
            out = lateout(reg) out,
            options(pure, nomem, nostack)
        )
    };
    f64::from_bits(out)
}

/// `fdiv.d`：双精度除法（舍入模式敏感）。
fn fdiv(a: f64, b: f64) -> f64 {
    let out: u64;
    // SAFETY: 纯计算指令序列（f0/f1 中转），无内存访问。
    unsafe {
        asm!(
            "fmv.d.x f0, {a}",
            "fmv.d.x f1, {b}",
            "fdiv.d f0, f0, f1",
            "fmv.x.d {out}, f0",
            a = in(reg) a.to_bits(),
            b = in(reg) b.to_bits(),
            out = lateout(reg) out,
            options(pure, nomem, nostack)
        )
    };
    f64::from_bits(out)
}

/// f30/f31 惟一位型跨 syscall 往返：trap 必须完整保存恢复全部 FPR，
/// 与用户 ABI 的 caller/callee-saved 约定无关。
fn check_fpr_across_trap(round: u64) {
    unsafe {
        asm!(
            "fmv.d.x f30, {a}",
            "fmv.d.x f31, {b}",
            a = in(reg) PROBE_F30,
            b = in(reg) PROBE_F31,
            options(nomem, nostack)
        );
    }
    debug!("fp fpr probe parked across trap (round {round})");
    let (back30, back31): (u64, u64);
    unsafe {
        asm!(
            "fmv.x.d {a}, f30",
            "fmv.x.d {b}, f31",
            a = lateout(reg) back30,
            b = lateout(reg) back31,
            options(nomem, nostack)
        );
    }
    expect(back30 == PROBE_F30, round, "f30 preserved across trap");
    expect(back31 == PROBE_F31, round, "f31 preserved across trap");
}

/// fcsr.frm = RTZ 跨 syscall 往返：5/7 的尾数余数 ≈ 6/7 ulp，RNE 进位、
/// RTZ 截断——两种舍入模式下位型必不同（正数 RTZ 严格更小）；fcsr
/// 属于 UserContext 的 FpState。结束时恢复 RNE（进程卫生）。
fn check_fcsr_across_trap(round: u64) {
    let rne = fdiv(black_box(5.0f64), black_box(7.0));
    unsafe {
        asm!("fsrmi 1", options(nomem, nostack)); // 1 = RTZ
    }
    let rtz = fdiv(black_box(5.0f64), black_box(7.0));
    expect(
        rtz.to_bits() < rne.to_bits(),
        round,
        "fdiv.d under RTZ truncates toward zero",
    );
    debug!("fp fcsr probe parked across trap (round {round})");
    let recheck = fdiv(black_box(5.0f64), black_box(7.0));
    let frm: u64;
    unsafe {
        asm!("frcsr {frm}", frm = lateout(reg) frm, options(nomem, nostack));
        asm!("fsrmi 0", options(nomem, nostack)); // 恢复 RNE
    }
    expect(recheck.to_bits() == rtz.to_bits(), round, "RTZ rounding preserved");
    expect((frm >> 5) & 0x7 == 1, round, "fcsr.frm still RTZ after trap");
    let back = fdiv(black_box(5.0f64), black_box(7.0));
    expect(back.to_bits() == rne.to_bits(), round, "fdiv.d back under RNE");
}

fn expect(condition: bool, round: u64, what: &str) {
    if !condition {
        // 失败走 panic：rt 的 panic handler 以非零码退出，init 监督面
        // 观察到 Exited(-1)；成功路径由 main 返回自然 Exited(0)。
        panic!("fp verification failed at round {round}: {what}");
    }
}
