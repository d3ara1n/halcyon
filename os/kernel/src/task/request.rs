//! 原线程持有的可让出请求。准备每轮推进一次，提交后转交独立 Work 的完成等待。
//! 没有后台用户指针写入；冻结退出在 Departure 前析构准备 owner。

use super::{
    Thread,
    memory_pool::MemoryPool,
    resources::{MetadataCharge, MetadataFunding, map_storage_error},
    wait::WaitPlan,
};
use crate::sched::AdmittedThread;
use alloc::{boxed::Box, sync::Arc};
use erhino_shared::call::SystemCallError;
use funded_storage::Funding;

pub(crate) enum Step {
    Pending,
    Complete(usize),
    Wait(WaitPlan),
}

/// advance 只由原线程的独占执行资格调用；一个调用必须有结构性工作上界。
/// 返回 Pending 后保留全部准备。Drop 须归还准备或移交已预付的失败 Work。
/// Wait 表示不可逆提交已由独立 owner 接管，不允许靠后续 advance 继续提交。
pub(crate) trait Request: Send {
    fn advance(&mut self, thread: &Thread) -> Result<Step, SystemCallError>;
}

pub(crate) struct Pending {
    value: Box<dyn Request>,
    _metadata: MetadataCharge,
}
impl Pending {
    pub(crate) fn new<T: Request + 'static>(
        value: T,
        pool: &Arc<MemoryPool>,
    ) -> Result<Self, SystemCallError> {
        let metadata = MetadataFunding(pool).charge(core::alloc::Layout::new::<T>())?;
        let value = Box::try_new(value)
            .map_err(|_| map_storage_error(funded_storage::StorageError::Allocation))?;
        Ok(Self {
            value,
            _metadata: metadata,
        })
    }
    pub(crate) fn advance(&mut self, thread: &Thread) -> Result<Step, SystemCallError> {
        self.value.advance(thread)
    }
}

/// 只在当前 syscall 执行点出生请求；槽锁不包含任何领域推进或析构。
pub(crate) fn start(thread: &Thread, pending: Pending) {
    let mut slot = thread.request.lock();
    assert!(slot.is_none(), "thread started a second request");
    *slot = Some(pending);
}

pub(crate) fn take(owner: &AdmittedThread) -> Option<Pending> {
    owner.request.lock().take()
}

/// # Safety
/// 调用点仍持本线程运行/VM 准入且 IRQ 关闭；请求只能访问本线程的用户空间。
/// 全量本地 fence 采用与 _ret_to_user / Switch 相同的 ASID=0 空间切换合同。
pub(crate) unsafe fn advance_in_space(
    pending: &mut Pending,
    owner: &AdmittedThread,
) -> Result<Step, SystemCallError> {
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            // SAFETY: 两份页表共享内核高半区/当前栈；先归一翻译，再允许 VM leave。
            unsafe {
                core::arch::asm!("csrw satp, {satp}", "sfence.vma", satp = in(reg) crate::mm::kernel_satp(), options(nostack));
            }
        }
    }
    // SAFETY: 调用者已通过 Shell/VM enter 并持独占执行资格；共同内核高半区
    // 保证代码/栈持续可用，SIE=0；函数内不让出，不返回用户态。
    unsafe {
        core::arch::asm!("csrw satp, {satp}", "sfence.vma", satp = in(reg) owner.satp(), options(nostack));
    }
    let _restore = Restore;
    pending.advance(owner)
}

/// 必须在内核 root 下且 RunPermit 已 leave 后调用。
pub(crate) fn settle(
    owner: AdmittedThread,
    pending: Pending,
    result: Result<Step, SystemCallError>,
    slot: usize,
) {
    match result {
        Ok(Step::Pending) => {
            start(&owner, pending);
            crate::sched::requeue(owner, slot);
        }
        Ok(Step::Wait(plan)) => {
            drop(pending);
            super::wait::install(owner, plan);
        }
        result => {
            drop(pending);
            let result = result.map(|step| match step {
                Step::Complete(value) => value,
                _ => unreachable!("request completion lost its result"),
            });
            // SAFETY: 执行资格独占保存现场；请求至此才推进原 ecall。
            crate::syscall::respond_result(unsafe { &mut *owner.frame_ptr() }, result);
            crate::sched::requeue(owner, slot);
        }
    }
}
