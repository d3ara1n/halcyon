use num_derive::{FromPrimitive, ToPrimitive};

/// Predefined system call errors
#[repr(usize)]
#[derive(Debug, FromPrimitive, ToPrimitive)]
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
    /// Undefined behavior in release environment
    Debug = 0x01,

    // -----Process control-----
    /// Finalized process notifies kernel to cleanup
    Exit = 0x10,
    /// Spawn a process from the given bytes
    ExecuteBytes = 0x16,
    /// Spawn a process from the file
    ExecuteFile = 0x17,

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

    // -----Signal-----
    /// Return from signal handler
    SignalReturn = 0x30,
    /// Send a signal to the process
    SignalSend = 0x31,
    /// Set signal handler for the current process
    SignalSet = 0x32,

    // -----Messaging-----
    /// Send a message carrying a huge payload then block until message received
    Send = 0x40,
    /// Check the mailbox if there is a message and get the payload size
    /// 
    /// **Note**: Empty mailbox causes an expected error [SystemCallError::ObjectNotAvailable].
    /// `Peek` will steal the content in the mailbox of the process and put it into the thread private space for `Receive` to use
    Peek = 0x41,
    /// Empty the mailbox
    Discard = 0x42,
    /// Retrieve payload
    Receive = 0x43,
    
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
    /// Allocate a key-marked random page
    TunnelBuild = 0x60,
    /// Link a allocated page with a key
    TunnelLink = 0x61,
    /// Dispose the tunnel and restore the slot
    TunnelDispose = 0x62,
    /// Interrupt for receiving
    TunnelRequest = 0x6a,
    /// Interrupt for transmitting
    TunnelResponse = 0x6b,

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
