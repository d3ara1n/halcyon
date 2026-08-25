//! 链接脚本与汇编提供的符号。
//!
//! 高半区代码不得直接取低段符号或 ABS 常量的地址（PC 相对寻址 ±2GiB，
//! 跨空间必溢出）——需要的值由汇编侧 `_ENTRY_CONSTS` 以 `.quad` 物化进
//! 高半区 .data（绝对重定位无范围限制），经下列访问器读取。

// 跨空间常量表（布局契约见 assembly.asm `_ENTRY_CONSTS`）。
unsafe extern "C" {
    #[link_name = "_ENTRY_CONSTS"]
    static ENTRY_CONSTS: [usize; 13];
}

/// SBI 段物理起点（帧池剔除区间用）。
pub fn sbi_start() -> usize {
    // SAFETY: 链接期物化的只读常量。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(ENTRY_CONSTS[0])) }
}

/// 链接脚本 HART_NUM_LIMIT（与内核常量启动期互校）。
pub fn hart_num_limit() -> usize {
    // SAFETY: 同上。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(ENTRY_CONSTS[2])) }
}

/// secondary hart 的 PA 入口（SBI HSM hart_start 的 start_addr 参数）。
pub fn awaken_pa() -> usize {
    // SAFETY: 同上。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(ENTRY_CONSTS[3])) }
}

/// bootstrap 可回收区间 [start, end)（PA）。
pub fn bootstrap_range() -> (usize, usize) {
    // SAFETY: 链接期物化的只读常量。
    unsafe {
        (
            core::ptr::read_volatile(core::ptr::addr_of!(ENTRY_CONSTS[4])),
            core::ptr::read_volatile(core::ptr::addr_of!(ENTRY_CONSTS[5])),
        )
    }
}

unsafe extern "C" {
    /// 高半区 formal entry 汇合点（高半区内符号，可直接取地址）。
    fn _enter_hart_high();
}

/// 高半区 formal entry 的 VMA。
pub fn enter_hart_high_va() -> usize {
    _enter_hart_high as *const () as usize
}

/// 栈窗口基（VMA）：正式内核栈的专用虚拟分区，与直映射解耦；
/// 布局与 guard 见 `os/stack_layout`（单一几何真值）。
pub fn stack_window_base() -> usize {
    // SAFETY: 链接期物化的只读常量。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(ENTRY_CONSTS[9])) }
}

/// 内核静态占用物理末端（SBI + 镜像 + 栈），帧池注册剔除区间用。
pub fn kernel_pa_end() -> usize {
    // SAFETY: 链接期物化的只读常量。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(ENTRY_CONSTS[10])) }
}

/// 每 hart 栈大小。
pub fn hart_stack_size() -> usize {
    // SAFETY: 链接期物化的只读常量。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(ENTRY_CONSTS[1])) }
}

/// guard 洞跨度（≥ 构建审计允许的单函数最大帧；数字真值在链接脚本）。
pub fn stack_guard() -> usize {
    // SAFETY: 同上。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(ENTRY_CONSTS[11])) }
}

/// emergency 栈大小（占槽顶，fatal 路径专用）。
pub fn emergency_size() -> usize {
    // SAFETY: 同上。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(ENTRY_CONSTS[12])) }
}
