//! 执行容器：用户现场与出生时固定的执行属性；生命周期归 Shell/Body。

use alloc::sync::Arc;
use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

use erhino_shared::{call::SystemCallError, proc::ThreadStartContext};

use crate::{
    context::UserContext,
    sched::{ReadyBatch, SchedDomain},
    sync::{Spinlock, ranks},
};

use super::{
    super::thread::ThreadControl, departure::ThreadDeparture, members::MemberKey,
    shell::ProcessShell,
};

pub struct Thread {
    member: MemberKey,
    pub(crate) process: Arc<ProcessShell>,
    pub(crate) domain: &'static SchedDomain,
    satp: usize,
    uses_fp: bool,
    frame: UnsafeCell<UserContext>,
    ready_batch: Spinlock<Option<ReadyBatch>>,
    output_fault: AtomicBool,
    fault_address: AtomicUsize,
    /// 同一翻译事实每hart最多修复一次，避免跨hart迁移使无进展重试循环。
    fault_repair: UnsafeCell<Option<(crate::context::FaultRepair, u64)>>,
    /// 到期/取消在队列锁内摘链，载荷的 Thread 强根与 epoch 锁外交出。
    pub(crate) timer: timer_queue::TimerNode<crate::sched::TimerDelivery>,
    /// 唯一等待 owner 随 AdmittedThread 移动，提示方不访问续体或节点。
    pub(crate) wait_entry: UnsafeCell<Option<super::super::wait::Entry>>,
    pub(crate) wait_node: super::super::wait_source::WaitNode,
    release: ThreadRelease,
}

struct ThreadRelease {
    metadata: Option<super::super::resources::MetadataCharge>,
    departure: Arc<ThreadDeparture>,
    requested: AtomicBool,
}

impl Drop for ThreadRelease {
    fn drop(&mut self) {
        // 最后执行容器引用已消散，现场、等待节点与就绪资格已先析构。
        drop(self.metadata.take());
        if self.requested.load(Ordering::Acquire) {
            self.departure.request();
        }
    }
}

// SAFETY: 不可复制的 AdmittedThread 独占现场/等待续体/节点生命周期；来源与
// timer 只复制 epoch 提示，成员锁仲裁 pending/容器，不访问执行 owner 字段。
unsafe impl Sync for Thread {}

impl Thread {
    /// # Safety
    /// 调用者是该Thread唯一执行owner；不得与另一个现场访问并发。
    pub(crate) unsafe fn record_fault_repair(&self, repair: crate::context::FaultRepair) -> bool {
        // SAFETY: 执行owner独占此续体，UnsafeCell未向观察者暴露。
        let previous = unsafe { &mut *self.fault_repair.get() };
        let bit = 1u64 << crate::hart::current().slot();
        match previous {
            Some((facts, attempted)) if *facts == repair => {
                if *attempted & bit != 0 {
                    false
                } else {
                    *attempted |= bit;
                    true
                }
            }
            _ => {
                *previous = Some((repair, bit));
                true
            }
        }
    }

