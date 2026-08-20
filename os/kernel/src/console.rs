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

/// 话题标签宽度（对齐）。
const TOPIC_WIDTH: usize = 8;

/// 话题 → ANSI 颜色码。未列出的话题用灰色，新增话题在此登记。
fn topic_color(topic: &str) -> &'static str {
    match topic {
        "Task" => "0;33",                      // 黄：任务生命周期
        "Hart" => "0;32",                      // 绿：hart 装配
        "MM" | "Frame" | "Heap" | "InitFS" | "Memory" => "0;36", // 青：内存与装载
        "Warn" => "0;35",                     // 品红：警告
        _ => "0;90",
    }
}

/// 话题行输出：`[topic     ] message`，标签按话题着色、固定宽度对齐。
pub fn log_topic(topic: &str, args: Arguments<'_>) {
    console_write(format_args!(
        "\x1b[{}m[{:<width$}]\x1b[0m {}\n",
        topic_color(topic),
        topic,
        args,
        width = TOPIC_WIDTH,
    ));
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

/// 话题制日志：`log!(Task, "pid {} 回收", pid)` 输出着色对齐的
/// `[Task    ] pid 3 回收`；无话题形态 `log!("{}", x)` 纯输出。
#[macro_export]
macro_rules! log {
    ($topic:ident, $($arg:tt)+) => {
        $crate::console::log_topic(stringify!($topic), format_args!($($arg)+))
    };
    ($fmt:expr) => {
        $crate::print!(concat!($fmt, "\n"))
    };
    ($fmt:expr, $($arg:tt)+) => {
        $crate::print!(concat!($fmt, "\n"), $($arg)+)
    };
}

/// [`log!`] 的 debug-only 变体：release 构建静默（编译期消除）。
#[macro_export]
macro_rules! dbg {
    ($topic:ident, $($arg:tt)+) => {
        if cfg!(debug_assertions) {
            $crate::console::log_topic(stringify!($topic), format_args!($($arg)+))
        }
    };
    ($fmt:expr) => {
        if cfg!(debug_assertions) {
            $crate::print!(concat!($fmt, "\n"))
        }
    };
    ($fmt:expr, $($arg:tt)+) => {
        if cfg!(debug_assertions) {
            $crate::print!(concat!($fmt, "\n"), $($arg)+)
        }
    };
}
