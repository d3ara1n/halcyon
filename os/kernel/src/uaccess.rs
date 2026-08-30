//! 用户内存边界：内核触碰用户 VA 的唯一通道（notes/impls/internals.md「地址空间」）。
//!
//! 职责集中于此，任何 syscall 不得自行拼装校验 + SUM + 裸指针：
//! - 区间合法：不溢出、不出用户半区、单次不超限长；
//! - 逐页权限：U 必备，读需 R、写需 W（此前只验「有映射」，读 X-only 页
//!   会在 S 态 fault 的缺口由此闭合）；
//! - SUM guard 收编，调用方无感知。
//!
//! 复检语义：每次访问在当次持有的 space 锁内重新校验（check 与拷贝
//! 同一临界区），不存在 TOCTOU 窗口；多线程下同进程线程可在两次
//! space 锁之间拆除输出页——复检失败不是编程错误，syscall 输出交付
//! 统一走 [`deliver_output`]（冻结 store-access 终因并杀调用进程）。

use crate::mm::SumGuard;
use crate::task::proc::{AddressSpaceState, PAGE_SIZE};
use crate::task::Thread;

/// 单次访问上限（防恶意长度；Debug 消息与初期 IPC 载荷远小于此）。
pub const MAX_USER_ACCESS: usize = 1 << 20;

/// 用户内存访问失败的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessError {
    /// 区间溢出 / 越出用户半区 / 超过限长。
    BadRange,
    /// 区间内有页未映射。
    NotMapped,
    /// 权限不足（缺 U，或方向位不符）。
    Permission,
}

impl From<AccessError> for erhino_shared::call::SystemCallError {
    fn from(e: AccessError) -> Self {
        use erhino_shared::call::SystemCallError;
        match e {
            AccessError::BadRange | AccessError::NotMapped | AccessError::Permission => {
                SystemCallError::MemoryNotAccessible
            }
        }
    }
}

/// 从用户内存拷入内核（src 为用户 VA，dst 为内核缓冲）。
pub fn copy_from_user(
    space: &mut AddressSpaceState,
    dst: &mut [u8],
    src: usize,
) -> Result<(), AccessError> {
    if dst.len() > MAX_USER_ACCESS {
        return Err(AccessError::BadRange);
    }
    space.check_range(src, dst.len(), false)?;
    // SAFETY: 区间已逐页校验（U+R、用户半区内、未超限）；持 space 锁期间
    // 无并发映射变更，guard 存活期不重入调度。
    let _sum = unsafe { SumGuard::open() };
    // SAFETY: 同上；src..src+len 可解引用且目标缓冲独占。
    unsafe {
        core::ptr::copy_nonoverlapping(src as *const u8, dst.as_mut_ptr(), dst.len());
    }
    Ok(())
}

/// 读取一个无 padding 且任意位型均有效的定宽 ABI 值。
///
/// # Safety
/// `T` 必须是纯整数/整数 newtype 组成的 ABI 类型；任何字节组合都必须有效。
pub unsafe fn read_user_value<T: Copy>(
    space: &mut AddressSpaceState,
    src: usize,
) -> Result<T, AccessError> {
    let mut value = core::mem::MaybeUninit::<T>::uninit();
    // SAFETY: MaybeUninit 的整段存储可作为待写字节；成功后全部字节已初始化。
    let bytes = unsafe {
        core::slice::from_raw_parts_mut(value.as_mut_ptr().cast::<u8>(), core::mem::size_of::<T>())
    };
    copy_from_user(space, bytes, src)?;
    // SAFETY: 调用方保证任意位型对 T 有效，且 copy_from_user 已写满全部字节。
    Ok(unsafe { value.assume_init() })
}

