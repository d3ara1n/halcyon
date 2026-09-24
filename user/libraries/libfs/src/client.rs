//! FAL2 Namespace walking 的正式同步运输适配器。

use alloc::{string::String, sync::Arc, vec::Vec};
use libfal::{
    client::{Client, Subscription},
    node::{NodeAttributes, NodeKind},
    protocol::{self, Expected, Request, ResolvePolicy, Response, Status},
};
use librunnel::blocking::{self, Consumer, Producer};
use rinlib::{
    ipc::{
        capability::Capability,
        invitation::{Invitation, InvitationFailure},
        message::{MailboxSender, SenderAdoptFailure},
        tunnel::{Endpoint, EndpointCleanup},
    },
    mm::Placement,
};

use crate::{
    prefix::DirectoryGrant,
    resolve::{LookupOutcome, NodeSummary, Position, WalkTransport},
};

mod copy;
pub use copy::{
    CopyCause, CopyCleanupError, CopyFailure, CopyRequest, CopySuccess, CreatedTarget,
    TargetCreationFailure, TargetLocator,
};

pub type Grant = DirectoryGrant<Arc<MailboxSender>>;

struct OpenTarget<'a> {
    parent: &'a Grant,
    path: &'a str,
    expected_identity: core::num::NonZeroU64,
    cancel: Option<erhino_shared::object::Handle>,
}

pub struct Transport {
    client: Client,
    deadline: rinlib::time::Deadline,
}

enum StreamRole {
    Reader(Consumer),
    Writer(Producer),
}

pub struct Stream {
    control: Option<MailboxSender>,
    role: Option<StreamRole>,
    offer: protocol::StreamOffer,
}

pub enum StreamOpenFailure {
    Call(libfal::client::ClientError),
    InvalidReply {
        owners: [Option<Capability>; 2],
    },
    Attach {
        control: Option<MailboxSender>,
        failure: AttachOwners,
    },
    Start {
        stream: Stream,
        error: libfal::client::ClientError,
    },
}

#[derive(Debug)]
pub enum AttachError {
    System(erhino_shared::call::SystemCallError),
    Protocol(librunnel::RunnelError),
}

#[derive(Debug)]
pub struct AttachOwners {
    pub error: AttachError,
    pub invitation: Option<Invitation>,
    pub endpoint: Option<Endpoint>,
    pub cleanup: Option<EndpointCleanup>,
}

fn client_error_has_owner(error: &libfal::client::ClientError) -> bool {
    match error {
        libfal::client::ClientError::Transport(error) => error.has_unretired_owners(),
        _ => false,
    }
}

fn retry_client_error(
    error: &mut libfal::client::ClientError,
) -> Result<(), erhino_shared::call::SystemCallError> {
    if let libfal::client::ClientError::Transport(error) = error {
        error.retry_cleanup()?;
    }
    Ok(())
}

impl StreamOpenFailure {
    pub fn has_unretired_owners(&self) -> bool {
        match self {
            Self::Call(error) => client_error_has_owner(error),
            Self::InvalidReply { owners } => owners.iter().any(Option::is_some),
            Self::Attach { control, failure } => {
                control.is_some()
                    || failure.invitation.is_some()
                    || failure.endpoint.is_some()
                    || failure.cleanup.is_some()
            }
            Self::Start { stream, error } => {
                stream.control.is_some() || stream.role.is_some() || client_error_has_owner(error)
            }
        }
    }

