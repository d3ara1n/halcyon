use core::sync::atomic::{AtomicU64, Ordering};

/// 进程虚拟地址。
pub type Address = usize;

/// Running 地址空间映射允许的页权限。
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryProtection {
    ReadOnly = 1,
    ReadWrite = 2,
    ReadExecute = 3,
}

impl MemoryProtection {
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            1 => Some(Self::ReadOnly),
            2 => Some(Self::ReadWrite),
            3 => Some(Self::ReadExecute),
            _ => None,
        }
    }
}

/// 匿名映射的选址政策。
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryPlacement {
    Anywhere = 0,
    FixedEmpty = 1,
}

impl MemoryPlacement {
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Anywhere),
            1 => Some(Self::FixedEmpty),
            _ => None,
        }
    }
}

/// `MemoryMap` 的固定宽请求。所有长度按字节表达，由内核向页边界取整。
///
/// backing 来源由 `source` 声明：`INVALID` 为匿名页，否则是具 `MAP` 的
/// MemoryObject Handle。对象来源不重新分配数据页，只验证 rights、对象内页
/// 对齐 offset 与范围。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryMapRequest {
    pub bytes: u64,
    pub guard_before: u64,
    pub guard_after: u64,
    /// `FixedEmpty` 时为 usable mapping 起点；`Anywhere` 时必须为零。
    pub address: u64,
    pub result_address: u64,
    /// 调用者生成的非零提交标识。
    pub cookie: u64,
    /// backing 来源：零为匿名页，否则为 MemoryObject Handle 的原值。
    pub source: u64,
    /// 对象来源的对象内起始偏移（字节，页对齐）；匿名来源必须为零。
    pub source_offset: u64,
    pub protection: u32,
    pub placement: u32,
    pub reserved: [u64; 1],
}

impl MemoryMapRequest {
    /// 匿名映射请求。
    #[expect(
        clippy::too_many_arguments,
        reason = "fixed-width ABI constructor lists every validated field explicitly"
    )]
    pub const fn new(
        bytes: u64,
        guard_before: u64,
        guard_after: u64,
        address: u64,
        result_address: u64,
        cookie: u64,
        protection: MemoryProtection,
        placement: MemoryPlacement,
    ) -> Self {
        Self {
            bytes,
            guard_before,
            guard_after,
            address,
            result_address,
            cookie,
            source: 0,
            source_offset: 0,
            protection: protection as u32,
            placement: placement as u32,
            reserved: [0; 1],
        }
    }

    /// MemoryObject view 请求：数据来自已存在的对象，不取得新的页额度。
    #[expect(
        clippy::too_many_arguments,
        reason = "fixed-width ABI constructor lists every validated field explicitly"
    )]
    pub const fn new_object_view(
        object: u64,
        source_offset: u64,
        bytes: u64,
        guard_before: u64,
        guard_after: u64,
        address: u64,
        result_address: u64,
        cookie: u64,
        protection: MemoryProtection,
        placement: MemoryPlacement,
    ) -> Self {
        Self {
            bytes,
            guard_before,
            guard_after,
            address,
            result_address,
            cookie,
            source: object,
            source_offset,
            protection: protection as u32,
            placement: placement as u32,
            reserved: [0; 1],
        }
    }
}

/// `MemoryMap` 的固定宽结果槽。`committed` 必须位于结构末尾。
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryMapResult {
    pub usable_base: u64,
    pub usable_bytes: u64,
    pub reservation_base: u64,
    pub reservation_bytes: u64,
    pub reserved: [u64; 3],
    pub committed: u64,
}

impl MemoryMapResult {
    pub const fn empty() -> Self {
        Self {
            usable_base: 0,
            usable_bytes: 0,
            reservation_base: 0,
            reservation_bytes: 0,
            reserved: [0; 3],
            committed: 0,
        }
    }

    /// 仅在 syscall 返回或 join 接管完成记录后读取提交标识。
    pub fn load_committed(&self) -> u64 {
        let ptr = core::ptr::addr_of!(self.committed);
        // SAFETY: repr(C) 保证末字段自然对齐；ABI 规定 committed 只以原子方式
        // 发布和观察，调用前初始化发生在任何并发观察之前。
        unsafe { AtomicU64::from_ptr(ptr.cast_mut()).load(Ordering::Acquire) }
    }
}

#[cfg(test)]
mod tests {
    use super::{MemoryMapRequest, MemoryMapResult};

    #[test]
    fn mapping_abi_is_fixed_width_and_cookie_is_last() {
        assert_eq!(core::mem::size_of::<MemoryMapRequest>(), 80);
        assert_eq!(core::mem::align_of::<MemoryMapRequest>(), 8);
        assert_eq!(core::mem::size_of::<MemoryMapResult>(), 64);
        assert_eq!(core::mem::align_of::<MemoryMapResult>(), 8);
        assert_eq!(core::mem::offset_of!(MemoryMapResult, committed), 56);
    }

    #[test]
    fn anonymous_request_declares_no_object_source() {
        let request = MemoryMapRequest::new(
            0x1000,
            0,
            0,
            0,
            0x2000,
            7,
            super::MemoryProtection::ReadWrite,
            super::MemoryPlacement::Anywhere,
        );
        assert_eq!(request.source, 0);
        assert_eq!(request.source_offset, 0);
        assert_eq!(request.reserved, [0; 1]);
    }
}
