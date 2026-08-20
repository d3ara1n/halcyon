//! debug 输出：向内核调试流发消息（测试观测通道）。无颜色无等级——
//! 内核侧统一以 `[pid N]` 话题、debug 等级色印出；要自定义格式/颜色
//! 自己拼进消息字符串。正式的用户态输出是未来的 console 服务。

use core::fmt::Arguments;

use alloc::fmt::format;

use crate::call::sys_debug;

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {{
        $crate::dbg::debug(format_args!($($arg)*));
    }};
}

pub fn debug(args: Arguments) {
    let str = format(args);
    unsafe {
        let _ = sys_debug(str.as_str());
    }
}