    /// # Safety
    /// 调用者是该Thread唯一执行owner。
    pub(crate) unsafe fn clear_fault_repair(&self) {
        // SAFETY: 同record_fault_repair的执行归属，不保留借用。
        unsafe { *self.fault_repair.get() = None };
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "出生阶段分别提供稳定身份、冻结执行环境和预付责任"
    )]
    pub(crate) fn try_new(
        member: MemberKey,
        process: &Arc<ProcessShell>,
        context: ThreadStartContext,
        domain: &'static SchedDomain,
        satp: usize,
        uses_fp: bool,
        control: Option<&Arc<ThreadControl>>,
        batch: ReadyBatch,
    ) -> Result<Arc<Self>, SystemCallError> {
        let metadata =
            super::super::resources::MetadataCharge::arc::<Self>(&process.funding_pool())?;
        let departure = ThreadDeparture::try_new(process, member, control)?;
        let mut frame = UserContext::zeroed();
        frame.sepc = context.entry;
        frame.x[2] = context.stack_pointer;
        frame.x[10] = context.arg1;
        frame.x[11] = context.arg2;
        Arc::try_new(Self {
            member,
            process: process.clone(),
            domain,
            satp,
            uses_fp,
            frame: UnsafeCell::new(frame),
            ready_batch: Spinlock::new(ranks::WORK_DEBT, Some(batch)),
            output_fault: AtomicBool::new(false),
            fault_address: AtomicUsize::new(0),
            fault_repair: UnsafeCell::new(None),
            timer: timer_queue::TimerNode::new(),
            wait_entry: UnsafeCell::new(None),
            wait_node: super::super::wait_source::WaitNode::new(),
            release: ThreadRelease {
                metadata: Some(metadata),
                departure,
                requested: AtomicBool::new(false),
            },
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub(crate) fn abandon_created(&self) {
        let Some(body) = self.process.lease_receipt() else {
            return;
        };
        let created = body.members.claim_created_departure(self.member);
        drop(body);
        if let Some(created) = created {
            self.finish_created_departure();
            drop(created);
        }
    }

    pub(crate) fn finish_created_departure(&self) {
        self.release.requested.store(true, Ordering::Release);
    }

    pub(crate) fn start(self: &Arc<Self>) -> Result<(), SystemCallError> {
        let body = self
            .process
            .lease_body()
            .ok_or(SystemCallError::ObjectClosed)?;
        let created = self.process.start_created(&body, self.member)?;
        assert!(Arc::ptr_eq(&created, self), "member thread changed");
        let batch = self
            .ready_batch
            .lock()
            .take()
            .expect("Created thread lost Ready admission");
        batch.publish_one(self.clone());
        drop(body);
        drop(created);
        Ok(())
    }

    /// ProcessStart 持 external lease 逐槽取 Created 后交付；不得跨越 lease 边界。
    pub(crate) fn publish_ready(self: &Arc<Self>) {
        let batch = self
            .ready_batch
            .lock()
            .take()
            .expect("Created thread lost Ready admission");
        batch.publish_one(self.clone());
    }

    pub fn frame_ptr(&self) -> *mut UserContext {
        self.frame.get()
    }

    pub(crate) const fn member(&self) -> MemberKey {
        self.member
    }

    pub fn uses_fp(&self) -> bool {
        self.uses_fp
    }

    pub fn satp(&self) -> usize {
        self.satp
    }

    /// 当前执行者只记录首次输出 fault；不在调用者仍持 VM 锁时重入 Frozen。
    pub(crate) fn defer_output_fault(&self, address: usize) {
        if !self.output_fault.load(Ordering::Relaxed) {
            self.fault_address.store(address, Ordering::Relaxed);
            self.output_fault.store(true, Ordering::Release);
        }
    }

    /// trap 的业务锁均释放后交接，已死亡调用者无需再进入 syscall。
    pub(crate) fn publish_output_fault(&self) {
        if self.output_fault.swap(false, Ordering::Acquire) {
            self.process.freeze_and_seal(
                erhino_shared::proc::TerminationCause::Faulted,
                erhino_shared::proc::TerminationDetail::fault(
                    self.member.tid(),
                    erhino_shared::proc::ProcessFaultCode::UserAccess,
                    self.fault_address.load(Ordering::Relaxed) as u64,
                ),
            );
        }
    }
}

impl super::super::wait_source::HintTarget for Thread {
    fn hint(&self, epoch: u64) {
        let Some(body) = self.process.lease_receipt() else {
            return;
        };
        let action = body.members.hint(&self.process, self.member, epoch);
        drop(body);
        match action {
            super::members::HintTransition::Ignored => {}
            super::members::HintTransition::Publish(owner, ingress) => {
                ingress.publish_admitted(owner);
            }
            super::members::HintTransition::Exit(owner) => crate::sched::reap(owner),
        }
    }
}
