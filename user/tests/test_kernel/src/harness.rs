//! 验收本地 owner；子进程失败升级为本进程失败，由 init 原 services 收集兜底。
use super::*;
use alloc::vec::Vec;
use libprocess::{CollectedProcess, Collector};

pub(super) struct Harness {
    capabilities: Vec<Capability>,
    processes: Vec<Supervised>,
}
impl Harness {
    pub(super) fn new() -> Self {
        Self {
            capabilities: Vec::new(),
            processes: Vec::new(),
        }
    }
    fn reserve(&mut self, count: usize) -> Result<(), SystemCallError> {
        self.capabilities
            .try_reserve(count)
            .map_err(|_| SystemCallError::OutOfMemory)
    }
    fn adopt(&mut self, handle: Handle) {
        // SAFETY: 调用方仅移交本次 create/duplicate 返回的唯一新表项。
        self.capabilities
            .push(unsafe { Capability::from_raw(handle) });
    }
    pub(super) fn create_job(
        &mut self,
        parent: Handle,
        pool: Handle,
        rights: Rights,
    ) -> Result<Handle, SystemCallError> {
        self.reserve(1)?;
        let handle = process::create_job(parent, pool, rights)?;
        self.adopt(handle);
        Ok(handle)
    }
    pub(super) fn duplicate(
        &mut self,
        source: Handle,
        rights: Rights,
    ) -> Result<Handle, SystemCallError> {
        self.reserve(1)?;
        let handle = duplicate(source, rights)?;
        self.adopt(handle);
        Ok(handle)
    }
    pub(super) fn derive_pool(
        &mut self,
        parent: Handle,
        budget: libkernel::shared::memory_pool::MemoryPoolBudget,
        rights: Rights,
    ) -> Result<Handle, SystemCallError> {
        self.reserve(1)?;
        let handle = libkernel::memory_pool::derive(parent, budget, rights)?.into_handle();
        self.adopt(handle);
        Ok(handle)
    }
    fn take(&mut self, handle: Handle) -> Capability {
        let index = self
            .capabilities
            .iter()
            .position(|owner| owner.as_handle() == handle)
            .expect("capability remains owned");
        self.capabilities.swap_remove(index)
    }
    pub(super) fn transferred(&mut self, handle: Handle) {
        self.take(handle).into_raw();
    }
    pub(super) fn close_control(&mut self, handle: Handle) -> Result<(), &'static str> {
        self.take(handle)
            .close()
            .map_err(|(_, error)| error)
            .expect("kernel acceptance capability close failed");
        Ok(())
    }
    pub(super) fn reserve_process(&mut self) -> Result<(), SystemCallError> {
        self.processes
            .try_reserve(1)
            .map_err(|_| SystemCallError::OutOfMemory)
    }
    pub(super) fn track_process(&mut self, process: Supervised) {
        self.processes.push(process);
    }
    pub(super) fn collect_process(&mut self, pid: u64) -> Result<(), &'static str> {
        let index = self
            .processes
            .iter()
            .position(|p| p.pid == pid)
            .ok_or("kernel acceptance process missing")?;
        let target = self.processes.swap_remove(index);
        self.collect_targets(alloc::vec![target])
    }
    pub(super) fn collect_targets(&mut self, targets: Vec<Supervised>) -> Result<(), &'static str> {
        for target in targets {
            let done = collect_process(
                SupervisionTarget::new(target.pid, target.control),
                DEFAULT_SUPERVISION_POLICY,
            )
            .unwrap_or_else(|failure| panic!("kernel acceptance collection failed: {:?}", failure));
            debug!(
                "pid {} supervised: queries={}, state={}, reason={}, detail0={}",
                done.pid,
                done.progress.query_attempts,
                done.snapshot.state,
                done.snapshot.cause,
                done.snapshot.detail.word0
            );
        }
        Ok(())
    }
    pub(super) fn collect_machine(
        &mut self,
        machine: Collector,
    ) -> Result<CollectedProcess, &'static str> {
        Ok(
            libprocess::collect_remaining(machine).unwrap_or_else(|failure| {
                panic!(
                    "kernel acceptance collection continuation failed: {:?}",
                    failure
                )
            }),
        )
    }
    pub(super) fn collect_services(&mut self) -> Result<(), &'static str> {
        let targets = core::mem::take(&mut self.processes);
        self.collect_targets(targets)
    }
    pub(super) fn collect_job(&mut self, job: Handle) -> Result<(), &'static str> {
        libprocess::job_kill(job).unwrap_or_else(|failure| {
            panic!("kernel acceptance Job collection failed: {:?}", failure)
        });
        Ok(())
    }
    pub(super) fn receive_until(
        &mut self,
        handle: Handle,
        deadline: Deadline,
    ) -> Result<libkernel::ipc::message::ReceivedMessage, SystemCallError> {
        loop {
            match receive(handle) {
                Ok(message) => return Ok(message),
                Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => {}
                Err(error) => return Err(error),
            }
            if !wait_until(handle, ObjectSignals::READABLE, deadline)?
                .observed
                .contains(ObjectSignals::READABLE)
            {
                return Err(SystemCallError::DeadlineExpired);
            }
        }
    }
}
