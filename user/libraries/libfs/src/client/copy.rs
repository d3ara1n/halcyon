use super::*;
use core::num::NonZeroU64;
use erhino_shared::call::SystemCallError;
use erhino_shared::{
    object::ObjectSignals,
    wait::{WaitItem, WaitReason},
};
use libfal::{
    authority::FalRights,
    client::{Client, ClientCallFailure, ClientError, ClientOperation, ClientProgress},
    protocol::{NodeInfo, Op},
};
use librpc::CallCause;

mod pump;
use pump::{PumpFault, PumpRunFailure, PumpSummary, run_pump};

pub struct TargetLocator {
    parent: Grant,
    name: String,
    provider_id: u64,
}

impl TargetLocator {
    pub fn parent(&self) -> &Grant {
        &self.parent
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub const fn provider_id(&self) -> u64 {
        self.provider_id
    }
}

pub struct CreatedTarget {
    locator: TargetLocator,
    info: NodeInfo,
}

impl CreatedTarget {
    pub fn locator(&self) -> &TargetLocator {
        &self.locator
    }

    pub const fn info(&self) -> NodeInfo {
        self.info
    }

    #[allow(clippy::result_large_err)]
    pub fn open_write(
        &self,
        transport: &mut Transport,
        length: Option<u64>,
        session_deadline: rinlib::time::Deadline,
    ) -> Result<Stream, StreamOpenFailure> {
        self.open_write_cancelable(transport, length, session_deadline, None)
    }

    #[allow(clippy::result_large_err)]
    fn open_write_cancelable(
        &self,
        transport: &mut Transport,
        length: Option<u64>,
        session_deadline: rinlib::time::Deadline,
        cancel: Option<erhino_shared::object::Handle>,
    ) -> Result<Stream, StreamOpenFailure> {
        let expected = NonZeroU64::new(self.info.identity).ok_or(StreamOpenFailure::Call(
            libfal::client::ClientError::Protocol,
        ))?;
        transport.open_stream_at(
            OpenTarget {
                parent: &self.locator.parent,
                path: &self.locator.name,
                expected_identity: expected,
                cancel,
            },
            protocol::StreamDirection::Write,
            0,
            length,
            session_deadline,
        )
    }

    #[allow(clippy::result_large_err)]
    pub fn delete_if_current(
        self,
        transport: &mut Transport,
        deadline: rinlib::time::Deadline,
    ) -> Result<(), (Self, libfal::client::ClientError)> {
        let lookup = match transport.client.call(
            self.locator.parent.endpoint(),
            &Request::Lookup {
                path: &self.locator.name,
            },
            deadline,
        ) {
            Ok(reply) => reply,
            Err(error) => return Err((self, error)),
        };
        let version = match protocol::decode_response(&lookup.payload) {
            Ok((header, Response::Node(info)))
                if header.op == Op::Lookup && info.identity == self.info.identity =>
            {
                info.version
            }
            Ok((header, Response::Node(_))) if header.op == Op::Lookup => {
                return Err((self, libfal::client::ClientError::Status(Status::Conflict)));
            }
            Ok((header, _)) if header.op == Op::Lookup => {
                return Err((self, libfal::client::ClientError::Status(Status::Conflict)));
            }
            _ => return Err((self, libfal::client::ClientError::Protocol)),
        };
        match transport.client.call(
            self.locator.parent.endpoint(),
            &Request::Delete {
                name: &self.locator.name,
                expected: Expected {
                    identity: self.info.identity,
                    version,
                },
            },
            deadline,
        ) {
            Ok(_) => Ok(()),
            Err(error) => Err((self, error)),
        }
    }
}

pub enum TargetCreationFailure {
    NoRequest(libfal::client::ClientError),
    Rejected {
        status: Status,
        target: TargetLocator,
    },
    Unknown {
        cause: libfal::client::ClientError,
        target: TargetLocator,
    },
}

impl Transport {
    #[allow(clippy::result_large_err)]
    pub fn create_stream_target(
        &mut self,
        parent: &Grant,
        name: &str,
        rights: FalRights,
        deadline: rinlib::time::Deadline,
    ) -> Result<CreatedTarget, TargetCreationFailure> {
        self.create_stream_target_cancelable(parent, name, rights, deadline, None)
    }