/// 写出一个不含未初始化 padding 的定宽 ABI 值。
///
/// # Safety
/// `T` 的对象表示不得包含 padding 或其它未初始化字节。
pub unsafe fn write_user_value<T: Copy>(
    space: &mut AddressSpaceState,
    dst: usize,
    value: &T,
) -> Result<(), AccessError> {
    // SAFETY: 调用方保证 T 的完整对象表示均已初始化。
    let bytes = unsafe {
        core::slice::from_raw_parts((value as *const T).cast::<u8>(), core::mem::size_of::<T>())
    };
    copy_to_user(space, dst, bytes)
}

/// 交付 syscall 输出：写回当次复检，失败即冻结 (Fault, StoreAccess)
/// 终止调用进程。
///
/// 复检失败唯一成因是同进程线程在两次 space 锁之间拆除了输出页
/// （HandleClose → unmap_external）——等价于一次由内核代为检出的
/// store access fault：用户可触发的 fault 杀进程，绝不 panic 内核；
/// 副作用已发生的歧义由进程死亡清理兑底。调用方把 Err 向上传播
/// 即可，分发出口的终止检查会把 Completed 改写为 Killed，线程不
/// 回用户态。
///
/// # Safety
/// `T` 的对象表示不得包含 padding 或其它未初始化字节（同
/// [`write_user_value`]）。
pub unsafe fn deliver_output<T: Copy>(
    thread: &Thread,
    space: &mut AddressSpaceState,
    dst: usize,
    value: &T,
) -> Result<(), erhino_shared::call::SystemCallError> {
    // SAFETY: 调用方保证 T 的完整对象表示均已初始化。
    unsafe { write_user_value(space, dst, value) }.map_err(|error| {
        let process = thread.process.clone();
        let todo = process.lifecycle.request_termination(
            erhino_shared::proc::ProcessExitReason::Fault,
            erhino_shared::proc::ProcessFaultCode::StoreAccess as i64,
            Some(thread.tid),
        );
        crate::task::process::run_termination_todo(&process, todo);
        error.into()
    })
}

/// 从内核拷入用户内存（dst 为用户 VA，src 为内核缓冲）。
pub fn copy_to_user(
    space: &mut AddressSpaceState,
    dst: usize,
    src: &[u8],
) -> Result<(), AccessError> {
    if src.len() > MAX_USER_ACCESS {
        return Err(AccessError::BadRange);
    }
    space.check_range(dst, src.len(), true)?;
    // SAFETY: 区间已逐页校验（U+W）；其余同 copy_from_user。
    let _sum = unsafe { SumGuard::open() };
    // SAFETY: 同上。
    unsafe {
        core::ptr::copy_nonoverlapping(src.as_ptr(), dst as *mut u8, src.len());
    }
    Ok(())
}

/// 跨地址空间拷入用户缓冲：完成方上下文目标空间**未激活**（调度循环/
/// 异核唤醒路径，SUM 直访不可用），逐页 translate 后经直映射写入。
/// 校验与拷贝同持 `space` 锁，无 TOCTOU；页间边界由逐页循环消除。
pub fn put_user_indirect(
    space: &mut AddressSpaceState,
    dst: usize,
    src: &[u8],
) -> Result<(), AccessError> {
    if src.len() > MAX_USER_ACCESS {
        return Err(AccessError::BadRange);
    }
    space.check_range(dst, src.len(), true)?;
    let mut off = 0;
    while off < src.len() {
        let va = dst + off;
        let in_page = va % PAGE_SIZE;
        let n = (PAGE_SIZE - in_page).min(src.len() - off);
        let pa = space.page_pa(va).ok_or(AccessError::NotMapped)?;
        // SAFETY: 页已校验含 U|W；pa 来自该页 translate，直映射区间
        // [pa+in_page, pa+in_page+n) 不出页且与 src 不重叠。
        unsafe {
            core::ptr::copy_nonoverlapping(
                src[off..].as_ptr(),
                crate::mm::phys_to_virt(pa + in_page) as *mut u8,
                n,
            );
        }
        off += n;
    }
    Ok(())
}