    pub fn retry_cleanup(&mut self) -> Result<(), erhino_shared::call::SystemCallError> {
        let mut first = None;
        match self {
            Self::Call(error) => return retry_client_error(error),
            Self::InvalidReply { owners } => {
                for owner in owners.iter_mut() {
                    if let Some(value) = owner.take()
                        && let Err((value, error)) = value.close()
                    {
                        *owner = Some(value);
                        first.get_or_insert(error);
                    }
                }
            }
            Self::Attach { control, failure } => {
                if let Some(value) = failure.endpoint.take()
                    && let Err((value, error)) = value.close()
                {
                    failure.endpoint = Some(value);
                    first.get_or_insert(error);
                }
                if let Some(value) = failure.cleanup.take()
                    && let Err((value, error)) = value.close()
                {
                    failure.cleanup = Some(value);
                    first.get_or_insert(error);
                }
                if let Some(value) = failure.invitation.take()
                    && let Err((value, error)) = value.close()
                {
                    failure.invitation = Some(value);
                    first.get_or_insert(error);
                }
                if let Some(value) = control.take()
                    && let Err((value, error)) = value.close()
                {
                    *control = Some(value);
                    first.get_or_insert(error);
                }
            }
            Self::Start { stream, error } => {
                if let Err(error) = stream.close_owned() {
                    first.get_or_insert(error);
                }
                if let Err(error) = retry_client_error(error) {
                    first.get_or_insert(error);
                }
            }
        }
        first.map_or(Ok(()), Err)
    }
}

pub enum StreamReadFailure {
    EmptyBuffer,
    Transport(librunnel::IoError),
    Control(libfal::client::ClientError),
    Business(protocol::StreamInfo),
}

impl Stream {
    pub const fn offer(&self) -> protocol::StreamOffer {
        self.offer
    }

    pub fn control_sender(&self) -> Option<&MailboxSender> {
        self.control.as_ref()
    }

    fn reader_mut(&mut self) -> Option<&mut Consumer> {
        match self.role.as_mut() {
            Some(StreamRole::Reader(role)) => Some(role),
            _ => None,
        }
    }

    fn writer_mut(&mut self) -> Option<&mut Producer> {
        match self.role.as_mut() {
            Some(StreamRole::Writer(role)) => Some(role),
            _ => None,
        }
    }

    fn read_until(
        &mut self,
        buffer: &mut [u8],
        deadline: rinlib::time::Deadline,
    ) -> Result<usize, librunnel::IoError> {
        match self.role.as_mut() {
            Some(StreamRole::Reader(role)) => role.read_exact_or_eof_until(buffer, deadline),
            _ => Err(librunnel::IoError {
                error: librunnel::RunnelError::BadFormat,
                completed: 0,
            }),
        }
    }

    pub fn write_until(
        &mut self,
        bytes: &[u8],
        deadline: rinlib::time::Deadline,
    ) -> Result<(), librunnel::IoError> {
        match self.role.as_mut() {
            Some(StreamRole::Writer(role)) => role.write_all_until(bytes, deadline),
            _ => Err(librunnel::IoError {
                error: librunnel::RunnelError::BadFormat,
                completed: 0,
            }),
        }
    }

    pub fn end_write(&mut self) -> Result<(), librunnel::IoError> {
        match self.role.as_mut() {
            Some(StreamRole::Writer(role)) => role.finish(),
            _ => Err(librunnel::IoError {
                error: librunnel::RunnelError::BadFormat,
                completed: 0,
            }),
        }
    }

    fn control_call(
        &self,
        client: &mut Client,
        request: &Request<'_>,
        deadline: rinlib::time::Deadline,
    ) -> Result<protocol::StreamInfo, libfal::client::ClientError> {
        self.control_call_cancelable(client, request, deadline, None)
    }

    fn control_call_cancelable(
        &self,
        client: &mut Client,
        request: &Request<'_>,
        deadline: rinlib::time::Deadline,
        cancel: Option<erhino_shared::object::Handle>,
    ) -> Result<protocol::StreamInfo, libfal::client::ClientError> {
        let control = self
            .control
            .as_ref()
            .ok_or(libfal::client::ClientError::Protocol)?;
        let reply = client.call_cancelable(control, request, deadline, cancel)?;
        let (_, Response::StreamInfo(info)) = protocol::decode_response(&reply.payload)
            .map_err(|_| libfal::client::ClientError::Protocol)?
        else {
            return Err(libfal::client::ClientError::Protocol);
        };
        if info.identity != self.offer.identity {
            return Err(libfal::client::ClientError::Protocol);
        }
        Ok(info)
    }

