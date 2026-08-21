//! 汇编与 Rust 的布局契约：`offset_of!` 是唯一真值，经 `global_asm!`
//! const operands 注入 `assembly.asm`，汇编侧不维护第二份数字。

use core::mem::offset_of;

use crate::context::{FatalFrame, SchedulerFrame, UserContext};
use crate::hart::off;
use crate::registry::HartBootRecord;

/// assembly.asm 的全部注入常量（global_asm! 展开使用）。
pub mod asm {
    use super::*;

    // ---- HartLocal 槽 ----
    pub const HL_HARTID: usize = off::HARTID;
    pub const HL_KERNEL_SP: usize = off::KERNEL_SP;
    pub const HL_SCHED_SP: usize = off::SCHED_SP;
    pub const HL_FRAME_PTR: usize = off::FRAME_PTR;
    pub const HL_USER_SATP: usize = off::USER_SATP;
    pub const HL_SCRATCH: usize = off::TRAP_SCRATCH;
    pub const HL_SLOT: usize = off::SLOT;
    pub const HL_EMERGENCY_SP: usize = off::EMERGENCY_SP;
    pub const HL_FATAL_GUARD: usize = off::FATAL_GUARD;
    pub const HL_SCRATCH2: usize = off::TRAP_SCRATCH2;
    pub const HL_FP_ENABLED: usize = off::FP_ENABLED;
    pub const HL_FATAL_SP: usize = off::FATAL_SP;
    pub const HL_RESERVATION: usize = off::RESERVATION;

    // ---- UserContext ----
    pub const UC_X0: usize = offset_of!(UserContext, x);
    pub const UC_X30: usize = offset_of!(UserContext, x) + 30 * 8;
    pub const UC_X31: usize = offset_of!(UserContext, x) + 31 * 8;
    pub const UC_SEPC: usize = offset_of!(UserContext, sepc);
    pub const UC_FP: usize = offset_of!(UserContext, fp);

    // ---- FatalFrame ----
    pub const FF_X0: usize = offset_of!(FatalFrame, x);
    pub const FF_X2: usize = offset_of!(FatalFrame, x) + 2 * 8;
    pub const FF_X10: usize = offset_of!(FatalFrame, x) + 10 * 8;
    pub const FF_X11: usize = offset_of!(FatalFrame, x) + 11 * 8;
    pub const FF_X30: usize = offset_of!(FatalFrame, x) + 30 * 8;
    pub const FF_X31: usize = offset_of!(FatalFrame, x) + 31 * 8;
    pub const FF_SCAUSE: usize = offset_of!(FatalFrame, scause);
    pub const FF_STVAL: usize = offset_of!(FatalFrame, stval);
    pub const FF_SEPC: usize = offset_of!(FatalFrame, sepc);
    pub const FF_SATP: usize = offset_of!(FatalFrame, satp);
    pub const FF_SSTATUS: usize = offset_of!(FatalFrame, sstatus);
    pub const FF_SIZE: usize = core::mem::size_of::<FatalFrame>();

    // ---- SchedulerFrame（112 bytes，psABI 16-byte 对齐）----
    // ---- pre-sret / 稳态 CSR 位值（csr.rs 为真值源）----
    pub const CSR_FS_CLEAN: usize = crate::csr::FS_CLEAN << 13;
    pub const CSR_FS_MASK: usize = crate::csr::SSTATUS_FS;
    pub const CSR_PRE_SRET_CLEAR: usize = crate::csr::PRE_SRET_CLEAR;

    pub const SF_RA: usize = offset_of!(SchedulerFrame, ra);
    pub const SF_S0: usize = offset_of!(SchedulerFrame, saved);
    pub const SF_SIZE: usize = core::mem::size_of::<SchedulerFrame>();

    // ---- HartBootRecord（PA 前导与 formal entry 按固定偏移消费）----
    pub const REC_STATE: usize = offset_of!(HartBootRecord, state);
    pub const REC_KERNEL_SATP: usize = offset_of!(HartBootRecord, kernel_satp);
    pub const REC_ENTRY_HIGH: usize = offset_of!(HartBootRecord, entry_high);
    pub const REC_HART_LOCAL: usize = offset_of!(HartBootRecord, hart_local);
    pub const REC_STACK_TOP: usize = offset_of!(HartBootRecord, stack_top);
    pub const REC_EMERGENCY_SP: usize = offset_of!(HartBootRecord, emergency_sp);
}
