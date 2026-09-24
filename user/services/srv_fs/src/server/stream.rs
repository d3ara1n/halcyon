use super::*;
use alloc::sync::Arc;
use librunnel::blocking;
use metadata_admission::{Counter, Permit};
use ordered_table::{InsertError, OrderedTable, PreparedEntry};
use rinlib::{
    ipc::{
        invitation::Invitation,
        tunnel::{Endpoint, EndpointCleanup},
    },
    mm::Placement,
};

pub(super) enum StreamRole {
    Producer(blocking::Producer),
    Consumer(blocking::Consumer),
}

impl StreamRole {
    pub(super) fn close(self) -> Result<(), (Self, SystemCallError)> {
        match self {
            Self::Producer(role) => role
                .close()
                .map_err(|(role, error)| (Self::Producer(role), error)),
            Self::Consumer(role) => role
                .close()
                .map_err(|(role, error)| (Self::Consumer(role), error)),
        }
    }
}

pub(super) struct StreamEntry {
    pub _slot: Permit,
    pub node: NodeRef,
    pub access: AccessSnapshot,
    pub direction: protocol::StreamDirection,
    pub role: Option<StreamRole>,
    pub failed_endpoint: Option<Endpoint>,
    pub failed_cleanup: Option<EndpointCleanup>,
    pub invitation: Option<Invitation>,
    pub sender: Option<MailboxSender>,
    pub lifetime: Option<Capability>,
    pub _slot_charge: Charge,
    pub lifetime_charge: Option<Charge>,
    pub data_charge: Option<Charge>,
    pub state: protocol::StreamState,
    pub outcome: protocol::Status,
    pub reason: protocol::StreamReason,
    pub accepted: u64,
    pub transported: u64,
    pub offset: u64,
    pub read_end: u64,
    pub limit: Option<u64>,
    pub buffer: [u8; 1024],
    pub buffered: usize,
    pub buffer_sent: usize,
    pub pending_error: Option<(protocol::Status, protocol::StreamReason)>,
    pub finished: bool,
    pub waiter: Option<u64>,
    pub wake_sent: bool,
    pub task: u64,
    pub session_deadline: Deadline,
    pub offer_deadline: Deadline,
}

impl StreamEntry {
    pub(super) fn info(&self, identity: u64) -> protocol::StreamInfo {
        protocol::StreamInfo {
            identity,
            state: self.state,
            outcome: self.outcome,
            reason: self.reason,
            accepted: self.accepted,
            transported: self.transported,
        }
    }

    pub(super) fn terminal(&mut self, outcome: protocol::Status, reason: protocol::StreamReason) {
        if self.state != protocol::StreamState::Terminal {
            self.outcome = outcome;
            self.reason = reason;
            self.state = protocol::StreamState::Terminal;
        }
    }

    fn refresh_read_accepted(&mut self) -> Result<(), librunnel::IoError> {
        if self.state == protocol::StreamState::Active
            && let Some(StreamRole::Producer(role)) = self.role.as_mut()
        {
            let free = role.writable()?;
            let in_flight = role.capacity().saturating_sub(free) as u64;
            self.accepted = self
                .accepted
                .max(self.transported.saturating_sub(in_flight));
        }
        Ok(())
    }
}

pub(super) struct StreamTable {
    entries: OrderedTable<StreamEntry>,
    slots: Arc<Counter>,
}

