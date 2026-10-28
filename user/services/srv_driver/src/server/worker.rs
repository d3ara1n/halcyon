//! 唯一进程/Pool/出生与原libprocess收集owner；成员关系只从原Run推导。
use super::*;
use erhino_shared::proc::ProcessState;
use libexecution::runtime::{SourceEvent, SourcePlan};
use libkernel::{
    ipc::message::{MessageStorage, ReceiveBuffer},
    memory_pool::{self, MemoryPool},
    process,
};
use libprocess::{OwnedGrant, OwnedSpawnRequest, Preparation, PreparedProcess};
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
#[expect(
    clippy::large_enum_variant,
    reason = "已准入的 ProcessTask 预留最大 owner，Start/取消提交后移交 Collector 不再分配"
)]
enum ProcessOwner {
    Preparing(Box<Preparation<'static>>),
    Prepared {
        process: PreparedProcess,
        inputs: Option<Box<Preparation<'static>>>,
    },
    Supervised(libprocess::SuperviseTask),
    Empty,
}
struct Worker {
    pool: MemoryPool,
    process: ProcessOwner,
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
    fn prepare(
        world: &World<'_>,
        implementation: usize,
        record: &mut Record,
    ) -> Result<Self, Error> {
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
        let mut payload = Vec::new();
        payload
            .try_reserve_exact(configuration.len())
            .map_err(|_| Error::Resource)?;
        payload.extend_from_slice(&configuration);
        let mut grants = Vec::new();
        grants.try_reserve_exact(2).map_err(|_| Error::Resource)?;
        grants.push(OwnedGrant::new(
            sender.into_capability(),
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        ));
        grants.push(OwnedGrant::new(pool_grant, Rights::CREATE));
        let image = world
            .images
            .get(implementation)
            .ok_or(Error::Invalid)?
            .map()?;
        let preparing = Preparation::new_owned(OwnedSpawnRequest {
            job: world.job.as_handle(),
            parent: world.process,
            memory_pool: pool.as_handle(),
            image,
            payload,
            grants,
            control_rights: Rights::READ | Rights::WAIT | Rights::MANAGE,
        });
        let preparing = Box::try_new(preparing).map_err(|_| Error::Resource)?;
        Ok(Self {
            pool,
            process: ProcessOwner::Preparing(preparing),
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
    fn collector(&mut self) -> Option<&mut libprocess::SuperviseTask> {
        match &mut self.process {
            ProcessOwner::Supervised(task) => Some(task),
            _ => None,
        }
    }
    fn collecting_deadline(&self) -> Deadline {
        match &self.process {
            ProcessOwner::Supervised(task) => {
                <libprocess::SuperviseTask as Task<World<'_>>>::deadline(task)
            }
            _ => Deadline::INFINITE,
        }
    }
    fn adopt_cancelled(
        record: &mut Record,
        collector: Option<libprocess::Collector>,
    ) -> ProcessOwner {
        match collector {
            Some(collector) => {
                record.pid = Some(collector.pid());
                record.control = Some(collector.control());
                ProcessOwner::Supervised(libprocess::SuperviseTask::from_collector(collector))
            }
            None => ProcessOwner::Empty,
        }
    }
    /// 每步推进同一 loader 或一个出生接点；失败/停止仍由原 owner 显式取消。
    fn birth(
        &mut self,
        record: &mut Record,
        world: &World<'_>,
        requests: &mut Requests<'_, Family>,
    ) -> Result<bool, SystemCallError> {
        match &mut self.process {
            ProcessOwner::Preparing(preparing) => {
                if !record.accepting {
                    let collector = preparing.cancel()?;
                    // 取消先保留 Collector；局部映射关闭失败将由服务故障终局接管。
                    let owner = Self::adopt_cancelled(record, collector);
                    preparing
                        .close_inputs()
                        .unwrap_or_else(|error| fatal(error.into()));
                    self.process = owner;
                    self.kill_requested = true;
                } else {
                    match preparing.advance() {
                        Ok(Some(process)) => {
                            record.pid = Some(process.pid());
                            record.control = Some(process.control());
                            let ProcessOwner::Preparing(inputs) =
                                core::mem::replace(&mut self.process, ProcessOwner::Empty)
                            else {
                                unreachable!("preparation owner changed within actor")
                            };
                            self.process = ProcessOwner::Prepared {
                                process,
                                inputs: Some(inputs),
                            };
                        }
                        Ok(None) => {}
                        Err(error) => {
                            libkernel::debug!("driver worker preparation failed: {:?}", error);
                            record.error.get_or_insert(Error::Resource);
                            record.accepting = false;
                        }
                    }
                }
                Ok(true)
            }
            ProcessOwner::Prepared { process, inputs } => {
                if !record.accepting || record.dead {
                    let collector = process.cancel_to_collector()?;
                    let owner = Self::adopt_cancelled(record, collector);
                    if let Some(inputs) = inputs {
                        inputs
                            .close_inputs()
                            .unwrap_or_else(|error| fatal(error.into()));
                    }
                    self.process = owner;
                    self.kill_requested = true;
                } else if let Some(preparing) = inputs {
                    preparing.close_inputs()?;
                    *inputs = None;
                } else if self.process_source.is_empty() {
                    self.process_source.install(
                        requests,
                        SourcePlan::new(process.control(), ObjectSignals::DONE),
                        3,
                    )?;
                } else if !self.process_source.is_installing() {
                    match process.try_start() {
                        Ok(spawned) => {
                            // 成功后只有不分配的 owner 移交，DONE 已实际附着。
                            self.process =
                                ProcessOwner::Supervised(libprocess::SuperviseTask::new(
                                    libprocess::SupervisionTarget::new(
                                        spawned.pid,
                                        spawned.control,
                                    ),
                                    libprocess::DEFAULT_SUPERVISION_POLICY,
                                ));
                            libkernel::debug!(
                                "driver process started after DONE attachment: process={}, pid={}",
                                record.id.serial(),
                                spawned.pid
                            );
                            let declaration = world
                                .catalog
                                .records()
                                .nth(record.implementation)
                                .expect("driver implementation remains owned")
                                .unwrap_or_else(|error| fatal(error));
                            libkernel::debug!(
                                "driver process spawned: key={}, process={}, pid={}, now_ns={}",
                                declaration.key,
                                record.id.serial(),
                                spawned.pid,
                                libkernel::time::snapshot()?.now_ns
                            );
                        }
                        Err(error) => {
                            libkernel::debug!(
                                "driver worker Start failed: pid={}, error={:?}",
                                process.pid(),
                                error
                            );
                            record.error.get_or_insert(Error::from(error));
                            record.accepting = false;
                        }
                    }
                }
                Ok(true)
            }
            ProcessOwner::Supervised(_) | ProcessOwner::Empty => Ok(false),
        }
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
                SourcePlan::new(
                    record.control.ok_or(SystemCallError::InternalError)?,
                    ObjectSignals::DONE,
                ),
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
        let published = record.entry.is_none() && self.ready;
        if published {
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
            let snapshot = process::query(record.control.ok_or(Error::NotReady)?)?;
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
            }
        }
        // 就绪发布与 bootstrap 来源退休是不同事实；最后注销回执不能承担首次唤醒。
        Ok(published)
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
            return worker.collecting_deadline();
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
                if let Some(collector) = worker.collector() {
                    <libprocess::SuperviseTask as Task<World<'_>>>::registered(
                        collector, world, kind, id,
                    );
                }
            } else {
                worker.registered(kind, id);
            }
        }
    }
    pub fn unregistered(&mut self, world: &mut World<'_>, kind: u64, id: SourceId) {
        if let Some(worker) = self.worker.as_mut() {
            if worker.retiring && worker.boot_source.is_empty() && worker.process_source.is_empty()
            {
                if let Some(collector) = worker.collector() {
                    <libprocess::SuperviseTask as Task<World<'_>>>::unregistered(
                        collector, world, kind, id,
                    );
                }
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
                if let Some(Worker {
                    process: ProcessOwner::Preparing(preparing),
                    ..
                }) = &self.worker
                {
                    libkernel::debug!(
                        "driver preparation deadline expired: process={}, now_ns={}, end_ns={}, state={:?}",
                        self.record.borrow().id.serial(),
                        input.now_ns(),
                        self.end.at_ns,
                        preparing
                    );
                } else {
                    libkernel::debug!(
                        "driver bootstrap deadline expired: process={}, now_ns={}, end_ns={}",
                        self.record.borrow().id.serial(),
                        input.now_ns(),
                        self.end.at_ns
                    );
                }
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
                Worker::prepare(world, implementation, &mut record)
            };
            match spawned {
                Ok(worker) => {
                    libkernel::debug!(
                        "driver process preparation admitted: key={}, process={}, now_ns={}, end_ns={}",
                        declaration.key,
                        process.serial(),
                        input.now_ns(),
                        self.end.at_ns
                    );
                    self.worker = Some(worker);
                }
                Err(error) => self.fail(error),
            }
            return Ok(advance(Step::Runnable));
        }
        if let Some(worker) = self.worker.as_mut() {
            let was_accepting = self.record.borrow().accepting;
            let progressed = worker.birth(&mut self.record.borrow_mut(), world, requests)?;
            if was_accepting && !self.record.borrow().accepting {
                self.wake_cursor = Some(0);
            }
            if progressed {
                return Ok(advance(Step::Runnable));
            }
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
                        self.record.borrow().pid.unwrap_or(0)
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
                    if let Some(control) = self.record.borrow().control
                        && process::query(control)?.state != ProcessState::Dead as u32
                    {
                        process::kill(control)?;
                    }
                    worker.kill_requested = true;
                }
                if !self.record.borrow().collected
                    && let Some(collector) = worker.collector()
                {
                    let result = collector.advance_in(world, requests, input)?;
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
                    self.record.borrow().pid.unwrap_or(0),
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
            if !matches!(worker.process, ProcessOwner::Empty) {
                worker.sources(&self.record.borrow(), requests)?;
            }
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