    #[allow(clippy::result_large_err)]
    fn create_stream_target_cancelable(
        &mut self,
        parent: &Grant,
        name: &str,
        rights: FalRights,
        deadline: rinlib::time::Deadline,
        cancel: Option<erhino_shared::object::Handle>,
    ) -> Result<CreatedTarget, TargetCreationFailure> {
        let mut owned = String::new();
        owned.try_reserve_exact(name.len()).map_err(|_| {
            TargetCreationFailure::NoRequest(libfal::client::ClientError::System(
                erhino_shared::call::SystemCallError::OutOfMemory,
            ))
        })?;
        owned.push_str(name);
        let provider_id = parent
            .endpoint()
            .description()
            .map_err(|error| {
                TargetCreationFailure::NoRequest(libfal::client::ClientError::System(error))
            })?
            .related_object_id;
        let locator = TargetLocator {
            parent: parent.clone(),
            name: owned,
            provider_id,
        };
        let reply = match self.client.call_classified(
            locator.parent.endpoint(),
            &Request::Create {
                name: &locator.name,
                kind: NodeKind::Stream,
                rights,
                value: &[],
            },
            deadline,
            cancel,
        ) {
            Ok(reply) => reply,
            Err(ClientCallFailure::Rejected(status)) => {
                return Err(TargetCreationFailure::Rejected {
                    status,
                    target: locator,
                });
            }
            Err(ClientCallFailure::NoRequest(error)) => {
                return Err(TargetCreationFailure::NoRequest(error));
            }
            Err(ClientCallFailure::Unsent(error)) => {
                return Err(TargetCreationFailure::NoRequest(
                    libfal::client::ClientError::Transport(error),
                ));
            }
            Err(ClientCallFailure::Unknown(error)) => {
                return Err(TargetCreationFailure::Unknown {
                    cause: error,
                    target: locator,
                });
            }
        };
        let info = match protocol::decode_response(&reply.payload) {
            Ok((header, Response::Node(info)))
                if header.op == Op::Create
                    && info.kind == NodeKind::Stream
                    && info.identity != 0
                    && info.rights == rights =>
            {
                info
            }
            _ => {
                return Err(TargetCreationFailure::Unknown {
                    cause: libfal::client::ClientError::Protocol,
                    target: locator,
                });
            }
        };
        Ok(CreatedTarget { locator, info })
    }
}

pub struct CopyRequest<'a> {
    pub source: &'a Position<Grant>,
    pub destination_parent: &'a Grant,
    pub destination_name: &'a str,
    pub destination_rights: FalRights,
    pub deadline: rinlib::time::Deadline,
    pub cancel: Option<&'a Capability>,
}

pub struct CopySuccess {
    pub target: CreatedTarget,
    pub source_result: protocol::StreamInfo,
    pub target_result: protocol::StreamInfo,
    pub bytes: u64,
}

pub enum CopyCause {
    Cancelled,
    CancellationProbe(SystemCallError),
    SourceOpen(StreamOpenFailure),
    SourceStart(libfal::client::ClientError),
    TargetCreate(TargetCreationFailure),
    TargetOpen(StreamOpenFailure),
    TargetStart(libfal::client::ClientError),
    Pump(PumpFault),
    SourceFinish(libfal::client::ClientError),
    TargetEof(librunnel::IoError),
    TargetFinish(libfal::client::ClientError),
    Business(protocol::StreamInfo),
    ProgressMismatch,
    Cleanup(SystemCallError),
    CleanupControl { source: bool },
}

