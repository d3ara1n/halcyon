//! hart 私有状态（hart 私有层，见 notes/internals.md）。
//!
//! 访问不变量：内核态运行期间 `tp ≡ 当前 hart 的 HartLocal 地址`，
//! 由 `_enter_hart_high`（两条启动路径的汇合点）设置、trap 路径维护。
//! HartLocal 只被所属 hart 读写，跨 hart 交互必须走全局层或 IPI。
//!
//! HartLocal 同时是 trap 锚：用户态 sscratch 恒指本结构，trap 进出
//! 经它定位当前 TrapFrame 与调度栈（见 assembly.asm `_user_trap`）。

use core::{arch::asm, sync::atomic::AtomicUsize};

#[allow(clippy::declare_interior_mutable_const)]
const ATOMIC_ZERO: AtomicUsize = AtomicUsize::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const ATOMIC_MAX: AtomicUsize = AtomicUsize::new(usize::MAX);

/// 与链接脚本 `HART_NUM_LIMIT` 一致（rust_start 启动时校验）。
pub const HART_NUM_LIMIT: usize = 8;

/// HartLocal 的字节大小：两 cache line，16 槽。`HART_SETUP` 宏用
/// `slli 7` 计算 tp，两者由下方断言绑定。
pub const HART_LOCAL_SIZE: usize = 128;

const _: () = assert!(HART_LOCAL_SIZE.is_power_of_two());
const _: () = assert!(core::mem::size_of::<HartLocal>() == HART_LOCAL_SIZE);
const _: () = assert!(core::mem::align_of::<HartLocal>() == 64);

/// 槽位偏移（字节）。汇编侧经 abi::asm 的 `offset_of!` 注入访问，
/// 本表是唯一真值。
#[allow(dead_code)]
pub mod off {
    pub const HARTID: usize = 0;
    pub const KERNEL_SP: usize = 8;
    pub const SCHED_SP: usize = 16;
    pub const FRAME_PTR: usize = 24;
    pub const USER_SATP: usize = 32;
    pub const TRAP_SCRATCH: usize = 40;
    pub const CURRENT_THREAD: usize = 48;
    /// 稠密内核身份（registry 分配；raw hartid 不兼任内部索引）。
    pub const SLOT: usize = 56;
    /// per-hart emergency 栈顶（fatal 路径切入）。
    pub const EMERGENCY_SP: usize = 64;
    /// fatal 递归 guard（usize::MAX = 无 fatal；首帧建立后置 1）。
    pub const FATAL_GUARD: usize = 72;
    /// trap 进入序列的第二暂存槽（用户 t6 中转）。
    pub const TRAP_SCRATCH2: usize = 80;
    /// 当前线程 FP 使能档位（D64=1/Base=0；调度循环装执行点时写入）。
    pub const FP_ENABLED: usize = 88;
    /// fatal 路径暂借槽（原 S 态 sp 中转；仅 fatal 入口序列使用）。
    pub const FATAL_SP: usize = 96;
    /// per-hart LR/SC reservation 清除槽（dummy SC 目标）。
    pub const RESERVATION: usize = 104;
    /// 等待意图槽：dispatcher 写入（syscall::dispatch），调度循环在
    /// clear_context 后的 Park 分支消费发布（sched::park_publish）。
    /// 发布严格晚于线程离开一切 hart 引用，闭合双容器竞态窗口。
    pub const PARK_KIND: usize = 112;
    /// 等待意图参数（如 sleep 的毫秒数）。
    pub const PARK_ARG: usize = 120;
}

/// 单个 hart 的私有状态（trap 锚 + 执行点），占一个 cache line。
///
/// 字段用原子类型以保持 `Sync`——访问纪律由 tp 不变量保证，
/// 原子性只是让类型系统不阻拦静态声明。
#[repr(C, align(64))]
pub struct HartLocal {
    hartid: AtomicUsize,
    /// 内核栈顶（boot 期装配；调度循环启动前的兜底栈锚）。
    kernel_sp: AtomicUsize,
    /// 调度循环保存点：`_ret_to_user` 压栈现场后写入，`_user_trap`
    /// 的 Switch 路径由此恢复调度循环，handler 栈从此向下生长。
    sched_sp: AtomicUsize,
    /// 当前线程的 TrapFrame 指针（执行点状态）。
    frame_ptr: AtomicUsize,
    /// 当前线程的用户 satp。
    user_satp: AtomicUsize,
    /// trap 进入序列的暂存槽（用户 t5 中转，见 `_user_trap`）。
    trap_scratch: AtomicUsize,
    /// 当前线程指针（执行点状态）。调度循环持有 Arc 时非空；
    /// trap handler 经此取线程上下文（pid 等），不做引用计数操作。
    current_thread: AtomicUsize,
    /// 稠密身份（registry 分配；raw hartid 不兼任内部索引）。
    slot: AtomicUsize,
    /// fatal 路径的 emergency 栈顶（formal entry 装配）。
    emergency_sp: AtomicUsize,
    /// fatal 递归 guard：usize::MAX = 无 fatal；首个 fatal 建立首帧后置 1，
    /// 再入者不得覆盖首帧、直接进入无栈停驻。
    fatal_guard: AtomicUsize,
    /// trap 进入序列的第二暂存槽。
    trap_scratch2: AtomicUsize,
    /// 当前线程 FP 使能档位。
    fp_enabled: AtomicUsize,
    /// fatal 入口序列的原 sp 中转。
    fatal_sp: AtomicUsize,
    /// dummy SC 目标（reservation 清除）。
    reservation: AtomicUsize,
    /// 等待意图类别（0 = 无；语义见 sched::PARK_*）。
    pub(crate) park_kind: AtomicUsize,
    /// 等待意图参数。
    pub(crate) park_arg: AtomicUsize,
}

