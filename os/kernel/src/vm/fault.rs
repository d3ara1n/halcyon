//! 用户页 fault 分类（内存分案设计轮次 3，Q3-c）。
//!
//! 先按访问类型（R/W/X）对照账本承诺的权限判定，再区分“尚未发布”与
//! “已发布但本 hart 翻译陈旧”。绝不在内核持锁循环 sfence。

use memory_space::{FaultClass, Protection};

/// 触发 fault 的访问类型。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Access {
    Read,
    Write,
    Execute,
}

/// fault 处置。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Disposition {
    /// 权限不允许：按用户 fault 终止/投递。
    Deliver,
    /// 终态允许但 PTE 尚未发布：锁外让出后重试（经正常调度轮转）。
    Yield,
    /// 已发布、本 hart 翻译陈旧：本地 sfence 后重试。
    Refetch,
}

fn allows(protection: Protection, access: Access) -> bool {
    match access {
        Access::Read => true,
        Access::Write => protection == Protection::ReadWrite,
        Access::Execute => protection == Protection::ReadExecute,
    }
}

/// 空间锁内调用：只读账本，返回处置；调用者解锁后执行。
pub(crate) fn classify(class: FaultClass, access: Access) -> Disposition {
    match class {
        FaultClass::Free | FaultClass::Guard => Disposition::Deliver,
        FaultClass::Mapping { protection } if allows(protection, access) => Disposition::Refetch,
        FaultClass::Mapping { .. } => Disposition::Deliver,
        FaultClass::Publishing {
            target: Some(target),
        } if allows(target, access) => Disposition::Yield,
        FaultClass::Publishing { .. } => Disposition::Deliver,
    }
}

/// 解锁后执行 Refetch：本地全量 sfence（ASID 恒 0）。
pub(crate) fn refetch_local() {
    // SAFETY: 只失效本 hart 翻译缓存，不访问内存。
    unsafe { core::arch::asm!("sfence.vma", options(nostack, preserves_flags)) };
}
