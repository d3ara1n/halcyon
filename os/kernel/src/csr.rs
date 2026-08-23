//! CSR 所有权：formal hart entry、内核稳态与用户出口三个边界集中拥有
//! 全部项目依赖字段（notes/execution-context.md「Trap 与 CSR」）。
//!
//! 原则：字段级操作，不整写 WPRI；未知/WPRI 位保持原值；WARL 字段写后
//! 读回核验，违约即拒绝该 hart（启动整体失败）。未 advertised 的可选
//! CSR 不访问（Ssstateen 平台准入时其拥有字段进入本表）。

use core::arch::asm;

use crate::sbi;

// ---- sstatus 字段（supervisor.adoc「sstatus」，RV64）----
pub const SSTATUS_SIE: usize = 1 << 1;
pub const SSTATUS_SPIE: usize = 1 << 5;
pub const SSTATUS_UBE: usize = 1 << 6;
pub const SSTATUS_SPP: usize = 1 << 8;
pub const SSTATUS_FS: usize = 0b11 << 13;
pub const SSTATUS_VS: usize = 0b11 << 15;
pub const SSTATUS_SUM: usize = 1 << 18;
pub const SSTATUS_MXR: usize = 1 << 19;
/// sstatus.UXL（bits 33:32；RV64 图中 SDT 后为 7 位 WPRI(25–31)，
/// UXL 紧随其后）。
pub const SSTATUS_UXL: usize = 0b11 << 32;

/// FS=Clean（machine.adoc「Extension Context Status」编码 10）：D64 恢复前
/// 临时开启的档位。Off/Initial/Dirty 不出现在 Rust 侧——稳态 Off 由
/// OWNED_STATUS_CLEAR/PRE_SRET_CLEAR 覆盖，Dirty 判定在汇编内联完成。
pub const FS_CLEAN: usize = 0b10;

/// UXL 值：XLEN=64。
const UXL_64: usize = 0b10 << 32;

/// formal entry 清除的项目状态位：SIE/SPIE/SPP、FS/VS、SUM/MXR。
pub const OWNED_STATUS_CLEAR: usize =
    SSTATUS_SIE | SSTATUS_SPIE | SSTATUS_SPP | SSTATUS_FS | SSTATUS_VS | SSTATUS_SUM | SSTATUS_MXR;

/// 用户出口（pre-sret）清零的状态位：SIE/SPIE/SPP、SUM/MXR、VS=Off。
/// FS 不在此列——Base 恒 Off、D64 先 Clean 再恢复。
pub const PRE_SRET_CLEAR: usize = SSTATUS_SIE | SSTATUS_SPIE | SSTATUS_SPP | SSTATUS_SUM | SSTATUS_MXR | SSTATUS_VS;

// ---- sie/sip 来源位 ----
pub const SIE_SSIE: usize = 1 << 1;
pub const SIE_STIE: usize = 1 << 5;

// ---- senvcfg 项目拥有字段（supervisor.adoc「senvcfg」，RV64）----
/// FIOM(bit0)、CBIE(bits 6:5)、CBCFE(bit7)、CBZE(bit8)：本内核不向用户
/// 开放 fence-of-I/O 语义与 cache block 维护指令。LPE/SSE/PMM 属未建模
/// 扩展，不在拥有面内、不触碰。
const SENVCFG_OWNED: usize = (1 << 0) | (0b11 << 5) | (1 << 7) | (1 << 8);

#[derive(Debug)]
pub enum CsrReject {
    /// UXL 不接受 64：hart 不能承载 LP64 用户态。
    Uxl(usize),
    /// UBE 不接受小端用户态。
    Ube,
    /// senvcfg 拥有字段清零未被硬件遵守。
    Senvcfg,
}

#[inline]
pub fn read_sstatus() -> usize {
    let v: usize;
    // SAFETY: 只读 CSR。
    unsafe { asm!("csrr {}, sstatus", out(reg) v, options(nomem)) };
    v
}

