//! 用户内存边界：内核触碰用户 VA 的唯一通道（notes/impls/internals.md「地址空间」）。
//!
//! 职责集中于此，任何 syscall 不得自行拼装校验 + SUM + 裸指针：
//! - 区间合法：不溢出、不出用户半区、单次不超限长；
//! - 逐页权限：U 必备，读需 R、写需 W（此前只验「有映射」，读 X-only 页
//!   会在 S 态 fault 的缺口由此闭合）；
//! - SUM guard 收编，调用方无感知。
//!
//! TOCTOU 不成立的前提由协作式内核背书：校验与拷贝之间持有
//! `Process.space` 锁，同进程无并发映射变更者。

use crate::mm::SumGuard;
use crate::task::proc::{AddressSpace, PAGE_SIZE};

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
    space: &mut AddressSpace,
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

/// 从内核拷入用户内存（dst 为用户 VA，src 为内核缓冲）。
pub fn copy_to_user(
    space: &mut AddressSpace,
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
    space: &mut AddressSpace,
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
