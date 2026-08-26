//! 内核对象 ABI：进程本地 Handle、rights 与对象电平状态。

use core::ops::{BitAnd, BitAndAssign, BitOr, BitOrAssign, Not};

/// 进程本地不透明对象引用。高 32 位是 generation，低 32 位是槽位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct Handle(u64);

impl Handle {
    /// 永远无效的 Handle。
    pub const INVALID: Self = Self(0);

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn from_parts(slot: u32, generation: u32) -> Self {
        Self(((generation as u64) << 32) | slot as u64)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn slot(self) -> u32 {
        self.0 as u32
    }

    pub const fn generation(self) -> u32 {
        (self.0 >> 32) as u32
    }

    pub const fn is_valid(self) -> bool {
        self.0 != 0 && self.slot() != 0 && self.generation() != 0
    }
}

/// 对象操作权利。派生和转移只能取已有 rights 的子集。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(transparent)]
pub struct Rights(u64);

impl Rights {
    pub const NONE: Self = Self(0);
    pub const READ: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    pub const WAIT: Self = Self(1 << 2);
    pub const SIGNAL: Self = Self(1 << 3);
    /// 允许 entry 暂存于有缓冲消息并由接收方安装。
    pub const TRANSIT: Self = Self(1 << 4);
    pub const DUPLICATE: Self = Self(1 << 5);
    pub const MANAGE: Self = Self(1 << 6);
    pub const MAP: Self = Self(1 << 7);
    /// 允许 ProcessStart 等直接跨 HandleTable grant，不进入对象容器。
    pub const GRANT: Self = Self(1 << 8);
    pub const KNOWN: Self = Self((1 << 9) - 1);

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    pub const fn is_subset_of(self, other: Self) -> bool {
        other.contains(self)
    }

    pub const fn is_known(self) -> bool {
        self.0 & !Self::KNOWN.0 == 0
    }
}

impl BitOr for Rights {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Rights {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for Rights {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for Rights {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl Not for Rights {
    type Output = Self;

    fn not(self) -> Self {
        Self(!self.0)
    }
}

/// 可等待对象公开的非消费式电平状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(transparent)]
pub struct ObjectSignals(u64);

impl ObjectSignals {
    pub const NONE: Self = Self(0);
    pub const READABLE: Self = Self(1 << 0);
    pub const WRITABLE: Self = Self(1 << 1);
    pub const DATA: Self = Self(1 << 2);
    pub const PEER_CLOSED: Self = Self(1 << 62);
    pub const CLOSED: Self = Self(1 << 63);
    pub const KNOWN: Self = Self(
        Self::READABLE.0 | Self::WRITABLE.0 | Self::DATA.0 | Self::PEER_CLOSED.0 | Self::CLOSED.0,
    );

    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u64 {
        self.0
    }

    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    pub const fn intersects(self, interest: Self) -> bool {
        self.0 & interest.0 != 0
    }

    pub const fn is_known(self) -> bool {
        self.0 & !Self::KNOWN.0 == 0
    }
}

impl BitOr for ObjectSignals {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for ObjectSignals {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for ObjectSignals {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

impl BitAndAssign for ObjectSignals {
    fn bitand_assign(&mut self, rhs: Self) {
        self.0 &= rhs.0;
    }
}

impl Not for ObjectSignals {
    type Output = Self;

    fn not(self) -> Self {
        Self(!self.0)
    }
}

/// 原子创建双角色对象时的 Handle 输出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct HandlePair {
    pub owner: Handle,
    pub peer: Handle,
}

impl HandlePair {
    pub const fn new(owner: Handle, peer: Handle) -> Self {
        Self { owner, peer }
    }
}

/// 内核生成且在本次启动中不复用的进程身份。
pub type ProcessId = u64;

const _: () = {
    assert!(core::mem::size_of::<Handle>() == 8);
    assert!(core::mem::size_of::<Rights>() == 8);
    assert!(core::mem::size_of::<ObjectSignals>() == 8);
    assert!(core::mem::size_of::<HandlePair>() == 16);
    assert!(core::mem::align_of::<HandlePair>() == 8);
};
