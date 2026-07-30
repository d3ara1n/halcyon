use core::{
    arch::asm,
    sync::atomic::{AtomicBool, Ordering},
};
use strum_macros::EnumIter;

use crate::hart::HartId;

#[repr(isize)]
#[derive(Debug)]
pub enum SbiError {
    Success,
    Failed,
    NotSupported,
    InvalidParameter,
    Denied,
    InvalidAddress,
    AlreadyAvailable,
    AlreadyStarted,
    AlreadyStopped,
    NoSharedMemory,

    Undefined(isize),
}

pub type SbiResult = Result<isize, SbiError>;

#[repr(usize)]
#[allow(unused)]
#[derive(Debug, Clone, Copy, EnumIter)]
pub enum SbiExtension {
    LegacySetTimer = 0x0,
    LegacyConsolePutchar = 0x01,
    LegacyConsoleGetchar = 0x02,
    LegacyClearIPI = 0x03,
    LegacySendIPI = 0x04,
    LegacyRemoteFenceI = 0x05,
    LegacyRemoteSFenceVma = 0x06,
    LegacyRemoteSFenceVmaAsid = 0x07,
    LegacySystemShutdown = 0x08,
    Base = 0x10,
    Time = 0x54494D45,
    InterProcessInterrupt = 0x735049,
    RemoteFence = 0x52464E43,
    HartStateManagement = 0x48534D,
    SystemReset = 0x53525354,
    PerformanceMonitorUnit = 0x504D55,
    DebugConsole = 0x4442434E,
    SystemSuspend = 0x53555350,
    CollaborativeProcessorPerformanceControl = 0x43505043,
    NestedAcceleration = 0x4E41434C,
    StealTimeAccounting = 0x535441,
}

#[inline]
fn raw_call(
    eid: SbiExtension,
    fid: usize,
    arg0: usize,
    arg1: usize,
    arg2: usize,
) -> (isize, isize) {
    let mut error: isize;
    let mut value: isize;
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") arg0 => error,
            inlateout("a1") arg1 => value,
            in("a2") arg2,
            in("a6") fid,
            in("a7") eid as usize
        );
    }
    (error, value)
}

#[inline]
fn legacy_call(eid: SbiExtension, arg0: usize, arg1: usize, arg2: usize) -> usize {
    let mut ret: usize;
    unsafe {
        asm!("ecall",
            inlateout("a0") arg0 => ret,
            in("a1") arg1,
            in("a2") arg2,
            in("a7") eid as usize);
        ret
    }
}

#[inline]
fn sbi_call(eid: SbiExtension, fid: usize, arg0: usize, arg1: usize, arg2: usize) -> SbiResult {
    let (error, value) = raw_call(eid, fid, arg0, arg1, arg2);
    if error == 0 {
        Ok(value)
    } else {
        Err(match error {
            0 => SbiError::Success,
            -1 => SbiError::Failed,
            -2 => SbiError::NotSupported,
            -3 => SbiError::InvalidParameter,
            -4 => SbiError::Denied,
            -5 => SbiError::InvalidAddress,
            -6 => SbiError::AlreadyAvailable,
            -7 => SbiError::AlreadyStarted,
            -8 => SbiError::AlreadyStopped,
            -9 => SbiError::NoSharedMemory,
            _ => SbiError::Undefined(error),
        })
    }
}

pub fn legacy_set_timer(time: usize) {
    legacy_call(SbiExtension::LegacySetTimer, time, 0, 0);
}

pub fn legacy_console_putchar(char: u8) {
    legacy_call(SbiExtension::LegacyConsolePutchar, char as usize, 0, 0);
}

// Base extension #0x10

pub fn sbi_get_spec_version() -> SbiResult {
    sbi_call(SbiExtension::Base, 0, 0, 0, 0)
}

pub fn sbi_get_impl_id() -> SbiResult {
    sbi_call(SbiExtension::Base, 1, 0, 0, 0)
}

pub fn sbi_get_impl_version() -> SbiResult {
    sbi_call(SbiExtension::Base, 2, 0, 0, 0)
}

pub fn sbi_probe_extension(eid: SbiExtension) -> SbiResult {
    sbi_call(SbiExtension::Base, 3, eid as usize, 0, 0)
}

pub fn sbi_get_mach_vendor_id() -> SbiResult {
    sbi_call(SbiExtension::Base, 4, 0, 0, 0)
}

pub fn sbi_get_mach_arch_id() -> SbiResult {
    sbi_call(SbiExtension::Base, 5, 0, 0, 0)
}

pub fn sbi_get_mach_impl_id() -> SbiResult {
    sbi_call(SbiExtension::Base, 6, 0, 0, 0)
}

// Timer extension #0x53394D45

pub fn set_timer(time: usize) -> SbiResult {
    sbi_call(SbiExtension::Time, 0, time, 0, 0)
}

// IPI extension #0x735049

pub fn send_ipi(hart_mask: usize, hart_mask_base: isize) -> SbiResult {
    sbi_call(
        SbiExtension::InterProcessInterrupt,
        0x0,
        hart_mask,
        hart_mask_base as usize,
        0,
    )
}

// Hart State Management extension #0x48534D

pub fn hart_start(hartid: HartId, start_addr: usize, opaque: usize) -> SbiResult {
    sbi_call(
        SbiExtension::HartStateManagement,
        0,
        hartid,
        start_addr,
        opaque,
    )
}

pub fn hart_stop() -> SbiResult {
    sbi_call(SbiExtension::HartStateManagement, 1, 0, 0, 0)
}

pub fn hart_get_status(hartid: HartId) -> SbiResult {
    sbi_call(SbiExtension::HartStateManagement, 2, hartid, 0, 0)
}

pub fn hart_suspend(suspend_type: u32, resume_addr: usize, opaque: usize) -> SbiResult {
    sbi_call(
        SbiExtension::HartStateManagement,
        3,
        suspend_type as usize,
        resume_addr,
        opaque,
    )
}

// System Reset extension $0x53525354

pub fn system_reset(reset_type: u32, reset_reason: u32) -> SbiResult {
    sbi_call(
        SbiExtension::SystemReset,
        0,
        reset_type as usize,
        reset_reason as usize,
        0,
    )
}

// Debug Console extension #0x4442434E

pub fn debug_console_write(text: &str) -> SbiResult {
    let ptr = text.as_ptr();
    let count = text.len();
    sbi_call(SbiExtension::DebugConsole, 0, count, ptr as usize, 0)
}

pub fn debug_console_write_byte(byte: u8) -> SbiResult {
    sbi_call(SbiExtension::DebugConsole, 2, byte as usize, 0 as usize, 0)
}

static TIME_SUPPORTED: AtomicBool = AtomicBool::new(false);
static DEBUG_CONSOLE_SUPPORTED: AtomicBool = AtomicBool::new(false);

pub fn is_debug_console_supported() -> bool {
    DEBUG_CONSOLE_SUPPORTED.load(Ordering::Relaxed)
}

pub fn is_time_supported() -> bool {
    TIME_SUPPORTED.load(Ordering::Relaxed)
}
pub fn init() {
    if let Ok(res) = sbi_probe_extension(SbiExtension::DebugConsole) {
        DEBUG_CONSOLE_SUPPORTED.store(res != 0, Ordering::Relaxed);
    }
    if let Ok(res) = sbi_probe_extension(SbiExtension::Time) {
        TIME_SUPPORTED.store(res != 0, Ordering::Relaxed);
    }
}
