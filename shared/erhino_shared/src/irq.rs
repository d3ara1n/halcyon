//! 有线 IRQ 角色、派生参数与领取凭证。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IrqToken {
    pub generation: u64,
    pub sequence: u64,
}

/// control 使用连续 source 范围及可选目标位图；use 必须 count=1、targets 单 bit。
/// targets 的 bit 是本次启动固定的内核 hart slot，不能当作原始硬件 hartid。
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IrqScope {
    pub first: u32,
    pub count: u32,
    pub targets: u64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct IrqSnapshot {
    pub scope: IrqScope,
    pub flags: u64,
    /// control 为零；use 为本次启动不复用的代次，和 Take 返回序号组成 token。
    pub generation: u64,
}
impl IrqSnapshot {
    pub const SEALED: u64 = 1;
    pub const REVOKING: u64 = 2;
    pub const DONE: u64 = 4;
}
const _: () = assert!(core::mem::size_of::<IrqToken>() == 16);
const _: () = assert!(core::mem::size_of::<IrqScope>() == 16);
const _: () = assert!(core::mem::size_of::<IrqSnapshot>() == 32);