impl CopyCause {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::CancellationProbe(_) => "cancellation observation",
            Self::SourceOpen(_) => "source open",
            Self::SourceStart(_) => "source start",
            Self::TargetCreate(_) => "target create",
            Self::TargetOpen(_) => "target open",
            Self::TargetStart(_) => "target start",
            Self::Pump(_) => "stream transfer",
            Self::SourceFinish(_) => "source finish",
            Self::TargetEof(_) => "target EOF",
            Self::TargetFinish(_) => "target finish",
            Self::Business(_) => "business result",
            Self::ProgressMismatch => "progress mismatch",
            Self::Cleanup(_) => "owner cleanup",
            Self::CleanupControl { .. } => "control cleanup",
        }
    }
}

fn cancellation_observed(cancel: Option<&Capability>) -> Result<bool, SystemCallError> {
    let Some(cancel) = cancel else {
        return Ok(false);
    };
    let now = rinlib::time::Instant::now()?.as_nanos();
    match rinlib::ipc::wait::wait_until(
        &[WaitItem::new(
            cancel.as_handle(),
            ObjectSignals::READABLE | ObjectSignals::CLOSED,
            0,
        )],
        rinlib::time::Deadline::at(now),
    ) {
        Ok(result) => {
            let reason =
                WaitReason::from_u32(result.reason).ok_or(SystemCallError::InternalError)?;
            Ok(matches!(
                reason,
                WaitReason::Signaled | WaitReason::Closed | WaitReason::Cancelled
            ) && result
                .observed
                .intersects(ObjectSignals::READABLE | ObjectSignals::CLOSED))
        }
        Err(SystemCallError::DeadlineExpired) => Ok(false),
        Err(error) => Err(error),
    }
}

pub enum CopyCleanupError {
    Pump(PumpFault),
    Control { source: bool },
    Close(SystemCallError),
    Deadline(SystemCallError),
}

fn finish_cancel(
    operation: &mut ClientOperation<'_>,
    mut step: Result<ClientProgress, CallCause>,
    identity: u64,
) -> Result<protocol::StreamInfo, ClientError> {
    loop {
        match step {
            Ok(ClientProgress::Reply(reply)) => {
                let (_, Response::StreamInfo(info)) =
                    protocol::decode_response(&reply.payload).map_err(|_| ClientError::Protocol)?
                else {
                    return Err(ClientError::Protocol);
                };
                return if info.identity == identity {
                    Ok(info)
                } else {
                    Err(ClientError::Protocol)
                };
            }
            Ok(ClientProgress::Rejected(error)) => return Err(error),
            Ok(ClientProgress::Pending) => {
                if let Err(cause) = operation.wait_ready() {
                    return Err(ClientError::Transport(
                        operation.abort(cause).expect("waiting cancellation remains pending"),
                    ));
                }
                step = operation.advance();
            }
            Err(cause) => {
                return Err(ClientError::Transport(
                    operation.abort(cause).expect("failed cancellation remains pending"),
                ));
            }
        }
    }
}

pub struct CopyFailure {
    pub cause: CopyCause,
    pub source_result: Option<protocol::StreamInfo>,
    pub target_result: Option<protocol::StreamInfo>,
    pub source_bytes: u64,
    pub target_bytes: u64,
    source: Option<Stream>,
    destination: Option<Stream>,
    target: Option<CreatedTarget>,
    pump: Option<PumpRunFailure>,
    cleanup_errors: [Option<libfal::client::ClientError>; 2],
}

impl CopyFailure {
    fn new(cause: CopyCause) -> Self {
        Self {
            cause,
            source_result: None,
            target_result: None,
            source_bytes: 0,
            target_bytes: 0,
            source: None,
            destination: None,
            target: None,
            pump: None,
            cleanup_errors: [None, None],
        }
    }

    pub fn target(&self) -> Option<&CreatedTarget> {
        self.target.as_ref()
    }

    pub fn take_target(&mut self) -> Option<CreatedTarget> {
        self.target.take()
    }

