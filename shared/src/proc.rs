use flagset::flags;

/// ExitCode(i64) type for process
pub type ExitCode = i64;
/// Pid(u32) type for process
pub type Pid = u32;
/// Tid(u32) type for thread
/// If uniform thread-id required, It is uni_tid = ((pid << 32) | tid)
pub type Tid = u32;
flags! {
    /// Permission of the process
    /// Invalid when fork means copy the permissions from the parent
    pub enum ProcessPermission: u32{
        /// Not available
        Invalid = 0,
        /// Should be always present
        Valid = 1 << 0,
        /// Process operations
        Process = 1 << 1,
        /// It's a service and can be registered as service
        Service = 1 << 2,
        /// Map
        Memory = 1 << 3,
        /// IDK
        Net = 1 << 4,
        /// All of them
        All = (ProcessPermission::Valid | ProcessPermission::Process | ProcessPermission::Service | ProcessPermission::Memory | ProcessPermission::Net).bits()
    }
}

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
