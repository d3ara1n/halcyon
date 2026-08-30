use crate::object::{Handle, Rights};

/// ExitCode(i64) type for process
pub type ExitCode = i64;
/// Pid(u64) type for process；单调不复用，宽度与 ProcessId 一致。
pub type Pid = u64;
/// Tid(u32) type for thread
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

/// ProcessCreate 输出：Builder 与 Control 原子安装，同一事务交付。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct ProcessCreateResult {
    pub builder: Handle,
    pub control: Handle,
    pub pid: Pid,
    pub reserved: u64,
}

/// 单进程并发线程数上界（成员表长度；tid 单调不复用，生灭循环不耗尽）。
pub const PROCESS_MAX_THREADS: usize = 1024;
/// 单次 ProcessGrant 的结构上界；约束内核短路径与用户态启动块容量。
pub const PROCESS_MAX_GRANTS: usize = 64;

/// ProcessGrant 的 grant 项；目标 rights 只能收窄。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct HandleGrant {
    pub handle: Handle,
    pub rights: Rights,
}

/// 线程出生现场，由 ProcessAttach（Building 外部通道）与 ThreadSpawn
/// （Running 内部通道）共用。所有地址都是目标进程用户 VA；entry 必须位于
/// 可执行页，sp 已映射且 16 字节对齐（内核做 translate 前置校验，运行期
/// fault 走用户 fault 杀进程）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct ThreadStartContext {
    pub entry: u64,
    pub stack_pointer: u64,
    /// 出生参数 a0/a1；首线程通常承载用户态约定的出生信息。
    pub arg1: u64,
    pub arg2: u64,
}

const _: () = {
    assert!(core::mem::size_of::<ThreadStartContext>() == 32);
};

/// ThreadSpawn 同步输出；control 是等待 DONE 的 ThreadControl handle。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct ThreadSpawnResult {
    pub tid: Tid,
    pub reserved: u32,
    pub control: Handle,
}

const _: () = {
    assert!(core::mem::size_of::<ThreadSpawnResult>() == 16);
    assert!(core::mem::align_of::<ThreadSpawnResult>() == 8);
};

/// ProcessControl 固定宽状态快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct ProcessSnapshot {
    pub pid: Pid,
    pub parent_pid: Pid,
    pub state: u32,
    pub reason: u32,
    pub code: i64,
    pub reserved: u64,
}

/// 生命周期状态判别值（`ProcessSnapshot::state`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ProcessState {
    Building = 0,
    Running = 1,
    Terminating = 2,
    Dead = 3,
}

/// 终因判别值（`ProcessSnapshot::reason`）。
/// Building/Running 要求 reason=None 且 code=0；Abandoned 的 code 固定为 0；
/// Terminating/Dead 的终因在首次终止线性化点冻结。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ProcessExitReason {
    None = 0,
    Exited = 1,
    Fault = 2,
    Killed = 3,
    Abandoned = 4,
}

/// Fault 终因的稳定编码；不把裸 `scause` 固化为生命周期 ABI。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum ProcessFaultCode {
    Unknown = 0,
    InstructionAccess = 1,
    IllegalInstruction = 2,
    Breakpoint = 3,
    LoadAccess = 4,
    StoreAccess = 5,
    InstructionMisaligned = 6,
    LoadMisaligned = 7,
    StoreMisaligned = 8,
}

impl ProcessFaultCode {
    /// 把用户态同步异常的裸 `scause` 编码映射为稳定值；未建模异常归 Unknown。
    /// 本内核无按需分配，页故障语义上归入对应 access 类。
    pub const fn from_scause(scause: usize) -> Self {
        match scause {
            0 => Self::InstructionMisaligned,
            1 | 12 => Self::InstructionAccess,
            2 => Self::IllegalInstruction,
            3 => Self::Breakpoint,
            4 => Self::LoadMisaligned,
            5 | 13 => Self::LoadAccess,
            6 => Self::StoreMisaligned,
            7 | 15 => Self::StoreAccess,
            _ => Self::Unknown,
        }
    }
}