impl HartLocal {
    const ZERO: Self = Self {
        hartid: AtomicUsize::new(usize::MAX),
        kernel_sp: AtomicUsize::new(0),
        sched_sp: AtomicUsize::new(0),
        frame_ptr: AtomicUsize::new(0),
        user_satp: AtomicUsize::new(0),
        trap_scratch: AtomicUsize::new(0),
        current_thread: AtomicUsize::new(0),
        slot: ATOMIC_MAX,
        emergency_sp: ATOMIC_ZERO,
        fatal_guard: ATOMIC_MAX,
        trap_scratch2: ATOMIC_ZERO,
        fp_enabled: ATOMIC_ZERO,
        fatal_sp: ATOMIC_ZERO,
        reservation: ATOMIC_ZERO,
        park_kind: ATOMIC_ZERO,
        park_arg: ATOMIC_ZERO,
    };

    /// hart 编号。
    pub fn hartid(&self) -> usize {
        self.hartid.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// 稠密内核身份（内部位图/索引一律用它，见 registry.rs 模块注释）。
    pub fn slot(&self) -> usize {
        self.slot.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// 本 hart 的内核栈顶（boot 期装配；调度循环启动前的兑底栈锚）。
    #[expect(dead_code, reason = "boot 早期与调试转储用")]
    pub fn kernel_sp(&self) -> usize {
        self.kernel_sp.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// 当前线程的 UserContext 指针；无当前线程时为 0（汇编与断言使用）。
    #[expect(dead_code, reason = "汇编经槽位直接访问")]
    pub fn frame_ptr(&self) -> *mut crate::context::UserContext {
        self.frame_ptr.load(core::sync::atomic::Ordering::Relaxed) as *mut _
    }

    /// 当前线程；调度循环运行期间非空（循环持有 Arc 保证存活）。
    pub fn current_thread(&self) -> Option<&'static crate::task::Thread> {
        let p = self.current_thread.load(core::sync::atomic::Ordering::Relaxed) as *const crate::task::Thread;
        if p.is_null() {
            None
        } else {
            // SAFETY: 执行点槽仅在调度循环置位/清位之间有效，调用者均处于
            // 该区间的本 hart 执行流上（trap handler / 调度循环）。
            Some(unsafe { &*p })
        }
    }

    /// 设置执行点（进入用户态前由调度循环写入）；fp_enabled 决定
    /// pre-sret 边界的 FS 档位（D64 完整恢复 / Base 恒 Off）。
    pub fn set_context(
        &self,
        frame: *mut crate::context::UserContext,
        user_satp: usize,
        thread: *const crate::task::Thread,
        fp_enabled: bool,
    ) {
        self.frame_ptr
            .store(frame as usize, core::sync::atomic::Ordering::Relaxed);
        self.user_satp
            .store(user_satp, core::sync::atomic::Ordering::Relaxed);
        self.current_thread
            .store(thread as usize, core::sync::atomic::Ordering::Relaxed);
        self.fp_enabled
            .store(fp_enabled as usize, core::sync::atomic::Ordering::Relaxed);
    }

    /// 清执行点（线程离开本 hart 后调用，杜绝悬挂指针）。
    pub fn clear_context(&self) {
        self.frame_ptr
            .store(0, core::sync::atomic::Ordering::Relaxed);
        self.user_satp
            .store(0, core::sync::atomic::Ordering::Relaxed);
        self.current_thread.store(0, core::sync::atomic::Ordering::Relaxed);
        self.fp_enabled.store(0, core::sync::atomic::Ordering::Relaxed);
    }
}

/// tp 指向的数组，`HART_SETUP` 按 hartid 索引。每个元素只被所属 hart 经 tp 访问
/// （访问纪律见 notes/internals.md）；字段为原子类型，静态声明无需 unsafe。
#[unsafe(no_mangle)]
static HART_LOCALS: [HartLocal; HART_NUM_LIMIT] = [HartLocal::ZERO; HART_NUM_LIMIT];

/// 按 slot 取 HartLocal 静态槽地址（registry 构造 record 用；运行期定位
/// 一律经 tp 不变量）。
pub fn hart_local_addr(slot: usize) -> usize {
    core::ptr::addr_of!(HART_LOCALS[slot]) as usize
}

/// 当前 hart 的私有状态。要求 tp 不变量已由启动汇编建立。
#[inline]
pub fn current() -> &'static HartLocal {
    let ptr: *const HartLocal;
    // SAFETY: tp 即 HartLocal 地址（启动汇编维护的不变量）。
    unsafe { asm!("mv {}, tp", out(reg) ptr) };
    // SAFETY: 指向静态数组内本 hart 的槽位，生命周期 'static。
    unsafe { &*ptr }
}

/// 永久停放当前 hart：SIE 关闭下 wfi 等待。致命错误的终态；
/// 调度循环的 idle 不走这里（见 sched.rs）。
pub fn park() -> ! {
    loop {
        // SAFETY: wfi 无副作用，仅等待中断 pending 唤醒。
        unsafe { asm!("wfi", options(nomem, preserves_flags)) };
    }
}
