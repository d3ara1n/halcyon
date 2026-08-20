//! 运行时启动：boot 契约、初始化顺序、panic 处理。
//!
//! boot 契约（考古报告 §1）：`_start` 收到 a0 = hartid、a1 = dtb，把
//! (hartid, dtb) 伪装成 (argc, argv) 经 rustc 生成的 `main` 包装器传入
//! `#[lang = "start"]`；secondary hart 不走此路径，由 HSM 直接进入
//! [`secondary_entry`]。
//!
//! 初始化顺序：SBI 探测 → 板级解析（零堆）→ 高半区切换 → 帧池 →
//! 堆（首次分配时从帧池取块）→ 内核主体 → 嚇醒 secondary → 停放。
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
    crate::sched::run()
}

/// 内核态 trap 的兜底：协作式内核中 trap 即致命。
#[unsafe(no_mangle)]
pub extern "C" fn handle_kernel_trap(cause: usize, val: usize, pc: usize) -> ! {
    // 现场采集（无锁 RawWriter）：satp/sstatus + 若为访存 fault，
    // 按当前 satp 走三级页表定位断点。
    let (satp, sstatus): (usize, usize);
    // SAFETY: 只读 CSR。
    unsafe {
        core::arch::asm!("csrr {}, satp", out(reg) satp, options(nomem));
        core::arch::asm!("csrr {}, sstatus", out(reg) sstatus, options(nomem));
    }
    let _ = write!(
        RawWriter,
        "\x1b[0;31mkernel trap\x1b[0m: unexpected trap in S-mode\n  cause={:#x} val={:#x} pc={:#x} hart={} satp={:#x} sstatus={:#x}\n",
        cause,
        val,
        pc,
        hart::hartid(),
        satp,
        sstatus,
    );
    if matches!(cause, 12 | 13 | 15) && satp >> 60 == 8 {
        dump_page_walk(satp & 0xFFF_FFFF_FFFF, val);
    }
    hart::park()
}

/// 按 root PPN 走 Sv39 三级表，打印 stval 的叶 PTE（诊断页故障用）。
fn dump_page_walk(root_ppn: usize, va: usize) {
    let vpn = [va >> 30 & 0x1FF, va >> 21 & 0x1FF, va >> 12 & 0x1FF];
    let mut table = root_ppn << 12;
    let _ = write!(RawWriter, "  walk va={:#x}:", va);
    for (level, &idx) in vpn.iter().enumerate() {
        // SAFETY: 表帧位于直映射覆盖的 DRAM 内（帧池分配约束）。
        let pte = unsafe { *((crate::mm::phys_to_virt(table) + idx * 8) as *const u64) };
        let _ = write!(RawWriter, " L{}[{}]={:#x}", 2 - level, idx, pte);
        if pte & 1 == 0 {
            break;
        }
        if pte & 0xE != 0 || level == 2 {
            let _ = write!(RawWriter, " <- leaf");
            break;
        }
        table = ((pte >> 10 & 0xFFF_FFFF_FFFF) << 12) as usize;
    }
    let _ = writeln!(RawWriter);
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
