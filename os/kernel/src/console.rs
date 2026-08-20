//! 内核控制台。
//!
//! 输出走 SBI（DBCN 优先，legacy putchar 回退）。多 hart 并发输出由
//! [`Spinlock`] 串行化；panic 路径绕过锁直写（见 [`console_write_raw`]），
//! 避免 panic 时持锁交叉导致死锁。

use core::fmt::{self, Arguments, Write};

use crate::{sbi, sync::Spinlock};

struct Console;

impl Console {
    const fn new() -> Self {
        Self
    }
}

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if sbi::is_debug_console_supported() {
            sbi::debug_console_write(s).map(|_| ()).map_err(|_| fmt::Error)
        } else {
            for b in s.bytes() {
                sbi::legacy_console_putchar(b);
            }
            Ok(())
        }
    }
}

static CONSOLE: Spinlock<Console> = Spinlock::new(Console::new());

/// 绕过锁的直写，仅 panic 等不可依赖锁纪律的路径使用。
pub fn console_write_raw(s: &str) {
    if sbi::is_debug_console_supported() {
        let _ = sbi::debug_console_write(s);
    } else {
        for b in s.bytes() {
            sbi::legacy_console_putchar(b);
        }
    }
}

/// 常规格式化输出（持锁，多 hart 安全）。
pub fn console_write(args: Arguments<'_>) {
    let mut console = CONSOLE.lock();
    console.write_fmt(args).ok();
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::console::console_write(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };
    ($fmt:expr) => {
        $crate::print!(concat!($fmt, "\n"))
    };
    ($fmt:expr, $($args:tt)+) => {
        $crate::print!(concat!($fmt, "\n"), $($args)+)
    };
}

#[macro_export]
macro_rules! debug {
    ($fmt:expr) => {
        #[cfg(debug_assertions)]
        $crate::print!(concat!("\x1b[0;35mDEBG\x1b[0m ", $fmt, "\n"))
    };
    ($fmt:expr, $($args:tt)+) => {
        #[cfg(debug_assertions)]
        $crate::print!(concat!("\x1b[0;35mDEBG\x1b[0m ", $fmt, "\n"), $($args)+)
    };
}

#[macro_export]
macro_rules! info {
    ($fmt:expr) => {
        $crate::print!(concat!("\x1b[0;32mINFO\x1b[0m ", $fmt, "\n"))
    };
    ($fmt:expr, $($args:tt)+) => {
        $crate::print!(concat!("\x1b[0;32mINFO\x1b[0m ", $fmt, "\n"), $($args)+)
    };
}

#[macro_export]
macro_rules! warning {
    ($fmt:expr) => {
        $crate::print!(concat!("\x1b[0;33mWARN\x1b[0m ", $fmt, "\n"))
    };
    ($fmt:expr, $($args:tt)+) => {
        $crate::print!(concat!("\x1b[0;33mWARN\x1b[0m ", $fmt, "\n"), $($args)+)
    };
}

#[macro_export]
macro_rules! error {
    ($fmt:expr) => {
        $crate::print!(concat!("\x1b[0;31mERRO\x1b[0m ", $fmt, "\n"))
    };
    ($fmt:expr, $($args:tt)+) => {
        $crate::print!(concat!("\x1b[0;31mERRO\x1b[0m ", $fmt, "\n"), $($args)+)
    };
}
