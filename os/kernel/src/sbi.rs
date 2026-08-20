//! SBI 调用封装（探测、控制台、HSM 启动、时钟、IPI）。

use core::{
    arch::asm,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbiExtension {
    LegacyConsolePutchar,
    Base,
    Timer,
    Ipi,
    HartStateManagement,
    DebugConsole,
}

impl SbiExtension {
    const fn number(self) -> usize {
        match self {
            SbiExtension::LegacyConsolePutchar => 0x01,
            SbiExtension::Base => 0x10,
            SbiExtension::Timer => 0x54494D45,
            SbiExtension::Ipi => 0x735049,
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
    // DBCN 契约：base_addr 是物理地址（SBI 在 M-mode 地址空间解引用），
    // 内核在高半区运行，指针必须先转 PA（纯算术，与当前 satp 无关）。
    sbi_call(
        SbiExtension::DebugConsole,
        0, // write
        text.len(),
        crate::mm::virt_to_phys(text.as_ptr() as usize),
        0,
    )
}

/// HSM：启动指定 hart，入口收到 a0 = hartid，a1 = opaque。
pub fn hart_start(hartid: usize, start_addr: usize, opaque: usize) -> SbiResult {
    sbi_call(SbiExtension::HartStateManagement, 0, hartid, start_addr, opaque)
}

/// 读 mtime（S 态可直接读 time CSR）。
#[inline]
pub fn read_time() -> u64 {
    let t: u64;
    // SAFETY: time 是 S 态可读的只读 CSR。
    unsafe { asm!("csrr {}, time", out(reg) t, options(nomem)) };
    t
}

/// TIME：编程下次时钟中断（stime_value 到达时置 STIP）。
/// `u64::MAX / 2` 为卸载语义——远超 mtime 可达范围，不触发亦不溢出。
pub fn set_timer(stime: u64) {
    let _ = sbi_call(SbiExtension::Timer, 0, stime as usize, 0, 0);
}

/// IPI：门铃。`mask` 按 hartid 置位（bit i = hartid i），
/// SBI 以 hart_mask 的物理地址取位图。
pub fn send_ipi(mask: &u64) {
    let pa = crate::mm::virt_to_phys(mask as *const u64 as usize);
    let _ = sbi_call(SbiExtension::Ipi, 0, pa, 0, 0);
}

/// 清除本 hart 的软件中断 pending（SSIP 位可由 S 态写）。
pub fn clear_ssip() {
    // SAFETY: 仅清 sip.SSIP 位。
    unsafe { asm!("csrc sip, {mask}", mask = in(reg) 2, options(nomem)) };
}
