//! 运行时启动：boot 契约、formal entry、启动收尾、panic 与 fatal 处理。
//!
//! boot 契约：`_start` 收到 a0 = raw boot hartid、a1 = dtb PA，经
//! rustc 生成的 `main` 包装器以 (argc, argv) 形态传入 [`rust_start`]；
//! secondary hart 不走此路径，由 HSM 经 `_awaken` PA 前导进入
//! [`hart_formal_entry`] 与 boot hart 汇合。
//!
//! 初始化顺序：SBI 探测 → 板级解析（零堆）→ 高半区切换 → 帧池 →
//! 堆 → registry/records 构造 → enter_hart（正式执行环境）→
//! bring_up_runtime（唤醒 secondary、全员 Online、bootstrap 回收、
//! 任务装载、Ready 发布）→ 调度循环。
//!
//! panic 与 fatal 路径不依赖堆与 console 锁。

use core::{
    alloc::Layout,
    fmt,
    fmt::Write,
    panic::PanicInfo,
    sync::atomic::{AtomicUsize, Ordering},
};

use erhino_shared::proc::Termination;

use crate::{context::FatalFrame, csr, external, hart, registry, sbi};

static BOOT_HARTID: AtomicUsize = AtomicUsize::new(usize::MAX);
static DTB: AtomicUsize = AtomicUsize::new(0);

/// 设备树物理地址（由 boot 契约传入）。
pub fn boot_dtb() -> usize {
    DTB.load(Ordering::Relaxed)
}

/// 固件传入的 raw boot hartid（外部边界值；内部定位一律用 slot）。
pub fn boot_hartid() -> usize {
    BOOT_HARTID.load(Ordering::Relaxed)
}

/// 绕过堆与锁的输出端，专供 panic / fatal 路径。
struct RawWriter;

impl fmt::Write for RawWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        // 诊断面必须可见：fatal/panic 不依赖 DBCN 可用性，直写 legacy。
        for b in s.bytes() {
            sbi::legacy_console_putchar(b);
        }
        Ok(())
    }
}

#[lang = "start"]
fn rust_start<T: Termination + 'static>(
    main: fn() -> T,
    hartid_as_argc: isize,
    argv: *const *const u8,
    _sigpipe: u8,
) -> isize {
    assert_eq!(
        external::hart_num_limit(),
        crate::registry::HART_NUM_LIMIT,
        "HART_NUM_LIMIT mismatch between linker script and kernel"
    );
    sbi::init();
    BOOT_HARTID.store(hartid_as_argc as usize, Ordering::Relaxed);
    DTB.store(argv as usize, Ordering::Relaxed);
    main();
    hart::park()
}

/// formal entry 的 Rust 尾段（汇编装配 gp/tp/sscratch/sp 之后调用，
/// 非返回）：CSR 基线与 WARL 核验 → 安装共同 trap vector → 发布 Online →
/// 按 record 角色分流：boot 进入启动收尾；secondary 等待 Ready 后进调度。
#[unsafe(no_mangle)]
extern "C" fn hart_formal_entry(record: &crate::registry::HartBootRecord) -> ! {
    // tp 已由汇编装配为 HartLocal；ladder 自此切换至 per-hart 帧。
    crate::sync::ladder::mark_tp_ready();
    if let Err(reject) = csr::formal_entry_baseline() {
        match reject {
            csr::CsrReject::Uxl(readback) => {
                fatal_msg(&format_args!(
                    "CSR baseline check failed: UXL readback {readback:#x} != 64"
                ));
            }
            other => fatal_msg(&format_args!("CSR baseline check failed: {other:?}")),
        }
    }
    // SAFETY: stvec 安装为共同 direct-mode 入口（地址对齐由链接保证）。
    unsafe {
        core::arch::asm!("la t0, _trap_entry", "csrw stvec, t0", options(nomem));
    }
    record.publish_online();

    if record.role_boot == 1 {
        info!(Hart, "#{:>2} online (boot)", hart::current().hartid());
        bring_up_runtime();
    }
    info!(Hart, "#{:>2} online", hart::current().hartid());
    if !registry::wait_for_runtime() {
        // 启动整体失败：晚到 hart 只能看到 Failed gate，停驻等待复位。
        hart::park()
    }
    crate::sched::run()
}

