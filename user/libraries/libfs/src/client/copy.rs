//! 文件组合 Copy 的分步 owner；运输、领域结果和实际清理保持在同一已准入任务。

use super::{ClientError, rpc_request, slots};
use crate::{
    FsRights,
    node::NodeKind,
    protocol::{
        self, Direction, OpenOptions, Request, Response, Status, StreamInfo, StreamReason,
        StreamState,
    },
    value::Property,
};
use alloc::{string::String, sync::Arc, vec::Vec};
use erhino_shared::{
    call::SystemCallError, message::PAYLOAD_MAX, object::ObjectSignals, time::Deadline,
};
use libexecution::{
    SourceRegistration,
    runtime::{Advance, Input, RequestFailure, Requests, SourceId, Step},
};
use libkernel::{
    ipc::{invitation::Invitation, message::MailboxSender},
    mm::Placement,
};
use liboal::{FollowBudget, LocalError, Namespace, walk::Walk};
use librpc::{
    CallError, CallPhase, Reply,
    dispatcher::{Completion, Dispatcher, ServiceOwner},
    exchange::CallCause,
};
use librunnel::{
    MappedEndpoint, RunnelError,
    blocking::{AttachFailure, Consumer, Producer},
};

const SOURCE: u64 = 1;
const DESTINATION: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Stream,
    Property,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Creation {
    NotAttempted,
    NotCreated,
    Created,
    Unknown,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StreamProgress {
    pub local_read: u64,
    pub transported: u64,
    pub destination_confirmed: u64,
    pub source_terminal: Option<StreamInfo>,
    pub destination_terminal: Option<StreamInfo>,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PropertyProgress {
    pub encoded_len: usize,
    pub destination_created: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    Stream(StreamProgress),
    Property(PropertyProgress),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cause {
    Local(LocalError),
    System(SystemCallError),
    Transport(CallCause),
    Status(Status),
    Protocol,
    Data(RunnelError),
    Cancelled,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Complete {
        progress: Progress,
    },
    Failed {
        creation: Creation,
        progress: Progress,
        cause: Cause,
        cleanup_pending: bool,
    },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Source,
    SourceStart,
    Destination,
    DestinationStart,
    SourceFinish,
    DestinationFinish,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Call(Command),
    AttachSource,
    ArmSource,
    AttachDestination,
    ArmDestination,
    Pump,
    Eof,
    Retiring,
    Complete,
}
/// 可嵌入宿主任务族；advance 只推进一次 RPC/一块搬运/一项实际清理。
/// 宿主在任何业务请求前准入一个任务、两项数据来源及一个 in-flight RPC 的容量。
pub struct CopyTask {
    namespace: Arc<Namespace>,
    destination: String,
    walk: Option<Walk<'static>>,
    deadline: Deadline,
    rights: FsRights,
    tunnel_bytes: u32,
    phase: Phase,
    pending: Option<(Command, u64)>,
    creation: Creation,
    before_creation: Creation,
    progress: Progress,
    failure: Option<Cause>,
    source_control: Option<MailboxSender>,
    destination_control: Option<MailboxSender>,
    source_session: u64,
    destination_session: u64,
    invitation: Option<(Invitation, usize)>,
    partial: Option<MappedEndpoint>,
    reader: Option<Consumer>,
    writer: Option<Producer>,
    source: SourceRegistration,
    destination_source: SourceRegistration,
    source_data: bool,
    destination_data: bool,
    source_observed: ObjectSignals,
    destination_observed: ObjectSignals,
    buffer: Vec<u8>,
    filled: usize,
    written: usize,
    cleanup_at: Option<u64>,
    succeeded: bool,
}
impl core::fmt::Debug for CopyTask {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CopyTask")
            .field("phase", &self.phase)
            .field("pending", &self.pending)
            .field("creation", &self.creation)
            .field("progress", &self.progress)
            .field("failure", &self.failure)
            .field("source", &self.source)
            .field("destination_source", &self.destination_source)
            .field("cleanup_at", &self.cleanup_at)
            .finish()
    }
}
impl CopyTask {
    pub const fn source_capacity() -> usize {
        2
    }
    pub const fn inflight_capacity() -> usize {
        1
    }
    pub fn new(
        namespace: Arc<Namespace>,
        source: &str,
        destination: &str,
        kind: Kind,
        rights: FsRights,
        tunnel_bytes: u32,
        budget: FollowBudget,
    ) -> Result<Self, ClientError> {
        let deadline = budget.deadline();
        let walk = Walk::owned(namespace.clone(), source, "/", budget)?;
        let mut target = String::new();
        target
            .try_reserve_exact(destination.len())
            .map_err(|_| SystemCallError::OutOfMemory)?;
        target.push_str(destination);
        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(match kind {
                Kind::Stream => PAYLOAD_MAX,
                Kind::Property => crate::value::VALUE_MAX,
            })
            .map_err(|_| SystemCallError::OutOfMemory)?;
        if kind == Kind::Stream {
            buffer.resize(PAYLOAD_MAX, 0);
        }
        Ok(Self {
            namespace,
            destination: target,
            walk: Some(walk),
            deadline,
            rights,
            tunnel_bytes,
            phase: Phase::Call(Command::Source),
            pending: None,
            creation: Creation::NotAttempted,
            before_creation: Creation::NotAttempted,
            progress: match kind {
                Kind::Stream => Progress::Stream(StreamProgress::default()),
                Kind::Property => Progress::Property(PropertyProgress::default()),
            },
            failure: None,
            source_control: None,
            destination_control: None,
            source_session: 0,
            destination_session: 0,
            invitation: None,
            partial: None,
            reader: None,
            writer: None,
            source: SourceRegistration::new(),
            destination_source: SourceRegistration::new(),
            source_data: false,
            destination_data: false,
            source_observed: ObjectSignals::NONE,
            destination_observed: ObjectSignals::NONE,
            buffer,
            filled: 0,
            written: 0,
            cleanup_at: None,
            succeeded: false,
        })
    }
    pub fn outcome(&self) -> Option<Outcome> {
        if let Some(cause) = self.failure {
            Some(Outcome::Failed {
                creation: self.creation,
                progress: self.progress,
                cause,
                cleanup_pending: self.phase != Phase::Complete,
            })
        } else if self.succeeded && self.phase == Phase::Complete {
            Some(Outcome::Complete {
                progress: self.progress,
            })
        } else {
            None
        }
    }
    pub fn progress(&self) -> Progress {
        self.progress
    }
    pub fn in_flight(&self) -> bool {
        self.pending.is_some()
    }
    pub fn creation(&self) -> Creation {
        self.creation
    }
    pub fn call_phase(&self, rpc: &Dispatcher) -> Option<CallPhase> {
        self.pending.and_then(|(_, txid)| rpc.call_phase(txid))
    }
    /// 提交取消意图并终结当前本地等待；清理仍由原任务 advance 推进。
    pub fn cancel(&mut self, rpc: &mut Dispatcher) {
        self.stop();
        if let Some((_, txid)) = self.pending {
            rpc.cancel(txid);
        }
    }
    pub fn deadline(&self) -> Deadline {
        if matches!(self.phase, Phase::Retiring | Phase::Complete) {
            self.cleanup_at.map_or(Deadline::INFINITE, Deadline::at)
        } else {
            self.deadline
        }
    }
    pub fn stop(&mut self) {
        if !self.succeeded {
            self.fail(Cause::Cancelled);
        }
    }
    pub fn abandon_unadmitted(&mut self) -> Result<(), SystemCallError> {
        if self.pending.is_some()
            || !self.source.is_empty()
            || !self.destination_source.is_empty()
            || self.reader.is_some()
            || self.writer.is_some()
            || self.source_control.is_some()
            || self.destination_control.is_some()
            || self.invitation.is_some()
            || self.partial.is_some()
            || !matches!(self.phase, Phase::Call(Command::Source) | Phase::Retiring)
        {
            return Err(SystemCallError::ObjectBusy);
        }
        self.fail(Cause::Cancelled);
        self.phase = Phase::Complete;
        Ok(())
    }
    fn fail(&mut self, cause: Cause) {
        if self.failure.is_none() {
            self.failure = Some(cause);
        }
        if self.pending.is_none() {
            self.phase = Phase::Retiring;
        }
    }
    fn fail_client(&mut self, error: ClientError) {
        match error {
            ClientError::Local(e) => self.fail(Cause::Local(e)),
            ClientError::System(e) => self.fail(Cause::System(e)),
            ClientError::Status(e) => self.fail(Cause::Status(e)),
            ClientError::Protocol => self.fail(Cause::Protocol),
            ClientError::Transport(e) => self.fail_call(e),
            ClientError::Adopt(e) => {
                self.fail(Cause::System(e.error));
            }
        }
    }
    fn fail_call(&mut self, error: CallError) {
        self.fail(Cause::Transport(error.cause));
        // Dispatcher 已退出调用来源，返回的错误/未发送 Packet 只持运输叶 owner。
    }
    fn request<'a>(&'a self, command: Command, path: &'a str) -> Request<'a> {
        let options = OpenOptions {
            direction: if command == Command::Source {
                Direction::Read
            } else {
                Direction::Write
            },
            offset: 0,
            length: None,
            session_deadline: self.deadline,
            tunnel_bytes: self.tunnel_bytes,
        };
        match command {
            Command::Source if matches!(self.progress, Progress::Property(_)) => {
                Request::Get { path }
            }
            Command::Source => Request::Open { path, options },
            Command::Destination if matches!(self.progress, Progress::Property(_)) => {
                Request::Create {
                    path,
                    kind: NodeKind::Property,
                    rights: self.rights,
                    initial: &self.buffer,
                }
            }
            Command::Destination => Request::CreateOpen {
                path,
                rights: self.rights,
                options,
            },
            Command::SourceStart | Command::DestinationStart => Request::Start,
            Command::SourceFinish | Command::DestinationFinish => Request::Finish,
        }
    }
    fn destination_walk(&mut self) -> Result<(), ClientError> {
        let budget = self.walk.take().ok_or(ClientError::Protocol)?.into_budget();
        self.walk = Some(Walk::owned(
            self.namespace.clone(),
            &self.destination,
            "/",
            budget,
        )?);
        self.phase = Phase::Call(Command::Destination);
        Ok(())
    }
    fn begin<F>(
        &mut self,
        command: Command,
        task: u64,
        rpc_task: u64,
        rpc: &mut Dispatcher,
        requests: &mut Requests<'_, F>,
    ) -> Result<(), ClientError> {
        let is_path = matches!(command, Command::Source | Command::Destination);
        {
            self.walk
                .as_mut()
                .ok_or(ClientError::Protocol)?
                .before_call(libkernel::time::snapshot()?.now_ns)?;
        }
        let (service, path) = if is_path {
            let (entry, path) = self.walk.as_ref().ok_or(ClientError::Protocol)?.target()?;
            (Some(entry.clone()), path)
        } else {
            (None, "")
        };
        let request = rpc_request(&self.request(command, path), self.deadline, None)?;
        let owner = if let Some(service) = service {
            ServiceOwner::Shared(service)
        } else {
            let control = match command {
                Command::SourceStart | Command::SourceFinish => &mut self.source_control,
                _ => &mut self.destination_control,
            };
            ServiceOwner::Owned(control.take().ok_or(ClientError::Protocol)?)
        };
        if command == Command::Destination {
            self.before_creation = self.creation;
            self.creation = Creation::Unknown;
        }
        match rpc.begin_for(
            owner,
            self.deadline,
            request,
            Some(task),
            requests.task_funding(),
        ) {
            Ok(txid) => {
                self.pending = Some((command, txid));
                let _ = requests.wake(rpc_task);
            }
            Err(failed) => {
                if command == Command::Destination {
                    self.creation = self.before_creation;
                }
                self.restore(command, failed.service);
                self.fail_call(failed.error);
            }
        }
        Ok(())
    }
    fn restore(&mut self, command: Command, service: ServiceOwner) {
        match command {
            Command::Source | Command::Destination => {
                // 返回的 Shared 是本次保活引用，固定 Namespace 继续持入口。
                assert!(
                    matches!(service, ServiceOwner::Shared(_)),
                    "path Copy call changed service owner"
                );
                drop(service);
            }
            _ => {
                // 同一 txid 原样返还本任务提交的 Owned，不重新查询角色。
                let sender = service
                    .into_owned()
                    .expect("control Copy call changed service owner");
                match command {
                    Command::SourceStart | Command::SourceFinish => {
                        self.source_control = Some(sender)
                    }
                    _ => self.destination_control = Some(sender),
                }
            }
        }
    }
    fn inspect(&mut self, command: Command, completion: Completion) -> Result<(), ClientError> {
        self.restore(command, completion.service);
        let mut reply = match completion.result {
            Ok(reply) => reply,
            Err(error) => {
                if command == Command::Destination && error.phase == CallPhase::Unsent {
                    self.creation = self.before_creation;
                }
                self.fail_call(error);
                return Ok(());
            }
        };
        // 已采用的输出进入实际控制/映射 owner；剩余至多8个 IPC 叶能力随回复
        // 必成关闭，不把回复存储与普通表项关闭另建为任务相位。
        self.decode(command, &mut reply)
    }
    fn decode(&mut self, command: Command, reply: &mut Reply) -> Result<(), ClientError> {
        let path = if matches!(command, Command::Source | Command::Destination) {
            self.walk.as_ref().ok_or(ClientError::Protocol)?.target()?.1
        } else {
            ""
        };
        let request = self.request(command, path);
        let response = protocol::decode_response(
            &reply.payload,
            &request,
            self.deadline,
            slots(&reply.handles),
        )
        .map_err(|_| ClientError::Protocol)?;
        if let Response::Boundary(boundary) = response {
            if !matches!(command, Command::Source | Command::Destination) {
                return Err(ClientError::Protocol);
            }
            if command == Command::Destination {
                self.creation = Creation::NotCreated;
            }
            if self.failure.is_some() {
                return Ok(());
            }
            let entry = if boundary.capability_count() == 1 {
                let capability = reply.handles.take(0)?;
                let sender = match MailboxSender::from_capability(capability) {
                    Ok((sender, _)) => sender,
                    Err(e) => {
                        return Err(ClientError::System(e.error));
                    }
                };
                Some(Arc::try_new(sender).map_err(|_| SystemCallError::OutOfMemory)?)
            } else {
                None
            };
            self.walk.as_mut().ok_or(ClientError::Protocol)?.follow(
                boundary,
                entry,
                libkernel::time::snapshot()?.now_ns,
            )?;
            self.phase = Phase::Call(command);
            return Ok(());
        }
        if let Response::Error(status) = response {
            if command == Command::Destination {
                self.creation = Creation::NotCreated;
            }
            return Err(ClientError::Status(status));
        }
        if command == Command::Destination {
            self.creation = Creation::Created;
        }
        if self.failure.is_some() {
            return Ok(());
        }
        match (command, response) {
            (Command::Source, Response::Value(value))
                if matches!(self.progress, Progress::Property(_)) =>
            {
                let bytes = value.as_bytes();
                Property::decode(bytes).map_err(ClientError::Status)?;
                self.buffer.extend_from_slice(bytes); // new 已预留整个纯值协议上界。
                self.progress = Progress::Property(PropertyProgress {
                    encoded_len: bytes.len(),
                    destination_created: false,
                });
                self.destination_walk()?;
            }
            (Command::Destination, Response::Empty)
                if matches!(self.progress, Progress::Property(_)) =>
            {
                if let Progress::Property(progress) = &mut self.progress {
                    progress.destination_created = true;
                }
                self.succeeded = true;
                self.phase = Phase::Retiring;
            }
            (Command::Source | Command::Destination, Response::Offer(info))
                if matches!(self.progress, Progress::Stream(_)) =>
            {
                let direction = if command == Command::Source {
                    Direction::Read
                } else {
                    Direction::Write
                };
                if info.direction != direction {
                    return Err(ClientError::Protocol);
                }
                let control_cap = reply.handles.take(0)?;
                let control = match MailboxSender::from_capability(control_cap) {
                    Ok((sender, _)) => sender,
                    Err(e) => {
                        return Err(ClientError::System(e.error));
                    }
                };
                if command == Command::Source {
                    self.source_control = Some(control);
                    self.source_session = info.session_id;
                } else {
                    self.destination_control = Some(control);
                    self.destination_session = info.session_id;
                }
                let cap = reply.handles.take(1)?;
                let invitation = match Invitation::from_capability(cap) {
                    Ok(invitation) => invitation,
                    Err(e) => {
                        return Err(ClientError::System(e.error));
                    }
                };
                self.invitation = Some((invitation, info.tunnel_bytes as usize));
                self.phase = if command == Command::Source {
                    Phase::AttachSource
                } else {
                    Phase::AttachDestination
                };
            }
            (Command::SourceStart, Response::Stream(info))
            | (Command::DestinationStart, Response::Stream(info)) => {
                let expected = if command == Command::SourceStart {
                    self.source_session
                } else {
                    self.destination_session
                };
                if info.state != StreamState::Active
                    || info.session_id != expected
                    || info.accepted > info.transported
                {
                    return Err(ClientError::Protocol);
                }
                if command == Command::SourceStart {
                    self.destination_walk()?;
                } else {
                    self.phase = Phase::Pump;
                }
            }
            (Command::SourceFinish, Response::Stream(info))
            | (Command::DestinationFinish, Response::Stream(info)) => {
                let expected = if command == Command::SourceFinish {
                    self.source_session
                } else {
                    self.destination_session
                };
                if info.state != StreamState::Terminal
                    || info.session_id != expected
                    || info.accepted > info.transported
                {
                    return Err(ClientError::Protocol);
                }
                if let Progress::Stream(progress) = &mut self.progress {
                    if command == Command::SourceFinish {
                        progress.source_terminal = Some(info);
                    } else {
                        progress.destination_confirmed = info.accepted;
                        progress.destination_terminal = Some(info);
                    }
                }
                if info.reason != StreamReason::Completed || info.outcome != Status::Ok {
                    return Err(ClientError::Status(info.outcome));
                }
                if command == Command::SourceFinish {
                    self.phase = Phase::Call(Command::DestinationFinish);
                } else {
                    let Progress::Stream(progress) = self.progress else {
                        return Err(ClientError::Protocol);
                    };
                    if progress.destination_confirmed != progress.local_read
                        || progress.transported != progress.local_read
                    {
                        return Err(ClientError::Protocol);
                    }
                    self.succeeded = true;
                    self.phase = Phase::Retiring;
                }
            }
            _ => return Err(ClientError::Protocol),
        }
        Ok(())
    }
    fn attach_failure(&mut self, failure: AttachFailure) {
        let cause = match failure {
            AttachFailure::Tunnel(e) => Cause::System(e.error),
            AttachFailure::Map(e) => Cause::System(e.error),
            AttachFailure::Protocol(e) => {
                self.partial = Some(e.owner);
                Cause::Data(e.error)
            }
        };
        self.fail(cause);
    }
    fn observe<F>(
        &mut self,
        requests: &mut Requests<'_, F>,
        read: bool,
        write: bool,
    ) -> Result<Option<Step>, SystemCallError> {
        for (is_source, wanted) in [(true, read), (false, write)] {
            let (registration, current, kind) = if is_source {
                (&mut self.source, &mut self.source_data, SOURCE)
            } else {
                (
                    &mut self.destination_source,
                    &mut self.destination_data,
                    DESTINATION,
                )
            };
            if *current != wanted && !registration.is_empty() {
                registration.remove(requests)?;
                return Ok(Some(
                    if registration.is_installing() || registration.is_removing() {
                        Step::Parked
                    } else {
                        Step::Runnable
                    },
                ));
            }
            if registration.is_empty() {
                let plan = if is_source {
                    let reader = self.reader.as_mut().expect("Copy observation owns source");
                    if wanted {
                        reader
                            .wait_plan()
                            .map_err(|_| SystemCallError::ObjectClosed)?
                    } else {
                        reader.terminal_wait_plan()
                    }
                } else {
                    let writer = self
                        .writer
                        .as_mut()
                        .expect("Copy observation owns destination");
                    if wanted {
                        writer
                            .wait_plan()
                            .map_err(|_| SystemCallError::ObjectClosed)?
                    } else {
                        writer.terminal_wait_plan()
                    }
                };
                registration.install(requests, plan, kind)?;
                *current = wanted;
                return Ok(Some(Step::Runnable));
            }
            if registration.source().is_none() || registration.is_removing() {
                return Ok(Some(Step::Parked));
            }
        }
        Ok(None)
    }
    fn pump(&mut self) -> Result<bool, RunnelError> {
        // DATA 只登记在当前方向；另一端保留终态观察。切换后重查共享 head/tail。
        let source_ready = self
            .reader
            .as_mut()
            .expect("Copy pump owns source plane")
            .poll(core::mem::replace(
                &mut self.source_observed,
                ObjectSignals::NONE,
            ))
            .map_err(|e| e.error)?;
        let destination_ready = self
            .writer
            .as_mut()
            .expect("Copy pump owns destination plane")
            .poll(core::mem::replace(
                &mut self.destination_observed,
                ObjectSignals::NONE,
            ))
            .map_err(|e| e.error)?;
        if self.written < self.filled {
            let writer = self
                .writer
                .as_mut()
                .expect("Copy pump owns destination plane");
            if destination_ready.is_none() {
                return Ok(false);
            }
            let (count, error) = match writer.write(&self.buffer[self.written..self.filled]) {
                Ok(n) => (n, None),
                Err(e) => (e.completed, Some(e.error)),
            };
            self.written += count;
            if let Progress::Stream(progress) = &mut self.progress {
                progress.transported += count as u64;
            }
            if let Some(error) = error {
                return Err(error);
            }
            return Ok(count != 0 || self.written == self.filled);
        }
        self.filled = 0;
        self.written = 0;
        let reader = self.reader.as_mut().expect("Copy pump owns source plane");
        if source_ready.is_none() {
            return Ok(false);
        }
        if reader.eof_reached().map_err(|e| e.error)? {
            self.phase = Phase::Eof;
            return Ok(true);
        }
        let (count, error) = match reader.read(&mut self.buffer) {
            Ok(n) => (n, None),
            Err(e) => (e.completed, Some(e.error)),
        };
        self.filled = count;
        if let Progress::Stream(progress) = &mut self.progress {
            progress.local_read += count as u64;
        }
        if let Some(error) = error {
            return Err(error);
        }
        Ok(count != 0)
    }
    fn retire<F>(&mut self, requests: &mut Requests<'_, F>) -> Result<Step, SystemCallError> {
        if let Some(control) = self.source_control.take() {
            drop(control);
            return Ok(Step::Runnable);
        }
        if let Some(control) = self.destination_control.take() {
            drop(control);
            return Ok(Step::Runnable);
        }
        for source in [&mut self.source, &mut self.destination_source] {
            if !source.is_empty() {
                source.remove(requests)?;
                return Ok(if source.is_installing() || source.is_removing() {
                    Step::Parked
                } else {
                    Step::Runnable
                });
            }
        }
        if let Some((invitation, _)) = self.invitation.take() {
            drop(invitation);
            return Ok(Step::Runnable);
        }
        if let Some(partial) = self.partial.take() {
            if let Err((owner, error)) = partial.close() {
                self.partial = Some(owner);
                return Err(error);
            }
            return Ok(Step::Runnable);
        }
        if let Some(reader) = self.reader.take() {
            if let Err((reader, error)) = reader.close() {
                self.reader = Some(reader);
                return Err(error);
            }
            return Ok(Step::Runnable);
        }
        if let Some(writer) = self.writer.take() {
            if let Err((writer, error)) = writer.close() {
                self.writer = Some(writer);
                return Err(error);
            }
            return Ok(Step::Runnable);
        }
        self.walk = None;
        self.phase = Phase::Complete;
        Ok(Step::Complete)
    }
    pub fn advance<F>(
        &mut self,
        task: u64,
        rpc_task: u64,
        rpc: &mut Dispatcher,
        requests: &mut Requests<'_, F>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Advance {
        while let Some(event) = input.pull() {
            if self.source.source() == Some(event.source) {
                self.source_observed |= event.observed;
            }
            if self.destination_source.source() == Some(event.source) {
                self.destination_observed |= event.observed;
            }
        }
        if input.take_timeout() && !matches!(self.phase, Phase::Retiring | Phase::Complete) {
            self.fail(Cause::Transport(CallCause::Timeout));
        }
        if let Some((command, txid)) = self.pending {
            if self.failure.is_some() {
                rpc.cancel(txid);
            }
            if requests.wake(rpc_task).is_err() {
                return step(Step::Runnable);
            }
            let Some(completion) = rpc.take(txid) else {
                return step(Step::Parked);
            };
            self.pending = None;
            if let Err(error) = self.inspect(command, completion) {
                self.fail_client(error);
            }
            if self.failure.is_some() {
                self.phase = Phase::Retiring;
            }
            return step(Step::Runnable);
        }
        if let Some(at) = self.cleanup_at {
            let now = match libkernel::time::snapshot() {
                Ok(clock) => clock.now_ns,
                Err(error) => {
                    self.fail(Cause::System(error));
                    return step(Step::Runnable);
                }
            };
            if now < at {
                return step(Step::Parked);
            }
            self.cleanup_at = None;
        }
        let result = (|| -> Result<Step, SystemCallError> {
            match self.phase {
                Phase::Call(command) => {
                    if let Err(e) = self.begin(command, task, rpc_task, rpc, requests) {
                        self.fail_client(e);
                    }
                    Ok(Step::Runnable)
                }
                Phase::AttachSource | Phase::AttachDestination => {
                    let (invitation, bytes) =
                        self.invitation.take().expect("Copy attach owns invitation");
                    if self.phase == Phase::AttachSource {
                        match Consumer::attach(invitation, bytes, Placement::Anywhere) {
                            Ok(reader) => {
                                self.reader = Some(reader);
                                self.phase = Phase::ArmSource;
                            }
                            Err(e) => self.attach_failure(e),
                        }
                    } else {
                        match Producer::attach(invitation, bytes, Placement::Anywhere) {
                            Ok(writer) => {
                                self.writer = Some(writer);
                                self.phase = Phase::ArmDestination;
                            }
                            Err(e) => self.attach_failure(e),
                        }
                    }
                    Ok(Step::Runnable)
                }
                Phase::ArmSource | Phase::ArmDestination => {
                    let is_source = self.phase == Phase::ArmSource;
                    let plan = if is_source {
                        self.reader
                            .as_mut()
                            .expect("Copy arm owns reader")
                            .terminal_wait_plan()
                    } else {
                        self.writer
                            .as_mut()
                            .expect("Copy arm owns writer")
                            .terminal_wait_plan()
                    };
                    {
                        let source = if is_source {
                            &mut self.source
                        } else {
                            &mut self.destination_source
                        };
                        if source.is_empty() {
                            source.install(
                                requests,
                                plan,
                                if is_source { SOURCE } else { DESTINATION },
                            )?;
                            Ok(Step::Runnable)
                        } else if source.source().is_some() {
                            self.phase = Phase::Call(if is_source {
                                Command::SourceStart
                            } else {
                                Command::DestinationStart
                            });
                            Ok(Step::Runnable)
                        } else {
                            Ok(Step::Parked)
                        }
                    }
                }
                Phase::Pump => {
                    let reading = self.written == self.filled;
                    if let Some(wait) = self.observe(requests, reading, !reading)? {
                        return Ok(wait);
                    }
                    match self.pump() {
                        Ok(ready) => Ok(if ready { Step::Runnable } else { Step::Parked }),
                        Err(error) => {
                            self.fail(Cause::Data(error));
                            Ok(Step::Runnable)
                        }
                    }
                }
                Phase::Eof => {
                    if let Some(wait) = self.observe(requests, false, false)? {
                        return Ok(wait);
                    }
                    match self
                        .writer
                        .as_mut()
                        .expect("Copy EOF owns producer")
                        .finish()
                    {
                        Ok(()) => self.phase = Phase::Call(Command::SourceFinish),
                        Err(e) => self.fail(Cause::Data(e.error)),
                    };
                    Ok(Step::Runnable)
                }
                Phase::Retiring => self.retire(requests),
                Phase::Complete => Ok(Step::Complete),
            }
        })();
        match result {
            Ok(next) => step(next),
            Err(error) => {
                // 已确认的业务完成不因关闭重试翻成失败，原任务仍持清理责任。
                if !self.succeeded {
                    self.fail(Cause::System(error));
                }
                self.cleanup_at = libkernel::time::snapshot().ok().map(|clock| {
                    clock
                        .now_ns
                        .saturating_add(libexecution::runtime::CLEANUP_RETRY_NS)
                });
                step(if self.cleanup_at.is_some() {
                    Step::Parked
                } else {
                    Step::Runnable
                })
            }
        }
    }
    pub fn registered(&mut self, kind: u64, source: SourceId) {
        match kind {
            SOURCE => self.source.registered(kind, source),
            DESTINATION => self.destination_source.registered(kind, source),
            _ => panic!("Copy source receipt changed role"),
        }
    }
    pub fn unregistered(&mut self, source: SourceId) {
        if self.source.unregistered(source) {
            self.source_observed = ObjectSignals::NONE;
        }
        if self.destination_source.unregistered(source) {
            self.destination_observed = ObjectSignals::NONE;
        }
    }
    pub fn refused<F>(&mut self, failure: RequestFailure<F>) {
        match failure {
            RequestFailure::Source { kind, error } => {
                self.source.refused(kind);
                self.destination_source.refused(kind);
                self.fail(Cause::System(error));
            }
            RequestFailure::Wake { error, .. } => self.fail(Cause::System(error)),
            RequestFailure::Spawn { .. } => {}
        }
    }
}
fn step(step: Step) -> Advance {
    Advance { step, work_done: 1 }
}