    pub fn close_data(&mut self) -> Result<(), erhino_shared::call::SystemCallError> {
        if let Some(role) = self.role.take() {
            let result = match role {
                StreamRole::Reader(role) => role
                    .close()
                    .map_err(|(role, error)| (StreamRole::Reader(role), error)),
                StreamRole::Writer(role) => role
                    .close()
                    .map_err(|(role, error)| (StreamRole::Writer(role), error)),
            };
            if let Err((role, error)) = result {
                self.role = Some(role);
                return Err(error);
            }
        }
        Ok(())
    }

    // 失败必须原样返还 affine owner，不能在错误路径再依赖分配。
    #[allow(clippy::result_large_err)]
    pub fn close(mut self) -> Result<(), (Self, erhino_shared::call::SystemCallError)> {
        if let Err(error) = self.close_owned() {
            return Err((self, error));
        }
        Ok(())
    }

    fn close_owned(&mut self) -> Result<(), erhino_shared::call::SystemCallError> {
        self.close_data()?;
        if let Some(control) = self.control.take()
            && let Err((control, error)) = control.close()
        {
            self.control = Some(control);
            return Err(error);
        }
        Ok(())
    }
}

impl Transport {
    pub const fn new(deadline: rinlib::time::Deadline) -> Self {
        Self {
            client: Client::new(),
            deadline,
        }
    }

    fn call_deadline(
        &self,
        session: rinlib::time::Deadline,
    ) -> Result<rinlib::time::Deadline, libfal::client::ClientError> {
        let session_end = session
            .instant()
            .map_err(|_| libfal::client::ClientError::Protocol)?
            .ok_or(libfal::client::ClientError::Protocol)?;
        let own_end = self
            .deadline
            .instant()
            .map_err(|_| libfal::client::ClientError::Protocol)?
            .unwrap_or(u64::MAX);
        let now = rinlib::time::Instant::now()
            .map_err(libfal::client::ClientError::System)?
            .as_nanos();
        Ok(rinlib::time::Deadline::at(
            session_end
                .min(own_end)
                .min(now.saturating_add(5_000_000_000)),
        ))
    }

    #[allow(clippy::result_large_err)]
    pub fn open_stream(
        &mut self,
        position: &Position<Grant>,
        direction: protocol::StreamDirection,
        offset: u64,
        length: Option<u64>,
        session_deadline: rinlib::time::Deadline,
    ) -> Result<Stream, StreamOpenFailure> {
        self.open_stream_with_cancel(position, direction, offset, length, session_deadline, None)
    }

    #[allow(clippy::result_large_err)]
    fn open_stream_with_cancel(
        &mut self,
        position: &Position<Grant>,
        direction: protocol::StreamDirection,
        offset: u64,
        length: Option<u64>,
        session_deadline: rinlib::time::Deadline,
        cancel: Option<erhino_shared::object::Handle>,
    ) -> Result<Stream, StreamOpenFailure> {
        if position.info.kind != NodeKind::Stream {
            return Err(StreamOpenFailure::Call(
                libfal::client::ClientError::Status(Status::Invalid),
            ));
        }
        let expected_identity = core::num::NonZeroU64::new(position.info.identity).ok_or(
            StreamOpenFailure::Call(libfal::client::ClientError::Protocol),
        )?;
        self.open_stream_at(
            OpenTarget {
                parent: &position.anchor,
                path: &position.rel,
                expected_identity,
                cancel,
            },
            direction,
            offset,
            length,
            session_deadline,
        )
    }

