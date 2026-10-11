//! 文件流控制表与真实 Runnel owner；会话由原建立 Request 的 Runtime 任务推进。
use crate::stream_state::{Finish, FinishWaiter, Phase, State};
use crate::{
    Funding,
    authority::AccessSnapshot,
    backend::MemoryBackend,
    protocol::{Direction, OpenOptions, Status, StreamInfo, StreamOffer, StreamReason},
    resource::FsResource,
    store::NodeRef,
};
use alloc::{boxed::Box, sync::Arc};
use core::mem::ManuallyDrop;
use erhino_shared::{
    call::SystemCallError,
    message::PAYLOAD_MAX,
    object::{Handle, ObjectSignals, Rights},
    proc::PROCESS_PAGE_SIZE,
    time::{ClockSnapshot, Deadline},
};
use libbudget::Charge;
use libexecution::{
    SourceRegistration,
    runtime::{Advance, Requests, SourcePlan, Step},
};
use libkernel::{
    ipc::{
        invitation::{CreateFailure as TunnelCreateFailure, Invitation},
        message::{Mailbox, MailboxSender},
        tunnel::Endpoint,
    },
    mm::Placement,
};
use librunnel::{
    MappedEndpoint, RunnelError,
    blocking::{Consumer, CreateFailure, Producer},
};
use metadata_admission::{Counter, Permit};
use ordered_table::{OrderedTable, PreparedEntry};

