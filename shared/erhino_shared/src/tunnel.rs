//! Tunnel 固定宽请求与结果。Tunnel 只交付 authority，不建立映射：
//! 共享 backing 经 `MemoryMap` 以 Endpoint 为来源单独映射，View 与 Endpoint
//! 各自独立持有与释放。

use crate::object::Handle;

pub const TUNNEL_MAX_PAGES: u64 = crate::memory_object::MEMORY_OBJECT_MAX_PAGES;

/// `TunnelCreate` 请求：付款 Pool（需 CREATE）与共享 backing 长度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct TunnelCreateRequest {
    pub pool: Handle,
    pub bytes: u64,
    pub result_address: u64,
    pub reserved: u64,
}

impl TunnelCreateRequest {
    pub const fn new(pool: Handle, bytes: u64, result_address: u64) -> Self {
        Self {
            pool,
            bytes,
            result_address,
            reserved: 0,
        }
    }
}

/// `TunnelCreate` 结果：本端 Endpoint 与一次性 Invitation。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct TunnelCreateResult {
    pub endpoint: Handle,
    pub invitation: Handle,
    /// 共享 backing 规范化长度（页对齐字节）。
    pub bytes: u64,
    pub reserved: u64,
}

impl TunnelCreateResult {
    pub const fn empty() -> Self {
        Self {
            endpoint: Handle::INVALID,
            invitation: Handle::INVALID,
            bytes: 0,
            reserved: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tunnel_wire_layout_is_fixed_width() {
        assert_eq!(core::mem::size_of::<TunnelCreateRequest>(), 32);
        assert_eq!(core::mem::size_of::<TunnelCreateResult>(), 32);
        assert_eq!(core::mem::offset_of!(TunnelCreateResult, invitation), 8);
    }
}
