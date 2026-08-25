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
    // Role of process
    /// Process must need the permission to do the system call
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
/// ipc_call is sent through SystemCall::IPC
#[repr(usize)]
#[derive(Debug, FromPrimitive, ToPrimitive, Clone, Copy)]
pub enum SystemCall {
    // System reserved
    /// Makes kernel panic
    Die = 0x0,
    /// 写调试流（测试观测通道）：a0=msg_ptr a1=msg_len
    Debug = 0x01,

    // -----Process control-----
    /// Finalized process notifies kernel to cleanup
    Exit = 0x10,
    /// Spawn a process from the given bytes
    ExecuteBytes = 0x16,
    /// Spawn a process from the file
    ExecuteFile = 0x17,
    /// 临时启动资源查询：返回本进程 bootstrap Mailbox owner Handle。
    /// 后续由通用 startup-resource 枚举替代。
    StartupMailbox = 0x18,

    // -----Thread-----
    /// Finalized thread notifies kernel to cleanup
    ThreadExit = 0x20,
    /// Be nice
    ThreadYield = 0x21,
    /// Create a thread for the process
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
    
    // -----Process memory-----
    /// Map a range of virtual addresses for the process with kernel served pages
    Extend = 0x50,
    /// Map a range of virtual addresses for the process with specific range of physical addresses
    /// 
    /// **Permissions**: *Haven't determined yet*
    Map = 0x51,
    /// Tell kernel to reuse a range of virtual addresses
    Free = 0x52,

    // -----Tunnel-----
    /// 创建共享页、Endpoint 和一次性 Invitation。
    TunnelCreate = 0x60,
    /// 原子消费 Invitation 并建立对端 Endpoint。
    TunnelAttach = 0x61,
    /// 向对端 Endpoint 发布 DATA 提示。
    TunnelNotify = 0x63,
    /// 在协议无进展点确认本端 DATA 提示。
    TunnelAcknowledgeData = 0x64,

    // -----Filesystem abstract layer-----
    /// Check if dentry exist
    Access = 0x70,
    /// Fetch a structure describing dentry(-ies) metadata
    Inspect = 0x71,
    /// Change dentry's metadata without touching its content
    Modify = 0x72,
    /// Create a dentry with specific type with no content appended
    Create = 0x73,
    /// Delete a dentry
    Delete = 0x74,
    /// Create another copy of file or directory with the same content(metadata may diffs)
    Copy = 0x75,
    /// Works like renaming
    Move = 0x76,
    /// Create a tunnel referring to the file if is stream
    Open = 0x77,
    /// Read underlying bytes into buffer if is property
    Read= 0x78,
    /// Write underlying bytes from buffer if is property with the same type
    Write = 0x79,
    /// Mount a filesystem service as a mount point at rootfs
    Mount = 0x7a,
    /// Unmount a mount point from rootfs
    Unmount = 0x7b,
}