impl StreamTable {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            entries: OrderedTable::new(limit),
            slots: Arc::new(Counter::new(limit)),
        }
    }

    pub(super) fn acquire_slot(&self) -> Result<Permit, protocol::Status> {
        Counter::try_acquire(&self.slots).map_err(|_| protocol::Status::Quota)
    }
    // 失败必须原样返还仍持 NodeRef、能力和账户的表项，不能在 OOM 时补分配 Box。
    #[allow(clippy::result_large_err)]
    pub(super) fn prepare(
        &self,
        entry: StreamEntry,
    ) -> Result<PreparedEntry<StreamEntry>, (protocol::Status, StreamEntry)> {
        self.entries
            .prepare_insert(0, entry)
            .map_err(|error| match error {
                InsertError::Limit(entry) => (protocol::Status::Quota, entry),
                InsertError::Allocation(entry) => (protocol::Status::Resource, entry),
            })
    }

    pub(super) fn install(
        &mut self,
        identity: u64,
        entry: PreparedEntry<StreamEntry>,
    ) -> Result<(), (protocol::Status, PreparedEntry<StreamEntry>)> {
        if identity == 0 || self.entries.get(identity).is_some() {
            return Err((protocol::Status::Invalid, entry));
        }
        self.entries.insert_prepared(entry.with_key(identity));
        Ok(())
    }

    pub(super) fn get(&self, identity: u64) -> Option<&StreamEntry> {
        self.entries.get(identity)
    }

    pub(super) fn get_mut(&mut self, identity: u64) -> Option<&mut StreamEntry> {
        self.entries.get_mut(identity)
    }

    pub(super) fn remove(&mut self, identity: u64) -> Option<StreamEntry> {
        self.entries.remove(identity)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub(super) struct OpenArgs {
    pub path: String,
    pub expected_identity: Option<core::num::NonZeroU64>,
    pub direction: protocol::StreamDirection,
    pub offset: u64,
    pub length: Option<u64>,
    pub session_deadline: Deadline,
    pub stream_protocol: u32,
    pub tunnel_bytes: u32,
}

pub(super) struct StreamTask {
    outbox: Option<Outbox>,
    header: protocol::Header,
    reply_deadline: Deadline,
    access: Option<AccessSnapshot>,
    args: Option<Result<OpenArgs, protocol::Status>>,
    pending: Option<PreparedEntry<StreamEntry>>,
    identity: Option<u64>,
    lifetime_source: Option<SourceId>,
    data_source: Option<SourceId>,
    lifetime_requested: bool,
    data_requested: bool,
    lifetime_removing: bool,
    data_removing: bool,
    returned: Vec<Capability>,
    wakes: Option<WakeBatch>,
    prepared: bool,
    encoded: bool,
    stopping: bool,
    retiring: bool,
    failure: protocol::Status,
    offer_deadline: Deadline,
    session_deadline: Deadline,
    current_deadline: Deadline,
    retry_deadline: Option<Deadline>,
}

impl StreamTask {
    pub(super) fn new(
        outbox: Outbox,
        header: protocol::Header,
        reply_deadline: Deadline,
        access: AccessSnapshot,
        args: Result<OpenArgs, protocol::Status>,
    ) -> Self {
        let session_deadline = args
            .as_ref()
            .map(|args| args.session_deadline)
            .unwrap_or(header.deadline);
        Self {
            outbox: Some(outbox),
            header,
            reply_deadline,
            access: Some(access),
            args: Some(args),
            pending: None,
            identity: None,
            lifetime_source: None,
            data_source: None,
            lifetime_requested: false,
            data_requested: false,
            lifetime_removing: false,
            data_removing: false,
            returned: Vec::new(),
            wakes: None,
            prepared: false,
            encoded: false,
            stopping: false,
            retiring: false,
            failure: protocol::Status::Ok,
            offer_deadline: reply_deadline,
            session_deadline,
            current_deadline: reply_deadline,
            retry_deadline: None,
        }
    }

    pub(super) fn into_refused_outbox(mut self) -> Outbox {
        self.outbox
            .take()
            .expect("Open task admission lost reply owner")
    }

    fn entry_mut(&mut self) -> Option<&mut StreamEntry> {
        self.pending.as_mut().map(|entry| entry.value_mut())
    }

    fn prepare<B: RuntimeBackend>(
        &mut self,
        id: u64,
        world: &mut World<B>,
    ) -> Result<(), protocol::Status> {
        let args = self.args.take().ok_or(protocol::Status::Internal)??;
        if args.stream_protocol != protocol::RNL2_PROTOCOL
            || (args.tunnel_bytes != 0
                && args.tunnel_bytes as usize != 3 * erhino_shared::proc::PROCESS_PAGE_SIZE)
        {
            return Err(protocol::Status::Unsupported);
        }
        let now = rinlib::time::Instant::now()
            .map_err(|_| protocol::Status::Internal)?
            .as_nanos();
        let session = args
            .session_deadline
            .instant()
            .map_err(|_| protocol::Status::Invalid)?
            .ok_or(protocol::Status::Invalid)?;
        if session <= now {
            return Err(protocol::Status::Cancelled);
        }
        let reply_end = self
            .reply_deadline
            .instant()
            .map_err(|_| protocol::Status::Invalid)?
            .ok_or(protocol::Status::Invalid)?;
        if reply_end <= now {
            return Err(protocol::Status::Cancelled);
        }
        let offer_end = reply_end
            .checked_add(2_000_000_000)
            .ok_or(protocol::Status::Invalid)?
            .min(session);
        self.offer_deadline = Deadline::at(offer_end);

        let access = self.access.as_ref().ok_or(protocol::Status::Internal)?;
        let backend = world
            .provider
            .backend
            .as_ref()
            .ok_or(protocol::Status::Cancelled)?;
        let node = FalBackend::resolve(backend, access, &args.path).map_err(backend_status)?;
        if args
            .expected_identity
            .is_some_and(|expected| node.id().raw() != expected.get())
        {
            return Err(protocol::Status::Conflict);
        }
        let metadata =
            FalBackend::metadata(backend, &node, access.rights()).map_err(backend_status)?;
        let required = match args.direction {
            protocol::StreamDirection::Read => FalRights::READ_STREAM,
            protocol::StreamDirection::Write => FalRights::WRITE_STREAM,
        };
        if !metadata.rights.contains(required) {
            return Err(protocol::Status::Permission);
        }
        if metadata.kind != NodeKind::Stream {
            return Err(protocol::Status::Invalid);
        }
        let bounded_end = args
            .length
            .map(|length| {
                args.offset
                    .checked_add(length)
                    .ok_or(protocol::Status::Invalid)
            })
            .transpose()?;
        if args.direction == protocol::StreamDirection::Write {
            self.wakes = Some(WakeBatch::new(WATCH_LIMIT).map_err(|_| protocol::Status::Resource)?);
        }
        let slot = world.streams.acquire_slot()?;
        let charge = access
            .account()
            .acquire(
                FalResource::Bytes,
                PreparedEntry::<StreamEntry>::allocation_bytes(),
            )
            .map_err(stream_status)?;
        let lifetime_charge = access
            .account()
            .acquire(FalResource::WaitSource, 1)
            .map_err(stream_status)?;
        let data_charge = access
            .account()
            .acquire(FalResource::WaitSource, 1)
            .map_err(stream_status)?;
        let read_end = if args.direction == protocol::StreamDirection::Read {
            bounded_end
                .unwrap_or(u64::MAX)
                .min(metadata.size.max(args.offset))
        } else {
            0
        };
        let entry = StreamEntry {
            _slot: slot,
            node,
            access: self.access.take().ok_or(protocol::Status::Internal)?,
            direction: args.direction,
            role: None,
            failed_endpoint: None,
            failed_cleanup: None,
            invitation: None,
            sender: None,
            lifetime: None,
            _slot_charge: charge,
            lifetime_charge: Some(lifetime_charge),
            data_charge: Some(data_charge),
            state: protocol::StreamState::Offered,
            outcome: protocol::Status::Ok,
            reason: protocol::StreamReason::None,
            accepted: 0,
            transported: 0,
            offset: args.offset,
            read_end,
            limit: bounded_end,
            buffer: [0; 1024],
            buffered: 0,
            buffer_sent: 0,
            pending_error: None,
            finished: false,
            waiter: None,
            wake_sent: false,
            task: id,
            session_deadline: args.session_deadline,
            offer_deadline: self.offer_deadline,
        };
        self.pending = Some(
            world
                .streams
                .prepare(entry)
                .map_err(|(error, _entry)| error)?,
        );
        self.prepared = true;
        Ok(())
    }

    fn issue<B: RuntimeBackend>(&mut self, world: &mut World<B>) -> Result<(), protocol::Status> {
        let minted = world
            .provider
            .mailbox
            .mint(0, Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
            .map_err(stream_status)?;
        let entry = self.entry_mut().ok_or(protocol::Status::Internal)?;
        entry.sender = Some(minted.sender);
        entry.lifetime = Some(minted.lifetime);
        let identity = entry
            .sender
            .as_ref()
            .ok_or(protocol::Status::Internal)?
            .description()
            .map_err(|_| protocol::Status::Internal)?
            .object_id;
        if identity == 0 || world.streams.get(identity).is_some() {
            return Err(protocol::Status::Internal);
        }
        self.identity = Some(identity);
        let entry = self.entry_mut().ok_or(protocol::Status::Internal)?;
        let created = match entry.direction {
            protocol::StreamDirection::Read => blocking::Producer::create(
                3 * erhino_shared::proc::PROCESS_PAGE_SIZE,
                Placement::Anywhere,
            )
            .map(|(role, invitation)| (StreamRole::Producer(role), invitation)),
            protocol::StreamDirection::Write => blocking::Consumer::create(
                3 * erhino_shared::proc::PROCESS_PAGE_SIZE,
                Placement::Anywhere,
            )
            .map(|(role, invitation)| (StreamRole::Consumer(role), invitation)),
        };
        match created {
            Ok((role, invitation)) => {
                entry.role = Some(role);
                entry.invitation = Some(invitation);
                Ok(())
            }
            Err(librunnel::blocking::CreateFailure::Tunnel(
                rinlib::ipc::invitation::CreateFailure::Published {
                    endpoint,
                    cleanup,
                    invitation,
                    ..
                },
            )) => {
                entry.failed_endpoint = endpoint;
                entry.failed_cleanup = cleanup;
                entry.invitation = invitation;
                Err(protocol::Status::Internal)
            }
            Err(librunnel::blocking::CreateFailure::Protocol {
                endpoint,
                invitation,
                ..
            }) => {
                entry.failed_endpoint = Some(endpoint);
                entry.invitation = Some(invitation);
                Err(protocol::Status::Internal)
            }
            Err(librunnel::blocking::CreateFailure::Tunnel(
                rinlib::ipc::invitation::CreateFailure::System(error),
            )) => Err(stream_status(error)),
        }
    }

    fn arm_sources<B: RuntimeBackend>(
        &mut self,
        requests: &mut Requests<ServiceTask<B>>,
    ) -> Result<(), protocol::Status> {
        if !self.lifetime_requested && self.lifetime_source.is_none() {
            let handle = self
                .entry_mut()
                .and_then(|entry| entry.lifetime.as_ref())
                .ok_or(protocol::Status::Internal)?
                .as_handle();
            let plan = libexecution::runtime::SourcePlan::new(handle, ObjectSignals::CLOSED);
            requests
                .arm_source(plan, KIND_STREAM_LIFETIME)
                .map_err(|_| protocol::Status::Quota)?;
            self.lifetime_requested = true;
        }
        if !self.data_requested && self.data_source.is_none() {
            let role = self
                .entry_mut()
                .and_then(|entry| entry.role.as_mut())
                .ok_or(protocol::Status::Internal)?;
            let plan = match role {
                StreamRole::Producer(role) => role.wait_plan(),
                StreamRole::Consumer(role) => role.wait_plan(),
            }
            .map_err(|_| protocol::Status::Internal)?;
            requests
                .arm_source(plan, KIND_STREAM_DATA)
                .map_err(|_| protocol::Status::Quota)?;
            self.data_requested = true;
        }
        Ok(())
    }

    fn sources_ready(&self) -> bool {
        self.lifetime_source.is_some() && self.data_source.is_some()
    }

    fn install<B: RuntimeBackend>(&mut self, world: &mut World<B>) -> Result<(), protocol::Status> {
        let identity = self.identity.ok_or(protocol::Status::Internal)?;
        let prepared = self.pending.take().ok_or(protocol::Status::Internal)?;
        world
            .streams
            .install(identity, prepared)
            .map_err(|(status, prepared)| {
                self.pending = Some(prepared);
                status
            })
    }

    fn encode_open<B: RuntimeBackend>(
        &mut self,
        world: &mut World<B>,
    ) -> Result<(), SystemCallError> {
        if self.encoded {
            return Ok(());
        }
        let status = self.failure;
        let identity = self.identity.unwrap_or(0);
        let response = if status == protocol::Status::Ok {
            let entry = world
                .streams
                .get(identity)
                .ok_or(SystemCallError::InternalError)?;
            protocol::Response::StreamOffer(protocol::StreamOffer {
                identity,
                direction: entry.direction,
                tunnel_bytes: (3 * erhino_shared::proc::PROCESS_PAGE_SIZE) as u32,
                start: entry.offset,
                read_end: entry.read_end,
                offer_deadline: entry.offer_deadline,
            })
        } else {
            protocol::Response::Empty
        };
        let outbox = self.outbox.as_mut().ok_or(SystemCallError::InternalError)?;
        let output = outbox.response_mut()?;
        let used = encode_v2(
            output.body_mut()?,
            protocol::Op::Open,
            status,
            self.header.deadline,
            response,
        )?;
        output.finish_body(used)?;
        if status == protocol::Status::Ok {
            let entry = world
                .streams
                .get_mut(identity)
                .ok_or(SystemCallError::InternalError)?;
            let sender = entry.sender.take().ok_or(SystemCallError::InternalError)?;
            if let Err(failure) = output.push(
                sender.into_capability(),
                Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
            ) {
                self.returned.push(failure.capability);
                return Err(failure.error);
            }
            let invitation = entry
                .invitation
                .take()
                .ok_or(SystemCallError::InternalError)?;
            if let Err(failure) =
                output.push(invitation.into_capability(), Rights::MAP | Rights::TRANSIT)
            {
                self.returned.push(failure.capability);
                return Err(failure.error);
            }
        }
        self.encoded = true;
        Ok(())
    }

    fn progress<B: RuntimeBackend>(&mut self, world: &mut World<B>) -> bool {
        let Some(identity) = self.identity else {
            return false;
        };
        let Some(entry) = world.streams.get_mut(identity) else {
            return false;
        };
        if entry.state != protocol::StreamState::Active {
            return false;
        }
        let Some(backend) = world.provider.backend.as_mut() else {
            entry.terminal(
                protocol::Status::Cancelled,
                protocol::StreamReason::ProviderStopped,
            );
            return true;
        };
        match entry.direction {
            protocol::StreamDirection::Read => {
                if entry.buffered == entry.buffer_sent
                    && !entry.finished
                    && entry.offset < entry.read_end
                {
                    let n = (entry.read_end - entry.offset).min(entry.buffer.len() as u64) as usize;
                    match backend.read_stream(
                        &entry.node,
                        &entry.access,
                        entry.offset,
                        &mut entry.buffer[..n],
                    ) {
                        Ok(0) => {
                            entry.terminal(
                                protocol::Status::Internal,
                                protocol::StreamReason::Backend,
                            );
                            return true;
                        }
                        Ok(n) => {
                            entry.buffered = n;
                            entry.buffer_sent = 0;
                        }
                        Err(error) => {
                            entry.terminal(backend_status(error), protocol::StreamReason::Backend);
                            return true;
                        }
                    }
                }
                let Some(StreamRole::Producer(role)) = entry.role.as_mut() else {
                    entry.terminal(protocol::Status::Internal, protocol::StreamReason::Backend);
                    return true;
                };
                if entry.buffer_sent < entry.buffered {
                    let result = role.write(&entry.buffer[entry.buffer_sent..entry.buffered]);
                    let written = match result {
                        Ok(written) => written,
                        Err(error) => {
                            entry.buffer_sent += error.completed;
                            entry.offset += error.completed as u64;
                            entry.transported += error.completed as u64;
                            entry.terminal(
                                io_status(error.error),
                                protocol::StreamReason::PeerClosed,
                            );
                            return true;
                        }
                    };
                    if written == 0 {
                        return false;
                    }
                    entry.buffer_sent += written;
                    entry.offset += written as u64;
                    entry.transported += written as u64;
                    match role.writable() {
                        Ok(free) => {
                            let in_flight = role.capacity().saturating_sub(free) as u64;
                            entry.accepted = entry.transported.saturating_sub(in_flight);
                        }
                        Err(error) => {
                            entry.terminal(
                                io_status(error.error),
                                protocol::StreamReason::PeerClosed,
                            );
                        }
                    }
                    return true;
                }
                if entry.offset == entry.read_end && !entry.finished {
                    match role.finish() {
                        Ok(()) => {
                            entry.finished = true;
                            return true;
                        }
                        Err(error) => {
                            entry.terminal(
                                io_status(error.error),
                                protocol::StreamReason::PeerClosed,
                            );
                            return true;
                        }
                    }
                }
                if entry.finished {
                    match role.poll(ObjectSignals::NONE) {
                        Ok(Some(librunnel::ProducerReady::EofConsumed)) => {
                            entry.accepted = entry.transported;
                            entry.terminal(protocol::Status::Ok, protocol::StreamReason::Completed);
                            return true;
                        }
                        Err(error) => {
                            entry.terminal(
                                io_status(error.error),
                                protocol::StreamReason::PeerClosed,
                            );
                            return true;
                        }
                        _ => {}
                    }
                }
                false
            }
            protocol::StreamDirection::Write => {
                let Some(StreamRole::Consumer(role)) = entry.role.as_mut() else {
                    entry.terminal(protocol::Status::Internal, protocol::StreamReason::Backend);
                    return true;
                };
                if entry.buffered == 0 {
                    let remaining = entry.limit.map(|end| end - entry.offset);
                    let read_limit = remaining
                        .map(|n| n.saturating_add(1).min(1024) as usize)
                        .unwrap_or(1024);
                    match role.read(&mut entry.buffer[..read_limit]) {
                        Ok(0) => match role.eof_reached() {
                            Ok(true) => {
                                entry.terminal(
                                    protocol::Status::Ok,
                                    protocol::StreamReason::Completed,
                                );
                                return true;
                            }
                            Ok(false) => return false,
                            Err(error) => {
                                entry.terminal(
                                    io_status(error.error),
                                    protocol::StreamReason::PeerClosed,
                                );
                                return true;
                            }
                        },
                        Ok(n) => {
                            entry.transported += n as u64;
                            entry.buffered =
                                remaining.map(|r| r.min(n as u64) as usize).unwrap_or(n);
                            if entry.buffered < n {
                                entry.pending_error = Some((
                                    protocol::Status::Invalid,
                                    protocol::StreamReason::Backend,
                                ));
                            }
                        }
                        Err(error) => {
                            entry.transported += error.completed as u64;
                            entry.buffered = remaining
                                .map(|r| r.min(error.completed as u64) as usize)
                                .unwrap_or(error.completed);
                            entry.pending_error =
                                Some((io_status(error.error), protocol::StreamReason::PeerClosed));
                        }
                    }
                }
                if entry.buffered == 0 {
                    let (status, reason) = entry
                        .pending_error
                        .take()
                        .unwrap_or((protocol::Status::Invalid, protocol::StreamReason::Backend));
                    entry.terminal(status, reason);
                    return true;
                }
                let Some(end) = entry.offset.checked_add(entry.buffered as u64) else {
                    entry.terminal(protocol::Status::Invalid, protocol::StreamReason::Backend);
                    return true;
                };
                let prepared = backend.prepare_write(
                    entry.node.clone(),
                    &entry.access,
                    entry.offset,
                    &entry.buffer[..entry.buffered],
                );
                match prepared {
                    Ok(prepared) => match backend.commit(prepared) {
                        Ok(CommitResult::Written) => {
                            entry.offset = end;
                            entry.accepted += entry.buffered as u64;
                            entry.buffered = 0;
                            let mut effects = Effects::default();
                            if push_watch_effect(
                                backend,
                                &mut effects,
                                &entry.node,
                                protocol::WatchMask::MODIFY,
                                None,
                            )
                            .is_ok()
                                && let Some(wakes) = self.wakes.as_mut()
                            {
                                world.provider.watches.publish(&effects, wakes);
                            } else {
                                world.failed = true;
                            }
                            if let Some((status, reason)) = entry.pending_error.take() {
                                entry.terminal(status, reason);
                            }
                        }
                        Ok(_) => entry
                            .terminal(protocol::Status::Internal, protocol::StreamReason::Backend),
                        Err(failure) => entry.terminal(
                            backend_status(failure.error),
                            protocol::StreamReason::Backend,
                        ),
                    },
                    Err(error) => {
                        entry.terminal(backend_status(error), protocol::StreamReason::Backend)
                    }
                }
                true
            }
        }
    }

    fn close_data<B: RuntimeBackend>(
        &mut self,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
    ) -> Advance {
        if let Some(identity) = self.identity
            && let Some(entry) = world.streams.get_mut(identity)
            && let Some(waiter) = entry.waiter
            && !entry.wake_sent
        {
            if requests.wake(waiter).is_err() {
                return Advance {
                    work_done: 1,
                    step: Step::Runnable,
                };
            }
            entry.wake_sent = true;
        }
        if let Some(source) = self.data_source {
            if !self.data_removing && requests.remove(source).is_ok() {
                self.data_removing = true;
            }
            return Advance {
                work_done: 1,
                step: Step::Runnable,
            };
        }
        if self.data_requested {
            return Advance {
                work_done: 1,
                step: Step::Runnable,
            };
        }
        let Some(identity) = self.identity else {
            return Advance {
                work_done: 1,
                step: Step::Complete,
            };
        };
        let Some(entry) = world.streams.get_mut(identity) else {
            return Advance {
                work_done: 1,
                step: Step::Complete,
            };
        };
        entry.data_charge = None;
        if let Some(role) = entry.role.take()
            && let Err((role, _)) = role.close()
        {
            entry.role = Some(role);
            self.retry_deadline = rinlib::time::timeout_millis(10).ok();
            return Advance {
                work_done: 1,
                step: Step::Parked,
            };
        }
        if let Some(owner) = entry.failed_endpoint.take()
            && let Err((owner, _)) = owner.close()
        {
            entry.failed_endpoint = Some(owner);
            self.retry_deadline = rinlib::time::timeout_millis(10).ok();
            return Advance {
                work_done: 1,
                step: Step::Parked,
            };
        }
        if let Some(owner) = entry.failed_cleanup.take()
            && let Err((owner, _)) = owner.close()
        {
            entry.failed_cleanup = Some(owner);
            self.retry_deadline = rinlib::time::timeout_millis(10).ok();
            return Advance {
                work_done: 1,
                step: Step::Parked,
            };
        }
        self.retry_deadline = None;
        Advance {
            work_done: 1,
            step: Step::Parked,
        }
    }

    fn retire<B: RuntimeBackend>(
        &mut self,
        world: &mut World<B>,
        requests: &mut Requests<ServiceTask<B>>,
    ) -> Advance {
        self.retiring = true;
        for (source, removing) in [
            (self.data_source, &mut self.data_removing),
            (self.lifetime_source, &mut self.lifetime_removing),
        ] {
            if let Some(source) = source
                && !*removing
                && requests.remove(source).is_ok()
            {
                *removing = true;
            }
        }
        if self.data_source.is_some()
            || self.data_requested
            || self.lifetime_source.is_some()
            || self.lifetime_requested
        {
            return Advance {
                work_done: 1,
                step: Step::Runnable,
            };
        }
        if let Some(owner) = self.returned.pop()
            && let Err((owner, _)) = owner.close()
        {
            self.returned.push(owner);
            self.retry_deadline = rinlib::time::timeout_millis(10).ok();
            return Advance {
                work_done: 1,
                step: Step::Parked,
            };
        }
        let entry = if let Some(prepared) = self.pending.as_mut() {
            prepared.value_mut()
        } else if let Some(identity) = self.identity
            && let Some(entry) = world.streams.get_mut(identity)
        {
            entry
        } else {
            return Advance {
                work_done: 1,
                step: Step::Complete,
            };
        };
        if let Some(role) = entry.role.take()
            && let Err((role, _)) = role.close()
        {
            entry.role = Some(role);
            self.retry_deadline = rinlib::time::timeout_millis(10).ok();
            return Advance {
                work_done: 1,
                step: Step::Parked,
            };
        }
        if let Some(owner) = entry.failed_endpoint.take()
            && let Err((owner, _)) = owner.close()
        {
            entry.failed_endpoint = Some(owner);
            self.retry_deadline = rinlib::time::timeout_millis(10).ok();
            return Advance {
                work_done: 1,
                step: Step::Parked,
            };
        }
        if let Some(owner) = entry.failed_cleanup.take()
            && let Err((owner, _)) = owner.close()
        {
            entry.failed_cleanup = Some(owner);
            self.retry_deadline = rinlib::time::timeout_millis(10).ok();
            return Advance {
                work_done: 1,
                step: Step::Parked,
            };
        }
        if let Some(owner) = entry.invitation.take()
            && let Err((owner, _)) = owner.into_capability().close()
        {
            self.returned.push(owner);
            self.retry_deadline = rinlib::time::timeout_millis(10).ok();
            return Advance {
                work_done: 1,
                step: Step::Parked,
            };
        }
        if let Some(owner) = entry.sender.take()
            && let Err((owner, _)) = owner.into_capability().close()
        {
            self.returned.push(owner);
            self.retry_deadline = rinlib::time::timeout_millis(10).ok();
            return Advance {
                work_done: 1,
                step: Step::Parked,
            };
        }
        if let Some(owner) = entry.lifetime.take()
            && let Err((owner, _)) = owner.close()
        {
            entry.lifetime = Some(owner);
            self.retry_deadline = rinlib::time::timeout_millis(10).ok();
            return Advance {
                work_done: 1,
                step: Step::Parked,
            };
        }
        if !self.returned.is_empty() {
            return Advance {
                work_done: 1,
                step: Step::Runnable,
            };
        }
        if let Some(identity) = self.identity
            && self.pending.is_none()
        {
            drop(world.streams.remove(identity));
        }
        self.pending = None;
        Advance {
            work_done: 1,
            step: Step::Complete,
        }
    }
}

fn stream_status(error: SystemCallError) -> protocol::Status {
    match error {
        SystemCallError::QuotaExceeded | SystemCallError::ReachLimit => protocol::Status::Quota,
        SystemCallError::OutOfMemory => protocol::Status::Resource,
        SystemCallError::DeadlineExpired => protocol::Status::Cancelled,
        _ => protocol::Status::Internal,
    }
}

fn io_status(error: librunnel::RunnelError) -> protocol::Status {
    match error {
        librunnel::RunnelError::BadFormat | librunnel::RunnelError::Broken => {
            protocol::Status::Invalid
        }
        librunnel::RunnelError::Closed | librunnel::RunnelError::TimedOut => {
            protocol::Status::Cancelled
        }
        librunnel::RunnelError::Syscall(error) => stream_status(error),
    }
}

impl<B: RuntimeBackend> Task<World<B>> for StreamTask {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        id: u64,
        world: &mut World<B>,
        requests: &mut Requests<Self::Family>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if budget == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        while let Some(event) = input.pull() {
            match event.kind {
                KIND_REPLY => {
                    if let Some(outbox) = self.outbox.as_mut() {
                        outbox.observe(event);
                    }
                }
                KIND_STREAM_LIFETIME => {
                    if event.error != 0 || event.observed.intersects(ObjectSignals::CLOSED) {
                        self.retiring = true;
                    }
                }
                KIND_STREAM_DATA => {
                    if event.observed.intersects(ObjectSignals::PEER_ATTACHED)
                        && let Some(source) = self.data_source
                        && !self.data_removing
                        && requests.remove(source).is_ok()
                    {
                        self.data_removing = true;
                    }
                    let entry = if let Some(identity) = self.identity {
                        world.streams.get_mut(identity)
                    } else {
                        self.pending.as_mut().map(PreparedEntry::value_mut)
                    };
                    if let Some(entry) = entry {
                        if let Err(error) = entry.refresh_read_accepted() {
                            entry.terminal(
                                io_status(error.error),
                                protocol::StreamReason::PeerClosed,
                            );
                        }
                        let terminal_observed = event.error != 0
                            || event
                                .observed
                                .intersects(ObjectSignals::PEER_CLOSED | ObjectSignals::CLOSED);
                        match entry.role.as_mut() {
                            Some(StreamRole::Producer(role)) => {
                                if entry.finished
                                    && matches!(
                                        role.poll(ObjectSignals::NONE),
                                        Ok(Some(librunnel::ProducerReady::EofConsumed))
                                    )
                                {
                                    entry.accepted = entry.transported;
                                    entry.terminal(
                                        protocol::Status::Ok,
                                        protocol::StreamReason::Completed,
                                    );
                                } else if terminal_observed {
                                    entry.terminal(
                                        protocol::Status::Cancelled,
                                        protocol::StreamReason::PeerClosed,
                                    );
                                } else if let Err(error) = role.poll(event.observed) {
                                    entry.terminal(
                                        io_status(error.error),
                                        protocol::StreamReason::PeerClosed,
                                    );
                                }
                            }
                            Some(StreamRole::Consumer(role)) => {
                                match role.poll(ObjectSignals::NONE) {
                                    Ok(Some(librunnel::ConsumerReady::EofDrained)) => {
                                        if entry.state == protocol::StreamState::Active
                                            && entry.buffered == 0
                                        {
                                            entry.terminal(
                                                protocol::Status::Ok,
                                                protocol::StreamReason::Completed,
                                            );
                                        } else if entry.state == protocol::StreamState::Offered
                                            && let Err(error) = role.poll(event.observed)
                                        {
                                            entry.terminal(
                                                io_status(error.error),
                                                protocol::StreamReason::PeerClosed,
                                            );
                                        }
                                    }
                                    Err(error) => {
                                        let status = io_status(error.error);
                                        entry.pending_error =
                                            Some((status, protocol::StreamReason::PeerClosed));
                                        if entry.buffered == 0 {
                                            entry.terminal(
                                                status,
                                                protocol::StreamReason::PeerClosed,
                                            );
                                        }
                                    }
                                    _ if terminal_observed => {
                                        entry.pending_error = Some((
                                            protocol::Status::Cancelled,
                                            protocol::StreamReason::PeerClosed,
                                        ));
                                        if entry.buffered == 0 {
                                            entry.terminal(
                                                protocol::Status::Cancelled,
                                                protocol::StreamReason::PeerClosed,
                                            );
                                        }
                                    }
                                    _ if let Err(error) = role.poll(event.observed) => {
                                        let status = io_status(error.error);
                                        entry.pending_error =
                                            Some((status, protocol::StreamReason::PeerClosed));
                                        if entry.buffered == 0 {
                                            entry.terminal(
                                                status,
                                                protocol::StreamReason::PeerClosed,
                                            );
                                        }
                                    }
                                    _ => {}
                                }
                            }
                            None => {}
                        }
                    }
                }
                _ => return Err(SystemCallError::InternalError),
            }
        }
        if self.identity.is_none()
            && let Some(entry) = self.pending.as_mut().map(PreparedEntry::value_mut)
            && entry.state == protocol::StreamState::Terminal
        {
            self.failure = entry.outcome;
        }
        if let Some(identity) = self.identity
            && let Some(entry) = world.streams.get(identity)
            && entry.state == protocol::StreamState::Terminal
            && self
                .outbox
                .as_ref()
                .is_some_and(|outbox| !matches!(outbox.result(), Some(OutboxResult::Sent)))
        {
            self.retiring = true;
        }
        let timed_out = input.take_timeout();
        let retry_due = timed_out && self.retry_deadline.take().is_some();
        if self.stopping {
            self.retiring = true;
        }
        if timed_out && let Some(outbox) = self.outbox.as_mut() {
            outbox.timed_out();
        }
        if self
            .outbox
            .as_ref()
            .is_some_and(|outbox| matches!(outbox.result(), Some(OutboxResult::Abandoned(_))))
        {
            self.retiring = true;
        }
        if self.retiring
            && let Some(outbox) = self.outbox.as_mut()
        {
            outbox.stop(world);
        }
        if !self.retiring && !self.prepared && self.failure == protocol::Status::Ok {
            let outbox = self.outbox.as_mut().ok_or(SystemCallError::InternalError)?;
            if !outbox.is_admitted() {
                return Ok(outbox.admit(requests));
            }
            if self.returned.try_reserve_exact(2).is_err() {
                self.failure = protocol::Status::Resource;
            } else if let Err(error) = self.prepare(id, world) {
                self.failure = error;
            } else if let Err(error) = self.issue(world) {
                self.failure = error;
            }
        }
        if !self.retiring
            && self.failure == protocol::Status::Ok
            && self.prepared
            && !self.sources_ready()
        {
            if let Err(error) = self.arm_sources(requests) {
                self.failure = error;
            } else {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                });
            }
        }
        if !self.retiring
            && self.failure == protocol::Status::Ok
            && self.sources_ready()
            && self.pending.is_some()
            && let Err(error) = self.install(world)
        {
            self.failure = error;
        }
        if !self.retiring && !self.encoded && self.encode_open(world).is_err() {
            self.retiring = true;
            if let Some(outbox) = self.outbox.as_mut() {
                outbox.stop(world);
            }
        }
        if let Some(outbox) = self.outbox.as_mut() {
            let progress = outbox.drive(requests, budget)?;
            if !outbox.is_complete() {
                return Ok(progress);
            }
            record_provider_response(world, outbox, false)?;
            let sent = self.failure == protocol::Status::Ok
                && matches!(outbox.result(), Some(OutboxResult::Sent));
            if !sent {
                outbox.drain_capabilities(|owner, _| self.returned.push(owner));
                self.retiring = true;
            }
            self.outbox = None;
            self.current_deadline = self.offer_deadline;
        }
        if let Some(wakes) = self.wakes.as_mut()
            && !wakes.is_empty()
            && !wakes.drive(requests)?
        {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if self.retiring {
            return Ok(self.retire(world, requests));
        }
        if let Some(identity) = self.identity
            && let Some(entry) = world.streams.get_mut(identity)
        {
            if timed_out && !retry_due {
                match entry.state {
                    protocol::StreamState::Offered => {
                        entry
                            .terminal(protocol::Status::Cancelled, protocol::StreamReason::Expired);
                    }
                    protocol::StreamState::Active => match entry.refresh_read_accepted() {
                        Ok(()) => entry
                            .terminal(protocol::Status::Cancelled, protocol::StreamReason::Expired),
                        Err(error) => entry
                            .terminal(io_status(error.error), protocol::StreamReason::PeerClosed),
                    },
                    protocol::StreamState::Terminal => self.retiring = true,
                }
            }
            if entry.state != protocol::StreamState::Offered {
                self.current_deadline = self.session_deadline;
            }
        }
        if self.retiring {
            return Ok(self.retire(world, requests));
        }
        if let Some(identity) = self.identity
            && let Some(entry) = world.streams.get(identity)
            && entry.state == protocol::StreamState::Terminal
        {
            return Ok(self.close_data(world, requests));
        }
        if let Some(identity) = self.identity
            && let Some(entry) = world.streams.get(identity)
            && entry.state == protocol::StreamState::Active
            && self.progress(world)
        {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if let Some(identity) = self.identity
            && let Some(entry) = world.streams.get_mut(identity)
            && entry.role.is_some()
            && self.data_source.is_none()
            && !self.data_requested
            && !self.data_removing
        {
            let plan = match entry.role.as_mut() {
                Some(StreamRole::Producer(role)) => role.wait_plan(),
                Some(StreamRole::Consumer(role)) => role.wait_plan(),
                None => unreachable!(),
            };
            match plan {
                Ok(plan) => {
                    if requests.arm_source(plan, KIND_STREAM_DATA).is_ok() {
                        self.data_requested = true;
                        return Ok(Advance {
                            work_done: 1,
                            step: Step::Runnable,
                        });
                    }
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Runnable,
                    });
                }
                Err(error) => {
                    entry.terminal(io_status(error.error), protocol::StreamReason::PeerClosed);
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Runnable,
                    });
                }
            }
        }
        if let Some(source) = self.data_source
            && !self.data_removing
            && requests.rearm(source).is_err()
        {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Parked,
        })
    }

    fn refused(&mut self, world: &mut World<B>, failure: RequestFailure<Self::Family>) {
        match failure {
            RequestFailure::Source {
                kind: KIND_REPLY, ..
            } => {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.refused(world, failure);
                }
            }
            RequestFailure::Source { kind, error } => {
                match kind {
                    KIND_STREAM_LIFETIME => {
                        self.lifetime_requested = false;
                        self.lifetime_removing = false;
                    }
                    KIND_STREAM_DATA => {
                        self.data_requested = false;
                        self.data_removing = false;
                    }
                    _ => world.failed = true,
                }
                self.failure = stream_status(error);
                if let Some(identity) = self.identity
                    && let Some(entry) = world.streams.get_mut(identity)
                {
                    entry.terminal(self.failure, protocol::StreamReason::Backend);
                }
            }
            RequestFailure::Wake { task, error } => {
                if let Some(identity) = self.identity
                    && let Some(entry) = world.streams.get_mut(identity)
                    && entry.waiter == Some(task)
                {
                    if error == SystemCallError::ObjectNotFound {
                        entry.waiter = None;
                    } else {
                        entry.wake_sent = false;
                    }
                } else if error != SystemCallError::ObjectNotFound {
                    world.failed = true;
                }
            }
            RequestFailure::Spawn { .. } => world.failed = true,
        }
    }

    fn registered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        match kind {
            KIND_REPLY => {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.registered(world, kind, source);
                }
            }
            KIND_STREAM_LIFETIME => {
                self.lifetime_requested = false;
                self.lifetime_source = Some(source);
            }
            KIND_STREAM_DATA => {
                self.data_requested = false;
                self.data_source = Some(source);
            }
            _ => world.failed = true,
        }
    }

    fn unregistered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        match kind {
            KIND_REPLY => {
                if let Some(outbox) = self.outbox.as_mut() {
                    outbox.unregistered(world, kind, source);
                }
            }
            KIND_STREAM_LIFETIME if self.lifetime_source == Some(source) => {
                self.lifetime_requested = false;
                self.lifetime_source = None;
                self.lifetime_removing = false;
                if let Some(entry) = self.pending.as_mut().map(PreparedEntry::value_mut) {
                    entry.lifetime_charge = None;
                } else if let Some(identity) = self.identity
                    && let Some(entry) = world.streams.get_mut(identity)
                {
                    entry.lifetime_charge = None;
                }
            }
            KIND_STREAM_DATA if self.data_source == Some(source) => {
                self.data_requested = false;
                self.data_source = None;
                self.data_removing = false;
                let retiring = self.retiring || self.failure != protocol::Status::Ok;
                if let Some(entry) = self.pending.as_mut().map(PreparedEntry::value_mut) {
                    if retiring || entry.state == protocol::StreamState::Terminal {
                        entry.data_charge = None;
                    }
                } else if let Some(identity) = self.identity
                    && let Some(entry) = world.streams.get_mut(identity)
                    && (retiring || entry.state == protocol::StreamState::Terminal)
                {
                    entry.data_charge = None;
                }
            }
            _ => world.failed = true,
        }
    }

    fn stop(&mut self, world: &mut World<B>) {
        self.stopping = true;
        if let Some(outbox) = self.outbox.as_mut() {
            outbox.stop(world);
        }
    }

    fn deadline(&self) -> Deadline {
        self.retry_deadline.unwrap_or(self.current_deadline)
    }
}

