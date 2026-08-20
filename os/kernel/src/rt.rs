//! 运行时启动：boot 契约、初始化顺序、panic 处理。
//!
//! boot 契约（考古报告 §1）：`_start` 收到 a0 = hartid、a1 = dtb，把
//! (hartid, dtb) 伪装成 (argc, argv) 经 rustc 生成的 `main` 包装器传入
//! `#[lang = "start"]`；secondary hart 不走此路径，由 HSM 直接进入
//! [`secondary_entry`]。
//!
//! 初始化顺序：SBI 探测 → `main`（内核主体，含堆初始化）→ 停放。
//!
//! panic 与 trap 路径不依赖堆与 console 锁。

use core::{
    alloc::Layout,
    fmt,
    fmt::Write,
    panic::PanicInfo,
    sync::atomic::{AtomicUsize, Ordering},
};

use erhino_shared::proc::Termination;

use crate::{console::console_write_raw, external, hart, println, sbi};

static DTB: AtomicUsize = AtomicUsize::new(0);

/// 设备树物理地址（由 boot 契约传入）。
pub fn dtb() -> usize {
    DTB.load(Ordering::Relaxed)
}

/// 绕过堆与锁的输出端，专供 panic / trap 路径。
struct RawWriter;

impl fmt::Write for RawWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        console_write_raw(s);
        Ok(())
    }
}

#[lang = "start"]
fn rust_start<T: Termination + 'static>(
    main: fn() -> T,
    _hartid_as_argc: isize,
    argv: *const *const u8,
    _sigpipe: u8,
) -> isize {
    assert_eq!(
        external::hart_num_limit(),
        hart::HART_NUM_LIMIT,
        "链接脚本 HART_NUM_LIMIT 与 hart::HART_NUM_LIMIT 不一致"
    );
    sbi::init();
    DTB.store(argv as usize, Ordering::Relaxed);
    main();
    hart::park()
}

/// secondary hart 入口（HSM opaque 传入），dtb 参数无效。
#[unsafe(no_mangle)]
pub extern "C" fn secondary_entry(_hartid: usize, _dtb: usize) -> ! {
    println!("[Hart #{:>2}] online", hart::hartid());
    hart::park()
}

/// 内核态 trap 的兜底：协作式内核中 trap 即致命。
#[unsafe(no_mangle)]
pub extern "C" fn handle_kernel_trap(cause: usize, val: usize, pc: usize) -> ! {
    let _ = write!(
        RawWriter,
        "\x1b[0;31mkernel trap\x1b[0m: unexpected trap in S-mode\n  cause={:#x} val={:#x} pc={:#x} hart={}\n",
        cause,
        val,
        pc,
        hart::hartid(),
    );
    hart::park()
}

#[panic_handler]
fn handle_panic(info: &PanicInfo) -> ! {
    // panic 路径绕过 console 锁与堆（见模块说明）
    if let Some(location) = info.location() {
        let _ = write!(
            RawWriter,
            "\x1b[0;31mKernel panicking #{}\x1b[0m\nin file {} at line {}: {}\n",
            hart::hartid(),
            location.file(),
            location.line(),
            info.message(),
        );
    } else {
        let _ = write!(
            RawWriter,
            "\x1b[0;31mKernel panicking\x1b[0m: no information available.\n"
        );
    }
    hart::park()
}

#[alloc_error_handler]
pub fn handle_alloc_error(layout: Layout) -> ! {
    panic!("heap allocation error, layout = {:?}", layout)
}