    /// 仅报告本地流/RPC owner；部分目标或 CreationUnknown 仍由 target/locator 单独持有。
    pub fn has_unretired_owners(&self) -> bool {
        let embedded = match &self.cause {
            CopyCause::SourceOpen(failure) | CopyCause::TargetOpen(failure) => {
                failure.has_unretired_owners()
            }
            CopyCause::SourceStart(error)
            | CopyCause::TargetStart(error)
            | CopyCause::SourceFinish(error)
            | CopyCause::TargetFinish(error) => client_error_has_owner(error),
            CopyCause::TargetCreate(TargetCreationFailure::NoRequest(error))
            | CopyCause::TargetCreate(TargetCreationFailure::Unknown { cause: error, .. }) => {
                client_error_has_owner(error)
            }
            _ => false,
        };
        embedded
            || self.pump.is_some()
            || self.source.is_some()
            || self.destination.is_some()
            || self
                .cleanup_errors
                .iter()
                .flatten()
                .any(client_error_has_owner)
    }

    pub fn cleanup_error(&self, source: bool) -> Option<&libfal::client::ClientError> {
        self.cleanup_errors[if source { 0 } else { 1 }].as_ref()
    }

    pub fn cleanup_error_mut(&mut self, source: bool) -> Option<&mut libfal::client::ClientError> {
        self.cleanup_errors[if source { 0 } else { 1 }].as_mut()
    }