/// 宿主的有限建立窗口和默认 Tunnel 几何；不由调用正文选择付款。
#[derive(Clone, Copy)]
pub struct Config {
    pub offer_window_ns: u64,
    pub tunnel_bytes: usize,
}
fn status(error: SystemCallError) -> Status {
    match error {
        SystemCallError::QuotaExceeded | SystemCallError::ReachLimit => Status::Quota,
        SystemCallError::RightsDenied => Status::Permission,
        SystemCallError::ObjectClosed => Status::Unavailable,
        SystemCallError::ObjectBusy => Status::Busy,
        SystemCallError::IllegalArgument => Status::Invalid,
        _ => Status::Resource,
    }
}
fn io_status(error: RunnelError) -> Status {
    match error {
        RunnelError::Closed => Status::Unavailable,
        RunnelError::TimedOut => Status::Expired,
        RunnelError::BadFormat | RunnelError::Broken => Status::Invalid,
        RunnelError::Syscall(error) => status(error),
    }
}
pub fn invitation_transport() -> Rights {
    Rights::READ | Rights::WRITE | Rights::WAIT | Rights::SIGNAL | Rights::MANAGE | Rights::TRANSIT
}
fn earlier(a: Deadline, b: Deadline) -> Deadline {
    match (a.instant().ok().flatten(), b.instant().ok().flatten()) {
        (Some(a), Some(b)) => Deadline::at(a.min(b)),
        (Some(a), None) | (None, Some(a)) => Deadline::at(a),
        (None, None) => Deadline::INFINITE,
    }
}
enum Plane {
    Empty,
    Producer(Producer),
    Consumer(Consumer),
    Partial {
        mapped: Option<MappedEndpoint>,
        endpoint: Option<Endpoint>,
        invitation: Option<Invitation>,
    },
}
impl Plane {
    fn failed(failure: CreateFailure) -> (Self, Status) {
        match failure {
            CreateFailure::Tunnel(TunnelCreateFailure::System(error)) => {
                (Self::Empty, status(error))
            }
            CreateFailure::Tunnel(TunnelCreateFailure::Published {
                error,
                endpoint,
                invitation,
            }) => (
                Self::Partial {
                    mapped: None,
                    endpoint,
                    invitation,
                },
                status(error),
            ),
            CreateFailure::Map {
                failure,
                invitation,
            } => (
                Self::Partial {
                    mapped: None,
                    endpoint: Some(failure.endpoint),
                    invitation: Some(invitation),
                },
                status(failure.error),
            ),
            CreateFailure::Protocol {
                endpoint,
                invitation,
                error,
            } => (
                Self::Partial {
                    mapped: Some(endpoint),
                    endpoint: None,
                    invitation: Some(invitation),
                },
                io_status(error),
            ),
        }
    }
    /// 来源须先实际注销；关闭失败只重试 Close，不能再读已经解除的映射。
    fn close(&mut self) -> Result<bool, SystemCallError> {
        match core::mem::replace(self, Self::Empty) {
            Self::Empty => Ok(true),
            Self::Producer(owner) => match owner.close() {
                Ok(()) => Ok(true),
                Err((owner, error)) => {
                    *self = Self::Producer(owner);
                    Err(error)
                }
            },
            Self::Consumer(owner) => match owner.close() {
                Ok(()) => Ok(true),
                Err((owner, error)) => {
                    *self = Self::Consumer(owner);
                    Err(error)
                }
            },
            Self::Partial {
                mut mapped,
                mut endpoint,
                mut invitation,
            } => {
                let result = if let Some(owner) = invitation.take() {
                    owner.close().map_err(|(owner, error)| {
                        invitation = Some(owner);
                        error
                    })
                } else if let Some(owner) = mapped.take() {
                    owner.close().map_err(|(owner, error)| {
                        mapped = Some(owner);
                        error
                    })
                } else if let Some(owner) = endpoint.take() {
                    owner.close().map_err(|(owner, error)| {
                        endpoint = Some(owner);
                        error
                    })
                } else {
                    Ok(())
                };
                let done = mapped.is_none() && endpoint.is_none() && invitation.is_none();
                if !done {
                    *self = Self::Partial {
                        mapped,
                        endpoint,
                        invitation,
                    };
                }
                result.map(|()| done)
            }
        }
    }
    fn terminal_plan(&self) -> Option<SourcePlan> {
        match self {
            Self::Producer(owner) => Some(owner.terminal_wait_plan()),
            Self::Consumer(owner) => Some(owner.terminal_wait_plan()),
            Self::Empty | Self::Partial { .. } => None,
        }
    }
    fn plan(&mut self) -> Result<Option<SourcePlan>, Status> {
        match self {
            Self::Producer(owner) => owner.wait_plan().map(Some).map_err(|e| io_status(e.error)),
            Self::Consumer(owner) => owner.wait_plan().map(Some).map_err(|e| io_status(e.error)),
            Self::Empty | Self::Partial { .. } => Ok(None),
        }
    }
}
struct Entry {
    state: State,
    funding: Funding,
    target: Option<(AccessSnapshot, NodeRef)>,
    plane: Plane,
    driver: u64,
    start: u64,
    read_end: u64,
    scratch: Box<[u8; PAYLOAD_MAX]>,
    pending_bytes: usize,
    eof_sent: bool,
    blocked_writer: bool,
    wake_driver: bool,
    wake_finish: bool,
    peer_closed: bool,
    force_retire: bool,
    cleanup_at: Option<u64>,
    _slot: Permit,
    _stream: Charge,
    offer: Option<Charge>,
    mapped_bytes: Option<Charge>,
    data_source: Option<Charge>,
    _control_source: Charge,
    _storage: Charge,
}
/// 预建记录先于 Tunnel 出生；失败也保留同一记录/清理承载。
pub(crate) struct PreparedStream {
    entry: ManuallyDrop<PreparedEntry<Entry, ()>>,
    pub(crate) sender: Option<MailboxSender>,
    pub(crate) lifetime: Option<libkernel::ipc::message::LifetimeObserver>,
    pub(crate) invitation: Option<Invitation>,
    pub(crate) error: Option<Status>,
    pub(crate) offer: StreamOffer,
    pub(crate) sender_rights: Rights,
    entry_released: bool,
}
pub struct StreamTable {
    records: ManuallyDrop<OrderedTable<Entry, ()>>,
    slots: Arc<Counter>,
    sealed: bool,
    wake_after: Option<u64>,
    wake_scanning: bool,
    wake_dirty: bool,
}
impl StreamTable {
    pub fn new(limit: usize) -> Result<Self, Status> {
        if limit == 0 {
            return Err(Status::Invalid);
        }
        Ok(Self {
            records: ManuallyDrop::new(OrderedTable::new(limit)),
            slots: Arc::try_new(Counter::new(limit)).map_err(|_| Status::Resource)?,
            sealed: false,
            wake_after: None,
            wake_scanning: false,
            wake_dirty: false,
        })
    }
    #[expect(
        clippy::too_many_arguments,
        reason = "真实建立边界显式借用目标、授权、运输、期限与宿主部署政策"
    )]
    pub(crate) fn prepare(
        &self,
        mailbox: &Mailbox,
        access: &AccessSnapshot,
        target: NodeRef,
        options: OpenOptions,
        request_deadline: Deadline,
        clock: ClockSnapshot,
        read_end: u64,
        driver: u64,
        pool: Handle,
        config: Config,
    ) -> Result<PreparedStream, Status> {
        if self.sealed || !access.is_active() {
            return Err(Status::Revoked);
        }
        if driver == 0 {
            return Err(Status::Invalid);
        }
        let mandatory = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
        let output = access.output_transport();
        if !output.contains(mandatory | invitation_transport()) {
            return Err(Status::Permission);
        }
        let sender_rights = mandatory | (output & Rights::DUPLICATE);
        let requested = if options.tunnel_bytes == 0 {
            config.tunnel_bytes
        } else {
            options.tunnel_bytes as usize
        };
        let bytes = requested
            .checked_add(PROCESS_PAGE_SIZE - 1)
            .map(|n| n / PROCESS_PAGE_SIZE * PROCESS_PAGE_SIZE)
            .ok_or(Status::Range)?;
        if bytes < PROCESS_PAGE_SIZE
            || bytes > erhino_shared::tunnel::TUNNEL_MAX_PAGES as usize * PROCESS_PAGE_SIZE
        {
            return Err(Status::Invalid);
        }
        let offer_deadline = earlier(
            earlier(request_deadline, options.session_deadline),
            clock
                .after_ns(config.offer_window_ns)
                .map_err(|_| Status::Range)?,
        );
        let account = access.account();
        let slot = Counter::try_acquire(&self.slots).map_err(|_| Status::Quota)?;
        let stream = account.acquire(FsResource::Stream, 1).map_err(status)?;
        let offer = account.acquire(FsResource::Offer, 1).map_err(status)?;
        let storage = account
            .acquire(
                FsResource::Bytes,
                PreparedEntry::<Entry, ()>::allocation_bytes() + PAYLOAD_MAX,
            )
            .map_err(status)?;
        let mapped_bytes = account.acquire(FsResource::Bytes, bytes).map_err(status)?;
        let control_source = account.acquire(FsResource::WaitSource, 1).map_err(status)?;
        let data_source = account.acquire(FsResource::WaitSource, 1).map_err(status)?;
        let buffer = Box::<[u8; PAYLOAD_MAX]>::try_new_zeroed().map_err(|_| Status::Resource)?;
        // SAFETY: try_new_zeroed 完整初始化该 u8 数组。
        let scratch = unsafe { buffer.assume_init() };
        let minted = mailbox.mint(0, sender_rights).map_err(status)?;
        let context = minted.sender.description().map_err(status)?.object_id;
        let state = State::new(
            context,
            options.direction,
            offer_deadline,
            options.session_deadline,
        )?;
        let entry = Entry {
            state,
            funding: access.funding().clone(),
            target: Some((access.clone(), target)),
            plane: Plane::Empty,
            driver,
            start: options.offset,
            read_end,
            scratch,
            pending_bytes: 0,
            eof_sent: false,
            blocked_writer: false,
            wake_driver: false,
            wake_finish: false,
            peer_closed: false,
            force_retire: false,
            cleanup_at: None,
            _slot: slot,
            _stream: stream,
            offer: Some(offer),
            mapped_bytes: Some(mapped_bytes),
            data_source: Some(data_source),
            _control_source: control_source,
            _storage: storage,
        };
        let mut entry = self
            .records
            .prepare_insert(context, entry, &libbudget::AllocatorBacked)
            .map_err(|error| match error {
                ordered_table::InsertError::Limit(_) => Status::Quota,
                ordered_table::InsertError::Storage(error, _) => {
                    status(libbudget::allocator_error(error))
                }
            })?;
        let (plane, invitation, error) = match options.direction {
            Direction::Read => match Producer::create(pool, bytes, Placement::Anywhere) {
                Ok((owner, invitation)) => (Plane::Producer(owner), Some(invitation), None),
                Err(failure) => {
                    let (plane, error) = Plane::failed(failure);
                    (plane, None, Some(error))
                }
            },
            Direction::Write => match Consumer::create(pool, bytes, Placement::Anywhere) {
                Ok((owner, invitation)) => (Plane::Consumer(owner), Some(invitation), None),
                Err(failure) => {
                    let (plane, error) = Plane::failed(failure);
                    (plane, None, Some(error))
                }
            },
        };
        entry.value_mut().plane = plane;
        Ok(PreparedStream {
            entry: ManuallyDrop::new(entry),
            sender: Some(minted.sender),
            lifetime: Some(minted.lifetime),
            invitation,
            error,
            sender_rights,
            entry_released: false,
            offer: StreamOffer {
                session_id: context,
                direction: options.direction,
                tunnel_bytes: bytes as u32,
                start: options.offset,
                read_end,
                offer_deadline,
            },
        })
    }
    pub fn funding(&self, context: u64) -> Option<&Funding> {
        self.records.get(context).map(|entry| &entry.funding)
    }
    pub fn query(&self, context: u64) -> Result<StreamInfo, Status> {
        self.records
            .get(context)
            .map(|entry| entry.state.info())
            .ok_or(Status::Permission)
    }
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}
impl PreparedStream {
    pub fn context(&self) -> u64 {
        self.offer.session_id
    }
    pub fn terminal_plan(&mut self) -> Option<SourcePlan> {
        self.entry.value_mut().plane.terminal_plan()
    }
}

