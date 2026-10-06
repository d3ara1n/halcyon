//! MemoryPool 的页与 metadata 双维预算 ABI。

pub const MEMORY_POOL_MAX_DEPTH: u32 = 32;
/// metadata 成本以一个 RV64 自然字宽的容量槽计量；不等同于堆字节隔离保证。
pub const METADATA_SLOT_BYTES: u64 = 8;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct MemoryPoolBudget {
    pub pages: u64,
    pub metadata: u64,
}
impl MemoryPoolBudget {
    pub const fn is_empty(self) -> bool {
        self.pages == 0 && self.metadata == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct MemoryPoolDeriveRequest {
    pub budget: MemoryPoolBudget,
    pub rights: crate::object::Rights,
    pub reserved: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct MetadataSnapshot {
    pub total: u64,
    pub available: u64,
    pub reserved: u64,
    pub allocated: u64,
    pub delegated: u64,
}
impl MetadataSnapshot {
    pub fn closes(self) -> bool {
        self.available
            .checked_add(self.reserved)
            .and_then(|v| v.checked_add(self.allocated))
            .and_then(|v| v.checked_add(self.delegated))
            == Some(self.total)
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct MemoryPoolSnapshot {
    pub identity: u64,
    /// root 为 0。
    pub parent_identity: u64,
    pub total: u64,
    pub available: u64,
    pub reserved: u64,
    pub allocated: u64,
    pub delegated: u64,
    pub depth: u32,
    pub reserved0: u32,
    pub metadata: MetadataSnapshot,
}
impl MemoryPoolSnapshot {
    pub fn closes(&self) -> bool {
        self.available
            .checked_add(self.reserved)
            .and_then(|v| v.checked_add(self.allocated))
            .and_then(|v| v.checked_add(self.delegated))
            == Some(self.total)
            && self.metadata.closes()
    }
}

const _: () = {
    assert!(core::mem::size_of::<MemoryPoolBudget>() == 16);
    assert!(core::mem::size_of::<MemoryPoolDeriveRequest>() == 32);
    assert!(core::mem::size_of::<MemoryPoolSnapshot>() == 104);
    assert!(core::mem::align_of::<MemoryPoolSnapshot>() == 8);
};

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn budget_and_snapshot_layouts_are_fixed() {
        assert_eq!(core::mem::offset_of!(MemoryPoolDeriveRequest, rights), 16);
        assert_eq!(core::mem::offset_of!(MemoryPoolDeriveRequest, reserved), 24);
        assert_eq!(core::mem::offset_of!(MemoryPoolSnapshot, depth), 56);
        assert_eq!(core::mem::offset_of!(MemoryPoolSnapshot, metadata), 64);
    }
    #[test]
    fn both_dimensions_must_close_without_overflow() {
        let mut snapshot = MemoryPoolSnapshot {
            total: 10,
            available: 2,
            reserved: 3,
            allocated: 4,
            delegated: 1,
            metadata: MetadataSnapshot {
                total: 4,
                available: 2,
                delegated: 2,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(snapshot.closes());
        snapshot.metadata.allocated = 1;
        assert!(!snapshot.closes());
        snapshot.available = u64::MAX;
        assert!(!snapshot.closes());
    }
}
