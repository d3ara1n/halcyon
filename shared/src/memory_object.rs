//! MemoryObject capability ABI。
//!
//! MemoryObject 是固定长度的共享 backing identity。长度在创建时冻结，此后只有
//! 单向的可执行发布（`SealExecutable`）改变它可授予的 view 权限；view 的建立、
//! 降权与解除属于内存映射 interface，不在本模块。

/// 单个对象的硬容量上限（页）。可由普通 Handle close 触发最终析构的对象必须受
/// 硬上限约束，因此更大的逻辑对象由用户态协议组合多个对象表达。
pub const MEMORY_OBJECT_MAX_PAGES: u64 = 512;

/// `MemoryObjectCreate` 的固定宽请求。
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryObjectCreateRequest {
    /// 对象长度（字节）；由内核向页边界取整后冻结。
    pub bytes: u64,
    /// 接收新对象 Handle 的用户地址。
    pub result_address: u64,
    pub reserved: [u64; 2],
}

impl MemoryObjectCreateRequest {
    pub const fn new(bytes: u64, result_address: u64) -> Self {
        Self {
            bytes,
            result_address,
            reserved: [0; 2],
        }
    }
}

/// 对象的可执行发布状态。单向推进，`Executable` 是终态。
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryObjectState {
    /// 允许只读 view 与持 WritePermit 的可写 view；拒绝可执行 view。
    Mutable = 0,
    /// 已请求发布，等待既有可写 view 全部退役；拒绝新的写入口。
    Sealing = 1,
    /// 终态：只允许只读或读执行 view，永久拒绝写入口。
    Executable = 2,
}

impl MemoryObjectState {
    pub const fn from_raw(raw: u32) -> Option<Self> {
        match raw {
            0 => Some(Self::Mutable),
            1 => Some(Self::Sealing),
            2 => Some(Self::Executable),
            _ => None,
        }
    }
}

/// 固定宽对象快照。identity 只作诊断，不能用于寻址或授权。
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryObjectSnapshot {
    pub identity: u64,
    /// 创建时冻结的对象长度（字节）。
    pub bytes: u64,
    /// 在途可写 view 数量：覆盖 reserved、published 与 retiring 三个阶段。
    pub write_views: u64,
    /// [`MemoryObjectState`] 的原始值。
    pub state: u32,
    pub reserved0: u32,
    pub reserved: [u64; 4],
}

impl MemoryObjectSnapshot {
    pub const fn state(&self) -> Option<MemoryObjectState> {
        MemoryObjectState::from_raw(self.state)
    }

    /// Executable 终态不允许任何在途可写 view。
    pub const fn closes(&self) -> bool {
        !matches!(self.state(), Some(MemoryObjectState::Executable)) || self.write_views == 0
    }
}

const _: () = {
    assert!(core::mem::size_of::<MemoryObjectCreateRequest>() == 32);
    assert!(core::mem::align_of::<MemoryObjectCreateRequest>() == 8);
    assert!(core::mem::size_of::<MemoryObjectSnapshot>() == 64);
    assert!(core::mem::align_of::<MemoryObjectSnapshot>() == 8);
};

#[cfg(test)]
mod tests {
    use super::{MemoryObjectSnapshot, MemoryObjectState};

    #[test]
    fn snapshot_layout_is_fixed() {
        assert_eq!(core::mem::offset_of!(MemoryObjectSnapshot, identity), 0);
        assert_eq!(core::mem::offset_of!(MemoryObjectSnapshot, bytes), 8);
        assert_eq!(core::mem::offset_of!(MemoryObjectSnapshot, write_views), 16);
        assert_eq!(core::mem::offset_of!(MemoryObjectSnapshot, state), 24);
        assert_eq!(core::mem::offset_of!(MemoryObjectSnapshot, reserved0), 28);
        assert_eq!(core::mem::offset_of!(MemoryObjectSnapshot, reserved), 32);
    }

    #[test]
    fn state_rejects_unknown_values() {
        assert_eq!(MemoryObjectState::from_raw(0), Some(MemoryObjectState::Mutable));
        assert_eq!(
            MemoryObjectState::from_raw(2),
            Some(MemoryObjectState::Executable)
        );
        assert_eq!(MemoryObjectState::from_raw(3), None);
    }

    #[test]
    fn executable_state_cannot_retain_write_views() {
        let published = MemoryObjectSnapshot {
            identity: 7,
            bytes: 4096,
            write_views: 0,
            state: MemoryObjectState::Executable as u32,
            reserved0: 0,
            reserved: [0; 4],
        };
        assert!(published.closes());
        assert!(
            !MemoryObjectSnapshot {
                write_views: 1,
                ..published
            }
            .closes()
        );
        // Mutable 允许在途可写 view。
        assert!(
            MemoryObjectSnapshot {
                state: MemoryObjectState::Mutable as u32,
                write_views: 3,
                ..published
            }
            .closes()
        );
    }
}
