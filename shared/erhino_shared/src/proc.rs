use crate::object::{Handle, Rights};

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
/// Builder持MANAGE|GRANT|TRANSIT、不可DUPLICATE；成功移交仍是同一启动authority。
/// 未消费的最后Builder释放则Aborted；Control权限为调用者请求的类型上限子集。
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
/// 单次组装映射的准备/清零工作上界；loader 按此分片。
pub const PROCESS_MAP_MAX_BYTES: usize = 256 * PROCESS_PAGE_SIZE;

/// ProcessGrant 的 grant 项；目标 rights 只能收窄。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct HandleGrant {
    pub handle: Handle,
    pub rights: Rights,
}

/// 线程出生现场，由 ThreadCreate 的 Building 外部通道与 Running 自身通道共用。所有地址都是目标进程用户 VA；entry 必须位于
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

/// ThreadCreate 同步输出：control 仅观察 DONE，start 持 Pending 启动责任。
/// Building 进程的预建线程由 ProcessBuilder 启动，start 为 INVALID。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct ThreadCreateResult {
    pub tid: Tid,
    pub reserved: u32,
    pub control: Handle,
    pub start: Handle,
}

const _: () = {
    assert!(core::mem::size_of::<ThreadCreateResult>() == 24);
    assert!(core::mem::align_of::<ThreadCreateResult>() == 8);
};

/// ProcessControl 固定宽状态快照。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct ProcessSnapshot {
    pub pid: Pid,
    pub parent_pid: Pid,
    pub state: u32,
    pub cause: u32,
    pub detail: TerminationDetail,
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

/// 终因判别值（`ProcessSnapshot::cause`）。
/// Building/Running 要求 None、零 detail；Terminating/Dead 在首次终止点冻结。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum TerminationCause {
    None = 0,
    Completed = 1,
    Terminated = 2,
    Aborted = 3,
    Killed = 4,
    Faulted = 5,
    Abandoned = 6,
}

/// 终因附带的内核事实。Faulted: word0 低 32 位是 tid，高 32 位是
/// ProcessFaultCode；word1 是故障用户 VA。Killed: word0=1 表示
/// ProcessControl 请求；其余原因两个 word 均为零。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct TerminationDetail {
    pub word0: u64,
    pub word1: u64,
}

impl TerminationDetail {
    pub const NONE: Self = Self { word0: 0, word1: 0 };
    pub const PROCESS_CONTROL_KILL: Self = Self { word0: 1, word1: 0 };

    pub const fn fault(tid: Tid, kind: ProcessFaultCode, address: u64) -> Self {
        Self {
            word0: (tid as u64) | ((kind as u64) << 32),
            word1: address,
        }
    }
}

/// Fault 终因的稳定编码；不把裸 `scause` 固化为生命周期 ABI。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
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
    /// syscall 用户内存访问或已提交结果的输出复检失败。
    UserAccess = 9,
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

/// JobId(u64) type；全局单调不复用，与 Pid 分立空间，root 恒为 1。
pub type JobId = u64;

/// 单次 JobEnumerate 的条目上界（内核封顶；条目为 8 字节 ID）。
pub const JOB_ENUMERATE_MAX: usize = 128;

/// JobControl 固定宽状态快照。计数是非精确近似值（不构成协议依据；
/// 正确性走 CLOSED 等待）。CLOSED 发布时本 Job 已从父 Job 子表移除；
/// 单独的 Dead 快照不替代这一异步屏障。
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
/// Open 即使空也存活；Seal 封创建/启动；自身 sealed 且成员/子表空时为 Dead，
/// 从父表异步移除后发布 CLOSED。
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
    assert!(core::mem::size_of::<ProcessSnapshot>() == 48);
    assert!(core::mem::size_of::<ThreadCreateResult>() == 24);
    assert!(core::mem::size_of::<ProcessState>() == 4);
    assert!(core::mem::size_of::<TerminationCause>() == 4);
    assert!(core::mem::size_of::<TerminationDetail>() == 16);
    assert!(core::mem::size_of::<JobSnapshot>() == 40);
    assert!(core::mem::size_of::<JobState>() == 4);
    assert!(core::mem::size_of::<JobMemberKind>() == 4);
    assert!(core::mem::size_of::<JobEnumerateResult>() == 16);
};
