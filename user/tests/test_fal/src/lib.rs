#![no_std]

pub mod startup {
    pub const PRIMARY_ROOT: usize = 0;
    pub const SERVICE_DIRECTORY: usize = 1;
    pub const COMMAND_OWNER: usize = 2;
    pub const REPORT_SIGNALER: usize = 3;
}

pub mod command {
    pub const CONTINUE: u64 = 1;
    pub const OBSERVE_SHUTDOWN: u64 = 2;
}

pub mod report {
    pub const DISCOVERY_ARMED: u64 = 1;
    pub const SECONDARY_DISCOVERED: u64 = 2;
    pub const COMPLETE: u64 = 4;
    pub const PROVIDER_CLOSED: u64 = 8;
}
