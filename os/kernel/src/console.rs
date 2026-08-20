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

/// 等级色（亮色系，与话题常规色分两档视觉层次）。
pub const COLOR_INFO: &str = "0;92";
pub const COLOR_WARN: &str = "0;93";
pub const COLOR_ERROR: &str = "0;91";
pub const COLOR_DBG: &str = "0;95";

/// 话题 → ANSI 颜色码。未列出的话题用灰色，新增话题在此登记。
fn topic_color(topic: &str) -> &'static str {
    match topic {
        "Task" => "0;33",                      // 黄：任务生命周期
        "Hart" => "0;32",                      // 绿：hart 装配
        "MM" | "Frame" | "Heap" | "InitFS" | "Memory" => "0;36", // 青：内存与装载
        _ => "0;90",
    }
}

/// 话题行输出：`[topic     ] message`，标签按话题映射色着色、固定宽度对齐。
pub fn log_topic(topic: &str, args: Arguments<'_>) {
    log_tagged(topic, topic_color(topic), args);
}

/// 指定色的话题行输出（等级宏与用户态日志入口）。
pub fn log_tagged(tag: &str, color: &str, args: Arguments<'_>) {
    console_write(format_args!(
        "\x1b[{}m[{:<width$}]\x1b[0m {}\n",
        color,
        tag,
        args,
        width = TOPIC_WIDTH,
    ));
}

/// 用户态日志入口：等级（0=无）选亮色，无等级时按话题色（未登记灰色）。
pub fn log_user(tag: &str, level: u8, args: Arguments<'_>) {
    let color = match level {
        1 => COLOR_INFO,
        2 => COLOR_WARN,
        3 => COLOR_ERROR,
        4 => COLOR_DBG,
        _ => topic_color(tag),
    };
    log_tagged(tag, color, args);
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
#[allow(unused_macros)] // debug-only 变体；内核侧暂无高频追踪点
macro_rules! dbg {
    ($topic:ident, $($arg:tt)+) => {
        if cfg!(debug_assertions) {
            $crate::console::log_tagged(stringify!($topic), $crate::console::COLOR_DBG, format_args!($($arg)+))
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
