//! 链接脚本与汇编提供的符号。
//!
//! 这些符号以函数声明形式引入，取地址（`as usize`）即得到链接期数值：
//! 链接脚本 `PROVIDE` 的绝对值符号（如 `_stack_size`）得到常量本身，
//! 段边界符号（如 `_kernel_end`）得到地址。

unsafe extern "C" {
    pub fn _memory_start();
    pub fn _kernel_end();
    pub fn _frame_start();
    pub fn _memory_end();
    pub fn _stack_size();
    pub fn _awaken();
    #[link_name = "HART_NUM_LIMIT"]
    pub fn hart_num_limit();
}