    #[allow(clippy::result_large_err)]
    fn open_stream_at(
        &mut self,
        target: OpenTarget<'_>,
        direction: protocol::StreamDirection,
        offset: u64,
        length: Option<u64>,
        session_deadline: rinlib::time::Deadline,
    ) -> Result<Stream, StreamOpenFailure> {
        let deadline = self
            .call_deadline(session_deadline)
            .map_err(StreamOpenFailure::Call)?;
        let request = Request::Open {
            path: target.path,
            expected_identity: Some(target.expected_identity),
            direction,
            offset,
            length,
            session_deadline,
            stream_protocol: protocol::RNL2_PROTOCOL,
            tunnel_bytes: 0,
        };
        let mut reply = self
            .client
            .call_cancelable(target.parent.endpoint(), &request, deadline, target.cancel)
            .map_err(StreamOpenFailure::Call)?;
        let (_, Response::StreamOffer(offer)) =
            protocol::decode_response(&reply.payload).map_err(|_| {
                StreamOpenFailure::InvalidReply {
                    owners: [reply.handles.take(0).ok(), reply.handles.take(1).ok()],
                }
            })?
        else {
            return Err(StreamOpenFailure::InvalidReply {
                owners: [reply.handles.take(0).ok(), reply.handles.take(1).ok()],
            });
        };
        let control_owner = reply
            .handles
            .take(0)
            .map_err(|_| StreamOpenFailure::InvalidReply {
                owners: [None, reply.handles.take(1).ok()],
            })?;
        let invitation_owner = match reply.handles.take(1) {
            Ok(owner) => owner,
            Err(_) => {
                return Err(StreamOpenFailure::InvalidReply {
                    owners: [Some(control_owner), None],
                });
            }
        };
        let (control, description) = match MailboxSender::from_capability(control_owner) {
            Ok(control) => control,
            Err(SenderAdoptFailure { owner, .. }) => {
                return Err(StreamOpenFailure::InvalidReply {
                    owners: [Some(owner), Some(invitation_owner)],
                });
            }
        };
        if description.object_id != offer.identity
            || offer.direction != direction
            || offer.start != offset
            || (direction == protocol::StreamDirection::Read && offer.read_end < offset)
            || offer.tunnel_bytes as usize != 3 * erhino_shared::proc::PROCESS_PAGE_SIZE
        {
            return Err(StreamOpenFailure::InvalidReply {
                owners: [Some(control.into_capability()), Some(invitation_owner)],
            });
        }
        let invitation = match Invitation::from_capability(invitation_owner) {
            Ok(invitation) => invitation,
            Err(InvitationFailure { owner, .. }) => {
                return Err(StreamOpenFailure::InvalidReply {
                    owners: [Some(control.into_capability()), Some(owner)],
                });
            }
        };
        let attachment = match direction {
            protocol::StreamDirection::Read => {
                Consumer::attach(invitation, Placement::Anywhere).map(StreamRole::Reader)
            }
            protocol::StreamDirection::Write => {
                Producer::attach(invitation, Placement::Anywhere).map(StreamRole::Writer)
            }
        };
        let role = match attachment {
            Ok(role) => role,
            Err(failure) => {
                let failure = match failure {
                    blocking::AttachFailure::Tunnel(
                        rinlib::ipc::invitation::AttachFailure::Unconsumed { invitation, error },
                    ) => AttachOwners {
                        error: AttachError::System(error),
                        invitation: Some(invitation),
                        endpoint: None,
                        cleanup: None,
                    },
                    blocking::AttachFailure::Tunnel(
                        rinlib::ipc::invitation::AttachFailure::Consumed { cleanup, error },
                    ) => AttachOwners {
                        error: AttachError::System(error),
                        invitation: None,
                        endpoint: None,
                        cleanup,
                    },
                    blocking::AttachFailure::Protocol(failure) => AttachOwners {
                        error: AttachError::Protocol(failure.error),
                        invitation: None,
                        endpoint: Some(failure.owner),
                        cleanup: None,
                    },
                };
                return Err(StreamOpenFailure::Attach {
                    control: Some(control),
                    failure,
                });
            }
        };
        let stream = Stream {
            control: Some(control),
            role: Some(role),
            offer,
        };
        loop {
            let now = match rinlib::time::Instant::now() {
                Ok(now) => now,
                Err(error) => {
                    return Err(StreamOpenFailure::Start {
                        stream,
                        error: libfal::client::ClientError::System(error),
                    });
                }
            };
            if !offer
                .offer_deadline
                .instant()
                .ok()
                .flatten()
                .is_some_and(|end| now.as_nanos() < end)
            {
                return Err(StreamOpenFailure::Start {
                    stream,
                    error: libfal::client::ClientError::Status(Status::Cancelled),
                });
            }
            match stream.control_call_cancelable(
                &mut self.client,
                &Request::Start,
                deadline,
                target.cancel,
            ) {
                Ok(info) if info.state == protocol::StreamState::Active => return Ok(stream),
                Ok(_) => {
                    return Err(StreamOpenFailure::Start {
                        stream,
                        error: libfal::client::ClientError::Protocol,
                    });
                }
                Err(libfal::client::ClientError::Status(Status::Busy)) => {
                    match stream.control_call_cancelable(
                        &mut self.client,
                        &Request::QueryStream,
                        deadline,
                        target.cancel,
                    ) {
                        Ok(info) if info.state == protocol::StreamState::Offered => continue,
                        Ok(info) if info.state == protocol::StreamState::Active => {
                            return Ok(stream);
                        }
                        Ok(_) => {
                            return Err(StreamOpenFailure::Start {
                                stream,
                                error: libfal::client::ClientError::Status(Status::Cancelled),
                            });
                        }
                        Err(error) => return Err(StreamOpenFailure::Start { stream, error }),
                    }
                }
                Err(error) => return Err(StreamOpenFailure::Start { stream, error }),
            }
        }
    }

