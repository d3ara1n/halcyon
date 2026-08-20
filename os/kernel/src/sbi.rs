//! SBI 调用封装（M0 子集：探测、控制台、HSM 启动）。

use core::{
    arch::asm,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbiExtension {
    LegacyConsolePutchar,
    Base,
    HartStateManagement,
    DebugConsole,
}

impl SbiExtension {
    const fn number(self) -> usize {
        match self {
            SbiExtension::LegacyConsolePutchar => 0x01,
            SbiExtension::Base => 0x10,
            SbiExtension::HartStateManagement => 0x48534D,
            SbiExtension::DebugConsole => 0x4442434E,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbiError {
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

/// 标准 SBI ecall（fid 有效），返回。
#[inline]
fn sbi_call(eid: SbiExtension, fid: usize, arg0: usize, arg1: usize, arg2: usize) -> SbiResult {
    // SAFETY: ecall 是 S 态下请求 SBI 的唯一途径；寄存器约定按 SBI 规范。
    let (error, value): (isize, isize);
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") arg0 as isize => error,
            inlateout("a1") arg1 as isize => value,
            in("a2") arg2 as isize,
            in("a6") fid as isize,
            in("a7") eid.number() as isize,
        );
    }
    match error {
        0 => Ok(value),
        -1 => Err(SbiError::Failed),
        -2 => Err(SbiError::NotSupported),
        -3 => Err(SbiError::InvalidParameter),
        -4 => Err(SbiError::Denied),
        -5 => Err(SbiError::InvalidAddress),
        -6 => Err(SbiError::AlreadyAvailable),
        -7 => Err(SbiError::AlreadyStarted),
        -8 => Err(SbiError::AlreadyStopped),
        -9 => Err(SbiError::NoSharedMemory),
        other => Err(SbiError::Undefined(other)),
    }
}

/// legacy ecall（无 fid，返回值在 a0）。
#[inline]
fn legacy_call(eid: SbiExtension, arg0: usize) {
    // SAFETY: 同上，legacy 寄存器约定。
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") arg0 as isize => _,
            in("a7") eid.number() as isize,
        );
    }
}

static DEBUG_CONSOLE_SUPPORTED: AtomicBool = AtomicBool::new(false);

pub fn is_debug_console_supported() -> bool {
    DEBUG_CONSOLE_SUPPORTED.load(Ordering::Relaxed)
}

/// 启动早期探测扩展可用性。
pub fn init() {
    if let Ok(res) = sbi_call(
        SbiExtension::Base,
        3, // probe_extension
        SbiExtension::DebugConsole.number() as isize as usize,
        0,
        0,
    ) {
        DEBUG_CONSOLE_SUPPORTED.store(res != 0, Ordering::Relaxed);
    }
}

pub fn legacy_console_putchar(char: u8) {
    legacy_call(SbiExtension::LegacyConsolePutchar, char as usize);
}

pub fn debug_console_write(text: &str) -> SbiResult {
    sbi_call(
        SbiExtension::DebugConsole,
        0, // write
        text.len(),
        text.as_ptr() as usize,
        0,
    )
}

/// HSM：启动指定 hart，入口收到 a0 = hartid，a1 = opaque。
pub fn hart_start(hartid: usize, start_addr: usize, opaque: usize) -> SbiResult {
    sbi_call(SbiExtension::HartStateManagement, 0, hartid, start_addr, opaque)
}
