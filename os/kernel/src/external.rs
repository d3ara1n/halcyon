//! 链接脚本与汇编提供的符号。
//!
//! 高半区代码不得直接取低段符号或 ABS 常量的地址（PC 相对寻址 ±2GiB，
//! 跨空间必溢出）——需要的值由汇编侧 `_PA_CONSTS` 以 `.quad` 物化进
//! 高半区 .data（绝对重定位无范围限制），经下列访问器读取。

// 链接期常量表（布局契约见 assembly.asm `_PA_CONSTS`）。
unsafe extern "C" {
    #[link_name = "_PA_CONSTS"]
    static PA_CONSTS: [usize; 4];
}

/// SBI 段物理起点（帧池剔除区间用）。
pub fn sbi_start() -> usize {
    // SAFETY: 链接期物化的只读常量。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(PA_CONSTS[0])) }
}

/// 链接脚本 HART_NUM_LIMIT（与 hart::HART_NUM_LIMIT 启动期互校）。
pub fn hart_num_limit() -> usize {
    // SAFETY: 同上。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(PA_CONSTS[2])) }
}

/// secondary hart 的 PA 入口（SBI HSM hart_start 的 start_addr 参数）。
pub fn awaken_pa() -> usize {
    // SAFETY: 同上。
    unsafe { core::ptr::read_volatile(core::ptr::addr_of!(PA_CONSTS[3])) }
}

// 内核镜像末端（高半区 VMA，含栈区；换算 PA 用 `mm::virt_to_phys`）。
unsafe extern "C" {
    pub fn _kernel_end();
}