#[inline]
fn clear_sstatus(bits: usize) {
    // SAFETY: 仅清列出的项目位。
    unsafe { asm!("csrc sstatus, {bits}", bits = in(reg) bits, options(nomem)) };
}

#[inline]
pub fn write_sie(bits: usize) {
    // SAFETY: sie 的唯一写边界；未接入来源恒 0。
    unsafe { asm!("csrw sie, {bits}", bits = in(reg) bits, options(nomem)) };
}

#[inline]
pub fn clear_ssip() {
    // SAFETY: SSIP 可由 S 态直接清除。
    unsafe { asm!("csrc sip, {bit}", bit = in(reg) SIE_SSIE, options(nomem)) };
}

#[inline]
fn write_scounteren_zero() {
    // SAFETY: scounteren 恒 0 是项目稳态契约。
    unsafe { asm!("csrw scounteren, zero", options(nomem)) };
}

// senvcfg 探测/收口的守卫动作（经 csr_try 进入，a0 入参、a0 返回值）：
// 读：原始值；写读回：写入拥有字段清零后的目标值并返回硬件实际值。
// 任一步异常都由 csr_try 整体放弃——部分实现的平台（如 sifive_u 模型
// 可读不可写）不会以裸 trap 形式逃逸出守卫窗口。
core::arch::global_asm!(
    "
    .section .text
    .align 2
    .global _read_senvcfg
_read_senvcfg:
    csrr a0, senvcfg
    ret
    .align 2
    .global _write_read_senvcfg
_write_read_senvcfg:
    csrw senvcfg, a0
    csrr a0, senvcfg
    ret
   "
);

// ---------------------------------------------------------------------------
// 可选 CSR 探测
// ---------------------------------------------------------------------------

/// 带恢复路径的 CSR 序列探测：执行 `action`，若期间发生异常则放弃整个
/// 序列并返回 `None`（未实现/不可用），否则返回其返回值。
///
/// 调用约定要点：action 是普通 Rust ABI 函数，以 `ret`（经 ra）返回；
/// 因此跳转前先把 ra 指向成功续段（3:），再无链接跳入 action。若把
/// 返回地址链入 ra 以外的寄存器（如 `jalr t0`），action 会直接返回到
/// 本函数的调用者：跳过 stvec/栈恢复、泄漏帧、且调用者把原始 CSR 值
/// 误读为 `Option` 判别值（曾致 formal-entry 竞态）。
///
/// 恢复路径依赖 trap 不破坏 t1 与 sp；trap 的 SPIE/SPP 污染由调用方
/// （formal entry）末尾重归零值。
///
/// # Safety
/// `action` 只能执行无副作用的单参数 CSR 序列（a0 入参、a0 返回值），
/// 不得触碰内存或调用其他函数。
pub unsafe fn csr_try(action: unsafe extern "C" fn(usize) -> usize, arg: usize) -> Option<usize> {
    let result: usize;
    // SAFETY: 恢复路径在汇编内完成 stvec/sp/ra 还原；异常时直接放弃动作。
    // trap 不换栈，栈上旧 stvec 槽在恢复点仍有效；action 不得触碰
    // [sp-32, sp) 栈区（当前调用方仅传入纯 CSR 序列）。trap 与成功两条
    // 路径在共同出口对称退栈，不得提前 ret——外层还有编译器生成的帧。
    unsafe {
        core::arch::asm!(
            "addi sp, sp, -32",
            "sd   ra, 24(sp)",
            "csrr t0, stvec",
            "sd   t0, 16(sp)",
            "la   t0, 2f",
            "csrw stvec, t0",              // 异常恢复向量（direct mode）
            "la   ra, 3f",                 // action 以 ret（经 ra）返回，先指向成功续段
            "mv   a0, {arg}",              // 入参经 a0
            "jr   t1",                     // 无链接进入 action（jalr x0）
            " .align 2",                    // 向量地址必须 4 字节对齐（direct mode）
            "2:",
            "li   {result}, -1",          // trap 恢复：弃整个动作
            "j    4f",
            "3:",
            "mv   {result}, a0",          // 成功：a0 = 动作返回值
            "4:",                          // 共同出口：两路径对称退栈，不提前 ret
            "ld   t0, 16(sp)",
            "csrw stvec, t0",
            "ld   ra, 24(sp)",
            "addi sp, sp, 32",
            in("t1") action,
            arg = in(reg) arg,
            result = lateout(reg) result,
            out("t0") _,                          // 向量装填与恢复中转
            out("a0") _,                          // 动作返回值经 a0 中转
            out("a7") _,                          // 动作内部可能用作 ecall 号
        );
    }
    (result != usize::MAX).then_some(result)
}

