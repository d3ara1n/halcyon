//! 服务发现、发布和控制；控制对象通过真实 sender 使用，不按名字猜测授权。
use crate::protocol::{
    self, Descriptors, ProtocolDescriptor, PublishRights, RegisterMode, RegistrationId,
    RegistrationInfo, Request, Response, Status, WatchInfo,
};
use alloc::{sync::Arc, vec::Vec};
use erhino_shared::{call::SystemCallError, object::Rights, time::Deadline};
use libkernel::ipc::{
    capability::{Capability, HandleSet},
    message::{MailboxSender, SenderAdoptFailure},
};
use liboal::{FollowLimits, LocalError, Namespace, walk::Walk};
use librpc::{CallError, Exchange, Request as RpcRequest};
mod watch;
#[derive(Debug)]
pub enum ClientError {
    Local(LocalError),
    Transport(CallError),
    System(SystemCallError),
    Protocol,
    Status(Status),
    Adopt(SenderAdoptFailure),
}
impl From<SystemCallError> for ClientError {
    fn from(e: SystemCallError) -> Self {
        Self::System(e)
    }
}
impl From<LocalError> for ClientError {
    fn from(e: LocalError) -> Self {
        Self::Local(e)
    }
}
pub struct Entry {
    pub id: RegistrationId,
    pub descriptors: Vec<ProtocolDescriptor>,
    pub endpoint: MailboxSender,
}
pub struct RegistrationControl {
    pub sender: MailboxSender,
}
pub struct WatchControl {
    pub sender: MailboxSender,
    pub waiter: libkernel::ipc::notification::NotificationObserver,
}
pub struct Watch {
    pub info: WatchInfo,
    pub control: WatchControl,
}
pub struct Publisher<'a> {
    pub sender: &'a MailboxSender,
    pub caller: &'a mut dyn Exchange,
}
impl Publisher<'_> {
    pub fn delegate_name(
        &mut self,
        name: &str,
        publish_rights: PublishRights,
        deadline: Deadline,
    ) -> Result<MailboxSender, ClientError> {
        let request = Request::DelegateName {
            name,
            publish_rights,
        };
        let mut reply = call(self.caller, self.sender, &request, deadline, None)?;
        let response = decode(&reply.payload, slots(&reply.handles), &request, deadline)?;
        match response {
            Response::Empty => sender(&mut reply.handles, 0),
            Response::Error(e) => Err(ClientError::Status(e)),
            _ => Err(ClientError::Protocol),
        }
    }
    pub fn register(
        &mut self,
        name: &str,
        mode: RegisterMode,
        descriptors: &[ProtocolDescriptor],
        endpoint: &MailboxSender,
        export_transport: Rights,
        deadline: Deadline,
    ) -> Result<(RegistrationInfo, RegistrationControl), ClientError> {
        let mut bytes = [0; protocol::BODY_MAX];
        let used = Descriptors::encode(descriptors, &mut bytes).map_err(ClientError::Status)?;
        let request = Request::Register {
            name,
            mode,
            descriptors: Descriptors::decode(&bytes[..used]).map_err(ClientError::Status)?,
            export_transport,
        };
        let mut reply = call(
            self.caller,
            self.sender,
            &request,
            deadline,
            Some((endpoint, export_transport | Rights::DUPLICATE)),
        )?;
        match decode(&reply.payload, slots(&reply.handles), &request, deadline)? {
            Response::Registration(info) => Ok((
                info,
                RegistrationControl {
                    sender: sender(&mut reply.handles, 0)?,
                },
            )),
            Response::Error(e) => Err(ClientError::Status(e)),
            _ => Err(ClientError::Protocol),
        }
    }
}
impl RegistrationControl {
    pub fn query(
        &self,
        caller: &mut dyn Exchange,
        deadline: Deadline,
    ) -> Result<RegistrationInfo, ClientError> {
        self.request(caller, Request::QueryRegistration, deadline)
    }
    pub fn withdraw(
        &self,
        caller: &mut dyn Exchange,
        deadline: Deadline,
    ) -> Result<RegistrationInfo, ClientError> {
        self.request(caller, Request::Withdraw, deadline)
    }
    fn request(
        &self,
        caller: &mut dyn Exchange,
        request: Request<'_>,
        deadline: Deadline,
    ) -> Result<RegistrationInfo, ClientError> {
        let reply = call(caller, &self.sender, &request, deadline, None)?;
        match decode(&reply.payload, slots(&reply.handles), &request, deadline)? {
            Response::Registration(info) => Ok(info),
            Response::Error(e) => Err(ClientError::Status(e)),
            _ => Err(ClientError::Protocol),
        }
    }
}
impl WatchControl {
    pub fn query(
        &self,
        caller: &mut dyn Exchange,
        deadline: Deadline,
    ) -> Result<WatchInfo, ClientError> {
        self.request(caller, Request::QueryWatch, deadline)
    }
    pub fn ack(
        &self,
        caller: &mut dyn Exchange,
        sequence: u64,
        deadline: Deadline,
    ) -> Result<WatchInfo, ClientError> {
        self.request(caller, Request::AckWatch { sequence }, deadline)
    }
    pub fn cancel(
        &self,
        caller: &mut dyn Exchange,
        deadline: Deadline,
    ) -> Result<WatchInfo, ClientError> {
        self.request(caller, Request::CancelWatch, deadline)
    }
    fn request(
        &self,
        caller: &mut dyn Exchange,
        request: Request<'_>,
        deadline: Deadline,
    ) -> Result<WatchInfo, ClientError> {
        let reply = call(caller, &self.sender, &request, deadline, None)?;
        match decode(&reply.payload, slots(&reply.handles), &request, deadline)? {
            Response::Watch(info) => Ok(info),
            Response::Error(e) => Err(ClientError::Status(e)),
            _ => Err(ClientError::Protocol),
        }
    }
}
pub struct Client<'a> {
    namespace: &'a Namespace,
    caller: &'a mut dyn Exchange,
    limits: FollowLimits,
}
impl<'a> Client<'a> {
    pub fn new(
        namespace: &'a Namespace,
        caller: &'a mut dyn Exchange,
        limits: FollowLimits,
    ) -> Self {
        Self {
            namespace,
            caller,
            limits,
        }
    }
    fn path<R>(
        &mut self,
        path: &str,
        watch: Option<Option<&str>>,
        deadline: Deadline,
        done: impl for<'r> FnOnce(Response<'r>, &mut HandleSet) -> Result<R, ClientError>,
    ) -> Result<R, ClientError> {
        let budget = self.limits.start(libkernel::time::snapshot()?, deadline)?;
        let mut walk = Walk::new(self.namespace, path, "/", budget)?;
        loop {
            walk.before_call(libkernel::time::snapshot()?.now_ns)?;
            let deadline = walk.budget().deadline();
            let (endpoint, path) = walk.target()?;
            let request = match watch {
                None => Request::GetEntry { path },
                Some(None) => Request::WatchDirectory {
                    directory_path: path,
                },
                Some(Some(name)) => Request::WatchName {
                    directory_path: path,
                    name,
                },
            };
            let mut reply = call(self.caller, endpoint, &request, deadline, None)?;
            match decode(&reply.payload, slots(&reply.handles), &request, deadline)? {
                Response::Boundary(boundary) => {
                    let next = if boundary.capability_count() == 1 {
                        Some(
                            Arc::try_new(sender(&mut reply.handles, 0)?)
                                .map_err(|_| SystemCallError::OutOfMemory)?,
                        )
                    } else {
                        None
                    };
                    walk.follow(boundary, next, libkernel::time::snapshot()?.now_ns)?;
                }
                Response::Error(e) => return Err(ClientError::Status(e)),
                response => return done(response, &mut reply.handles),
            }
        }
    }
    pub fn get_entry(&mut self, path: &str, deadline: Deadline) -> Result<Entry, ClientError> {
        self.path(path, None, deadline, |response, handles| {
            let Response::Entry { id, descriptors } = response else {
                return Err(ClientError::Protocol);
            };
            let mut items = Vec::new();
            items
                .try_reserve_exact(descriptors.count() as usize)
                .map_err(|_| SystemCallError::OutOfMemory)?;
            items.extend(descriptors.iter());
            Ok(Entry {
                id,
                descriptors: items,
                endpoint: sender(handles, 0)?,
            })
        })
    }
    pub fn watch_directory(
        &mut self,
        path: &str,
        deadline: Deadline,
    ) -> Result<Watch, ClientError> {
        self.path(path, Some(None), deadline, watch)
    }
    pub fn watch_name(
        &mut self,
        directory: &str,
        name: &str,
        deadline: Deadline,
    ) -> Result<Watch, ClientError> {
        self.path(directory, Some(Some(name)), deadline, watch)
    }
}
fn watch(response: Response<'_>, handles: &mut HandleSet) -> Result<Watch, ClientError> {
    let Response::Watch(info) = response else {
        return Err(ClientError::Protocol);
    };
    let sender = sender(handles, 0)?;
    let (waiter, description) =
        libkernel::ipc::notification::NotificationObserver::from_capability(handles.take(1)?)
            .map_err(|f| ClientError::System(f.error))?;
    if !description.rights.contains(Rights::WAIT | Rights::TRANSIT) {
        return Err(ClientError::Protocol);
    }
    Ok(Watch {
        info,
        control: WatchControl { sender, waiter },
    })
}
fn sender(handles: &mut HandleSet, index: usize) -> Result<MailboxSender, ClientError> {
    let cap = handles.take(index)?;
    let (sender, description) = MailboxSender::from_capability(cap).map_err(ClientError::Adopt)?;
    if !description
        .rights
        .contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
    {
        return Err(ClientError::Protocol);
    }
    Ok(sender)
}
fn slots(handles: &HandleSet) -> u64 {
    let mut value = 0;
    for i in 0..handles.len() {
        if handles.get(i).is_ok() {
            value |= 1 << i;
        }
    }
    value
}
fn decode<'a>(
    bytes: &'a [u8],
    handles: u64,
    request: &Request<'_>,
    deadline: Deadline,
) -> Result<Response<'a>, ClientError> {
    protocol::decode_response(bytes, request, deadline, handles).map_err(|_| ClientError::Protocol)
}
fn call(
    caller: &mut dyn Exchange,
    sender: &MailboxSender,
    request: &Request<'_>,
    deadline: Deadline,
    input: Option<(&MailboxSender, Rights)>,
) -> Result<librpc::Reply, ClientError> {
    let rpc = rpc_request(request, deadline, input)?;
    caller
        .call(sender, deadline, rpc)
        .map_err(ClientError::Transport)
}
fn rpc_request(
    request: &Request<'_>,
    deadline: Deadline,
    input: Option<(&MailboxSender, Rights)>,
) -> Result<RpcRequest, ClientError> {
    let size = protocol::HEADER_LEN + request.body_len().map_err(ClientError::Status)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(size)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    bytes.resize(size, 0);
    let used =
        protocol::encode_request(request, deadline, &mut bytes).map_err(ClientError::Status)?;
    let mut rpc = RpcRequest::new(protocol::ID, &bytes[..used])?;
    if let Some((endpoint, rights)) = input {
        let handle = libkernel::ipc::object::duplicate(endpoint.as_handle(), rights)?; // SAFETY: duplicate 交付唯一新、未映射 handle，失败 Packet owner 仍持其关闭责任。
        let cap = unsafe { Capability::from_raw(handle) };
        rpc.push(cap, rights)
            .map_err(|e| ClientError::System(e.error))?;
    }
    Ok(rpc)
}