    pub fn retry_cleanup(
        &mut self,
        deadline: rinlib::time::Deadline,
    ) -> Result<(), CopyCleanupError> {
        if let Some(pump) = self.pump.take() {
            let (Some(source), Some(destination)) =
                (self.source.as_mut(), self.destination.as_mut())
            else {
                self.pump = Some(pump);
                return Err(CopyCleanupError::Pump(PumpFault::System(
                    SystemCallError::InternalError,
                )));
            };
            match pump.retry_cleanup(source, destination) {
                Ok(summary) => {
                    self.source_bytes = summary.source_bytes;
                    self.target_bytes = summary.target_bytes;
                }
                Err(pump) => {
                    let fault = pump.summary.fault.unwrap_or(PumpFault::System(
                        erhino_shared::call::SystemCallError::ObjectBusy,
                    ));
                    self.pump = Some(pump);
                    return Err(CopyCleanupError::Pump(fault));
                }
            }
        }
        let mut first = None;
        if let Some(source) = self.source.as_mut()
            && let Err(error) = source.close_data()
        {
            first = Some(CopyCleanupError::Close(error));
        }
        if let Some(destination) = self.destination.as_mut()
            && let Err(error) = destination.close_data()
        {
            first.get_or_insert(CopyCleanupError::Close(error));
        }
        for (index, stored) in self.cleanup_errors.iter_mut().enumerate() {
            if let Some(error) = stored.as_mut() {
                if retry_client_error(error).is_err() {
                    if first.is_none() {
                        first = Some(CopyCleanupError::Control { source: index == 0 });
                    }
                } else {
                    *stored = None;
                }
            }
        }
        let needs_cancel = (self.source.is_some()
            && self.source_result.is_none()
            && self.cleanup_errors[0].is_none())
            || (self.destination.is_some()
                && self.target_result.is_none()
                && self.cleanup_errors[1].is_none());
        let deadline = if needs_cancel {
            let limit = rinlib::time::timeout_millis(5_000)
                .map_err(CopyCleanupError::Deadline)?;
            let limit_ns = limit.instant().map_err(|_| {
                CopyCleanupError::Deadline(SystemCallError::IllegalArgument)
            })?.expect("finite cleanup deadline");
            match deadline.instant().map_err(|_| {
                CopyCleanupError::Deadline(SystemCallError::IllegalArgument)
            })? {
                Some(requested) => rinlib::time::Deadline::at(requested.min(limit_ns)),
                None => limit,
            }
        } else {
            deadline
        };
        let mut source_client = Client::new();
        let mut target_client = Client::new();
        let source_cancel = self.source.as_ref().filter(|_| {
            self.source_result.is_none() && self.cleanup_errors[0].is_none()
        }).map(|stream| {
            stream.control_sender().ok_or(ClientCallFailure::NoRequest(ClientError::Protocol))
                .and_then(|control| source_client.begin_call(
                    control, &Request::CancelStream, deadline, None,
                ))
        });
        let target_cancel = self.destination.as_ref().filter(|_| {
            self.target_result.is_none() && self.cleanup_errors[1].is_none()
        }).map(|stream| {
            stream.control_sender().ok_or(ClientCallFailure::NoRequest(ClientError::Protocol))
                .and_then(|control| target_client.begin_call(
                    control, &Request::CancelStream, deadline, None,
                ))
        });
        let mut source_cancel = source_cancel;
        let mut target_cancel = target_cancel;
        let source_step = source_cancel.as_mut().and_then(|result| result.as_mut().ok())
            .map(|operation| operation.advance());
        let target_step = target_cancel.as_mut().and_then(|result| result.as_mut().ok())
            .map(|operation| operation.advance());
        for (index, (attempt, step)) in [
            (source_cancel, source_step),
            (target_cancel, target_step),
        ].into_iter().enumerate() {
            let Some(result) = attempt else { continue };
            let outcome = match result {
                Ok(mut operation) => finish_cancel(
                    &mut operation,
                    step.expect("prepared cancellation has an initial step"),
                    if index == 0 {
                        self.source.as_ref().expect("source cancellation prepared").offer.identity
                    } else {
                        self.destination.as_ref().expect("target cancellation prepared").offer.identity
                    },
                ),
                Err(error) => Err(error.into_error()),
            };
            match outcome {
                Ok(info) if index == 0 => self.source_result = Some(info),
                Ok(info) => self.target_result = Some(info),
                Err(error) => {
                    self.cleanup_errors[index] = Some(error);
                    first.get_or_insert(CopyCleanupError::Control { source: index == 0 });
                }
            }
        }
        if let Some(source) = self.source.take()
            && let Err((source, error)) = source.close()
        {
            self.source = Some(source);
            if first.is_none() {
                first = Some(CopyCleanupError::Close(error));
            }
        }
        if let Some(destination) = self.destination.take()
            && let Err((destination, error)) = destination.close()
        {
            self.destination = Some(destination);
            if first.is_none() {
                first = Some(CopyCleanupError::Close(error));
            }
        }
        if let CopyCause::SourceOpen(failure) | CopyCause::TargetOpen(failure) = &mut self.cause
            && let Err(error) = failure.retry_cleanup()
            && first.is_none()
        {
            first = Some(CopyCleanupError::Close(error));
        }
        let client_error = match &mut self.cause {
            CopyCause::SourceStart(error)
            | CopyCause::TargetStart(error)
            | CopyCause::SourceFinish(error)
            | CopyCause::TargetFinish(error) => Some(error),
            CopyCause::TargetCreate(TargetCreationFailure::NoRequest(error))
            | CopyCause::TargetCreate(TargetCreationFailure::Unknown { cause: error, .. }) => {
                Some(error)
            }
            _ => None,
        };
        if let Some(error) = client_error
            && let Err(error) = retry_client_error(error)
            && first.is_none()
        {
            first = Some(CopyCleanupError::Close(error));
        }
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Transport {
    #[allow(clippy::result_large_err)]
    pub fn copy_stream(&mut self, request: CopyRequest<'_>) -> Result<CopySuccess, CopyFailure> {
        let mut state = CopyFailure::new(CopyCause::ProgressMismatch);
        let cancel = request.cancel.map(Capability::as_handle);
        macro_rules! fail {
            ($cause:expr) => {{
                state.cause = $cause;
                let cleanup_deadline = rinlib::time::timeout_millis(5_000).unwrap_or(request.deadline);
                let _ = state.retry_cleanup(cleanup_deadline);
                return Err(state);
            }};
        }
        macro_rules! check_cancel {
            () => {
                match cancellation_observed(request.cancel) {
                    Ok(true) => fail!(CopyCause::Cancelled),
                    Ok(false) => {}
                    Err(error) => fail!(CopyCause::CancellationProbe(error)),
                }
            };
        }
        check_cancel!();
        let source = match self.open_stream_with_cancel(
            request.source,
            protocol::StreamDirection::Read,
            0,
            None,
            request.deadline,
            cancel,
        ) {
            Ok(source) => source,
            Err(StreamOpenFailure::Start { stream, error }) => {
                state.source = Some(stream);
                fail!(CopyCause::SourceStart(error));
            }
            Err(failure) => fail!(CopyCause::SourceOpen(failure)),
        };
        state.source = Some(source);
        check_cancel!();
        let target = match self.create_stream_target_cancelable(
            request.destination_parent,
            request.destination_name,
            request.destination_rights,
            request.deadline,
            cancel,
        ) {
            Ok(target) => target,
            Err(failure) => fail!(CopyCause::TargetCreate(failure)),
        };
        state.target = Some(target);
        check_cancel!();
        let destination = match state
            .target
            .as_ref()
            .expect("Copy target prepared")
            .open_write_cancelable(self, None, request.deadline, cancel)
        {
            Ok(destination) => destination,
            Err(StreamOpenFailure::Start { stream, error }) => {
                state.destination = Some(stream);
                fail!(CopyCause::TargetStart(error));
            }
            Err(failure) => fail!(CopyCause::TargetOpen(failure)),
        };
        state.destination = Some(destination);
        check_cancel!();
        let pump = run_pump(
            state.source.as_mut().expect("Copy source prepared"),
            state
                .destination
                .as_mut()
                .expect("Copy destination prepared"),
            request.cancel,
            request.deadline,
        );
        match pump {
            Ok(PumpSummary {
                source_bytes,
                target_bytes,
                fault,
            }) => {
                state.source_bytes = source_bytes;
                state.target_bytes = target_bytes;
                if let Some(fault) = fault {
                    fail!(CopyCause::Pump(fault));
                }
            }
            Err(failure) => {
                state.source_bytes = failure.summary.source_bytes;
                state.target_bytes = failure.summary.target_bytes;
                let fault = failure
                    .summary
                    .fault
                    .unwrap_or(PumpFault::System(SystemCallError::InternalError));
                state.pump = Some(failure);
                fail!(CopyCause::Pump(fault));
            }
        }
        check_cancel!();
        let source_result = match self.finish_stream_cancelable(
            state.source.as_ref().expect("Copy source prepared"),
            request.deadline,
            cancel,
        ) {
            Ok(info) => info,
            Err(error) => fail!(CopyCause::SourceFinish(error)),
        };
        state.source_result = Some(source_result);
        if source_result.outcome != Status::Ok
            || source_result.reason != protocol::StreamReason::Completed
        {
            fail!(CopyCause::Business(source_result));
        }
        check_cancel!();
        if let Err(error) = state
            .destination
            .as_mut()
            .expect("Copy destination prepared")
            .end_write()
        {
            fail!(CopyCause::TargetEof(error));
        }
        check_cancel!();
        let target_result = match self.finish_stream_cancelable(
            state
                .destination
                .as_ref()
                .expect("Copy destination prepared"),
            request.deadline,
            cancel,
        ) {
            Ok(info) => info,
            Err(error) => fail!(CopyCause::TargetFinish(error)),
        };
        state.target_result = Some(target_result);
        if target_result.outcome != Status::Ok
            || target_result.reason != protocol::StreamReason::Completed
        {
            fail!(CopyCause::Business(target_result));
        }
        if state.source_bytes != state.target_bytes
            || state.source_bytes != source_result.accepted
            || state.target_bytes != target_result.accepted
            || source_result.transported != source_result.accepted
            || target_result.transported != target_result.accepted
        {
            fail!(CopyCause::ProgressMismatch);
        }
        if let Err(error) = state.retry_cleanup(request.deadline) {
            state.cause = match error {
                CopyCleanupError::Close(error) => CopyCause::Cleanup(error),
                CopyCleanupError::Deadline(error) => CopyCause::Cleanup(error),
                CopyCleanupError::Pump(fault) => CopyCause::Pump(fault),
                CopyCleanupError::Control { source } => CopyCause::CleanupControl { source },
            };
            return Err(state);
        }
        Ok(CopySuccess {
            target: state.target.take().expect("Copy target prepared"),
            source_result,
            target_result,
            bytes: state.source_bytes,
        })
    }
}