pub(super) struct StreamControlTask {
    outbox: Outbox,
    header: protocol::Header,
    reply_deadline: Deadline,
    identity: u64,
    op: protocol::Op,
    encoded: bool,
    waiting: bool,
    recorded: bool,
}

impl StreamControlTask {
    pub(super) fn new(
        outbox: Outbox,
        header: protocol::Header,
        reply_deadline: Deadline,
        identity: u64,
    ) -> Self {
        Self {
            outbox,
            header,
            reply_deadline,
            identity,
            op: header.op,
            encoded: false,
            waiting: false,
            recorded: false,
        }
    }

    pub(super) fn into_refused_outbox(self) -> Outbox {
        self.outbox
    }

    fn release_waiter<B: RuntimeBackend>(&mut self, id: u64, world: &mut World<B>) {
        if let Some(entry) = world.streams.get_mut(self.identity)
            && entry.waiter == Some(id)
        {
            entry.waiter = None;
            entry.wake_sent = false;
        }
        self.waiting = false;
    }
}

impl<B: RuntimeBackend> Task<World<B>> for StreamControlTask {
    type Family = ServiceTask<B>;

    fn advance(
        &mut self,
        id: u64,
        world: &mut World<B>,
        requests: &mut Requests<Self::Family>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            if event.kind == KIND_REPLY {
                self.outbox.observe(event);
            } else {
                return Err(SystemCallError::InternalError);
            }
        }
        if input.take_timeout() {
            self.outbox.timed_out();
        }
        if self.outbox.is_complete() {
            if !self.recorded {
                record_provider_response(world, &self.outbox, false)?;
                self.recorded = true;
            }
            self.release_waiter(id, world);
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        if self.outbox.result().is_some() {
            self.release_waiter(id, world);
            return self.outbox.drive(requests, budget);
        }
        if !self.outbox.is_admitted() {
            return Ok(self.outbox.admit(requests));
        }
        if !self.encoded {
            let (status, info) = match world.streams.get_mut(self.identity) {
                None => (protocol::Status::Permission, None),
                Some(entry) => match self.op {
                    protocol::Op::QueryStream => {
                        let _ = entry.refresh_read_accepted();
                        (protocol::Status::Ok, Some(entry.info(self.identity)))
                    }
                    protocol::Op::Start
                        if entry.state == protocol::StreamState::Offered
                            && entry
                                .offer_deadline
                                .instant()
                                .ok()
                                .flatten()
                                .is_some_and(|end| input.now_ns() >= end) =>
                    {
                        if requests.wake(entry.task).is_ok() {
                            entry.terminal(
                                protocol::Status::Cancelled,
                                protocol::StreamReason::Expired,
                            );
                        }
                        (protocol::Status::Cancelled, None)
                    }
                    protocol::Op::Start if entry.state == protocol::StreamState::Offered => {
                        let attached = match entry.role.as_ref() {
                            Some(StreamRole::Producer(role)) => role.peer_attached(),
                            Some(StreamRole::Consumer(role)) => role.peer_attached(),
                            None => false,
                        };
                        if attached {
                            if requests.wake(entry.task).is_ok() {
                                entry.state = protocol::StreamState::Active;
                                (protocol::Status::Ok, Some(entry.info(self.identity)))
                            } else {
                                (protocol::Status::Busy, None)
                            }
                        } else {
                            (protocol::Status::Busy, None)
                        }
                    }
                    protocol::Op::Start if entry.state == protocol::StreamState::Active => {
                        (protocol::Status::Ok, Some(entry.info(self.identity)))
                    }
                    protocol::Op::Start => (protocol::Status::Busy, None),
                    protocol::Op::FinishStream
                        if entry.state != protocol::StreamState::Terminal =>
                    {
                        if entry.waiter.is_some() {
                            (protocol::Status::Busy, None)
                        } else {
                            entry.waiter = Some(id);
                            self.waiting = true;
                            return Ok(Advance {
                                work_done: 1,
                                step: Step::Parked,
                            });
                        }
                    }
                    protocol::Op::FinishStream => {
                        (protocol::Status::Ok, Some(entry.info(self.identity)))
                    }
                    protocol::Op::CancelStream
                        if entry.state == protocol::StreamState::Terminal =>
                    {
                        (protocol::Status::Ok, Some(entry.info(self.identity)))
                    }
                    protocol::Op::CancelStream => {
                        if requests.wake(entry.task).is_ok() {
                            match entry.refresh_read_accepted() {
                                Ok(()) => entry.terminal(
                                    protocol::Status::Cancelled,
                                    protocol::StreamReason::Cancelled,
                                ),
                                Err(error) => entry.terminal(
                                    io_status(error.error),
                                    protocol::StreamReason::PeerClosed,
                                ),
                            }
                            (protocol::Status::Ok, Some(entry.info(self.identity)))
                        } else {
                            (protocol::Status::Busy, None)
                        }
                    }
                    _ => (protocol::Status::Invalid, None),
                },
            };
            let response = info
                .map(protocol::Response::StreamInfo)
                .unwrap_or(protocol::Response::Empty);
            let output = self.outbox.response_mut()?;
            let used = encode_v2(
                output.body_mut()?,
                self.op,
                status,
                self.header.deadline,
                response,
            )?;
            output.finish_body(used)?;
            self.encoded = true;
            self.release_waiter(id, world);
        }
        let progress = self.outbox.drive(requests, budget)?;
        if self.outbox.is_complete() {
            if !self.recorded {
                record_provider_response(world, &self.outbox, false)?;
                self.recorded = true;
            }
            self.release_waiter(id, world);
            return Ok(Advance {
                work_done: progress.work_done,
                step: Step::Complete,
            });
        }
        Ok(progress)
    }

    fn refused(&mut self, world: &mut World<B>, failure: RequestFailure<Self::Family>) {
        if let RequestFailure::Source {
            kind: KIND_REPLY, ..
        } = failure
        {
            self.outbox.refused(world, failure);
        } else if let RequestFailure::Wake { .. } = failure {
            if world.streams.get(self.identity).is_some() {
                world.failed = true;
            }
        } else {
            world.failed = true;
        }
    }

    fn registered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        self.outbox.registered(world, kind, source);
    }

    fn unregistered(&mut self, world: &mut World<B>, kind: SourceKind, source: SourceId) {
        self.outbox.unregistered(world, kind, source);
    }

    fn stop(&mut self, world: &mut World<B>) {
        self.outbox.stop(world);
    }

    fn deadline(&self) -> Deadline {
        self.reply_deadline
    }
}
