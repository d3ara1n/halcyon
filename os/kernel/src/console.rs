//! 内核控制台。
//!
//! 输出走 SBI（初始化后 DBCN，初始化前 legacy putchar）。多 hart 并发输出由
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
        if sbi::is_debug_console_ready() {
            sbi::debug_console_write_best_effort(s);
            Ok(())
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
    if sbi::is_debug_console_ready() {
        sbi::debug_console_write_best_effort(s);
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

/// 话题标签宽度（对齐）。
const TOPIC_WIDTH: usize = 8;

/// 等级色（基础调色板，随终端亮暗主题）：颜色只来源于等级——
/// info 绿、warn 黄、error 红、debug 灰；log! 无等级故无色，
/// 正文颜色由发送方自行拼入消息。色只染话题头，不染正文。
pub const COLOR_INFO: &str = "32";
pub const COLOR_WARN: &str = "33";
#[allow(dead_code)] // 引用在 error! 宏体内，调用点随非致命错误场景出现
pub const COLOR_ERROR: &str = "31";
pub const COLOR_DEBUG: &str = "90";

/// 无色话题行（log!）：`[topic     ] message`，对齐不者色。
pub fn log_topic(topic: &str, args: Arguments<'_>) {
    console_write(format_args!(
        "[{:<width$}] {}\n",
        topic,
        args,
        width = TOPIC_WIDTH,
    ));
}

/// 等级话题行（等级宏）：话题头按等级色着色、固定宽度对齐，正文不着色。
pub fn log_tagged(tag: &str, color: &str, args: Arguments<'_>) {
    console_write(format_args!(
        "\x1b[{}m[{:<width$}]\x1b[0m {}\n",
        color,
        tag,
        args,
        width = TOPIC_WIDTH,
    ));
}


macro_rules! print {
    ($($arg:tt)*) => {
        $crate::console::console_write(format_args!($($arg)*))
    };
}

macro_rules! println {
    () => {
        print!("\n")
    };
    ($fmt:expr) => {
        print!(concat!($fmt, "\n"))
    };
    ($fmt:expr, $($args:tt)+) => {
        print!(concat!($fmt, "\n"), $($args)+)
    };
}

/// 话题制日志：`log!(Task, "pid {} 回收", pid)` 输出着色对齐的
/// `[Task    ] pid 3 回收`；无话题形态 `log!("{}", x)` 纯输出。
macro_rules! log {
    ($topic:ident, $($arg:tt)+) => {
        $crate::console::log_topic(stringify!($topic), format_args!($($arg)+))
    };
    ($fmt:expr) => {
        print!(concat!($fmt, "\n"))
    };
    ($fmt:expr, $($arg:tt)+) => {
        print!(concat!($fmt, "\n"), $($arg)+)
    };
}

/// [`log!`] 的等级变体（亮色系）：等级色覆盖话题色；无话题时等级名
/// 当标签。`dbg!` 额外叠 debug-only（release 编译期消除）。
macro_rules! info {
    ($topic:ident, $($arg:tt)+) => {
        $crate::console::log_tagged(stringify!($topic), $crate::console::COLOR_INFO, format_args!($($arg)+))
    };
    ($fmt:expr) => {
        $crate::console::log_tagged("Info", $crate::console::COLOR_INFO, format_args!(concat!($fmt, "\n")))
    };
    ($fmt:expr, $($arg:tt)+) => {
        $crate::console::log_tagged("Info", $crate::console::COLOR_INFO, format_args!(concat!($fmt, "\n"), $($arg)+))
    };
}

macro_rules! warn {
    ($topic:ident, $($arg:tt)+) => {
        $crate::console::log_tagged(stringify!($topic), $crate::console::COLOR_WARN, format_args!($($arg)+))
    };
    ($fmt:expr) => {
        $crate::console::log_tagged("Warn", $crate::console::COLOR_WARN, format_args!(concat!($fmt, "\n")))
    };
    ($fmt:expr, $($arg:tt)+) => {
        $crate::console::log_tagged("Warn", $crate::console::COLOR_WARN, format_args!(concat!($fmt, "\n"), $($arg)+))
    };
}

#[allow(unused_macros)] // 等级面完整性；内核致命路径走 panic，尚未有非致命 error 场景
macro_rules! error {
    ($topic:ident, $($arg:tt)+) => {
        $crate::console::log_tagged(stringify!($topic), $crate::console::COLOR_ERROR, format_args!($($arg)+))
    };
    ($fmt:expr) => {
        $crate::console::log_tagged("Error", $crate::console::COLOR_ERROR, format_args!(concat!($fmt, "\n")))
    };
    ($fmt:expr, $($arg:tt)+) => {
        $crate::console::log_tagged("Error", $crate::console::COLOR_ERROR, format_args!(concat!($fmt, "\n"), $($arg)+))
    };
}

/// [`log!`] 的 debug-only 变体：release 构建静默（编译期消除）。
#[allow(unused_macros)] // 门面五级对齐；内核侧暂无高频追踪点，随 IPC/FS 接入使用
macro_rules! debug {
    ($topic:ident, $($arg:tt)+) => {
        if cfg!(debug_assertions) {
            $crate::console::log_tagged(stringify!($topic), $crate::console::COLOR_DEBUG, format_args!($($arg)+))
        }
    };
    ($fmt:expr) => {
        if cfg!(debug_assertions) {
            print!(concat!($fmt, "\n"))
        }
    };
    ($fmt:expr, $($arg:tt)+) => {
        if cfg!(debug_assertions) {
            print!(concat!($fmt, "\n"), $($arg)+)
        }
    };
}
