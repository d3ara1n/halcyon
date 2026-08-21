//! 用户浮点状态与 F/D 汇编 helper（notes/execution-context.md「用户上下文与 FP」）。
//!
//! `FpState` 是用户可见 FP 持久状态的唯一容器，创建时完整清零——不存在
//! 依赖 hart 残留的 valid 状态。save/restore 是只有 eligible hart 才能
//! 进入的局部 F/D 汇编代码，独立于 `.text.ctx_fp` 输出节供链接后审计：
//! 该节之外不得出现任何 FP 指令或 f* CSR 访问。代码 section 与
//! FpState 的数据所有权无关。

/// 用户 FP 持久状态：f0..f31 与 fcsr（含 frm/fflags）。
#[repr(C)]
pub struct FpState {
    pub f: [u64; 32],
    pub fcsr: u64,
}

const _: () = assert!(core::mem::offset_of!(FpState, fcsr) == 256);
const _: () = assert!(core::mem::size_of::<FpState>() == 264);

/// 完整保存 FPR/fcsr 到内存状态。
///
/// # Safety
/// 仅当当前 hart 有效 FLEN 恰为 64 且任务属于 D64 档位时才可调用；
/// 调用前硬件 FS 必须已确认 Dirty（否则保存的是陈旧值）。
#[inline]
pub unsafe fn save_fp(state: *mut FpState) {
    // SAFETY: 契约由调用方（trap 出入口的 D64 分支）保证。
    unsafe { asm_save_fp(state) }
}

/// 从内存状态完整恢复 FPR/fcsr。
///
/// # Safety
/// 仅当当前 hart 有效 FLEN 恰为 64 且任务属于 D64 档位时才可调用。
#[inline]
pub unsafe fn restore_fp(state: *const FpState) {
    // SAFETY: 同上。
    unsafe { asm_restore_fp(state) }
}

unsafe extern "C" {
    #[link_name = "_save_fp"]
    fn asm_save_fp(state: *mut FpState);
    #[link_name = "_restore_fp"]
    fn asm_restore_fp(state: *const FpState);
}

core::arch::global_asm!(
    "
    .section .text.ctx_fp, \"ax\", @progbits
    .option push
    .option arch, +f, +d

    .align 2
    .global _save_fp
// a0 = &mut FpState；FS=Dirty 已由调用方确认
_save_fp:
    fsd f0, 8*0(a0)
    fsd f1, 8*1(a0)
    fsd f2, 8*2(a0)
    fsd f3, 8*3(a0)
    fsd f4, 8*4(a0)
    fsd f5, 8*5(a0)
    fsd f6, 8*6(a0)
    fsd f7, 8*7(a0)
    fsd f8, 8*8(a0)
    fsd f9, 8*9(a0)
    fsd f10, 8*10(a0)
    fsd f11, 8*11(a0)
    fsd f12, 8*12(a0)
    fsd f13, 8*13(a0)
    fsd f14, 8*14(a0)
    fsd f15, 8*15(a0)
    fsd f16, 8*16(a0)
    fsd f17, 8*17(a0)
    fsd f18, 8*18(a0)
    fsd f19, 8*19(a0)
    fsd f20, 8*20(a0)
    fsd f21, 8*21(a0)
    fsd f22, 8*22(a0)
    fsd f23, 8*23(a0)
    fsd f24, 8*24(a0)
    fsd f25, 8*25(a0)
    fsd f26, 8*26(a0)
    fsd f27, 8*27(a0)
    fsd f28, 8*28(a0)
    fsd f29, 8*29(a0)
    fsd f30, 8*30(a0)
    fsd f31, 8*31(a0)
    csrr t0, fcsr
    sd t0, 256(a0)
    ret

    .align 2
    .global _restore_fp
// a0 = &FpState；调用方随后请求 FS=Clean
_restore_fp:
    fld f0, 8*0(a0)
    fld f1, 8*1(a0)
    fld f2, 8*2(a0)
    fld f3, 8*3(a0)
    fld f4, 8*4(a0)
    fld f5, 8*5(a0)
    fld f6, 8*6(a0)
    fld f7, 8*7(a0)
    fld f8, 8*8(a0)
    fld f9, 8*9(a0)
    fld f10, 8*10(a0)
    fld f11, 8*11(a0)
    fld f12, 8*12(a0)
    fld f13, 8*13(a0)
    fld f14, 8*14(a0)
    fld f15, 8*15(a0)
    fld f16, 8*16(a0)
    fld f17, 8*17(a0)
    fld f18, 8*18(a0)
    fld f19, 8*19(a0)
    fld f20, 8*20(a0)
    fld f21, 8*21(a0)
    fld f22, 8*22(a0)
    fld f23, 8*23(a0)
    fld f24, 8*24(a0)
    fld f25, 8*25(a0)
    fld f26, 8*26(a0)
    fld f27, 8*27(a0)
    fld f28, 8*28(a0)
    fld f29, 8*29(a0)
    fld f30, 8*30(a0)
    fld f31, 8*31(a0)
    ld t0, 256(a0)
    csrw fcsr, t0
    ret

    .option pop
    "
);
