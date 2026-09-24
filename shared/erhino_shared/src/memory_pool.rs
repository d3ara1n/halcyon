//! MemoryPool capability ABI。

pub const MEMORY_POOL_MAX_DEPTH: u32 = 32;

/// 固定宽 Pool 账户快照。所有额度以页为单位。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

impl MemoryPoolSnapshot {
    pub fn closes(&self) -> bool {
        self.available
            .checked_add(self.reserved)
            .and_then(|value| value.checked_add(self.allocated))
            .and_then(|value| value.checked_add(self.delegated))
            == Some(self.total)
    }
}

const _: () = {
    assert!(core::mem::size_of::<MemoryPoolSnapshot>() == 64);
    assert!(core::mem::align_of::<MemoryPoolSnapshot>() == 8);
};

#[cfg(test)]
mod tests {
    use super::MemoryPoolSnapshot;

    #[test]
    fn snapshot_layout_is_fixed() {
        assert_eq!(core::mem::offset_of!(MemoryPoolSnapshot, identity), 0);
        assert_eq!(
            core::mem::offset_of!(MemoryPoolSnapshot, parent_identity),
            8
        );
        assert_eq!(core::mem::offset_of!(MemoryPoolSnapshot, total), 16);
        assert_eq!(core::mem::offset_of!(MemoryPoolSnapshot, available), 24);
        assert_eq!(core::mem::offset_of!(MemoryPoolSnapshot, reserved), 32);
        assert_eq!(core::mem::offset_of!(MemoryPoolSnapshot, allocated), 40);
        assert_eq!(core::mem::offset_of!(MemoryPoolSnapshot, delegated), 48);
        assert_eq!(core::mem::offset_of!(MemoryPoolSnapshot, depth), 56);
        assert_eq!(core::mem::offset_of!(MemoryPoolSnapshot, reserved0), 60);
    }

    #[test]
    fn closure_check_rejects_overflow() {
        let valid = MemoryPoolSnapshot {
            identity: 1,
            parent_identity: 0,
            total: 10,
            available: 2,
            reserved: 3,
            allocated: 4,
            delegated: 1,
            depth: 1,
            reserved0: 0,
        };
        assert!(valid.closes());
        assert!(
            !MemoryPoolSnapshot {
                available: u64::MAX,
                ..valid
            }
            .closes()
        );
    }
}
