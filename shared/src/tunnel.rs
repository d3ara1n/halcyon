//! Tunnel 固定宽请求与规范化映射结果。

use crate::{mem::MemoryPlacement, object::Handle};

pub const TUNNEL_MAX_PAGES: u64 = crate::memory_object::MEMORY_OBJECT_MAX_PAGES;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct TunnelCreateRequest {
    pub bytes: u64,
    pub address: u64,
    pub result_address: u64,
    pub placement: u32,
    pub reserved: u32,
}

impl TunnelCreateRequest {
    pub const fn new(
        bytes: u64,
        address: u64,
        result_address: u64,
        placement: MemoryPlacement,
    ) -> Self {
        Self {
            bytes,
            address,
            result_address,
            placement: placement as u32,
            reserved: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct TunnelAttachRequest {
    pub invitation: Handle,
    pub address: u64,
    pub result_address: u64,
    pub placement: u32,
    pub reserved: u32,
}

impl TunnelAttachRequest {
    pub const fn new(
        invitation: Handle,
        address: u64,
        result_address: u64,
        placement: MemoryPlacement,
    ) -> Self {
        Self {
            invitation,
            address,
            result_address,
            placement: placement as u32,
            reserved: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct TunnelEndpointResult {
    pub endpoint: Handle,
    pub base: u64,
    pub bytes: u64,
}

impl TunnelEndpointResult {
    pub const fn empty() -> Self {
        Self {
            endpoint: Handle::INVALID,
            base: 0,
            bytes: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub struct TunnelCreateResult {
    pub local: TunnelEndpointResult,
    pub invitation: Handle,
}

impl TunnelCreateResult {
    pub const fn empty() -> Self {
        Self {
            local: TunnelEndpointResult::empty(),
            invitation: Handle::INVALID,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tunnel_wire_layout_is_fixed_width() {
        assert_eq!(core::mem::size_of::<TunnelCreateRequest>(), 32);
        assert_eq!(core::mem::size_of::<TunnelAttachRequest>(), 32);
        assert_eq!(core::mem::size_of::<TunnelEndpointResult>(), 24);
        assert_eq!(core::mem::size_of::<TunnelCreateResult>(), 32);
        assert_eq!(core::mem::align_of::<TunnelCreateResult>(), 8);
        assert_eq!(core::mem::offset_of!(TunnelCreateRequest, placement), 24);
        assert_eq!(
            core::mem::offset_of!(TunnelAttachRequest, result_address),
            16
        );
        assert_eq!(core::mem::offset_of!(TunnelCreateResult, invitation), 24);
    }
}
