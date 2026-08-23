//! 持久执行现场的三个容器：UserContext、FatalFrame、SchedulerFrame。
//! 三者不复用（notes/impls/execution-context.md「布局的单一真值」）。
//!
//! 所有被汇编访问的字段偏移以本文件的 `offset_of!` 为唯一真值，
//! 经 `global_asm!` const operands 注入（见 assembly 侧引用）；
//! 此处的断言表同时锁定关键偏移与总大小。

use crate::fp::FpState;

/// 用户持久现场：GPR、sepc 与嵌入的 FP 状态。
///
/// 访问纪律：帧只在线程挂起于某 hart 期间被该 hart 访问（汇编存取、
/// syscall 写响应、sleep 唤醒补结果均处于该区间）；随线程跨 hart 迁移。
#[repr(C)]
pub struct UserContext {
    /// x0..x31（x0 槽恒 0，占位保持下标直查）。
    pub x: [u64; 32],
    /// 用户返回地址（sret 装载）。
    pub sepc: u64,
    /// FP 持久状态：Base64 从不触碰；D64 完整恢复/条件保存。
    pub fp: FpState,
}

impl UserContext {
    /// 创建全零现场。FP 状态不存在依赖 hart 残留的 valid 状态；
    /// sp/gp/tp 等由装载方按 ABI 约定填写。
    pub fn zeroed() -> Self {
        Self { x: [0; 32], sepc: 0, fp: FpState { f: [0; 32], fcsr: 0 } }
    }
}

// 布局真值：汇编侧经 offset_of! 注入，不维护第二份数字。
const _: () = assert!(core::mem::offset_of!(UserContext, x) == 0);
const _: () = assert!(core::mem::offset_of!(UserContext, sepc) == 256);
const _: () = assert!(core::mem::offset_of!(UserContext, fp) == 264);
const _: () = assert!(core::mem::size_of::<UserContext>() == 528);

/// S 态致命 trap 的首帧证据：完整整数现场 + 分发所需 CSR。
///
/// 保存到 per-hart 独立槽位，递归 guard 保证诊断故障不覆盖原始证据；
/// 首帧建立后才清 SDT 进入软件诊断（存在 Ssdbltrp 时）。
#[repr(C)]
pub struct FatalFrame {
    pub x: [u64; 32],
    /// trap 时的原始 scause（含 Interrupt bit）。
    pub scause: u64,
    pub stval: u64,
    pub sepc: u64,
    pub satp: u64,
    /// trap 时的 sstatus（SPP/SPIE/FS 等现场）。
    pub sstatus: u64,
    _pad: [u64; 1],
}

const _: () = assert!(core::mem::offset_of!(FatalFrame, x) == 0);
const _: () = assert!(core::mem::offset_of!(FatalFrame, scause) == 256);
const _: () = assert!(core::mem::offset_of!(FatalFrame, stval) == 264);
const _: () = assert!(core::mem::offset_of!(FatalFrame, sepc) == 272);
const _: () = assert!(core::mem::offset_of!(FatalFrame, satp) == 280);
const _: () = assert!(core::mem::offset_of!(FatalFrame, sstatus) == 288);
const _: () = assert!(core::mem::size_of::<FatalFrame>() % 16 == 0);

/// 调度循环的调用现场：ra + callee-saved s0..s11 + 尾部填充。
///
/// psABI 要求标准过程入口及执行期间 sp 保持 16-byte 对齐；
/// 整个 frame 保持 16 的倍数，后续新增保存项不得破坏该性质。
#[repr(C)]
pub struct SchedulerFrame {
    pub ra: u64,
    /// s0..s11。
    pub saved: [u64; 12],
    /// 对齐填充（预留：调度器 FP 状态随 D64-capable 调度需求演进）。
    _pad: [u64; 1],
}

const _: () = assert!(core::mem::offset_of!(SchedulerFrame, ra) == 0);
const _: () = assert!(core::mem::offset_of!(SchedulerFrame, saved) == 8);
const _: () = assert!(core::mem::size_of::<SchedulerFrame>() == 112);
