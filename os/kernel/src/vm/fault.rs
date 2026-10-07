//! U态 page-fault：按事务承诺判权限，等待现有完成来源或修复本地翻译。

use core::sync::atomic::Ordering;

use memory_space::{FaultClass, Protection};
use page_table::{Vpn, flags};

use crate::{
    context::FaultRepair,
    task::{Thread, wait::WaitPlan},
};

pub(crate) enum Action {
    Terminate,
    Retry,
    Wait(WaitPlan),
}

/// 不持access/结果租约跨等待。
///
/// # Safety
/// 调用者是该Thread唯一执行owner，现场来自当前U态page-fault。
pub(crate) unsafe fn handle(thread: &Thread, pc: u64, cause: usize, address: usize) -> Action {
    let (access, flag) = match cause {
        12 => (Protection::ReadExecute, flags::X),
        13 => (Protection::ReadOnly, flags::R),
        15 => (Protection::ReadWrite, flags::W),
        _ => return Action::Terminate,
    };
    let Some(body) = thread.process.lease_body() else {
        return Action::Terminate;
    };
    let mut vm = body.vm.space.lock();
    let Some(space) = vm.space.as_mut() else {
        return Action::Terminate;
    };
    let (target, change) = match space.ledger().fault_class(address) {
        FaultClass::Publishing { change, target } => (target, Some(change)),
        FaultClass::Mapping { protection, .. } => (Some(protection), None),
        _ => return Action::Terminate,
    };
    if !target.is_some_and(|protection| protection.permits(access)) {
        return Action::Terminate;
    }
    if let Some(translation) = space.translate(Vpn(address / memory_space::PAGE_SIZE))
        && translation.flags & (flags::U | flag) == flags::U | flag
    {
        // Kernel pte_flags预置A，对可写用户页预置D；SFENCE不是A/D修复通道。
        let ad = flags::A
            | if access == Protection::ReadWrite {
                flags::D
            } else {
                0
            };
        assert!(
            translation.flags & ad == ad,
            "user translation lacks preset access bits"
        );
        let repair = FaultRepair {
            pc,
            address,
            cause,
            translation,
            epoch: body.vm.translation_epoch.load(Ordering::Acquire),
            change,
        };
        // SAFETY: 当前执行owner独占Thread的现场与重试续体；无引用跨等待保存。
        if !unsafe { thread.record_fault_repair(repair) } {
            return Action::Terminate;
        }
        // SAFETY: 空间锁保持软件PTE稳定，当前S态同步本hart翻译；不持锁循环。
        // 默认memory effect排除编译器重排，X的指令流另作本地同步。
        unsafe {
            core::arch::asm!("sfence.vma", options(nostack));
            if access == Protection::ReadExecute {
                core::arch::asm!("fence.i", options(nostack));
            }
        }
        Action::Retry
    } else if let Some(change) = change {
        let source = vm
            .dependencies
            .iter()
            .find(|(key, _)| *key == change)
            .map(|(_, source)| source.clone())
            .expect("publishing change lost its fault dependency");
        // SAFETY: 同上；真实依赖推进后允许重新判定，而不是持锁忙重试。
        unsafe { thread.clear_fault_repair() };
        // 返回前Vm锁/Body短loan已析构。install沿原epoch attach→probe→park握手，
        // DONE先到也会复检命中；Frozen会detach离场而不等待事务完成。
        Action::Wait(WaitPlan::retry_fault(source, cause, address))
    } else {
        Action::Terminate
    }
}