/// ProcessDrain 输出。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct ProcessDrainResult {
    pub work_done: u32,
    pub status: u32,
    pub reserved: u64,
}

/// Drain 批次状态判别值（`ProcessDrainResult::status`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ProcessDrainStatus {
    More = 0,
    Complete = 1,
}

/// 单次 Drain 的工作上界（内核封顶；work unit 由内核定义）。
pub const PROCESS_DRAIN_MAX: u32 = 256;

/// JobId(u64) type；全局单调不复用，与 Pid 分立空间，root 恒为 1。
pub type JobId = u64;

/// 单次 JobEnumerate 的条目上界（内核封顶；条目为 8 字节 ID）。
pub const JOB_ENUMERATE_MAX: usize = 128;

/// JobControl 固定宽状态快照。计数是非精确近似值（不构成协议依据；
/// 正确性走 CLOSED 等待与枚举收敛）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct JobSnapshot {
    pub jid: JobId,
    /// root 无父，为 0。
    pub parent_jid: JobId,
    pub state: u32,
    pub live_processes: u32,
    pub live_children: u32,
    pub reserved: u32,
    pub reserved2: u64,
}

/// Job 生命周期状态判别值（`JobSnapshot::state`）。
/// Open 即使空也存活；Seal 只封创建；完成 = 自身 sealed && 成员空，
/// 完成即 Dead/CLOSED。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum JobState {
    Open = 0,
    Sealed = 1,
    Dead = 2,
}

/// 枚举/派生的成员维度判别值（JobEnumerate 与 JobDerive 的 kind）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum JobMemberKind {
    /// 直接 child Jobs（JobId 序）。
    ChildJobs = 0,
    /// 直接 member processes（Pid 序）。
    MemberProcesses = 1,
}

/// JobEnumerate 输出。契约：`more=1 ⇒ actual ≥ 1 ∨ next_cursor == 入参
/// cursor`（后者是未决事务占位的零进展屏障，调用方以原 cursor 重试；
/// 占位窗口在创建方单个 syscall 内，重试不活锁）；`more=0` 表示表内无
/// 任何 ID > next_cursor 的（可见或占位）条目。违反即内核违约，
/// 用户态拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct JobEnumerateResult {
    /// 本批最后返回条目的 ID；无返回时等于入参 cursor。
    pub next_cursor: u64,
    pub actual: u32,
    pub more: u32,
}

const _: () = {
    assert!(core::mem::size_of::<ProcessMapFlags>() == 4);
    assert!(core::mem::size_of::<ProcessCreateResult>() == 32);
    assert!(core::mem::size_of::<HandleGrant>() == 16);
    assert!(core::mem::size_of::<ThreadStartContext>() == 32);
    assert!(core::mem::size_of::<ProcessSnapshot>() == 40);
    assert!(core::mem::size_of::<ThreadSpawnResult>() == 16);
    assert!(core::mem::size_of::<ProcessDrainResult>() == 16);
    assert!(core::mem::size_of::<ProcessState>() == 4);
    assert!(core::mem::size_of::<ProcessExitReason>() == 4);
    assert!(core::mem::size_of::<ProcessDrainStatus>() == 4);
    assert!(core::mem::size_of::<JobSnapshot>() == 40);
    assert!(core::mem::size_of::<JobState>() == 4);
    assert!(core::mem::size_of::<JobMemberKind>() == 4);
    assert!(core::mem::size_of::<JobEnumerateResult>() == 16);
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
        if self { 0 } else { -1 }
    }
}

/// ExitCode for process result which treated as Termination
pub type ProgramResult = Result<(), ExitCode>;

impl Termination for ProgramResult {
    fn to_exit_code(self) -> ExitCode {
        if let Err(code) = self { code } else { 0 }
    }
}
