use crate::object::{Handle, Rights};

/// ExitCode(i64) type for process
pub type ExitCode = i64;
/// Pid(u32) type for process
pub type Pid = u32;
/// Tid(u32) type for thread
/// If uniform thread-id required, It is uni_tid = ((pid << 32) | tid)
pub type Tid = u32;

/// 当前用户地址空间 ABI；process loader 与内核共同遵守。
pub const PROCESS_PAGE_SIZE: usize = 4096;
pub const PROCESS_USER_TOP: usize = 1 << 38;
pub const PROCESS_MAIN_STACK_SIZE: usize = 8 << 20;

/// Building process 用户映射权限；U/A/D 位由内核生成。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct ProcessMapFlags(u32);

impl ProcessMapFlags {
    pub const READ: Self = Self(1 << 0);
    pub const WRITE: Self = Self(1 << 1);
    pub const EXECUTE: Self = Self(1 << 2);
    pub const KNOWN: Self = Self((1 << 3) - 1);

    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    pub const fn raw(self) -> u32 {
        self.0
    }

    pub const fn contains(self, required: Self) -> bool {
        self.0 & required.0 == required.0
    }

    pub const fn is_known(self) -> bool {
        self.0 & !Self::KNOWN.0 == 0
    }
}

impl core::ops::BitOr for ProcessMapFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// launcher 声明的执行上下文需求；错误声明最多使目标进程不可运行，
/// 不能扩大 capability。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ExecutionProfile {
    Base64 = 0,
    D64 = 1,
}

/// ProcessCreate 输出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct ProcessCreateResult {
    pub builder: Handle,
    pub pid: Pid,
    pub reserved: u32,
}

/// ProcessStart 直接 grant 项；目标 rights 只能收窄。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct HandleGrant {
    pub handle: Handle,
    pub rights: Rights,
}

/// ProcessStart 固定宽输入。所有地址都是调用进程用户 VA。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct ProcessStartDescriptor {
    pub entry: u64,
    pub stack_pointer: u64,
    pub payload_ptr: u64,
    pub grants_ptr: u64,
    pub payload_len: u32,
    pub grant_count: u32,
    pub profile: u32,
    pub reserved: u32,
    pub control_rights: Rights,
}

const _: () = {
    assert!(core::mem::size_of::<ProcessMapFlags>() == 4);
    assert!(core::mem::size_of::<ProcessCreateResult>() == 16);
    assert!(core::mem::size_of::<HandleGrant>() == 16);
    assert!(core::mem::size_of::<ProcessStartDescriptor>() == 56);
};
/// Process's main function product
pub trait Termination {
    /// Get completed process's exit code
    fn to_exit_code(self) -> ExitCode;
}

impl Termination for () {
    fn to_exit_code(self) -> ExitCode {
        0
    }
}

impl Termination for bool {
    fn to_exit_code(self) -> ExitCode {
        if self {
            0
        } else {
            -1
        }
    }
}

/// ExitCode for process result which treated as Termination
pub type ProgramResult = Result<(), ExitCode>;

impl Termination for ProgramResult {
    fn to_exit_code(self) -> ExitCode {
        if let Err(code) = self {
            code
        } else {
            0
        }
    }
}
