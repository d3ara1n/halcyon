use num_derive::{FromPrimitive, ToPrimitive};

/// Predefined system call errors
#[repr(usize)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, FromPrimitive, ToPrimitive)]
pub enum SystemCallError {
    // Generic errors
    /// [SystemCallError::NoError] means no errors at all
    NoError = 0x00,
    /// Undefined error
    Unknown = 0x01,
    /// Undefined error
    InternalError = 0x02,
    /// Argument out of range or illegal
    IllegalArgument = 0x3,
    /// System call can not be performed
    FunctionNotAvailable = 0x04,
    // Capability policy
    /// 调用者缺少请求操作所需 authority。
    PermissionDenied = 0x10,
    // Memory related
    /// System is out of memory or the process reached the allocation limit
    OutOfMemory = 0x20,
    /// Address is not power of two or page-aligned
    InvalidAddress = 0x21,
    /// The region accessed is not available
    MemoryNotAccessible = 0x22,
    /// 请求范围与既有地址空间所有权冲突。
    AddressConflict = 0x23,
    /// 请求范围未被普通 mapping/reservation 完整覆盖。
    NotMapped = 0x24,
    /// Pool 额度不足；不表示物理库存或 metadata 耗尽。
    QuotaExceeded = 0x25,
    // Special operations
    /// Specific operation cannot be applied due to bad reference
    ObjectNotFound = 0x30,
    /// Found but unready to use
    ObjectNotAvailable = 0x31,
    /// Found but owned by others
    ObjectNotAccessible = 0x32,
    /// Can not own more objects
    ReachLimit = 0x33,
    /// Cannot perform operation on this type of objects
    NotSupported = 0x34,
    /// Target mailbox is full (message delivery never blocks)
    MailboxFull = 0x35,
    /// 对象存在尚未完成的独占事务。
    ObjectBusy = 0x36,
    /// 输出容量无法容纳完整结果。
    BufferTooSmall = 0x37,
    /// 对象已经进入不可逆关闭状态。
    ObjectClosed = 0x38,
    /// Handle 不具备操作所需 rights。
    RightsDenied = 0x39,
    /// Handle 的对象类型或 lifecycle role 不符合操作要求。
    WrongObjectType = 0x3a,
    /// Handle generation 已不匹配槽位。
    StaleHandle = 0x3b,
    /// 单调时钟或期限超出本次启动支持的区间。
    ClockRange = 0x3c,
    /// 投递线性化点前期限已到，全部源能力未消费。
    DeadlineExpired = 0x3d,
}

impl SystemCallError {
    pub const fn from_u32(raw: u32) -> Option<Self> {
        match raw {
            0x00 => Some(Self::NoError),
            0x01 => Some(Self::Unknown),
            0x02 => Some(Self::InternalError),
            0x03 => Some(Self::IllegalArgument),
            0x04 => Some(Self::FunctionNotAvailable),
            0x10 => Some(Self::PermissionDenied),
            0x20 => Some(Self::OutOfMemory),
            0x21 => Some(Self::InvalidAddress),
            0x22 => Some(Self::MemoryNotAccessible),
            0x23 => Some(Self::AddressConflict),
            0x24 => Some(Self::NotMapped),
            0x25 => Some(Self::QuotaExceeded),
            0x30 => Some(Self::ObjectNotFound),
            0x31 => Some(Self::ObjectNotAvailable),
            0x32 => Some(Self::ObjectNotAccessible),
            0x33 => Some(Self::ReachLimit),
            0x34 => Some(Self::NotSupported),
            0x35 => Some(Self::MailboxFull),
            0x36 => Some(Self::ObjectBusy),
            0x37 => Some(Self::BufferTooSmall),
            0x38 => Some(Self::ObjectClosed),
            0x39 => Some(Self::RightsDenied),
            0x3a => Some(Self::WrongObjectType),
            0x3b => Some(Self::StaleHandle),
            0x3c => Some(Self::ClockRange),
            0x3d => Some(Self::DeadlineExpired),
            _ => None,
        }
    }
}

/// Predefined system calls
///
/// Only accessible in userspace
#[repr(usize)]
#[derive(Debug, FromPrimitive, ToPrimitive, Clone, Copy)]
pub enum SystemCall {
    /// 写调试流（测试观测通道）：a0=msg_ptr a1=msg_len
    Debug = 0x01,
    /// 以 `SystemReset` capability 提交系统终局。
    SystemReset = 0x02,

    // -----Process control-----
    /// 当前进程主动中止执行，终因 Terminated；不等待 Drain。
    ProcessTerminate = 0x10,
    /// 当前进程的 runtime 报告完整性失败，终因 Aborted。
    ProcessAbort = 0x23,
    /// 从已有 JobControl 创建子 JobControl；a0=parent Job[CREATE]，
    /// a1=付款 Pool[CREATE]，a2=rights，a3=输出 Handle 地址。
    JobCreate = 0x11,
    /// 在 Job（CREATE）内以 MemoryPool（CREATE）付款创建 Building process，
    /// a0=Job，a1=Pool，a2=control rights，a3=ProcessCreateResult 输出地址。
    /// 返回 affine ProcessBuilder；付款账户出生即定，不再改绑，两份源 capability 不消费。
    ProcessCreate = 0x12,
    /// 为 Building process 映射 anonymous zero pages。
    ProcessMap = 0x13,
    /// 向 Building process 已映射页写入有界数据。
    ProcessWrite = 0x14,
    /// 消费 ProcessBuilder，入册进程（活体门：已附线程 ≥1）。
    ProcessStart = 0x15,
    /// 创建挂起线程；target 为 ProcessBuilder 或 INVALID self 哨兵。
    ThreadCreate = 0x1d,
    /// 为 Building process 安装 grants 并输出目标侧句柄值。
    ProcessGrant = 0x1e,
    /// 读 ProcessControl 的固定宽生命周期快照。
    ProcessQuery = 0x16,
    /// 持 MANAGE authority 的非等待幂等终止请求；a1=0 首次接受，1 已终止中。
    ProcessKill = 0x17,
    /// 封闭 Job 及其后代的创建/启动口（幂等）。
    JobSeal = 0x19,
    /// 读 JobControl 的固定宽生命周期快照。
    JobQuery = 0x1a,
    /// 单调 ID 序游标分页枚举 Job 直接成员。
    JobEnumerate = 0x1b,
    /// 在直接成员域内按 ID 派生 child JobControl / ProcessControl。
    JobDerive = 0x1c,