    pub fn query_stream(
        &mut self,
        stream: &Stream,
        deadline: rinlib::time::Deadline,
    ) -> Result<protocol::StreamInfo, libfal::client::ClientError> {
        stream.control_call(&mut self.client, &Request::QueryStream, deadline)
    }

    pub fn read_stream_until(
        &mut self,
        stream: &mut Stream,
        buffer: &mut [u8],
        deadline: rinlib::time::Deadline,
    ) -> Result<usize, StreamReadFailure> {
        if buffer.is_empty() {
            return Err(StreamReadFailure::EmptyBuffer);
        }
        let read = stream
            .read_until(buffer, deadline)
            .map_err(StreamReadFailure::Transport)?;
        if read != 0 {
            return Ok(read);
        }
        let info = self
            .finish_stream(stream, deadline)
            .map_err(StreamReadFailure::Control)?;
        if info.outcome != Status::Ok || info.reason != protocol::StreamReason::Completed {
            return Err(StreamReadFailure::Business(info));
        }
        Ok(0)
    }

    pub fn finish_stream(
        &mut self,
        stream: &Stream,
        deadline: rinlib::time::Deadline,
    ) -> Result<protocol::StreamInfo, libfal::client::ClientError> {
        stream.control_call(&mut self.client, &Request::FinishStream, deadline)
    }

    fn finish_stream_cancelable(
        &mut self,
        stream: &Stream,
        deadline: rinlib::time::Deadline,
        cancel: Option<erhino_shared::object::Handle>,
    ) -> Result<protocol::StreamInfo, libfal::client::ClientError> {
        stream.control_call_cancelable(&mut self.client, &Request::FinishStream, deadline, cancel)
    }

    pub fn cancel_stream(
        &mut self,
        stream: &Stream,
        deadline: rinlib::time::Deadline,
    ) -> Result<protocol::StreamInfo, libfal::client::ClientError> {
        stream.control_call(&mut self.client, &Request::CancelStream, deadline)
    }

