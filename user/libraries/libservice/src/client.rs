//! 注册权限的同步客户端入口；服务循环使用同一协议的异步 Dispatcher。

use alloc::vec;
use erhino_shared::{object::Rights, time::Deadline};
use librpc::{CallError, Caller, Request as RpcRequest};
use rinlib::ipc::message::MailboxSender;

use crate::protocol::{self, Request, Response, Scope, Status};

#[derive(Debug)]
pub enum ClientError {
    Transport(CallError),
    Protocol,
    Status(Status),
}

pub fn delegate_name(
    root: &MailboxSender,
    name: &str,
    deadline: Deadline,
) -> Result<MailboxSender, ClientError> {
    let request = Request::DelegateName { name };
    let capacity = protocol::HEADER_LEN
        .checked_add(request.encoded_len().ok_or(ClientError::Protocol)?)
        .ok_or(ClientError::Protocol)?;
    let mut bytes = vec![0; capacity];
    let used =
        protocol::encode_request(&request, deadline, &mut bytes).ok_or(ClientError::Protocol)?;
    let rpc = RpcRequest::new(protocol::ID, &bytes[..used]).map_err(|_| ClientError::Protocol)?;
    let mut reply = Caller::new()
        .call(root, deadline, rpc)
        .map_err(ClientError::Transport)?;
    let (header, response) =
        protocol::decode_response(&reply.payload).map_err(|_| ClientError::Protocol)?;
    if header.op != protocol::Op::DelegateName {
        return Err(ClientError::Protocol);
    }
    if header.status != Status::Ok {
        if !reply.handles.is_empty() {
            return Err(ClientError::Protocol);
        }
        return Err(ClientError::Status(header.status));
    }
    let Response::Authority(info) = response else {
        return Err(ClientError::Protocol);
    };
    if info.scope != Scope::ExactName || reply.handles.remaining() != 1 {
        return Err(ClientError::Protocol);
    }
    let capability = reply.handles.take(0).map_err(|_| ClientError::Protocol)?;
    let (sender, description) =
        MailboxSender::from_capability(capability).map_err(|_| ClientError::Protocol)?;
    let required =
        Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT | Rights::GRANT;
    if description.object_id != info.identity
        || description.related_object_id
            != root
                .description()
                .map_err(|_| ClientError::Protocol)?
                .related_object_id
        || !description.rights.contains(required)
    {
        return Err(ClientError::Protocol);
    }
    Ok(sender)
}
