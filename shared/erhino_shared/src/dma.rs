//! DMA 模式、方向和定宽调用参数；地址不是可撤销的用户引用。
use crate::object::{Handle, Rights};
pub const TRUSTED_PHYSICAL: u32 = 1;
pub const COHERENT: u32 = 1;
pub const DEVICE_READS: u32 = 1;
pub const DEVICE_WRITES: u32 = 2;
pub const BIDIRECTIONAL: u32 = 3;
pub const SEALED: u64 = 1;
/// 域已封口且全部活子项/映射责任完成；不能由 members=0 单独推导。
pub const DONE: u64 = 2;
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct DomainSnapshot {
    pub mode: u32,
    pub coherency: u32,
    pub flags: u64,
    /// 本节点的直接 mapping 责任数，不包含后代；完整收束以 DONE 为准。
    pub members: u64,
    pub reserved: u64,
}
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MappingPhase {
    Preparing = 0,
    ReadyUnexposed = 1,
    MayBeExposed = 2,
    Cancelling = 3,
    Cancelled = 4,
    Retiring = 5,
    Retired = 6,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct MappingSnapshot {
    pub bytes: u64,
    pub phase: u32,
    pub direction: u32,
    pub readers: u64,
    pub reserved: u64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PinRequest {
    pub memory: Handle,
    pub offset: u64,
    pub bytes: u64,
    pub direction: u32,
    pub reserved: u32,
    pub rights: Rights,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Segment {
    pub address: u64,
    pub bytes: u64,
}
/// 一次结果最多一页；限制单次输出与准备存储成本，不限制 Pin 大小或总 SG 段数。
pub const SEGMENTS_PER_READ: usize =
    crate::proc::PROCESS_PAGE_SIZE / core::mem::size_of::<Segment>();
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ReadRequest {
    pub offset: u64,
    pub segments: u64,
    pub capacity: u32,
    pub reserved: u32,
    pub result: u64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct ReadResult {
    pub count: u64,
    pub next_offset: u64,
}
const _: () = {
    assert!(core::mem::size_of::<DomainSnapshot>() == 32);
    assert!(core::mem::size_of::<MappingSnapshot>() == 32);
    assert!(core::mem::size_of::<PinRequest>() == 40);
    assert!(core::mem::size_of::<ReadRequest>() == 32);
    assert!(core::mem::size_of::<ReadResult>() == 16);
};