impl StreamTable {
    pub fn start(&mut self, context: u64, now: u64) -> Result<StreamInfo, Status> {
        let entry = self.records.get_mut(context).ok_or(Status::Permission)?;
        entry.state.observe_time(now);
        if matches!(entry.state.phase(), Phase::Stopping { .. }) {
            entry.wake_driver = true;
            self.wake_dirty = true;
            return Err(Status::Conflict);
        }
        let ready = match entry.state.phase() {
            Phase::Offered => {
                let result = match &mut entry.plane {
                    Plane::Producer(owner) => owner
                        .poll(ObjectSignals::NONE)
                        .map(|_| owner.peer_started()),
                    Plane::Consumer(owner) => owner
                        .poll(ObjectSignals::NONE)
                        .map(|_| owner.peer_started()),
                    Plane::Empty | Plane::Partial { .. } => return Err(Status::Unavailable),
                };
                match result {
                    Ok(ready) => ready,
                    Err(error) => {
                        let outcome = io_status(error.error);
                        let reason = if outcome == Status::Unavailable {
                            StreamReason::PeerClosed
                        } else {
                            StreamReason::Backend
                        };
                        entry.state.stop(outcome, reason);
                        entry.wake_driver = true;
                        self.wake_dirty = true;
                        return Err(outcome);
                    }
                }
            }
            Phase::Active => return Ok(entry.state.info()),
            Phase::Stopping { .. } | Phase::Terminal { .. } => return Err(Status::Conflict),
        };
        let info = entry.state.start(ready, now)?;
        entry.offer = None;
        entry.wake_driver = true;
        self.wake_dirty = true;
        Ok(info)
    }
    pub fn cancel(&mut self, context: u64) -> Result<StreamInfo, Status> {
        let entry = self.records.get_mut(context).ok_or(Status::Permission)?;
        let info = entry.state.cancel();
        entry.wake_driver = true;
        self.wake_dirty = true;
        Ok(info)
    }
    pub fn finish(
        &mut self,
        context: u64,
        task: u64,
        deadline: Deadline,
        now: u64,
    ) -> Result<Option<StreamInfo>, Status> {
        if task == 0 || deadline.instant().is_err() {
            return Err(Status::Invalid);
        }
        let entry = self.records.get_mut(context).ok_or(Status::Permission)?;
        if let Some(waiter) = entry.state.finish_waiter()
            && waiter
                .deadline
                .instant()
                .ok()
                .flatten()
                .is_some_and(|at| at <= now)
        {
            // 旧请求仍由自己的 Outbox/Runtime 期限收束，不能占住新的等待槽。
            entry.state.release_finish(waiter.task);
        }
        match entry.state.finish(FinishWaiter { task, deadline })? {
            Finish::Ready(info) => Ok(Some(info)),
            Finish::Wait => {
                entry.wake_driver = true;
                self.wake_dirty = true;
                Ok(None)
            }
        }
    }
    pub fn release_finish(&mut self, context: u64, task: u64) {
        if let Some(entry) = self.records.get_mut(context) {
            entry.state.release_finish(task);
            entry.wake_driver = true;
            self.wake_dirty = true;
        }
    }
    pub fn has_finish_waiter(&self, context: u64) -> bool {
        self.records
            .get(context)
            .is_some_and(|entry| entry.state.finish_waiter().is_some())
    }
    pub fn stop(&mut self, context: u64, outcome: Status, reason: StreamReason) {
        if let Some(entry) = self.records.get_mut(context) {
            let before = entry.state.phase();
            let retiring = entry.force_retire;
            entry.state.stop(outcome, reason);
            entry.force_retire = true;
            if entry.state.phase() != before || !retiring {
                entry.wake_driver = true;
                self.wake_dirty = true;
            }
        }
    }
    pub fn seal(&mut self) {
        self.sealed = true;
        let mut after = None;
        while let Some((&context, _)) = self.records.next_after::<u64>(after.as_ref()) {
            after = Some(context);
            self.stop(context, Status::Unavailable, StreamReason::ProviderStopped);
        }
    }
    /// 后端变化只要求有界重查；不保存文件内容/长度的第二份真值。
    pub fn backend_changed(&mut self) {
        self.wake_dirty = true;
    }
    pub fn has_wake_work(&self) -> bool {
        self.wake_dirty || self.wake_scanning
    }
    pub fn wake_step<F>(&mut self, backend: &MemoryBackend, requests: &mut Requests<'_, F>) {
        if !self.wake_scanning {
            if !self.wake_dirty {
                return;
            }
            self.wake_scanning = true;
            self.wake_dirty = false;
            self.wake_after = None;
        }
        let Some((&context, _)) = self.records.next_after::<u64>(self.wake_after.as_ref()) else {
            self.wake_scanning = false;
            return;
        };
        self.wake_after = Some(context);
        let entry = self
            .records
            .get_mut(context)
            .expect("selected stream record disappeared");
        if let Some((access, target)) = entry.target.as_ref() {
            if !access.is_active() {
                entry.state.stop(Status::Revoked, StreamReason::Revoked);
                entry.wake_driver = true;
            }
            if entry.blocked_writer && !backend.stream_is_trimming(target).unwrap_or(false) {
                entry.blocked_writer = false;
                entry.wake_driver = true;
            }
        }
        if entry.wake_driver {
            if requests.wake(entry.driver).is_ok() {
                entry.wake_driver = false;
            } else {
                self.wake_dirty = true;
            }
        }
        if entry.wake_finish {
            if let Some(waiter) = entry.state.finish_waiter() {
                if requests.wake(waiter.task).is_ok() {
                    entry.wake_finish = false;
                } else {
                    self.wake_dirty = true;
                }
            } else {
                entry.wake_finish = false;
            }
        }
    }
}

