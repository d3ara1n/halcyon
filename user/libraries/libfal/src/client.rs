//! FAL2 同步客户端：统一 wire、状态与 capability 槽契约。

use alloc::vec;
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    time::Deadline,
    wait::{WaitItem, WaitReason},
};
use librpc::{CallError, Caller, Reply, Request as RpcRequest};
use rinlib::ipc::{
    capability::Capability,
    message::{MailboxSender, SenderAdoptFailure},
    notification,
    object::duplicate,
    wait::wait_until,
};

use crate::protocol::{self, Request, Response, Status};

#[derive(Debug)]
pub enum ClientError {
    Transport(CallError),
    System(SystemCallError),
    Status(Status),
    Protocol,
}

pub struct Client {
    caller: Caller,
}

#[derive(Debug, Clone, Copy)]
pub struct MoveEntry<'a> {
    pub source_parent: &'a str,
    pub source_name: &'a str,
    pub destination_name: &'a str,
    pub expected: protocol::Expected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionEvent {
    Events(protocol::WatchMask),
    ProviderClosed,
}

pub struct Subscription {
    grant: MailboxSender,
    owner: Capability,
    info: protocol::SubscriptionInfo,
}

impl Subscription {
    pub const fn info(&self) -> protocol::SubscriptionInfo {
        self.info
    }

    pub fn owner_handle(&self) -> Handle {
        self.owner.as_handle()
    }

    pub fn take(&self) -> Result<protocol::WatchMask, SystemCallError> {
        let bits = notification::take(self.owner.as_handle(), protocol::WatchMask::ALL.raw())?;
        protocol::WatchMask::from_raw(bits).ok_or(SystemCallError::InternalError)
    }

    pub fn wait(&self, deadline: Deadline) -> Result<SubscriptionEvent, SystemCallError> {
        let items = [
            WaitItem::new(
                self.owner.as_handle(),
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                0,
            ),
            WaitItem::new(self.grant.as_handle(), ObjectSignals::CLOSED, 1),
        ];
        let result = wait_until(&items, deadline)?;
        if WaitReason::from_u32(result.reason) == Some(WaitReason::Timeout) {
            return Err(SystemCallError::DeadlineExpired);
        }
        if result.cookie == 1 || result.observed.intersects(ObjectSignals::CLOSED) {
            return Ok(SubscriptionEvent::ProviderClosed);
        }
        self.take().map(SubscriptionEvent::Events)
    }
}

impl Client {
    pub const fn new() -> Self {
        Self {
            caller: Caller::new(),
        }
    }

    pub fn call(
        &mut self,
        grant: &MailboxSender,
        request: &Request<'_>,
        deadline: Deadline,
    ) -> Result<Reply, ClientError> {
        let op = request.op();
        let rpc = Self::encode(request, deadline)?;
        self.finish_call(grant, op, deadline, rpc)
    }

    pub fn call_with_target(
        &mut self,
        grant: &MailboxSender,
        request: &Request<'_>,
        target: &MailboxSender,
        deadline: Deadline,
    ) -> Result<Reply, ClientError> {
        self.call_with_target_rights(
            grant,
            request,
            target,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
            deadline,
        )
    }

    pub fn call_with_target_rights(
        &mut self,
        grant: &MailboxSender,
        request: &Request<'_>,
        target: &MailboxSender,
        rights: Rights,
        deadline: Deadline,
    ) -> Result<Reply, ClientError> {
        let op = request.op();
        let mut rpc = Self::encode(request, deadline)?;
        let handle = duplicate(target.as_handle(), rights).map_err(ClientError::System)?;
        // SAFETY: ObjectDuplicate returned a fresh capability owned by this request.
        let capability = unsafe { Capability::from_raw(handle) };
        rpc.push(capability, rights)
            .map_err(|failure| ClientError::System(failure.error))?;
        self.finish_call(grant, op, deadline, rpc)
    }