    // -----Thread-----
    /// 正常结束当前线程；若为末线程则冻结进程 Completed 终态。
    ThreadExit = 0x20,
    /// 在完整事务边界把当前线程重新放入调度队列。
    ThreadYield = 0x21,
    /// 将 Created ThreadControl 发布为 Ready，无输出、无分配。
    ThreadStart = 0x22,
    /// 冻结 Building process 的执行需求与调度域。
    ProcessBindExecution = 0x24,
    /// 当前线程等待绝对 Deadline（异步：Waiting，到期唤醒）。
    Sleep = 0x25,
    // -----对象与等待-----
    /// 关闭一个进程本地 Handle。
    HandleClose = 0x30,
    /// 裁剪 rights 后复制 Handle。
    HandleDuplicate = 0x31,
    /// 等待任一对象电平命中。
    WaitOne = 0x32,
    /// 创建 Notification owner/signaler 对。
    NotificationCreate = 0x33,
    /// 向 Notification OR 提交待决位。
    NotificationSignal = 0x34,
    /// 原子取走 Notification 待决位。
    NotificationTake = 0x35,
    /// 从 Notification Owner/Signaler 派生只观察 Owner 关闭的能力。
    NotificationMintObserver = 0x3d,
    /// 无 capability 的公共单调时钟快照。
    MonotonicNow = 0x36,
    /// 查询本进程 Handle 的对象、role、rights 和关联身份。
    HandleQuery = 0x37,
    /// 创建有显式注册容量的持久观察集合。
    WaitSetCreate = 0x38,
    /// 注册一个来源并交付不复用的 token。
    WaitSetRegister = 0x39,
    /// 原子接收有界 ready 批次。
    WaitSetReceive = 0x3b,
    /// 撤销 token 的后续 ready 交付并启动源清理。
    WaitSetRemove = 0x3c,

    // -----消息-----
    /// 创建唯一 Mailbox receiver-owner。
    MailboxCreate = 0x40,
    /// 向 sender Handle 指向的邮箱原子投递消息和 Handle moves。
    Send = 0x41,
    /// 非阻塞观察队头 MessageHeader。
    Peek = 0x42,
    /// 非阻塞原子接收完整消息。
    Receive = 0x43,
    /// 丢弃队头及其 transit Handles。
    Discard = 0x44,
    /// 从具 DUPLICATE 权的 sender 派生一次性投递权（send-once）。
    MailboxMakeSendOnce = 0x45,
    /// 由 mailbox owner 铸造带不可变 badge 的 sender capability。
    MailboxMintSender = 0x46,

    // -----Process memory-----
    /// 为当前 Running process 建立 anonymous mapping。
    MemoryMap = 0x50,
    /// 精确解除当前 Running process 的普通 mapping/reservation。
    MemoryUnmap = 0x51,
    /// 改变当前 Running process 已映射范围的页权限。
    MemoryProtect = 0x52,
    /// 读取 MemoryPool 的固定宽账户快照。
    MemoryPoolQuery = 0x53,
    /// 从 parent Pool 不可撤销地派生 child Pool。
    MemoryPoolDerive = 0x54,
    /// 从当前进程绑定池创建固定长度 MemoryObject。
    MemoryObjectCreate = 0x55,
    /// 读取 MemoryObject 的固定宽快照。
    MemoryObjectQuery = 0x56,
    /// 单向发布 MemoryObject 为可执行（幂等）。
    MemoryObjectSeal = 0x57,

    // -----Tunnel-----
    /// 以付款 MemoryPool 创建共享 backing、本端 Endpoint 与一次性 Invitation；
    /// 不建立映射，映射经 MemoryMap 以 Endpoint 为 backing 来源。
    TunnelCreate = 0x60,
    /// 原子消费 Invitation，只返回对端 Endpoint 一个 Handle。
    TunnelAttach = 0x61,
    /// 置对端 DATA 提示（SIGNAL）；可合并，最后关闭状态压过迟到通知。
    EndpointNotify = 0x62,
    /// 清本端 DATA 提示（SIGNAL）；调用者确认后须重探共享状态。
    EndpointAcknowledgeData = 0x63,
}

#[cfg(test)]
mod tests {
    use super::{SystemCall, SystemCallError};

    #[test]
    fn memory_pool_numbers_are_stable() {
        assert_eq!(SystemCallError::QuotaExceeded as usize, 0x25);
        assert_eq!(SystemCall::MemoryPoolQuery as usize, 0x53);
        assert_eq!(SystemCall::MemoryPoolDerive as usize, 0x54);
    }

    #[test]
    fn memory_object_numbers_are_stable() {
        assert_eq!(SystemCall::MemoryObjectCreate as usize, 0x55);
        assert_eq!(SystemCall::MemoryObjectQuery as usize, 0x56);
        assert_eq!(SystemCall::MemoryObjectSeal as usize, 0x57);
    }
}