impl Entry {
    fn fail(&mut self, outcome: Status) {
        let reason = if outcome == Status::Unavailable {
            StreamReason::PeerClosed
        } else if outcome == Status::Revoked {
            StreamReason::Revoked
        } else {
            StreamReason::Backend
        };
        self.state.stop(outcome, reason);
    }
    fn progress(
        &mut self,
        backend: &mut MemoryBackend,
        observed: ObjectSignals,
    ) -> Result<Step, Status> {
        let (access, target) = self.target.as_ref().ok_or(Status::Revoked)?;
        if !access.is_active() {
            return Err(Status::Revoked);
        }
        match &mut self.plane {
            Plane::Producer(owner) => {
                let free = owner.writable().map_err(|e| io_status(e.error))?;
                let inflight = owner.capacity() - free;
                let confirmed = self
                    .state
                    .info()
                    .transported
                    .checked_sub(inflight as u64)
                    .ok_or(Status::Invalid)?;
                self.state.confirm_read(confirmed)?;
                if self.eof_sent {
                    if matches!(
                        owner.poll(observed).map_err(|e| io_status(e.error))?,
                        Some(librunnel::ProducerReady::EofConsumed)
                    ) {
                        self.state.confirm_read(self.state.info().transported)?;
                        self.state.stop(Status::Ok, StreamReason::Completed);
                        return Ok(Step::Runnable);
                    }
                    return Ok(Step::Parked);
                }
                let position = self
                    .start
                    .checked_add(self.state.info().transported)
                    .ok_or(Status::Range)?;
                let available = self
                    .read_end
                    .saturating_sub(position)
                    .min(free as u64)
                    .min(PAYLOAD_MAX as u64) as usize;
                if position >= self.read_end {
                    owner.finish().map_err(|e| io_status(e.error))?;
                    self.eof_sent = true;
                    return Ok(Step::Runnable);
                }
                if available == 0 {
                    return Ok(
                        match owner.poll(observed).map_err(|e| io_status(e.error))? {
                            Some(_) => Step::Runnable,
                            None => Step::Parked,
                        },
                    );
                }
                let read =
                    backend.read_at(access, target, position, &mut self.scratch[..available])?;
                if read == 0 {
                    owner.finish().map_err(|e| io_status(e.error))?;
                    self.eof_sent = true;
                    return Ok(Step::Runnable);
                }
                match owner.write(&self.scratch[..read]) {
                    Ok(written) => {
                        self.state.add_transported(written)?;
                        Ok(if written == 0 {
                            Step::Parked
                        } else {
                            Step::Runnable
                        })
                    }
                    Err(error) => {
                        self.state.add_transported(error.completed)?;
                        Err(io_status(error.error))
                    }
                }
            }
            Plane::Consumer(owner) => {
                if backend.stream_is_trimming(target)? {
                    self.blocked_writer = true;
                    return Ok(Step::Parked);
                }
                if self.pending_bytes == 0 {
                    let ready = owner.poll(observed).map_err(|e| io_status(e.error))?;
                    match ready {
                        Some(librunnel::ConsumerReady::EofDrained) => {
                            self.state.stop(Status::Ok, StreamReason::Completed);
                            return Ok(Step::Runnable);
                        }
                        Some(librunnel::ConsumerReady::Readable { bytes, .. }) => {
                            let remaining = u64::MAX - self.state.info().transported;
                            let count = bytes
                                .min(PAYLOAD_MAX)
                                .min(remaining.min(usize::MAX as u64) as usize);
                            if count == 0 {
                                return Err(Status::Range);
                            }
                            match owner.read(&mut self.scratch[..count]) {
                                Ok(read) => {
                                    self.state.add_transported(read)?;
                                    self.pending_bytes = read;
                                }
                                Err(error) => {
                                    self.state.add_transported(error.completed)?;
                                    self.pending_bytes = error.completed;
                                    return Err(io_status(error.error));
                                }
                            }
                        }
                        Some(librunnel::ConsumerReady::Started) => return Ok(Step::Runnable),
                        None => return Ok(Step::Parked),
                    }
                }
                if self.pending_bytes == 0 {
                    return Ok(Step::Parked);
                }
                let offset = self
                    .start
                    .checked_add(self.state.info().accepted)
                    .ok_or(Status::Range)?;
                let write = match backend.prepare_write(
                    access,
                    target.clone(),
                    offset,
                    &self.scratch[..self.pending_bytes],
                ) {
                    Ok(write) => write,
                    Err(Status::Busy) => {
                        self.blocked_writer = true;
                        return Ok(Step::Parked);
                    }
                    Err(status) => return Err(status),
                };
                backend.commit(write).map_err(|failed| failed.status)?;
                self.state.accept_write(self.pending_bytes)?;
                self.pending_bytes = 0;
                Ok(Step::Runnable)
            }
            Plane::Empty | Plane::Partial { .. } => Err(Status::Unavailable),
        }
    }
}
impl StreamTable {
    pub fn deadline(&self, context: u64) -> Deadline {
        self.records
            .get(context)
            .map_or(Deadline::INFINITE, |entry| {
                entry
                    .cleanup_at
                    .map_or_else(|| entry.state.deadline(), Deadline::at)
            })
    }
    pub fn data_released(&self, context: u64) -> bool {
        self.records
            .get(context)
            .is_none_or(|entry| matches!(entry.plane, Plane::Empty))
    }
    /// 每推进最多一个数据块或一次清理；来源撤销完成前一直保管映射。
    #[expect(
        clippy::too_many_arguments,
        reason = "真实会话推进借用原任务来源与唯一 backing"
    )]
    pub fn advance<F>(
        &mut self,
        context: u64,
        backend: &mut MemoryBackend,
        source: &mut SourceRegistration,
        active_source: &mut bool,
        kind: u64,
        requests: &mut Requests<'_, F>,
        now: u64,
        observed: ObjectSignals,
    ) -> Result<Advance, SystemCallError> {
        let Some(entry) = self.records.get_mut(context) else {
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        };
        entry.state.observe_time(now);
        if observed.intersects(ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED) {
            entry.peer_closed = true;
            entry
                .state
                .stop(Status::Unavailable, StreamReason::PeerClosed);
        }
        if entry
            .target
            .as_ref()
            .is_some_and(|(access, _)| !access.is_active())
        {
            entry.state.stop(Status::Revoked, StreamReason::Revoked);
        }
        let mut step = Step::Parked;
        if entry.state.phase() == Phase::Active {
            match entry.progress(backend, observed) {
                Ok(progress) => step = progress,
                Err(status) => {
                    entry.fail(status);
                    step = Step::Runnable;
                }
            }
        }
        if matches!(entry.state.phase(), Phase::Stopping { .. }) {
            // 此内存后端没有在途异步写；pending 未接受块丢弃，不再提交。
            if let Plane::Producer(owner) = &mut entry.plane
                && let Ok(free) = owner.writable()
                && let Some(confirmed) = entry
                    .state
                    .info()
                    .transported
                    .checked_sub((owner.capacity() - free) as u64)
            {
                let _ = entry.state.confirm_read(confirmed);
            }
            entry.pending_bytes = 0;
            entry.wake_finish |= entry.state.freeze().is_some();
            entry.target = None;
            self.wake_dirty |= entry.wake_finish;
        }
        let retain_completed_reader = matches!(
            entry.state.phase(),
            Phase::Terminal {
                outcome: Status::Ok,
                reason: StreamReason::Completed
            }
        ) && matches!(entry.plane, Plane::Producer(_))
            && !entry.peer_closed
            && !entry.force_retire;
        if matches!(entry.state.phase(), Phase::Terminal { .. }) && !retain_completed_reader {
            if !source.is_empty() {
                if source.remove(requests).is_err() {
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Runnable,
                    });
                }
                return Ok(Advance {
                    work_done: 1,
                    step: if source.is_removing() || source.is_installing() {
                        Step::Parked
                    } else {
                        Step::Runnable
                    },
                });
            }
            if entry.cleanup_at.is_some_and(|at| now < at) {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            entry.cleanup_at = None;
            match entry.plane.close() {
                Ok(true) => {
                    entry.offer = None;
                    entry.mapped_bytes = None;
                    entry.data_source = None;
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Parked,
                    });
                }
                Ok(false) => {
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Runnable,
                    });
                }
                Err(_) => {
                    entry.cleanup_at =
                        Some(now.saturating_add(libexecution::runtime::CLEANUP_RETRY_NS));
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Parked,
                    });
                }
            }
        }
        let desired_active = entry.state.phase() == Phase::Active && !entry.blocked_writer;
        if !source.is_empty() && *active_source != desired_active {
            if source.remove(requests).is_err() {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                });
            }
            return Ok(Advance {
                work_done: 1,
                step: if source.is_removing() || source.is_installing() {
                    Step::Parked
                } else {
                    Step::Runnable
                },
            });
        }
        if source.is_empty() {
            let plan = if desired_active {
                match entry.plane.plan() {
                    Ok(plan) => plan,
                    Err(status) => {
                        entry.fail(status);
                        return Ok(Advance {
                            work_done: 1,
                            step: Step::Runnable,
                        });
                    }
                }
            } else {
                entry.plane.terminal_plan()
            };
            if let Some(plan) = plan {
                *active_source = desired_active;
                if source.install(requests, plan, kind).is_err() {
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Runnable,
                    });
                }
                step = Step::Runnable;
            }
        }
        Ok(Advance { work_done: 1, step })
    }
}

