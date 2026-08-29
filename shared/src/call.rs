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
    /// Finalized process notifies kernel to cleanup
    Exit = 0x10,
    /// 从已有 JobControl 创建子 JobControl。
    JobCreate = 0x11,
    /// 在 Job 内创建空的 Building process，返回 affine ProcessBuilder。
    ProcessCreate = 0x12,
    /// 为 Building process 映射 anonymous zero pages。
    ProcessMap = 0x13,
    /// 向 Building process 已映射页写入有界数据。
    ProcessWrite = 0x14,
    /// 消费 ProcessBuilder，入册进程（活体门：已附线程 ≥1）。
    ProcessStart = 0x15,
    /// 为 Building process 附线程（线程是组装资源，无观察壳）。
    ProcessAttach = 0x1d,
    /// 为 Building process 安装 grants 并输出目标侧句柄值。
    ProcessGrant = 0x1e,
    /// 读 ProcessControl 的固定宽生命周期快照。
    ProcessQuery = 0x16,
    /// 持 MANAGE authority 的异步幂等终止请求。
    ProcessKill = 0x17,
    /// REAPABLE/Dead 上推进有界资源收束批次。
    ProcessDrain = 0x18,
    /// 封闭 Job 及其后代的创建/启动口（幂等）。
    JobSeal = 0x19,
    /// 读 JobControl 的固定宽生命周期快照。
    JobQuery = 0x1a,
    /// 单调 ID 序游标分页枚举 Job 直接成员。
    JobEnumerate = 0x1b,
    /// 在直接成员域内按 ID 派生 child JobControl / ProcessControl。
    JobDerive = 0x1c,

    // -----Thread-----
    /// Finalized thread notifies kernel to cleanup
    ThreadExit = 0x20,
    /// Be nice
    ThreadYield = 0x21,
    /// 向当前 Running process 附入 ThreadStartContext 描述的线程。
    ThreadSpawn = 0x22,
    /// Wait another owned thread to exit
    ThreadJoin = 0x23,
    /// Kill owned thread
    ThreadKill = 0x24,
    /// 当前线程睡眠指定毫秒（异步：登记期限后 Waiting，到期唤醒）
    Sleep = 0x25,
    // -----对象与等待-----
    /// 关闭一个进程本地 Handle。
    HandleClose = 0x30,
    /// 裁剪 rights 后复制 Handle。
    HandleDuplicate = 0x31,
    /// 等待任一对象电平命中。
    WaitMany = 0x32,
    /// 创建 Notification owner/signaler 对。
    NotificationCreate = 0x33,
    /// 向 Notification OR 提交待决位。
    NotificationSignal = 0x34,
    /// 原子取走 Notification 待决位。
    NotificationTake = 0x35,

    // -----消息-----
    /// 创建 Mailbox receiver-owner/sender 对。
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
    /// 字节粒度 sbrk：按请求量扩展/收缩堆，返回新堆顶。
    Extend = 0x50,

    // -----Tunnel-----
    /// 创建共享页、Endpoint 和一次性 Invitation。
    TunnelCreate = 0x60,
    /// 原子消费 Invitation 并建立对端 Endpoint。
    TunnelAttach = 0x61,
    /// 向对端 Endpoint 发布 DATA 提示。
    TunnelNotify = 0x62,
    /// 在协议无进展点确认本端 DATA 提示。
    TunnelAcknowledgeData = 0x63,
}