    pub fn move_entry(
        &mut self,
        source: &Grant,
        source_parent: &str,
        source_name: &str,
        destination: &Grant,
        destination_name: &str,
        expected: Expected,
    ) -> Result<(), Status> {
        self.client
            .move_entry(
                source.endpoint(),
                destination.endpoint(),
                libfal::client::MoveEntry {
                    source_parent,
                    source_name,
                    destination_name,
                    expected,
                },
                self.deadline,
            )
            .map_err(|error| match error {
                libfal::client::ClientError::Status(status) => status,
                _ => Status::Internal,
            })
    }

    pub fn copy_property(
        &mut self,
        source: &Grant,
        source_path: &str,
        destination: &Grant,
        destination_name: &str,
        rights: libfal::authority::FalRights,
    ) -> Result<(), Status> {
        self.client
            .copy_property(
                source.endpoint(),
                source_path,
                destination.endpoint(),
                destination_name,
                rights,
                self.deadline,
            )
            .map_err(|error| match error {
                libfal::client::ClientError::Status(status) => status,
                _ => Status::Internal,
            })
    }

    pub fn subscribe(
        &mut self,
        grant: &Grant,
        path: &str,
        mask: protocol::WatchMask,
    ) -> Result<Subscription, Status> {
        self.client
            .subscribe(grant.endpoint(), path, mask, self.deadline)
            .map_err(|error| match error {
                libfal::client::ClientError::Status(status) => status,
                _ => Status::Internal,
            })
    }

    pub fn query_subscription(
        &mut self,
        subscription: &Subscription,
    ) -> Result<protocol::SubscriptionInfo, Status> {
        self.client
            .query_subscription(subscription, self.deadline)
            .map_err(|error| match error {
                libfal::client::ClientError::Status(status) => status,
                _ => Status::Internal,
            })
    }

    pub fn unsubscribe(&mut self, subscription: &Subscription) -> Result<(), Status> {
        self.client
            .unsubscribe(subscription, self.deadline)
            .map_err(|error| match error {
                libfal::client::ClientError::Status(status) => status,
                _ => Status::Internal,
            })
    }
}

fn summary(info: protocol::NodeInfo, value: Vec<u8>) -> NodeSummary {
    NodeSummary {
        identity: info.identity,
        version: info.version,
        kind: info.kind,
        rights: info.rights,
        attributes: NodeAttributes::NONE,
        size: info.size,
        value,
    }
}

impl WalkTransport for Transport {
    type Endpoint = Arc<MailboxSender>;

    fn lookup(
        &mut self,
        dir: &Grant,
        policy: ResolvePolicy,
        path: &str,
    ) -> Result<LookupOutcome<Grant>, Status> {
        let mut reply = self
            .client
            .call(dir.endpoint(), &Request::Lookup { path }, self.deadline)
            .map_err(|error| match error {
                libfal::client::ClientError::Status(status) => status,
                _ => Status::Internal,
            })?;
        let (_, response) =
            protocol::decode_response(&reply.payload).map_err(|_| Status::Internal)?;
        match response {
            Response::Node(info) => Ok(LookupOutcome::Found(summary(info, Vec::new()))),
            Response::Delegate {
                node: _,
                consumed,
                remaining,
            } => {
                let capability = reply.handles.take(0).map_err(|_| Status::Internal)?;
                let (sender, _) = MailboxSender::from_capability(capability)
                    .map_err(|SenderAdoptFailure { .. }| Status::Internal)?;
                Ok(LookupOutcome::Delegate {
                    dir: DirectoryGrant::new(Arc::new(sender)),
                    consumed: String::from(consumed),
                    remaining: String::from(remaining),
                })
            }
            Response::LinkBoundary {
                node,
                consumed: _,
                target,
                remaining,
            } if policy == ResolvePolicy::NoFollowFinal && remaining.is_empty() => Ok(
                LookupOutcome::Found(summary(node, target.as_bytes().to_vec())),
            ),
            Response::LinkBoundary {
                consumed,
                target,
                remaining,
                ..
            } => Ok(LookupOutcome::Link {
                consumed: String::from(consumed),
                target: String::from(target),
                remaining: String::from(remaining),
            }),
            _ => Err(Status::Internal),
        }
    }
}
