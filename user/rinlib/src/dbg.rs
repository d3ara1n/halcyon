//! 日志宏：与内核 console 同构的话题制——`log!(fs, "...")` 输出着色
//! 对齐的 `[fs      ] ...`（内核侧统一格式化）；等级变体 `info!/warn!/
//! error!/dbg!` 用亮色覆盖话题色；`debug!` 为无话题纯输出的兼容形态。

use core::fmt::Arguments;

use alloc::fmt::format;

use crate::call::sys_debug_leveled;
use erhino_shared::call::debug_level;

pub fn debug(args: Arguments) {
    log_leveled("", debug_level::NONE, args);
}

pub fn log_leveled(tag: &str, level: u8, args: Arguments) {
    let msg = format(args);
    unsafe {
        let _ = sys_debug_leveled(tag, &msg, level);
    }
}

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {{
        $crate::dbg::debug(format_args!($($arg)*))
    }};
}

#[macro_export]
macro_rules! log {
    ($topic:ident, $($arg:tt)+) => {{
        $crate::dbg::log_leveled(stringify!($topic), 0, format_args!($($arg)+))
    }};
    ($fmt:expr) => {{
        $crate::dbg::debug(format_args!($fmt))
    }};
    ($fmt:expr, $($arg:tt)+) => {{
        $crate::dbg::debug(format_args!($fmt, $($arg)+))
    }};
}

#[macro_export]
macro_rules! info {
    ($topic:ident, $($arg:tt)+) => {{
        $crate::dbg::log_leveled(stringify!($topic), 1, format_args!($($arg)+))
    }};
    ($fmt:expr) => {{
        $crate::dbg::log_leveled("Info", 1, format_args!($fmt))
    }};
    ($fmt:expr, $($arg:tt)+) => {{
        $crate::dbg::log_leveled("Info", 1, format_args!($fmt, $($arg)+))
    }};
}

#[macro_export]
macro_rules! warn {
    ($topic:ident, $($arg:tt)+) => {{
        $crate::dbg::log_leveled(stringify!($topic), 2, format_args!($($arg)+))
    }};
    ($fmt:expr) => {{
        $crate::dbg::log_leveled("Warn", 2, format_args!($fmt))
    }};
    ($fmt:expr, $($arg:tt)+) => {{
        $crate::dbg::log_leveled("Warn", 2, format_args!($fmt, $($arg)+))
    }};
}

#[macro_export]
macro_rules! error {
    ($topic:ident, $($arg:tt)+) => {{
        $crate::dbg::log_leveled(stringify!($topic), 3, format_args!($($arg)+))
    }};
    ($fmt:expr) => {{
        $crate::dbg::log_leveled("Error", 3, format_args!($fmt))
    }};
    ($fmt:expr, $($arg:tt)+) => {{
        $crate::dbg::log_leveled("Error", 3, format_args!($fmt, $($arg)+))
    }};
}

/// debug-only 变体：release 构建静默（编译期消除）。
#[macro_export]
macro_rules! dbg {
    ($topic:ident, $($arg:tt)+) => {{
        if cfg!(debug_assertions) {
            $crate::dbg::log_leveled(stringify!($topic), 4, format_args!($($arg)+))
        }
    }};
    ($fmt:expr) => {{
        if cfg!(debug_assertions) {
            $crate::dbg::debug(format_args!($fmt))
        }
    }};
    ($fmt:expr, $($arg:tt)+) => {{
        if cfg!(debug_assertions) {
            $crate::dbg::debug(format_args!($fmt, $($arg)+))
        }
    }};
}