    fn encode(request: &Request<'_>, deadline: Deadline) -> Result<RpcRequest, ClientError> {
        let capacity = protocol::HEADER_LEN
            .checked_add(request.encoded_len().ok_or(ClientError::Protocol)?)
            .ok_or(ClientError::Protocol)?;
        let mut payload = vec![0; capacity];
        let used = protocol::encode_request(request, deadline, &mut payload)
            .ok_or(ClientError::Protocol)?;
        payload.truncate(used);
        RpcRequest::new(protocol::ID, &payload).map_err(|_| ClientError::Protocol)
    }

    fn finish_call(
        &mut self,
        grant: &MailboxSender,
        op: protocol::Op,
        deadline: Deadline,
        rpc: RpcRequest,
    ) -> Result<Reply, ClientError> {
        let reply = self
            .caller
            .call(grant, deadline, rpc)
            .map_err(ClientError::Transport)?;
        let (header, response) =
            protocol::decode_response(&reply.payload).map_err(|_| ClientError::Protocol)?;
        if header.op != op {
            return Err(ClientError::Protocol);
        }
        if header.status != Status::Ok {
            if !reply.handles.is_empty() {
                return Err(ClientError::Protocol);
            }
            return Err(ClientError::Status(header.status));
        }
        if matches!(op, protocol::Op::Read | protocol::Op::Take) {
            let Response::Value(value) = response else {
                return Err(ClientError::Protocol);
            };
            crate::value::validate(
                value,
                0,
                reply.handles.remaining(),
                erhino_shared::message::PAYLOAD_MAX,
            )
            .map_err(|_| ClientError::Protocol)?;
            return Ok(reply);
        }
        let expected = response
            .capability_count(header.op)
            .ok_or(ClientError::Protocol)?;
        if reply.handles.remaining() != expected {
            return Err(ClientError::Protocol);
        }
        Ok(reply)
    }

    pub fn move_entry(
        &mut self,
        source: &MailboxSender,
        destination: &MailboxSender,
        move_entry: MoveEntry<'_>,
        deadline: Deadline,
    ) -> Result<(), ClientError> {
        let request = Request::Move {
            source_parent: move_entry.source_parent,
            source_name: move_entry.source_name,
            destination_name: move_entry.destination_name,
            expected: move_entry.expected,
        };
        let _ = self.call_with_target(source, &request, destination, deadline)?;
        Ok(())
    }

    pub fn derive(
        &mut self,
        parent: &MailboxSender,
        path: &str,
        rights: crate::authority::FalRights,
        deadline: Deadline,
    ) -> Result<MailboxSender, ClientError> {
        let mut reply = self.call(parent, &Request::Derive { path, rights }, deadline)?;
        let (_, response) =
            protocol::decode_response(&reply.payload).map_err(|_| ClientError::Protocol)?;
        if !matches!(response, Response::Node(_)) {
            return Err(ClientError::Protocol);
        }
        let capability = reply.handles.take(0).map_err(|_| ClientError::Protocol)?;
        MailboxSender::from_capability(capability)
            .map(|(sender, _)| sender)
            .map_err(|SenderAdoptFailure { .. }| ClientError::Protocol)
    }

    pub fn take(
        &mut self,
        grant: &MailboxSender,
        path: &str,
        deadline: Deadline,
    ) -> Result<Reply, ClientError> {
        self.call(grant, &Request::Take { path }, deadline)
    }

