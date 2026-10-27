//! MMIO 区间与关闭快照；物理地址是事实，不是可铸造的 authority。
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MmioSnapshot {
    pub physical: u64,
    pub bytes: u64,
    pub members: u64,
    pub flags: u32,
    pub writable: u32,
    pub reserved: [u64; 2],
}
impl MmioSnapshot {
    pub const SEALED: u32 = 1;
    pub const REVOKING: u32 = 2;
    pub const DONE: u32 = 4;
}
const _: () = {
    assert!(core::mem::size_of::<MmioSnapshot>() == 48);
    assert!(core::mem::align_of::<MmioSnapshot>() == 8);
};