unsafe extern "C" {
    #[link_name = "_read_senvcfg"]
    fn __read_senvcfg(arg: usize) -> usize;
    #[link_name = "_write_read_senvcfg"]
    fn __write_read_senvcfg(arg: usize) -> usize;
}

// ---------------------------------------------------------------------------
// 边界程序
// ---------------------------------------------------------------------------

/// formal entry 的 CSR 基线（notes/execution-context.md「CSR 所有权表」
/// 「formal hart entry」列）：
///
/// - `sie` 精确为 SSIE|STIE；SSIP 清零；timer 先经 SBI TIME 卸载；
/// - SIE/SPIE/SPP 归零，FS/VS Off，SUM/MXR 关闭；
/// - UXL 写 64 并读回核验；UBE 写 0 并读回核验；
/// - scounteren 清零；senvcfg 仅在实现提供时清拥有字段并核验。
///
/// 任一核验失败返回 [`CsrReject`]——由调用方使本次启动整体失败。
pub fn formal_entry_baseline() -> Result<(), CsrReject> {
    // 中断面：精确两个来源。U 态忽略 SIE；U 态中断天然可进入，
    // 来源是否置入 S 态由 sie 来源位控制（TRAP-008 收口结论）。
    write_sie(SIE_SSIE | SIE_STIE);
    clear_ssip();
    sbi::set_timer(sbi::DISARM).expect("failed to disarm timer in formal entry");

    // 状态面：清本项目拥有的全部控制位（FS/VS=Off、SUM/MXR=0、SIE=0）。
    clear_sstatus(OWNED_STATUS_CLEAR);

    // WARL 核验：UXL 写 64 读回。
    // SAFETY: 仅置换 UXL 位段。
    unsafe {
        asm!(
            "csrr {t}, sstatus",
            "and  {t}, {t}, {keep}",
            "or   {t}, {t}, {uxl}",
            "csrw sstatus, {t}",
            t = lateout(reg) _,
            keep = in(reg) !SSTATUS_UXL,
            uxl = in(reg) UXL_64,
            options(nomem)
        );
    }
    let uxl_readback = read_sstatus() & SSTATUS_UXL;
    if uxl_readback != UXL_64 {
        return Err(CsrReject::Uxl(uxl_readback >> 32));
    }

    // WARL 核验：UBE 写 0 读回。
    clear_sstatus(SSTATUS_UBE);
    if read_sstatus() & SSTATUS_UBE != 0 {
        return Err(CsrReject::Ube);
    }

    // 计数器面：用户态不可访问任何计数器。
    write_scounteren_zero();

    // 环境面：senvcfg 自 privileged 1.12 才存在（sifive_u 等为 1.10），
    // 未实现的 CSR 不访问——读与写读回都在带恢复路径的探测守卫内，
    // 任一异常视为不可用；实现提供时仅清拥有字段并核验，未知/WPRI
    // 位原样保留。可读不可写的部分实现同样拒绝（违约即整体失败）。
    if let Some(raw) = unsafe { csr_try(__read_senvcfg, 0) } {
        let cleared = raw & !SENVCFG_OWNED;
        if unsafe { csr_try(__write_read_senvcfg, cleared) } != Some(cleared) {
            return Err(CsrReject::Senvcfg);
        }
    }

    // 探测路径的 S→S trap 会污染 SPIE/SPP；基线末尾重归零值。
    clear_sstatus(SSTATUS_SPIE | SSTATUS_SPP);
    Ok(())
}