    /// 复制不携带 capability 的属性值。复制由客户端编排为 Read → Create，
    /// 不覆盖已有目标，也不伪造跨 provider 的原子性。
    pub fn copy_property(
        &mut self,
        source: &MailboxSender,
        source_path: &str,
        destination: &MailboxSender,
        destination_name: &str,
        rights: crate::authority::FalRights,
        deadline: Deadline,
    ) -> Result<(), ClientError> {
        let read = self.call(source, &Request::Read { path: source_path }, deadline)?;
        if !read.handles.is_empty() {
            return Err(ClientError::Protocol);
        }
        let (_, response) =
            crate::protocol::decode_response(&read.payload).map_err(|_| ClientError::Protocol)?;
        let crate::protocol::Response::Value(value) = response else {
            return Err(ClientError::Protocol);
        };
        let value = value.to_vec();
        let request = Request::Create {
            name: destination_name,
            kind: crate::node::NodeKind::Property,
            rights,
            value: &value,
        };
        let _ = self.call(destination, &request, deadline)?;
        Ok(())
    }

    pub fn subscribe(
        &mut self,
        grant: &MailboxSender,
        path: &str,
        mask: protocol::WatchMask,
        deadline: Deadline,
    ) -> Result<Subscription, ClientError> {
        if mask.is_empty() || mask.intersects(protocol::WatchMask::TERMINATED) {
            return Err(ClientError::Protocol);
        }
        let grant_rights = Rights::WRITE | Rights::WAIT;
        let grant_copy = duplicate(grant.as_handle(), grant_rights).map_err(ClientError::System)?;
        // SAFETY: ObjectDuplicate returned a fresh capability owned by this subscription.
        let grant_copy = unsafe { Capability::from_raw(grant_copy) };
        let (grant_copy, _) = MailboxSender::from_capability(grant_copy)
            .map_err(|SenderAdoptFailure { .. }| ClientError::Protocol)?;
        let event = notification::create(
            Rights::READ | Rights::WAIT | Rights::MANAGE,
            Rights::SIGNAL | Rights::WAIT | Rights::TRANSIT,
        )
        .map_err(ClientError::System)?;
        // SAFETY: NotificationCreate returned two fresh affine entries owned by this call.
        let owner = unsafe { Capability::from_raw(event.owner) };
        // SAFETY: the peer is moved into the Subscribe RPC request below.
        let signaler = unsafe { Capability::from_raw(event.peer) };
        let request = Request::Subscribe { path, mask };
        let op = request.op();
        let mut rpc = Self::encode(&request, deadline)?;
        rpc.push(signaler, Rights::SIGNAL | Rights::WAIT | Rights::TRANSIT)
            .map_err(|failure| ClientError::System(failure.error))?;
        let reply = self.finish_call(grant, op, deadline, rpc)?;
        let (_, Response::Subscription(info)) =
            protocol::decode_response(&reply.payload).map_err(|_| ClientError::Protocol)?
        else {
            return Err(ClientError::Protocol);
        };
        if !info
            .effective_mask
            .contains(protocol::WatchMask::TERMINATED)
            || !mask.contains(info.effective_mask.intersect(protocol::WatchMask::EVENTS))
            || info.reason != protocol::WatchReason::Active
        {
            return Err(ClientError::Protocol);
        }
        Ok(Subscription {
            grant: grant_copy,
            owner,
            info,
        })
    }

    pub fn query_subscription(
        &mut self,
        subscription: &Subscription,
        deadline: Deadline,
    ) -> Result<protocol::SubscriptionInfo, ClientError> {
        let reply = self.call(
            &subscription.grant,
            &Request::QuerySubscription {
                id: subscription.info.id,
            },
            deadline,
        )?;
        let (_, Response::Subscription(info)) =
            protocol::decode_response(&reply.payload).map_err(|_| ClientError::Protocol)?
        else {
            return Err(ClientError::Protocol);
        };
        if info.id != subscription.info.id {
            return Err(ClientError::Protocol);
        }
        Ok(info)
    }

    pub fn unsubscribe(
        &mut self,
        subscription: &Subscription,
        deadline: Deadline,
    ) -> Result<(), ClientError> {
        let _ = self.call(
            &subscription.grant,
            &Request::Unsubscribe {
                id: subscription.info.id,
            },
            deadline,
        )?;
        Ok(())
    }
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}
