//! 文件客户端直接发领域路径请求，只有明确未提交 Boundary 才使用同一 walk 接续。
use crate::{
    FsRights,
    node::NodeKind,
    protocol::{
        self, IoEnd, IoResult, OpenOptions, Request, Response, Status, StreamInfo, StreamOffer,
        WatchInfo,
    },
    value::{FromProperty, Value},
};
use alloc::{string::String, sync::Arc, vec::Vec};
use erhino_shared::{
    call::SystemCallError,
    object::{ObjectKind, Rights},
    time::Deadline,
};
use libkernel::ipc::{
    capability::{Capability, HandleSet},
    message::{MailboxSender, SenderAdoptFailure},
};
use liboal::{FollowBudget, FollowLimits, LocalError, Namespace, walk::Walk};
use librpc::{CallError, Caller, Request as RpcRequest};
pub mod copy;
mod watch;
#[derive(Debug)]
pub enum ClientError {
    Local(LocalError),
    Transport(CallError),
    System(SystemCallError),
    Status(Status),
    Protocol,
    Adopt(SenderAdoptFailure),
}
impl From<LocalError> for ClientError {
    fn from(e: LocalError) -> Self {
        Self::Local(e)
    }
}
impl From<SystemCallError> for ClientError {
    fn from(e: SystemCallError) -> Self {
        Self::System(e)
    }
}
pub struct Io {
    pub count: u64,
    pub end: IoEnd,
    pub cause: Status,
    pub data: Vec<u8>,
}
pub struct Offer {
    pub info: StreamOffer,
    pub control: MailboxSender,
    pub invitation: Capability,
}
pub struct Watch {
    pub info: WatchInfo,
    pub control: WatchControl,
}
pub struct WatchControl {
    pub sender: MailboxSender,
    pub waiter: libkernel::ipc::notification::NotificationObserver,
}
impl WatchControl {
    pub fn query(&self, caller: &mut Caller, deadline: Deadline) -> Result<WatchInfo, ClientError> {
        self.request(caller, Request::QueryWatch, deadline)
    }
    pub fn ack(
        &self,
        caller: &mut Caller,
        sequence: u64,
        deadline: Deadline,
    ) -> Result<WatchInfo, ClientError> {
        self.request(caller, Request::AckWatch { sequence }, deadline)
    }
    pub fn cancel(
        &self,
        caller: &mut Caller,
        deadline: Deadline,
    ) -> Result<WatchInfo, ClientError> {
        self.request(caller, Request::CancelWatch, deadline)
    }
    fn request(
        &self,
        caller: &mut Caller,
        request: Request<'_>,
        deadline: Deadline,
    ) -> Result<WatchInfo, ClientError> {
        let reply = call(caller, &self.sender, &request, deadline, None)?;
        match protocol::decode_response(&reply.payload, &request, deadline, slots(&reply.handles))
            .map_err(|_| ClientError::Protocol)?
        {
            Response::Watch(info) => Ok(info),
            Response::Error(status) => Err(ClientError::Status(status)),
            _ => Err(ClientError::Protocol),
        }
    }
}
pub struct Client<'a> {
    namespace: &'a Namespace,
    caller: &'a mut Caller,
    limits: FollowLimits,
}
impl<'a> Client<'a> {
    pub fn new(namespace: &'a Namespace, caller: &'a mut Caller, limits: FollowLimits) -> Self {
        Self {
            namespace,
            caller,
            limits,
        }
    }
    fn path<R>(
        &mut self,
        path: &str,
        request: &Request<'_>,
        deadline: Deadline,
        input: Option<&MailboxSender>,
        decode: impl for<'r> FnOnce(Response<'r>, &mut HandleSet) -> Result<R, ClientError>,
    ) -> Result<R, ClientError> {
        let budget = self.limits.start(libkernel::time::snapshot()?, deadline)?;
        self.path_budget(path, request, budget, input, decode)
            .map(|(value, _)| value)
    }
    fn path_budget<R>(
        &mut self,
        path: &str,
        prototype: &Request<'_>,
        budget: FollowBudget,
        input: Option<&MailboxSender>,
        decode: impl for<'r> FnOnce(Response<'r>, &mut HandleSet) -> Result<R, ClientError>,
    ) -> Result<(R, FollowBudget), ClientError> {
        let mut walk = Walk::new(self.namespace, path, "/", budget)?;
        loop {
            walk.before_call(libkernel::time::snapshot()?.now_ns)?;
            let deadline = walk.budget().deadline();
            let (entry, path) = walk.target()?;
            let request = retarget(prototype, path)?;
            let mut reply = call(self.caller, entry, &request, deadline, input)?;
            let response = protocol::decode_response(
                &reply.payload,
                &request,
                deadline,
                slots(&reply.handles),
            )
            .map_err(|_| ClientError::Protocol)?;
            match response {
                Response::Boundary(boundary) => {
                    let next = if boundary.capability_count() == 1 {
                        Some(
                            Arc::try_new(sender(&mut reply.handles, 0)?)
                                .map_err(|_| ClientError::System(SystemCallError::OutOfMemory))?,
                        )
                    } else {
                        None
                    };
                    walk.follow(boundary, next, libkernel::time::snapshot()?.now_ns)?;
                }
                Response::Error(status) => return Err(ClientError::Status(status)),
                response => {
                    let value = decode(response, &mut reply.handles)?;
                    return Ok((value, walk.into_budget()));
                }
            }
        }
    }
    pub fn get<T: FromProperty>(
        &mut self,
        path: &str,
        deadline: Deadline,
    ) -> Result<T, ClientError> {
        self.path(
            path,
            &Request::Get { path: "" },
            deadline,
            None,
            |r, _| match r {
                Response::Value(value) => value.get().map_err(ClientError::Status),
                _ => Err(ClientError::Protocol),
            },
        )
    }
    pub fn get_value(&mut self, path: &str, deadline: Deadline) -> Result<Vec<u8>, ClientError> {
        self.path(
            path,
            &Request::Get { path: "" },
            deadline,
            None,
            |r, _| match r {
                Response::Value(value) => copy(value.as_bytes()),
                _ => Err(ClientError::Protocol),
            },
        )
    }
    pub fn set(
        &mut self,
        path: &str,
        value: Value<'_>,
        deadline: Deadline,
    ) -> Result<(), ClientError> {
        let mut bytes = [0u8; protocol::BODY_MAX];
        let used = value.encode(&mut bytes).map_err(ClientError::Status)?;
        self.set_value(path, &bytes[..used], deadline)
    }
    pub fn set_value(
        &mut self,
        path: &str,
        value: &[u8],
        deadline: Deadline,
    ) -> Result<(), ClientError> {
        self.path(
            path,
            &Request::Set { path: "", value },
            deadline,
            None,
            empty,
        )
    }
    pub fn create(
        &mut self,
        path: &str,
        kind: NodeKind,
        rights: FsRights,
        initial: &[u8],
        deadline: Deadline,
    ) -> Result<(), ClientError> {
        self.path(
            path,
            &Request::Create {
                path: "",
                kind,
                rights,
                initial,
            },
            deadline,
            None,
            empty,
        )
    }
    pub fn remove(&mut self, path: &str, deadline: Deadline) -> Result<(), ClientError> {
        self.path(path, &Request::Remove { path: "" }, deadline, None, empty)
    }
    pub fn read_at(
        &mut self,
        path: &str,
        offset: u64,
        count: u32,
        deadline: Deadline,
    ) -> Result<Io, ClientError> {
        self.path(
            path,
            &Request::ReadAt {
                path: "",
                offset,
                count,
            },
            deadline,
            None,
            io,
        )
    }
    pub fn write_at(
        &mut self,
        path: &str,
        offset: u64,
        data: &[u8],
        deadline: Deadline,
    ) -> Result<Io, ClientError> {
        self.path(
            path,
            &Request::WriteAt {
                path: "",
                offset,
                data,
            },
            deadline,
            None,
            io,
        )
    }
    pub fn read_link(&mut self, path: &str, deadline: Deadline) -> Result<String, ClientError> {
        self.path(
            path,
            &Request::ReadLink { path: "" },
            deadline,
            None,
            |r, _| match r {
                Response::LinkTarget(s) => {
                    let mut text = String::new();
                    text.try_reserve_exact(s.len())
                        .map_err(|_| ClientError::System(SystemCallError::OutOfMemory))?;
                    text.push_str(s);
                    Ok(text)
                }
                _ => Err(ClientError::Protocol),
            },
        )
    }
    pub fn set_length(
        &mut self,
        path: &str,
        length: u64,
        deadline: Deadline,
    ) -> Result<(), ClientError> {
        self.path(
            path,
            &Request::SetLength { path: "", length },
            deadline,
            None,
            empty,
        )
    }
    pub fn sync(&mut self, path: &str, deadline: Deadline) -> Result<(), ClientError> {
        self.path(path, &Request::Sync { path: "" }, deadline, None, empty)
    }
    pub fn derive_directory(
        &mut self,
        path: &str,
        rights: FsRights,
        sender_transport: Rights,
        output_transport: Rights,
        deadline: Deadline,
    ) -> Result<MailboxSender, ClientError> {
        self.path(
            path,
            &Request::DeriveDirectory {
                path: "",
                rights,
                sender_transport,
                output_transport,
            },
            deadline,
            None,
            |r, handles| match r {
                Response::Empty => issued_sender(handles, sender_transport),
                _ => Err(ClientError::Protocol),
            },
        )
    }
    pub fn rename(
        &mut self,
        source: &str,
        destination: &str,
        replace: bool,
        deadline: Deadline,
    ) -> Result<(), ClientError> {
        let mut budget = self.limits.start(libkernel::time::snapshot()?, deadline)?;
        let destination = liboal::view::normalize(destination, "/", &mut budget)?;
        let (parent, name) = destination
            .rsplit_once('/')
            .ok_or(LocalError::InvalidPath)?;
        liboal::path::validate_name(name)
            .map_err(|_| ClientError::Local(LocalError::InvalidPath))?;
        let requested = Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE;
        let target = Request::DeriveDirectory {
            path: "",
            rights: FsRights::CREATE
                | if replace {
                    FsRights::REMOVE
                } else {
                    FsRights::NONE
                },
            sender_transport: requested,
            output_transport: Rights::NONE,
        };
        let (grant, budget) = self.path_budget(
            if parent.is_empty() { "/" } else { parent },
            &target,
            budget,
            None,
            |r, handles| match r {
                Response::Empty => issued_sender(handles, requested),
                _ => Err(ClientError::Protocol),
            },
        )?;
        self.path_budget(
            source,
            &Request::Rename {
                source: "",
                destination_name: name,
                replace,
            },
            budget,
            Some(&grant),
            empty,
        )
        .map(|(value, _)| value)
    }
    pub fn open(
        &mut self,
        path: &str,
        options: OpenOptions,
        deadline: Deadline,
    ) -> Result<Offer, ClientError> {
        self.path(
            path,
            &Request::Open { path: "", options },
            deadline,
            None,
            offer,
        )
    }
    pub fn create_open(
        &mut self,
        path: &str,
        options: OpenOptions,
        rights: FsRights,
        deadline: Deadline,
    ) -> Result<Offer, ClientError> {
        self.path(
            path,
            &Request::CreateOpen {
                path: "",
                options,
                rights,
            },
            deadline,
            None,
            offer,
        )
    }
    pub fn watch(
        &mut self,
        path: &str,
        mask: u64,
        deadline: Deadline,
    ) -> Result<Watch, ClientError> {
        self.path(
            path,
            &Request::Watch { path: "", mask },
            deadline,
            None,
            |r, handles| match r {
                Response::Watch(info) => {
                    let control_description = handles.get(0)?.description()?;
                    let observer_description = handles.get(1)?.description()?;
                    if control_description.kind != ObjectKind::MailboxSender as u32
                        || !control_description
                            .rights
                            .contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
                        || observer_description.kind != ObjectKind::NotificationObserver as u32
                        || !observer_description
                            .rights
                            .contains(Rights::WAIT | Rights::TRANSIT)
                    {
                        return Err(ClientError::Protocol);
                    }
                    let control = sender(handles, 0)?;
                    let (waiter, desc) =
                        libkernel::ipc::notification::NotificationObserver::from_capability(
                            handles.take(1)?,
                        )
                        .map_err(|f| ClientError::System(f.error))?;
                    if !desc.rights.contains(Rights::WAIT | Rights::TRANSIT) {
                        return Err(ClientError::Protocol);
                    }
                    Ok(Watch {
                        info,
                        control: WatchControl {
                            sender: control,
                            waiter,
                        },
                    })
                }
                _ => Err(ClientError::Protocol),
            },
        )
    }
}
fn copy(bytes: &[u8]) -> Result<Vec<u8>, ClientError> {
    let mut out = Vec::new();
    out.try_reserve_exact(bytes.len())
        .map_err(|_| SystemCallError::OutOfMemory)?;
    out.extend_from_slice(bytes);
    Ok(out)
}
fn slots(handles: &HandleSet) -> u64 {
    let mut value = 0;
    for i in 0..handles.len() {
        if handles.get(i).is_ok() {
            value |= 1u64 << i;
        }
    }
    value
}
fn sender(handles: &mut HandleSet, index: usize) -> Result<MailboxSender, ClientError> {
    let cap = handles.take(index)?;
    let (sender, desc) = MailboxSender::from_capability(cap).map_err(ClientError::Adopt)?;
    if !desc
        .rights
        .contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
    {
        return Err(ClientError::Protocol);
    }
    Ok(sender)
}
fn issued_sender(handles: &mut HandleSet, requested: Rights) -> Result<MailboxSender, ClientError> {
    // 槽数已由领域响应校验；先核实际角色和完整运输上限再转移 owner。
    let description = handles.get(0)?.description()?;
    if description.kind != ObjectKind::MailboxSender as u32 || description.rights != requested {
        return Err(ClientError::Protocol);
    }
    sender(handles, 0)
}
fn empty(r: Response<'_>, _: &mut HandleSet) -> Result<(), ClientError> {
    if matches!(r, Response::Empty) {
        Ok(())
    } else {
        Err(ClientError::Protocol)
    }
}
fn io(r: Response<'_>, _: &mut HandleSet) -> Result<Io, ClientError> {
    let Response::Io(IoResult {
        count,
        end,
        cause,
        data,
    }) = r
    else {
        return Err(ClientError::Protocol);
    };
    Ok(Io {
        count,
        end,
        cause,
        data: copy(data)?,
    })
}
fn offer(r: Response<'_>, handles: &mut HandleSet) -> Result<Offer, ClientError> {
    let Response::Offer(info) = r else {
        return Err(ClientError::Protocol);
    };
    let control = sender(handles, 0)?;
    let invitation = handles.take(1)?;
    let desc = invitation.description()?;
    if desc.kind != ObjectKind::TunnelInvitation as u32 {
        return Err(ClientError::Protocol);
    }
    Ok(Offer {
        info,
        control,
        invitation,
    })
}
fn call(
    caller: &mut Caller,
    entry: &MailboxSender,
    request: &Request<'_>,
    deadline: Deadline,
    input: Option<&MailboxSender>,
) -> Result<librpc::Reply, ClientError> {
    let rpc = rpc_request(request, deadline, input)?;
    caller
        .call(entry, deadline, rpc)
        .map_err(ClientError::Transport)
}
fn rpc_request(
    request: &Request<'_>,
    deadline: Deadline,
    input: Option<&MailboxSender>,
) -> Result<RpcRequest, ClientError> {
    let len = protocol::HEADER_LEN + request.body_len().map_err(ClientError::Status)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(len)
        .map_err(|_| ClientError::System(SystemCallError::OutOfMemory))?;
    bytes.resize(len, 0);
    let used =
        protocol::encode_request(request, deadline, &mut bytes).map_err(ClientError::Status)?;
    let mut rpc = RpcRequest::new(protocol::ID, &bytes[..used])?;
    if let Some(input) = input {
        let rights = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
        let handle = libkernel::ipc::object::duplicate(input.as_handle(), rights)?; // SAFETY: duplicate 为本函数交付唯一新handle，Packet失败仍返还cap owner。
        let cap = unsafe { Capability::from_raw(handle) };
        rpc.push(cap, rights)
            .map_err(|e| ClientError::System(e.error))?;
    }
    Ok(rpc)
}
fn retarget<'a>(r: &'a Request<'_>, path: &'a str) -> Result<Request<'a>, ClientError> {
    Ok(match r {
        Request::Get { .. } => Request::Get { path },
        Request::Set { value, .. } => Request::Set { path, value },
        Request::ReadAt { offset, count, .. } => Request::ReadAt {
            path,
            offset: *offset,
            count: *count,
        },
        Request::WriteAt { offset, data, .. } => Request::WriteAt {
            path,
            offset: *offset,
            data,
        },
        Request::Open { options, .. } => Request::Open {
            path,
            options: *options,
        },
        Request::CreateOpen {
            options, rights, ..
        } => Request::CreateOpen {
            path,
            options: *options,
            rights: *rights,
        },
        Request::Create {
            kind,
            rights,
            initial,
            ..
        } => Request::Create {
            path,
            kind: *kind,
            rights: *rights,
            initial,
        },
        Request::Remove { .. } => Request::Remove { path },
        Request::Rename {
            destination_name,
            replace,
            ..
        } => Request::Rename {
            source: path,
            destination_name,
            replace: *replace,
        },
        Request::ReadLink { .. } => Request::ReadLink { path },
        Request::SetLength { length, .. } => Request::SetLength {
            path,
            length: *length,
        },
        Request::Sync { .. } => Request::Sync { path },
        Request::DeriveDirectory {
            rights,
            sender_transport,
            output_transport,
            ..
        } => Request::DeriveDirectory {
            path,
            rights: *rights,
            sender_transport: *sender_transport,
            output_transport: *output_transport,
        },
        Request::Watch { mask, .. } => Request::Watch { path, mask: *mask },
        _ => return Err(ClientError::Protocol),
    })
}
pub fn stream_control(
    caller: &mut Caller,
    sender: &MailboxSender,
    request: &Request<'_>,
    deadline: Deadline,
) -> Result<StreamInfo, ClientError> {
    let reply = call(caller, sender, request, deadline, None)?;
    match protocol::decode_response(&reply.payload, request, deadline, slots(&reply.handles))
        .map_err(|_| ClientError::Protocol)?
    {
        Response::Stream(info) => Ok(info),
        Response::Error(e) => Err(ClientError::Status(e)),
        _ => Err(ClientError::Protocol),
    }
}
