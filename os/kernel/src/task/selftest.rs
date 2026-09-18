//! Ready 前公共启动夹具：真实绑定、域准入、用户复制和正常终止/退休。

use super::{
    Thread, lifecycle,
    memory_pool::MemoryPool,
    proc::{self, Process},
    process, wait,
};
use alloc::{
    sync::{Arc, Weak},
    vec::Vec,
};
use erhino_shared::proc::{ProcessExitReason, ThreadStartContext};

pub(crate) struct Caller {
    pub(crate) process: Arc<Process>,
    pub(crate) thread: Option<crate::sched::AdmittedThread>,
    pub(crate) output: usize,
}

pub(crate) struct Activated(usize);

impl Activated {
    pub(crate) fn new(process: &Process) -> Self {
        let next = process.space.lock().satp();
        let previous;
        // SAFETY: fixture 强持页表，用户页表包含同一内核映射；ASID=0，
        // 按 supervisor.adoc ASID Usage 在每次写 satp 后全量 fence。
        unsafe {
            core::arch::asm!("csrr {previous}, satp", "csrw satp, {next}", "sfence.vma zero, zero",
                previous = out(reg) previous, next = in(reg) next, options(nostack));
        }
        Self(previous)
    }
}

impl Drop for Activated {
    fn drop(&mut self) {
        // SAFETY: guard 内不退休映射；返回处理函数后恢复启动页表。
        unsafe {
            core::arch::asm!("csrw satp, {}", "sfence.vma zero, zero", in(reg) self.0, options(nostack));
        }
    }
}

pub(crate) fn bound(root: &Arc<MemoryPool>) -> Arc<Process> {
    let process = Arc::new(
        Process::new(
            0,
            0,
            Weak::new(),
            super::resources::ProcessResources::try_new().expect("fixture sponsor failed"),
        )
        .expect("fixture process failed"),
    );
    process::bind_memory_internal(&process, root.clone()).expect("fixture memory binding failed");
    process
}

pub(crate) fn pump() {
    for _ in 0..8192 {
        if super::notify_work::drain_current() + crate::deferred_work::drain_current() == 0 {
            return;
        }
    }
    panic!("fixture queues failed to converge");
}

pub(crate) fn terminate(process: &Arc<Process>) {
    let todo = process
        .lifecycle
        .request_termination(ProcessExitReason::Killed, 73, None);
    process::run_termination_todo(process, todo);
}

pub(crate) fn collect(process: &Arc<Process>) {
    assert!(
        process.lifecycle.is_reapable(),
        "fixture cleanup started before reapability"
    );
    for _ in 0..8192 {
        let (_, outcome) = proc::selftest::advance_unmanaged(process, 128);
        pump();
        if matches!(outcome, super::proc::DrainBatchOutcome::Complete) {
            return;
        }
    }
    panic!("fixture process cleanup failed to converge");
}

impl Caller {
    pub(crate) fn new(root: &Arc<MemoryPool>) -> Self {
        Self::group(root, 1).pop().unwrap()
    }

    pub(crate) fn pair(root: &Arc<MemoryPool>) -> (Self, Self) {
        let mut callers = Self::group(root, 2).into_iter();
        (callers.next().unwrap(), callers.next().unwrap())
    }

    fn group(root: &Arc<MemoryPool>, count: usize) -> Vec<Self> {
        let process = bound(root);
        process
            .space
            .map_stack()
            .expect("fixture result mapping failed");
        let output = proc::USER_TOP - 256;
        for _ in 0..count {
            process
                .lifecycle
                .attach_member(|_, member| {
                    Arc::try_new(
                        Thread::new_thread(
                            member,
                            &process,
                            ThreadStartContext {
                                entry: 0,
                                stack_pointer: output as u64,
                                arg1: 0,
                                arg2: 0,
                            },
                        )
                        .map_err(|_| lifecycle::AttachFault::Oom)?,
                    )
                    .map_err(|_| lifecycle::AttachFault::Oom)
                })
                .expect("fixture thread creation failed");
        }
        assert!(process.lifecycle.enter_building_op());
        let domain = crate::sched::resolve_domain(elf::IsaRequirement::Base64)
            .expect("fixture domain unavailable");
        let mut batch = domain
            .reserve_ready(count)
            .expect("fixture ready admission failed");
        let mut staged = Vec::with_capacity(count);
        process
            .lifecycle
            .begin_running(count, &mut staged)
            .expect("fixture start failed");
        process.bind_execution(elf::IsaRequirement::Base64, domain);
        let callers: Vec<_> = staged
            .into_iter()
            .map(|thread| Self {
                process: process.clone(),
                thread: Some(batch.admit(thread)),
                output,
            })
            .collect();
        callers[0].enter();
        callers
    }

    pub(crate) fn enter(&self) {
        assert!(matches!(
            self.process.lifecycle.enter_running_if(
                self.thread().member(),
                crate::hart::current().slot(),
                || true
            ),
            lifecycle::EnterRunning::Entered
        ));
    }

    pub(crate) fn thread(&self) -> &Thread {
        self.thread.as_ref().unwrap()
    }

    pub(crate) fn put<T: Copy>(&self, address: usize, value: &T) {
        let _active = Activated::new(&self.process);
        let mut space = self.process.space.lock();
        // SAFETY: 夹具只传全部初始化、无 padding 的共享固定宽类型。
        unsafe { crate::uaccess::write_user_value(&mut space, address, value) }
            .expect("fixture input write failed");
    }

    pub(crate) fn read<T: Copy>(&self, address: usize) -> T {
        let _active = Activated::new(&self.process);
        let mut space = self.process.space.lock();
        // SAFETY: 夹具只读取合法整数值域的共享固定宽类型。
        unsafe { crate::uaccess::read_user_value(&mut space, address) }
            .expect("fixture output read failed")
    }

    pub(crate) fn park(&mut self, plan: wait::WaitPlan) {
        assert!(
            self.process
                .lifecycle
                .clear_active_if(crate::hart::current().slot(), || true)
        );
        wait::install(self.thread.take().unwrap(), plan);
    }

    pub(crate) fn cleanup(mut self) {
        if let Some(thread) = self.thread.take() {
            if self
                .process
                .lifecycle
                .snapshot_running()
                .is_some_and(|snapshot| snapshot.active() != 0)
            {
                assert!(
                    self.process
                        .lifecycle
                        .clear_active_if(crate::hart::current().slot(), || true)
                );
            }
            terminate(&self.process);
            let departure = thread.departure();
            drop(thread);
            departure.request(super::thread::DepartureKind::Terminated);
        } else {
            terminate(&self.process);
        }
        pump();
        collect(&self.process);
    }
}