/// 启动收尾（boot hart，正式栈上运行）：发布并启动全部 secondary →
/// 等待全员 Online → 回收 bootstrap 页 → 装载初始任务 → 发布 Ready →
/// 进入调度循环。任何矛盾使本次启动整体失败（不做部分降级）。
fn bring_up_runtime() -> ! {
    let timebase = crate::sched::ticks_per_sec();
    let deadline = sbi::read_time() + 10 * timebase;

    // 先完整发布预期集合，再发出任何 HSM start（SBI-003 收口：
    // 异步启动前 expected 集合必须闭合）。
    let mut pending: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
    registry::with_registry(|reg| {
        for (slot, record) in reg.records() {
            if record.role_boot == 1 {
                continue;
            }
            record.publish_starting();
            pending.push(slot.0);
        }
        for &slot in &pending {
            let record = reg.record(crate::registry::HartSlot(slot));
            let record_pa = crate::mm::virt_to_phys(record as *const _ as usize);
            sbi::require(
                sbi::hart_start(record.hartid, external::awaken_pa(), record_pa),
                "HSM.hart_start",
            );
        }
    });

    // 等待全员 Online（Acquire 观察）；超时或状态矛盾即整体失败。
    loop {
        let all_online = registry::with_registry(|reg| {
            reg.records()
                .all(|(_, r)| r.state() == crate::registry::BootState::Online)
        });
        if all_online {
            break;
        }
        if sbi::read_time() > deadline {
            registry::publish_failed();
            fatal_msg(&format_args!(
                "secondary hart bring-up timed out; boot aborted"
            ));
        }
        core::hint::spin_loop();
    }

    // bootstrap 页回收：secondary 只依赖永久 entry 设施（过渡表/PA 前导），
    // 全员 Online 后 cold-bootstrap 区间不再被引用。回收动作在正式栈上
    // 执行，不可能从 bootstrap 栈返回。
    let (start, end) = external::bootstrap_range();
    frame::free_range(start, end);
    log!(Memory, "bootstrap reclaim [{:#x}, {:#x})", start, end);

    // 冻结 active 集合、构造调度域（按能力签名划分，此后只读）、装载唯一
    // initial process，最后发布 Ready。
    crate::sched::build_domains();
    let (addr, len) = crate::rt::boot_package_region()
        .expect("BootPackage unavailable; initial process cannot start");
    boot::load(addr, len);
    registry::publish_ready();
    crate::remote_call::selftest();
    crate::sched::run()
}

/// RISC-V 异常 cause：load/store page fault（guard 命中判定只关心这两者）。
const LOAD_PAGE_FAULT: u64 = 13;
const STORE_PAGE_FAULT: u64 = 15;

use crate::{boot, frame, mm};

/// BootPackage 实际物理区间（board 解析结果经 rt 中转）。
static BOOT_PACKAGE: AtomicUsize = AtomicUsize::new(0);
static BOOT_PACKAGE_LEN: AtomicUsize = AtomicUsize::new(0);

pub fn set_boot_package_region(addr: usize, len: usize) {
    BOOT_PACKAGE.store(addr, Ordering::Relaxed);
    BOOT_PACKAGE_LEN.store(len, Ordering::Relaxed);
}

pub fn boot_package_region() -> Option<(usize, usize)> {
    let addr = BOOT_PACKAGE.load(Ordering::Relaxed);
    (addr != 0).then(|| (addr, BOOT_PACKAGE_LEN.load(Ordering::Relaxed)))
}

/// fatal 诊断（无锁 RawWriter）：打印 FatalFrame 完整证据后永久停放。
#[unsafe(no_mangle)]
extern "C" fn handle_fatal(frame: &FatalFrame) -> ! {
    // guard 页命中是内核栈溢出的第一现场特征，单独点出便于定位。
    let hint = if matches!(frame.scause, LOAD_PAGE_FAULT | STORE_PAGE_FAULT)
        && mm::is_guard_fault(frame.stval as usize)
    {
        " (kernel stack overflow: guard page hit)"
    } else {
        ""
    };
    let _ = write!(
        RawWriter,
        "\x1b[0;31mfatal trap\x1b[0m: unexpected trap in S-mode{}\n  cause={:#x} val={:#x} pc={:#x}\n  satp={:#x} sstatus={:#x}\n",
        hint, frame.scause, frame.stval, frame.sepc, frame.satp, frame.sstatus,
    );
    let _ = write!(RawWriter, "  gpr:");
    for i in (1..32).step_by(4) {
        let _ = write!(
            RawWriter,
            " x{}={:#x} x{}={:#x} x{}={:#x} x{}={:#x}",
            i,
            frame.x[i],
            i + 1,
            frame.x[i + 1],
            i + 2,
            frame.x[i + 2],
            (i + 3).min(31),
            frame.x[(i + 3).min(31)],
        );
    }
    let _ = writeln!(RawWriter);
    hart::park()
}

/// bootstrap 阶段 fatal 的最小诊断（汇编调用，仅读 CSR，无栈依赖）。
#[unsafe(no_mangle)]
extern "C" fn bootstrap_fatal_report(cause: usize, val: usize, pc: usize) -> ! {
    let _ = write!(
        RawWriter,
        "\x1b[0;31mbootstrap fatal\x1b[0m: cause={cause:#x} val={val:#x} pc={pc:#x}\n"
    );
    hart::park()
}

/// 无 FatalFrame 的致命错误报告（启动期 CSR 拒绝等）。
pub fn fatal_msg(args: &fmt::Arguments<'_>) -> ! {
    let _ = write!(RawWriter, "\x1b[0;31mfatal\x1b[0m: {args}\n");
    hart::park()
}

#[panic_handler]
fn handle_panic(info: &PanicInfo) -> ! {
    // panic 路径绕过 console 锁与堆（见模块说明）
    if let Some(location) = info.location() {
        let _ = write!(
            RawWriter,
            "\x1b[0;31mKernel panicking #{}\x1b[0m\nin file {} at line {}: {}\n",
            hart_id_or_unknown(),
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

/// tp 不变量可用时返回 hart 编号；bootstrap/formal entry 过渡期返回 ?。
fn hart_id_or_unknown() -> &'static str {
    // panic 可能发生在 tp 尚未建立的过渡窗口；此处不读 tp，
    // 由 FatalFrame 路径提供精确现场。
    "?"
}

#[alloc_error_handler]
pub fn handle_alloc_error(layout: Layout) -> ! {
    panic!("heap allocation error, layout = {:?}", layout)
}
