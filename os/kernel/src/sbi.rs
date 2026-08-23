//! SBI 调用封装（探测、控制台、HSM 启动、时钟、IPI）。

use core::{
    arch::asm,
    fmt::Write,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SbiExtension {
    Base,
    Timer,
    Ipi,
    HartStateManagement,
    DebugConsole,
    SystemReset,
}

impl SbiExtension {
    const fn number(self) -> usize {
        match self {
            SbiExtension::Base => 0x10,
            SbiExtension::Timer => 0x54494D45,
            SbiExtension::Ipi => 0x735049,
            SbiExtension::HartStateManagement => 0x48534D,
            SbiExtension::DebugConsole => 0x4442434E,
            SbiExtension::SystemReset => 0x53525354,
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
    InvalidState,
    BadRange,
    Timeout,
    Io,
    DeniedLocked,
    Undefined(isize),
}

pub type SbiResult = Result<usize, SbiError>;

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
        0 => Ok(value as usize),
        -1 => Err(SbiError::Failed),
        -2 => Err(SbiError::NotSupported),
        -3 => Err(SbiError::InvalidParameter),
        -4 => Err(SbiError::Denied),
        -5 => Err(SbiError::InvalidAddress),
        -6 => Err(SbiError::AlreadyAvailable),
        -7 => Err(SbiError::AlreadyStarted),
        -8 => Err(SbiError::AlreadyStopped),
        -9 => Err(SbiError::NoSharedMemory),
        -10 => Err(SbiError::InvalidState),
        -11 => Err(SbiError::BadRange),
        -12 => Err(SbiError::Timeout),
        -13 => Err(SbiError::Io),
        -14 => Err(SbiError::DeniedLocked),
        other => Err(SbiError::Undefined(other)),
    }
}

static DEBUG_CONSOLE_READY: AtomicBool = AtomicBool::new(false);

pub fn is_debug_console_ready() -> bool {
    DEBUG_CONSOLE_READY.load(Ordering::Relaxed)
}

/// 启动早期确认现代 SBI 基线与必需扩展。
pub fn init() {
    let version = require(
        sbi_call(SbiExtension::Base, 0, 0, 0, 0),
        "BASE.get_spec_version",
    );
    let major = version >> 24 & 0x7f;
    if major < 2 {
        fatal("SBI specification older than 2.0", SbiError::NotSupported);
    }

    for extension in [
        SbiExtension::Timer,
        SbiExtension::Ipi,
        SbiExtension::HartStateManagement,
        SbiExtension::DebugConsole,
    ] {
        let supported = require(
            sbi_call(SbiExtension::Base, 3, extension.number(), 0, 0),
            "BASE.probe_extension",
        );
        if supported == 0 {
            fatal("required SBI extension unavailable", SbiError::NotSupported);
        }
    }
    DEBUG_CONSOLE_READY.store(true, Ordering::Release);
}

/// 将维持内核不变量的 SBI 失败转为可观测的致命错误。
pub fn require(result: SbiResult, operation: &str) -> usize {
    match result {
        Ok(value) => value,
        Err(err) => fatal(operation, err),
    }
}

struct LegacyWriter;

impl Write for LegacyWriter {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        for byte in text.bytes() {
            legacy_console_putchar(byte);
        }
        Ok(())
    }
}

fn fatal(operation: &str, err: SbiError) -> ! {
    let _ = LegacyWriter.write_fmt(format_args!(
        "\x1b[0;31mSBI fatal\x1b[0m: {} failed: {:?}\n",
        operation, err
    ));
    crate::hart::park()
}

/// 仅用于现代 DBCN 尚不可用时报告启动失败；不作为运行期兼容路径。
pub(crate) fn legacy_console_putchar(byte: u8) {
    unsafe {
        asm!(
            "ecall",
            inlateout("a0") byte as usize => _,
            in("a7") 0x01usize,
        );
    }
}

/// DBCN 仅承载内核观测日志：单次尽力写入，部分写、零写或错误均丢弃。
pub fn debug_console_write_best_effort(text: &str) {
    if !text.is_empty() {
        let _ = debug_console_write_bytes(text.as_bytes());
    }
}

fn debug_console_write_bytes(bytes: &[u8]) -> SbiResult {
    sbi_call(
        SbiExtension::DebugConsole,
        0,
        bytes.len(),
        crate::mm::virt_to_phys(bytes.as_ptr() as usize),
        0,
    )
}

/// TIME 的卸载值：SBI 规范规定使用无符号最大值。
pub const DISARM: u64 = u64::MAX;

/// HSM：启动指定 hart，入口收到 a0 = hartid，a1 = opaque。
pub fn hart_start(hartid: usize, start_addr: usize, opaque: usize) -> SbiResult {
    sbi_call(
        SbiExtension::HartStateManagement,
        0,
        hartid,
        start_addr,
        opaque,
    )
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
pub fn set_timer(stime: u64) -> SbiResult {
    sbi_call(SbiExtension::Timer, 0, stime as usize, 0, 0)
}

/// IPI：现代 SBI ABI 的 `hart_mask` 是值而非指针，base 固定为 0。
pub fn send_ipi(mask: u64) -> SbiResult {
    sbi_call(SbiExtension::Ipi, 0, mask as usize, 0, 0)
}

/// 清除本 hart 的软件中断 pending（SSIP 位可由 S 态写）。
pub fn clear_ssip() {
    // SAFETY: 仅清 sip.SSIP 位。
    unsafe { asm!("csrc sip, {mask}", mask = in(reg) 2, options(nomem)) };
}

/// SRST 复位类型。
pub const RESET_SHUTDOWN: u32 = 0;
/// SRST 复位类型（保留完整面；冷/热重启随重启需求启用）。
#[allow(dead_code)]
pub const RESET_COLD_REBOOT: u32 = 1;
#[allow(dead_code)]
pub const RESET_WARM_REBOOT: u32 = 2;

/// SRST：系统停机/重启（QEMU virt/sifive_u 的 OpenSBI 均支持，
/// 开机日志 `SysReset: yes` 即本扩展）。
pub fn system_reset(reset_type: u32, reset_reason: u32) -> SbiResult {
    sbi_call(
        SbiExtension::SystemReset,
        0,
        reset_type as usize,
        reset_reason as usize,
        0,
    )
}

/// 停机。SRST 成功后不返回；平台没有关机后端时记录具体错误并永久停放。
pub fn shutdown() -> ! {
    match system_reset(RESET_SHUTDOWN, 0) {
        Ok(value) => warn!(SBI, "SRST returned unexpectedly after success: {}", value),
        Err(err) => warn!(SBI, "SRST shutdown unavailable: {:?}", err),
    }
    crate::hart::park()
}
