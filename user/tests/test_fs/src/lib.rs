#![no_std]
#![cfg_attr(target_arch = "riscv64", feature(allocator_api))]
extern crate alloc;
#[cfg(target_arch = "riscv64")]
pub mod registry;

pub mod startup {
    pub const PRIMARY_ROOT: usize = 0;
    pub const SERVICE_DIRECTORY: usize = 1;
    pub const COMMAND_OWNER: usize = 2;
    pub const REPORT_SIGNALER: usize = 3;
    pub const AUTHORITY_INBOX: usize = 4;
    pub const PRIMARY_ROOT_B: usize = 5;
    pub const SERVICE_DIRECTORY_B: usize = 6;
}

/// init 的可信发行交付：同一空正文按固定角色位置携带 primary A/B。
pub mod handoff {
    pub const AUTHORITY_A: usize = 0;
    pub const AUTHORITY_B: usize = 1;
    pub const MANAGEMENT_A: usize = 2;
    pub const MANAGEMENT_B: usize = 3;
    pub const PRIMARY_SLOTS: usize = 4;
}

pub mod command {
    /// 根编排的阶段预算；双向报告、命令和最终收集使用同一真值。
    pub const PHASE_TIMEOUT_MILLIS: u64 = 120_000;
    pub const CONTINUE: u64 = 1;
    pub const OBSERVE_SHUTDOWN: u64 = 2;
}

pub mod report {
    pub const DISCOVERY_ARMED: u64 = 1;
    pub const SECONDARY_DISCOVERED: u64 = 2;
    pub const COMPLETE: u64 = 4;
    pub const PROVIDER_CLOSED: u64 = 8;
}
