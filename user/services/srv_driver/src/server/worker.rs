//! 唯一进程/Pool/出生与原libprocess收集owner；成员关系只从原Run推导。
use super::*;
use erhino_shared::proc::{HandleGrant, ProcessState};
use libexecution::runtime::{SourceEvent, SourcePlan};
use libkernel::{
    ipc::message::{MessageStorage, ReceiveBuffer},
    memory_pool::{self, MemoryPool},
    process,
};
use libprocess::{GrantOutcome, SpawnRequest};
fn log_birth_pool(role: &str, handle: erhino_shared::object::Handle) {
    match memory_pool::query(handle) {
        Ok(s) => libkernel::debug!(
            "driver birth Pool {}: id={}, pages(total/available/allocated/delegated)={}/{}/{}/{}, metadata slots(total/available/allocated/delegated)={}/{}/{}/{}; slot_bytes={}",
            role,
            s.identity,
            s.total,
            s.available,
            s.allocated,
            s.delegated,
            s.metadata.total,
            s.metadata.available,
            s.metadata.allocated,
            s.metadata.delegated,
            erhino_shared::memory_pool::METADATA_SLOT_BYTES
        ),
        Err(error) => libkernel::debug!("driver birth Pool {} query failed: {:?}", role, error),
    }
}
pub(super) struct Record {
    pub id: InstanceId,
    pub implementation: usize,
    pub task: u64,
    pub pid: Option<u64>,
    // 仅关联成员存活期间借用查询；关闭权唯一在ProcessTask的原collector。
    control: Option<erhino_shared::object::Handle>,
    pub entry: Option<Arc<MailboxSender>>,
    pub mailbox: u64,
    pub accepting: bool,
    pub dead: bool,
    pub error: Option<Error>,
    pub collected: bool,
}
impl Record {
    fn new(id: InstanceId, implementation: usize) -> Self {
        Self {
            id,
            implementation,
            task: 0,
            pid: None,
            control: None,
            entry: None,
            mailbox: 0,
            accepting: true,
            dead: false,
            error: None,
            collected: false,
        }
    }
    pub fn ready(&self) -> bool {
        self.entry.is_some() && !self.dead && self.error.is_none() && self.accepting
    }
}
struct Worker {
    pool: MemoryPool,
    control: erhino_shared::object::Handle,
    collector: libprocess::SuperviseTask,
    boot: Option<Mailbox>,
    expected: u64,
    receiver: ReceiveBuffer,
    storage: Option<MessageStorage>,
    boot_source: SourceRegistration,
    process_source: SourceRegistration,
    ready: bool,
    retiring: bool,
    kill_requested: bool,
}
impl Worker {
    fn spawn(world: &World<'_>, implementation: usize, record: &mut Record) -> Result<Self, Error> {
        let pool = memory_pool::derive(
            world.pool.as_handle(),
            libdriver::boot::WORKER_POOL,
            Rights::READ | Rights::CREATE | Rights::DUPLICATE | Rights::GRANT,
        )?;
        let boot = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)?;
        let sender = boot.mint(0, TRANSPORT)?.sender;
        let expected = sender.description()?.object_id;
        let pool_grant = object::duplicate(pool.as_handle(), Rights::CREATE | Rights::GRANT)?;
        // SAFETY: duplicate 返回独占的新 entry；仅在实际 Grant 已消费时放弃本地关闭责任。
        let pool_grant = unsafe { Capability::from_raw(pool_grant) };
        let receiver = ReceiveBuffer::new()?;
        let storage = MessageStorage::new()?;
        let grants = [
            HandleGrant {
                handle: sender.as_handle(),
                rights: Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
            },
            HandleGrant {
                handle: pool_grant.as_handle(),
                rights: Rights::CREATE,
            },
        ];
        let declaration = world
            .catalog
            .records()
            .nth(implementation)
            .ok_or(Error::Invalid)??;
        let configuration = libdriver::boot::WorkerConfig {
            capacity: declaration.capacity,
        }
        .encode()?;
        libkernel::debug!(
            "driver worker birth budgets: process={}",
            record.id.serial()
        );
        log_birth_pool("parent", world.pool.as_handle());
        log_birth_pool("child", pool.as_handle());
        let image = world
            .images
            .get(implementation)
            .ok_or(Error::Invalid)?
            .map()?;
        let result = libprocess::spawn(SpawnRequest {
            job: world.job.as_handle(),
            parent: world.process,
            memory_pool: pool.as_handle(),
            image: image.bytes(),
            payload: &configuration,
            grants: &grants,
            control_rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
        });
        if result.is_ok()
            || matches!(&result, Err(failure) if failure.grants == GrantOutcome::Consumed)
        {
            sender.into_capability().into_raw();
            pool_grant.into_raw();
        }
        let spawned = result.map_err(|failure| {
            libkernel::debug!(
                "driver worker spawn failed: error={:?}, grants={:?}, cleanup={:?}",
                failure.error,
                failure.grants,
                failure.cleanup_error
            );
            log_birth_pool("failed-parent", world.pool.as_handle());
            log_birth_pool("failed-child", pool.as_handle());
            Error::Resource
        })?;
        // 原监督机器在任何 Activate 前接管唯一 Control；标量 handle 仅供存活期借用。
        let collector = libprocess::SuperviseTask::new(
            libprocess::SupervisionTarget::new(spawned.pid, spawned.control),
            libprocess::DEFAULT_SUPERVISION_POLICY,
        );
        record.pid = Some(spawned.pid);
        record.control = Some(spawned.control);
        Ok(Self {
            pool,
            control: spawned.control,
            collector,
            boot: Some(boot),
            expected,
            receiver,
            storage: Some(storage),
            boot_source: SourceRegistration::new(),
            process_source: SourceRegistration::new(),
            ready: false,
            retiring: false,
            kill_requested: false,
        })
    }
    fn observe(&mut self, record: &mut Record, event: SourceEvent) {
        if event.kind == 2 {
            self.ready = true;
        } else if event.kind == 3 && event.observed.contains(ObjectSignals::DONE) {
            record.dead = true;
            record.accepting = false;
        }
    }
    fn registered(&mut self, kind: u64, id: SourceId) {
        match kind {
            2 => self.boot_source.registered(kind, id),
            3 => self.process_source.registered(kind, id),
            _ => {}
        }
    }
    fn unregistered(&mut self, id: SourceId) {
        self.boot_source.unregistered(id);
        self.process_source.unregistered(id);
    }
    fn sources(
        &mut self,
        record: &Record,
        requests: &mut Requests<'_, Family>,
    ) -> Result<(), SystemCallError> {
        if self.retiring || record.dead {
            return Ok(());
        }
        if self.process_source.is_empty() {
            self.process_source.install(
                requests,
                SourcePlan::new(self.control, ObjectSignals::DONE),
                3,
            )?;
        }
        if record.accepting
            && record.entry.is_none()
            && self.boot_source.is_empty()
            && let Some(boot) = self.boot.as_ref()
        {
            self.boot_source.install(
                requests,
                SourcePlan::new(
                    boot.as_handle(),
                    ObjectSignals::READABLE | ObjectSignals::CLOSED,
                ),
                2,
            )?;
        }
        Ok(())
    }
    fn boot(
        &mut self,
        record: &mut Record,
        requests: &mut Requests<'_, Family>,
    ) -> Result<bool, Error> {
        if record.entry.is_none() && self.ready {
            let boot = self.boot.as_ref().ok_or(Error::Closed)?;
            self.receiver.receive(boot.as_handle())?;
            let mut message = self
                .storage
                .take()
                .ok_or(Error::Invalid)?
                .take(&mut self.receiver)?;
            if message.header.kind != protocol::BOOT_READY
                || message.header.sender_pid != record.pid.ok_or(Error::NotReady)?
                || message.header.sender_context_id != self.expected
                || message.payload.len() != 8
                || message.handles.remaining() != 1
            {
                return Err(Error::Permission);
            }
            let mailbox = u64::from_le_bytes(
                message.payload[..8]
                    .try_into()
                    .map_err(|_| Error::Invalid)?,
            );
            let (entry, description) = MailboxSender::from_capability(message.handles.take(0)?)
                .map_err(|failure| Error::from(failure.error))?;
            if mailbox == 0 || description.related_object_id != mailbox {
                return Err(Error::Permission);
            }
            let snapshot = process::query(self.control)?;
            if snapshot.state != ProcessState::Running as u32 {
                return Err(Error::Unavailable);
            }
            record.entry = Some(Arc::try_new(entry).map_err(|_| Error::Resource)?);
            record.mailbox = mailbox;
            libkernel::debug!(
                "driver bootstrap accepted: process={}, pid={}, now_ns={}",
                record.id.serial(),
                record.pid.ok_or(Error::NotReady)?,
                libkernel::time::snapshot()?.now_ns
            );
        }
        if record.entry.is_some() {
            self.boot_source.remove(requests)?;
            if self.boot_source.is_empty() {
                self.boot = None;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

pub(super) struct ProcessTask {
    record: Rc<RefCell<Record>>,
    worker: Option<Worker>,
    end: Deadline,
    wake_cursor: Option<u64>,
}
impl ProcessTask {
    fn new(record: Rc<RefCell<Record>>) -> Self {
        Self {
            record,
            worker: None,
            end: Deadline::INFINITE,
            wake_cursor: None,
        }
    }
    pub fn deadline(&self) -> Deadline {
        if let Some(worker) = self.worker.as_ref()
            && worker.retiring
            && worker.boot_source.is_empty()
            && worker.process_source.is_empty()
        {
            return <libprocess::SuperviseTask as Task<World<'_>>>::deadline(&worker.collector);
        }
        if self.record.borrow().accepting && self.record.borrow().entry.is_none() {
            self.end
        } else {
            Deadline::INFINITE
        }
    }
    pub fn registered(&mut self, world: &mut World<'_>, kind: u64, id: SourceId) {
        if let Some(worker) = self.worker.as_mut() {
            if worker.retiring && worker.boot_source.is_empty() && worker.process_source.is_empty()
            {
                <libprocess::SuperviseTask as Task<World<'_>>>::registered(
                    &mut worker.collector,
                    world,
                    kind,
                    id,
                );
            } else {
                worker.registered(kind, id);
            }
        }
    }
    pub fn unregistered(&mut self, world: &mut World<'_>, kind: u64, id: SourceId) {
        if let Some(worker) = self.worker.as_mut() {
            if worker.retiring && worker.boot_source.is_empty() && worker.process_source.is_empty()
            {
                <libprocess::SuperviseTask as Task<World<'_>>>::unregistered(
                    &mut worker.collector,
                    world,
                    kind,
                    id,
                );
            } else {
                worker.unregistered(id);
            }
        }
    }
    pub fn advance(
        &mut self,
        id: u64,
        world: &mut World<'_>,
        requests: &mut Requests<'_, Family>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        self.record.borrow_mut().task = id;
        let collecting = self.worker.as_ref().is_some_and(|worker| {
            worker.retiring && worker.boot_source.is_empty() && worker.process_source.is_empty()
        });
        if !collecting {
            let was_dead = self.record.borrow().dead;
            while let Some(event) = input.pull() {
                if let Some(worker) = self.worker.as_mut() {
                    worker.observe(&mut self.record.borrow_mut(), event);
                }
            }
            if input.take_timeout() && self.record.borrow().entry.is_none() {
                self.fail(Error::Unavailable);
            }
            if self.record.borrow().dead && !was_dead {
                self.wake_cursor = Some(0);
            }
        }
        if world.stopping && self.record.borrow().accepting {
            self.record.borrow_mut().accepting = false;
            self.wake_cursor = Some(0);
        }
        let process = self.record.borrow().id;
        let associated = member_count(world, process);
        if associated == 0 {
            self.record.borrow_mut().accepting = false;
        }
        if self.worker.is_none() && self.record.borrow().accepting {
            let implementation = self.record.borrow().implementation;
            // 出生/认证boot单独持绝对10s，不扩大普通RPC的5s期限。
            self.end = libkernel::time::timeout_millis(10_000)?;
            let declaration = world
                .catalog
                .records()
                .nth(implementation)
                .ok_or(SystemCallError::InternalError)?
                .unwrap_or_else(|e| fatal(e));
            let spawned = {
                let mut record = self.record.borrow_mut();
                Worker::spawn(world, implementation, &mut record)
            };
            match spawned {
                Ok(worker) => {
                    libkernel::debug!(
                        "driver process spawned: key={}, process={}, pid={}, now_ns={}",
                        declaration.key,
                        process.serial(),
                        self.record
                            .borrow()
                            .pid
                            .expect("spawned process PID missing"),
                        libkernel::time::snapshot()?.now_ns
                    );
                    self.worker = Some(worker);
                }
                Err(error) => self.fail(error),
            }
            return Ok(advance(Step::Runnable));
        }
        if let Some(cursor) = self.wake_cursor {
            if let Some((serial, run)) = world.runs.next_after(Some(&cursor)) {
                let serial = *serial;
                let run = run.borrow();
                if run.process == Some(process) {
                    for task in [run.task, run.binding_watch, run.activation_watch] {
                        if task != 0 {
                            requests.wake(task)?;
                        }
                    }
                }
                self.wake_cursor = Some(serial);
                return Ok(advance(Step::Runnable));
            }
            self.wake_cursor = None;
        }
        if associated == 0 {
            if let Some(worker) = self.worker.as_mut() {
                if !worker.retiring {
                    libkernel::debug!(
                        "driver process collection admitted after all binding detach: process={}, pid={}",
                        process.serial(),
                        self.record.borrow().pid.expect("worker PID missing")
                    );
                }
                worker.retiring = true;
                self.record.borrow_mut().entry = None;
                worker.boot_source.remove(requests)?;
                worker.process_source.remove(requests)?;
                if !worker.boot_source.is_empty() || !worker.process_source.is_empty() {
                    return Ok(advance(Step::Runnable));
                }
                worker.boot = None;
                if !worker.kill_requested {
                    if process::query(worker.control)?.state != ProcessState::Dead as u32 {
                        process::kill(worker.control)?;
                    }
                    worker.kill_requested = true;
                }
                if !self.record.borrow().collected {
                    let result = worker.collector.advance_in(world, requests, input)?;
                    if result.step != Step::Complete {
                        return Ok(result);
                    }
                    if !self.record.borrow().collected {
                        fatal(Error::Unavailable);
                    }
                }
                let credit = worker.pool.query()?;
                libkernel::debug!(
                    "driver collected worker Pool retained: pid={}, allocated_pages={}, allocated_metadata={}",
                    self.record
                        .borrow()
                        .pid
                        .expect("collected worker PID missing"),
                    credit.allocated,
                    credit.metadata.allocated
                );
            }
            self.worker = None;
            self.record.borrow_mut().control = None;
            world.processes.remove(process.serial());
            requests.wake(world.manager_task)?;
            return Ok(advance(Step::Complete));
        }
        let mut failure = None;
        let mut changing = false;
        if let Some(worker) = self.worker.as_mut() {
            worker.sources(&self.record.borrow(), requests)?;
            if !self.record.borrow().accepting {
                worker.boot_source.remove(requests)?;
                if worker.boot_source.is_empty() {
                    worker.boot = None;
                }
            }
            if self.record.borrow().dead {
                worker.process_source.remove(requests)?;
            }
            if self.record.borrow().accepting
                && (self.record.borrow().entry.is_none() || !worker.boot_source.is_empty())
            {
                let result = {
                    let mut record = self.record.borrow_mut();
                    worker.boot(&mut record, requests)
                };
                match result {
                    Ok(true) => {
                        self.end = Deadline::INFINITE;
                        self.wake_cursor = Some(0);
                    }
                    Ok(false) => {}
                    Err(error) => failure = Some(error),
                }
            }
            changing = worker.boot_source.is_installing()
                || worker.boot_source.is_removing()
                || worker.process_source.is_installing()
                || worker.process_source.is_removing();
        }
        if let Some(error) = failure {
            self.fail(error);
        }
        if changing || self.wake_cursor.is_some() {
            return Ok(advance(Step::Runnable));
        }
        Ok(advance(Step::Parked))
    }
    fn fail(&mut self, error: Error) {
        let mut record = self.record.borrow_mut();
        record.error.get_or_insert(error);
        record.accepting = false;
        self.wake_cursor = Some(0);
    }
}
pub(super) fn member_count(world: &World<'_>, process: InstanceId) -> usize {
    let mut count = 0;
    let mut after = 0;
    while let Some((serial, run)) = world.runs.next_after(Some(&after)) {
        after = *serial;
        if run.borrow().process == Some(process) {
            count += 1;
        }
    }
    count
}
/// 所有存储/执行提交准备后，在同一actor提交原Run关联；OS出生由已准入ProcessTask承担。
pub(super) fn associate(
    world: &mut World<'_>,
    requests: &mut Requests<'_, Family>,
    run: &Rc<RefCell<Run>>,
) -> Result<InstanceId, Error> {
    if world.stopping {
        return Err(Error::Closed);
    }
    let implementation = run.borrow().implementation;
    let declaration = world
        .catalog
        .records()
        .nth(implementation)
        .ok_or(Error::Invalid)??;
    if declaration.reusable {
        let mut after = 0;
        while let Some((key, record)) = world.processes.next_after(Some(&after)) {
            after = *key;
            let record = record.borrow();
            if record.implementation == implementation
                && record.accepting
                && !record.dead
                && record.error.is_none()
                && member_count(world, record.id) < declaration.capacity as usize
            {
                run.borrow_mut().process = Some(record.id);
                return Ok(record.id);
            }
        }
    }
    let process = InstanceId::new(
        world.incarnation,
        world.next_process.checked_add(1).ok_or(Error::Exhausted)?,
    )?;
    let record = Rc::try_new(RefCell::new(Record::new(process, implementation)))
        .map_err(|_| Error::Resource)?;
    let entry = world
        .processes
        .prepare_insert(process.serial(), record.clone(), &AllocatorBacked)
        .map_err(|_| Error::Quota)?;
    let task = Box::try_new(ProcessTask::new(record)).map_err(|_| Error::Resource)?;
    requests
        .spawn(TaskSubmission::new(
            Family::Process(task),
            2,
            world.funding.clone(),
        ))
        .map_err(|_| Error::Quota)?;
    world.processes.insert_prepared(entry);
    world.next_process = process.serial();
    run.borrow_mut().process = Some(process);
    Ok(process)
}

/// 同一actor内只读真实Process状态；调用者仍持关联Run，进程owner尚不能收集Control。
pub(super) fn cpu_state(world: &World<'_>, process: InstanceId) -> Result<u32, Error> {
    let record = world
        .processes
        .get(process.serial())
        .ok_or(Error::Unavailable)?
        .borrow();
    let control = record.control.ok_or(Error::NotReady)?;
    Ok(process::query(control)?.state)
}
