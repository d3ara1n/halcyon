//! debug 输出：向内核调试流发消息（测试观测通道）。无颜色无等级——
//! 内核侧统一以 `[pid N]` 话题、debug 等级色印出；要自定义格式/颜色
//! 自己拼进消息字符串。正式的用户态输出是未来的 console 服务。
//!
//! 零分配是硬性要求：panic/OOM 路径必须仍可观测——若在此 format! 到堆，
//! 分配失败引发的 panic 会对着自己持有的分配器锁自旋死锁（sifive_u
//! 卡死事故的教训，见 plans/2026-09-pre-ipc-groundwork.md）。

use core::fmt::{self, Arguments, Write};

use crate::call::sys_debug;

/// 栈上消息缓冲上限；超长消息截断（best-effort 通道，允许丢失）。
const DEBUG_BUF: usize = 512;

#[macro_export]
macro_rules! debug {
    ($($arg:tt)*) => {{
        $crate::dbg::debug(format_args!($($arg)*));
    }};
}

pub fn debug(args: Arguments) {
    let mut buf = [0u8; DEBUG_BUF];
    let mut w = SliceWriter { buf: &mut buf, len: 0 };
    // Arguments 实现 Display；写入失败只发生在截断，静默收尾。
    let _ = write!(w, "{}", args);
    unsafe {
        let _ = sys_debug(w.as_slice());
    }
}

/// 写入固定栈缓冲的 fmt::Write 实现，不触碰分配器。
struct SliceWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
}

impl SliceWriter<'_> {
    fn as_slice(&self) -> &str {
        // SAFETY: write_str 只写入完整 UTF-8 片段，边界对齐字符边界。
        unsafe { core::str::from_utf8_unchecked(&self.buf[..self.len]) }
    }
}

impl Write for SliceWriter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let rem = &mut self.buf[self.len..];
        if s.len() > rem.len() {
            // 截断到能容纳的前缀（按字节；UTF-8 边界由调用方消息质量决定，
            // 内核侧非 UTF-8 已有兜底显示）。
            let n = rem.len();
            rem.copy_from_slice(&s.as_bytes()[..n]);
            self.len += n;
            return Err(fmt::Error);
        }
        rem[..s.len()].copy_from_slice(s.as_bytes());
        self.len += s.len();
        Ok(())
    }
}