impl PreparedStream {
    pub fn cleanup_deadline(&mut self) -> Deadline {
        self.entry
            .value_mut()
            .cleanup_at
            .map_or(Deadline::INFINITE, Deadline::at)
    }
    /// 未公开准备的所有数据来源亦须先由 Request 注销；失败仍保留同一准备。
    pub fn cleanup(&mut self, now: u64) -> Result<bool, SystemCallError> {
        let entry = self.entry.value_mut();
        if entry.cleanup_at.is_some_and(|at| now < at) {
            return Ok(false);
        }
        entry.cleanup_at = None;
        match entry.plane.close() {
            Ok(false) => return Ok(false),
            Err(error) => {
                entry.cleanup_at =
                    Some(now.saturating_add(libexecution::runtime::CLEANUP_RETRY_NS));
                return Err(error);
            }
            Ok(true) => {}
        }
        if let Some(invitation) = self.invitation.take()
            && let Err((invitation, error)) = invitation.close()
        {
            self.invitation = Some(invitation);
            entry.cleanup_at = Some(now.saturating_add(libexecution::runtime::CLEANUP_RETRY_NS));
            return Err(error);
        }
        if let Some(sender) = self.sender.take()
            && let Err((sender, error)) = sender.close()
        {
            self.sender = Some(sender);
            entry.cleanup_at = Some(now.saturating_add(libexecution::runtime::CLEANUP_RETRY_NS));
            return Err(error);
        }
        Ok(true)
    }
}
impl Drop for PreparedStream {
    fn drop(&mut self) {
        if !self.entry_released
            && matches!(self.entry.value_mut().plane, Plane::Empty)
            && self.sender.is_none()
            && self.lifetime.is_none()
            && self.invitation.is_none()
        {
            // SAFETY: 只在 entry 尚未取走且映射/Endpoint 已实际释放后析构一次；
            // 异常丢弃未清准备时保留其 owner/Charge，不伪装退款。
            unsafe {
                ManuallyDrop::drop(&mut self.entry);
            }
        }
    }
}
impl StreamTable {
    pub(crate) fn can_install(&self, prepared: &mut PreparedStream) -> Result<(), Status> {
        if self.sealed {
            return Err(Status::Revoked);
        }
        if let Some(status) = prepared.error {
            return Err(status);
        }
        if prepared
            .entry
            .value_mut()
            .target
            .as_ref()
            .is_none_or(|(access, _)| !access.is_active())
        {
            return Err(Status::Revoked);
        }
        Ok(())
    }
    pub(crate) fn install(&mut self, mut prepared: PreparedStream) {
        assert!(
            !self.sealed && prepared.error.is_none(),
            "validated stream publication changed"
        );
        assert!(
            prepared.sender.is_none()
                && prepared.invitation.is_none()
                && prepared.lifetime.is_none(),
            "stream publication retained transferable owners"
        );
        prepared.entry_released = true;
        // SAFETY: released 标记先关闭唯一 Drop 路径，预建 entry 完整移交给唯一表。
        let entry = unsafe { ManuallyDrop::take(&mut prepared.entry) };
        self.records.insert_prepared(entry);
    }
    pub fn remove(&mut self, context: u64) {
        if let Some(entry) = self.records.get(context) {
            assert!(
                matches!(entry.plane, Plane::Empty),
                "live stream data retired before release"
            );
        }
        drop(self.records.remove(context));
    }
}
impl Drop for StreamTable {
    fn drop(&mut self) {
        if self.records.is_empty() {
            // SAFETY: 空表仅在这里析构一次；非空表保留全部未收束业务 owner。
            unsafe {
                ManuallyDrop::drop(&mut self.records);
            }
        }
    }
}
