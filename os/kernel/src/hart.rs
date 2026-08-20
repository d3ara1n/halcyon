//! hart 私有状态（hart 私有层，见 notes/internals.md）。
//!
//! 访问不变量：内核态运行期间 `tp ≡ 当前 hart 的 HartLocal 地址`，
//! 由 `_awaken` 设置、trap 进出与上下文切换保存/恢复（M3 起生效）。
//! HartLocal 只被所属 hart 读写，跨 hart 交互必须走全局层或 IPI。

use core::{arch::asm, sync::atomic::AtomicUsize};

/// 与链接脚本 `HART_NUM_LIMIT` 一致（rust_start 启动时校验）。
pub const HART_NUM_LIMIT: usize = 8;

/// HartLocal 的字节大小，`_awaken` 用 `slli 6` 计算 tp，两者由下方断言绑定。
pub const HART_LOCAL_SIZE: usize = 64;

const _: () = assert!(HART_LOCAL_SIZE.is_power_of_two());
const _: () = assert!(core::mem::size_of::<HartLocal>() == HART_LOCAL_SIZE);
const _: () = assert!(core::mem::align_of::<HartLocal>() == 64);

/// 单个 hart 的私有状态，占一个 cache line。
///
/// M0 仅含装配信息；调度队列、trap 上下文等随 M3 扩充。
/// 字段用原子类型以保持 `Sync`——访问纪律由 tp 不变量保证，
/// 原子性只是让类型系统不阻拦静态声明。
#[repr(C, align(64))]
pub struct HartLocal {
    hartid: AtomicUsize,
    kernel_sp: AtomicUsize,
    _reserved: [usize; 6],
}

impl HartLocal {
    const ZERO: Self = Self {
        hartid: AtomicUsize::new(usize::MAX),
        kernel_sp: AtomicUsize::new(0),
        _reserved: [0; 6],
    };

    /// hart 编号。
    pub fn hartid(&self) -> usize {
        self.hartid.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// 本 hart 的内核栈顶（M3 起 trampoline 直取，汇编侧已写入）。
    #[expect(dead_code, reason = "M3 trampoline 使用")]
    pub fn kernel_sp(&self) -> usize {
        self.kernel_sp.load(core::sync::atomic::Ordering::Relaxed)
    }
}

/// tp 指向的数组，`_awaken` 按 hartid 索引。每个元素只被所属 hart 经 tp 访问
/// （访问纪律见 notes/internals.md）；字段为原子类型，静态声明无需 unsafe。
#[unsafe(no_mangle)]
static HART_LOCALS: [HartLocal; HART_NUM_LIMIT] = [HartLocal::ZERO; HART_NUM_LIMIT];

/// 当前 hart 的私有状态。要求 tp 不变量已由 `_awaken` 建立。
#[inline]
pub fn current() -> &'static HartLocal {
    let ptr: *const HartLocal;
    unsafe { asm!("mv {}, tp", out(reg) ptr) };
    unsafe { &*ptr }
}

/// 当前 hart 编号。
#[inline]
pub fn hartid() -> usize {
    current().hartid()
}

/// 永久停放当前 hart：SIE 关闭下 wfi 等待，不可被唤醒（M0 语义）。
pub fn park() -> ! {
    loop {
        unsafe { asm!("wfi", options(nomem, preserves_flags)) };
    }
}
