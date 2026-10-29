//! 验收本地 owner；子进程失败升级为本进程失败，由 init 原 services 收集兜底。
use super::tunnel_checks::{ReadWorld, StreamReadTask};
use super::*;
use alloc::vec::Vec;
use libexecution::assembly::Assembly;
use libprocess::{CollectedProcess, Collector};

pub(super) struct Harness {
    capabilities: Vec<Owner>,
    processes: Vec<Supervised>,
    read: Option<ReadWorld>,
    failed_capability: Option<Capability>,
    failed_endpoint: Option<librunnel::MappedEndpoint>,
    failed_create: Option<libkernel::ipc::tunnel::Endpoint>,
    failed_invitation: Option<libkernel::ipc::invitation::Invitation>,
}
enum Owner {
    Plain(Capability),
    Sender(MailboxSender),
    Notification(notification::NotificationOwner),
    Signaler(notification::NotificationSignaler),
}
impl Owner {
    fn handle(&self) -> Handle {
        match self {
            Self::Plain(cap) => cap.as_handle(),
            Self::Sender(cap) => cap.as_handle(),
            Self::Notification(cap) => cap.as_handle(),
            Self::Signaler(cap) => cap.as_handle(),
        }
    }
    fn transferred(self) {
        match self {
            Self::Plain(cap) => {
                cap.into_raw();
            }
            Self::Sender(cap) => {
                cap.into_capability().into_raw();
            }
            Self::Notification(cap) => {
                cap.into_raw();
            }
            Self::Signaler(cap) => {
                cap.into_raw();
            }
        }
    }
    fn close(self) {
        let result = match self {
            Self::Plain(cap) => cap.close().map_err(|(_, error)| error),
            Self::Sender(cap) => cap.close().map_err(|(_, error)| error),
            Self::Notification(cap) => cap.close().map_err(|(_, error)| error),
            Self::Signaler(cap) => cap.close().map_err(|(_, error)| error),
        };
        // 关闭失败不能继续报告验收成功；init 持有本进程与整棵验收 Job。
        result.expect("kernel acceptance capability close failed");
    }
}
impl Harness {
    pub(super) fn new() -> Self {
        Self {
            capabilities: Vec::new(),
            processes: Vec::new(),
            read: None,
            failed_capability: None,
            failed_endpoint: None,
            failed_create: None,
            failed_invitation: None,
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
            .push(Owner::Plain(unsafe { Capability::from_raw(handle) }));
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
    pub(super) fn create_mailbox(
        &mut self,
        owner: Rights,
        sender: Rights,
    ) -> Result<libkernel::shared::object::HandlePair, SystemCallError> {
        self.reserve(2)?;
        let pair = create(owner, sender)?;
        self.adopt(pair.owner);
        // SAFETY: 工厂刚返回的 sender 表项在此唯一采用。
        let cap = unsafe { Capability::from_raw(pair.peer) };
        let sender = MailboxSender::from_capability(cap)
            .map_err(|failure| failure.error)?
            .0;
        self.capabilities.push(Owner::Sender(sender));
        Ok(pair)
    }
    pub(super) fn create_notification(
        &mut self,
        owner: Rights,
        signaler: Rights,
    ) -> Result<libkernel::shared::object::HandlePair, SystemCallError> {
        self.reserve(2)?;
        let pair = notification::create(owner, signaler)?;
        let handles = libkernel::shared::object::HandlePair::new(
            pair.owner.as_handle(),
            pair.peer.as_handle(),
        );
        self.capabilities.push(Owner::Notification(pair.owner));
        self.capabilities.push(Owner::Signaler(pair.peer));
        Ok(handles)
    }
    pub(super) fn notification_owner(&self, handle: Handle) -> &notification::NotificationOwner {
        self.capabilities
            .iter()
            .find_map(|owner| match owner {
                Owner::Notification(cap) if cap.as_handle() == handle => Some(cap),
                _ => None,
            })
            .expect("notification remains owned")
    }
    pub(super) fn sender(&self, handle: Handle) -> &MailboxSender {
        self.capabilities
            .iter()
            .find_map(|owner| match owner {
                Owner::Sender(cap) if cap.as_handle() == handle => Some(cap),
                _ => None,
            })
            .expect("sender remains owned")
    }
    fn take(&mut self, handle: Handle) -> Owner {
        let index = self
            .capabilities
            .iter()
            .position(|owner| owner.handle() == handle)
            .expect("capability remains owned");
        self.capabilities.swap_remove(index)
    }
    pub(super) fn transferred(&mut self, handle: Handle) {
        self.take(handle).transferred();
    }
    pub(super) fn close_control(&mut self, handle: Handle) -> Result<(), &'static str> {
        self.take(handle).close();
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
    pub(super) fn retain_capability(&mut self, capability: Capability) {
        self.failed_capability = Some(capability);
    }
    pub(super) fn retain_transport(
        &mut self,
        endpoint: librunnel::MappedEndpoint,
        invitation: libkernel::ipc::invitation::Invitation,
    ) {
        self.failed_endpoint = Some(endpoint);
        self.failed_invitation = Some(invitation);
    }
    pub(super) fn retain_failed_create(
        &mut self,
        endpoint: Option<libkernel::ipc::tunnel::Endpoint>,
        invitation: Option<libkernel::ipc::invitation::Invitation>,
    ) {
        self.failed_create = endpoint;
        self.failed_invitation = invitation;
    }
    pub(super) fn start_read(&mut self, tunnel: blocking::Consumer, buffer: Vec<u8>) {
        assert!(
            self.read.is_none(),
            "kernel acceptance stream already owned"
        );
        self.read = Some(ReadWorld {
            tunnel: Some(tunnel),
            buffer,
            ..ReadWorld::default()
        });
    }
    pub(super) fn read_world(&mut self) -> &mut ReadWorld {
        self.read
            .as_mut()
            .expect("kernel acceptance stream remains owned")
    }
    pub(super) fn await_read(&mut self) -> Result<(), &'static str> {
        let budget = Budget::new(&[1, 64 * 1024], 1).expect("stream budget failed");
        let account = budget
            .account(&[1, 64 * 1024])
            .unwrap()
            .view::<ExecutionResource>(&[budget.slot(0).unwrap(), budget.slot(1).unwrap()])
            .unwrap();
        let mut assembly =
            Assembly::new(WaitSet::create(2).expect("stream WaitSet failed"), &account)
                .map_err(|(_, error)| error)
                .expect("stream Assembly failed");
        let mut runtime =
            Runtime::new(assembly.bind().unwrap(), 1, 2, &account).expect("stream Runtime failed");
        runtime
            .spawn(libexecution::TaskSubmission::new(
                StreamReadTask::new(),
                2,
                account,
            ))
            .map_err(|failure| failure.error)
            .expect("stream task failed");
        let world = self.read_world();
        world.buffer.resize(STREAM_LEN + 1, 0);
        assembly
            .run(&mut runtime, world, 1)
            .expect("stream execution failed");
        runtime
            .close()
            .map_err(|(_, error)| error)
            .expect("stream Runtime close failed");
        assembly
            .close()
            .map_err(|(_, error)| error)
            .expect("stream Assembly close failed");
        if world.failed {
            return Err("stream read failed");
        }
        Ok(())
    }
    pub(super) fn close_read(&mut self) -> Result<(), &'static str> {
        let mut world = self
            .read
            .take()
            .expect("kernel acceptance stream remains owned");
        if let Some(tunnel) = world.tunnel.take() {
            tunnel
                .close()
                .map_err(|(_, error)| error)
                .expect("stream close failed");
        }
        Ok(())
    }
}
