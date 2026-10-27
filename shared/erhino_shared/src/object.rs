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
    // bit 7 保留（原 MAP，已删除：映射授权按 READ/WRITE/EXECUTE 矩阵检查）。
    /// 允许 ProcessStart 等直接跨 HandleTable grant，不进入对象容器。
    pub const GRANT: Self = Self(1 << 8);
    /// 允许在创建域内构造新对象：Job 中创建 Process/子 Job；Pool 上表示
    /// 创建由本 Pool 付款的资源（ProcessCreate、派生子 Pool、TunnelCreate）。
    pub const CREATE: Self = Self(1 << 9);
    /// 允许把对象内容映射为可执行页；与 READ/WRITE 正交。
    pub const EXECUTE: Self = Self(1 << 10);
    pub const KNOWN: Self = Self(((1 << 11) - 1) & !(1 << 7));

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
    pub const DONE: Self = Self(1 << 4);
    /// MemoryObject 已进入 Executable 终态：可建立读执行 view。置位后永不清除。
    pub const EXECUTABLE: Self = Self(1 << 5);
    /// 对端 Endpoint 已接入且尚未关闭；普通 Map 与页内协议 Start 分别提交。
    pub const PEER_ATTACHED: Self = Self(1 << 6);
    pub const PEER_CLOSED: Self = Self(1 << 62);
    pub const CLOSED: Self = Self(1 << 63);
    pub const BITS: &'static [Self] = &[
        Self::READABLE,
        Self::WRITABLE,
        Self::DATA,
        Self::DONE,
        Self::EXECUTABLE,
        Self::PEER_ATTACHED,
        Self::PEER_CLOSED,
        Self::CLOSED,
    ];

    pub const KNOWN: Self = {
        let mut index = 0;
        let mut mask = 0;
        while index < Self::BITS.len() {
            mask |= Self::BITS[index].0;
            index += 1;
        }
        Self(mask)
    };

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

/// 公开的对象类型判别，不依赖 Rust enum 内存布局。角色由对象类型表达：
/// 不同最后持有语义或消费语义的 capability 是不同类型，不另附 role。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ObjectKind {
    Job = 1,
    MemoryPool = 2,
    MemoryObject = 3,
    ProcessBuilder = 4,
    ProcessControl = 5,
    ThreadControl = 6,
    MailboxReceiver = 7,
    NotificationOwner = 8,
    TunnelEndpoint = 9,
    TunnelInvitation = 10,
    SystemReset = 11,
    MailboxSender = 12,
    LifetimeObserver = 13,
    Delivery = 14,
    WaitSet = 15,
    MailboxOnceSender = 16,
    NotificationSignaler = 17,
    ReplyObserver = 18,
    ThreadStartTicket = 19,
    NotificationObserver = 20,
    MmioControl = 21,
    MmioUse = 22,
    IrqControl = 23,
    IrqUse = 24,
    DmaDomainControl = 25,
    DmaUse = 26,
    DmaMapping = 27,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct HandleDescription {
    pub object_id: u64,
    pub related_object_id: u64,
    pub kind: u32,
    pub reserved0: u32,
    pub rights: Rights,
    pub badge: u64,
    pub reserved: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct SenderResult {
    pub sender: Handle,
    pub lifetime: Handle,
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

/// 内核生成且在本次启动中不复用的进程身份；与 [`crate::proc::Pid`] 同一类型。
pub type ProcessId = crate::proc::Pid;

const _: () = {
    assert!(core::mem::size_of::<Handle>() == 8);
    assert!(core::mem::size_of::<Rights>() == 8);
    assert!(core::mem::size_of::<ObjectSignals>() == 8);
    assert!(core::mem::size_of::<HandlePair>() == 16);
    assert!(core::mem::align_of::<HandlePair>() == 8);
};

#[cfg(test)]
mod tests {
    use super::Rights;

    #[test]
    fn rights_wire_bits_are_stable_and_known() {
        assert_eq!(Rights::CREATE.raw(), 1 << 9);
        assert_eq!(Rights::EXECUTE.raw(), 1 << 10);
        assert_eq!(Rights::KNOWN.raw(), ((1 << 11) - 1) & !(1 << 7));
        assert!(
            !Rights::from_raw(1 << 7).is_known(),
            "retired MAP bit stays reserved"
        );
        assert!(Rights::EXECUTE.is_known());
        assert!(!Rights::from_raw(1 << 11).is_known());
    }
}
