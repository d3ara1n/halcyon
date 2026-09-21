//! FAL2 Namespace walking 的正式同步运输适配器。

use alloc::{string::String, sync::Arc, vec::Vec};
use libfal::{
    client::{Client, Subscription},
    node::NodeAttributes,
    protocol::{self, Expected, Request, ResolvePolicy, Response, Status},
};
use rinlib::ipc::message::{MailboxSender, SenderAdoptFailure};

use crate::{
    prefix::DirectoryGrant,
    resolve::{LookupOutcome, NodeSummary, WalkTransport},
};

pub type Grant = DirectoryGrant<Arc<MailboxSender>>;

pub struct Transport {
    client: Client,
    deadline: rinlib::time::Deadline,
}

impl Transport {
    pub const fn new(deadline: rinlib::time::Deadline) -> Self {
        Self {
            client: Client::new(),
            deadline,
        }
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
