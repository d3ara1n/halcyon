//! 直接借用既有 RPC Caller；只消费 OAL 响应，不建立另一套等待运行体。

use crate::{
    FollowBudget, FollowLimits, LocalError, Namespace, ObjectInfo,
    info::InfoLayout,
    protocol::{self, Page, PageLayout, Request, Response, Status},
    walk::Walk,
};
use alloc::{sync::Arc, vec::Vec};
use erhino_shared::{
    call::SystemCallError,
    message::MESSAGE_HANDLE_MAX,
    object::{ObjectKind, Rights},
    time::Deadline,
};
use libkernel::ipc::{
    capability::Capability,
    message::{MailboxSender, SenderAdoptFailure},
};
use librpc::{CallError, Caller, Reply, Request as RpcRequest};

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
    fn from(value: LocalError) -> Self {
        Self::Local(value)
    }
}
impl From<SystemCallError> for ClientError {
    fn from(value: SystemCallError) -> Self {
        Self::System(value)
    }
}

/// 每次调用的视图基点、调用者期限及可选取消 capability；方法返回后不留引用。
pub struct CallOptions<'a> {
    pub cwd: &'a str,
    pub deadline: Deadline,
    pub cancel: Option<&'a Capability>,
}
impl CallOptions<'_> {
    pub const fn new(deadline: Deadline) -> Self {
        Self {
            cwd: "/",
            deadline,
            cancel: None,
        }
    }
}

/// 只保存已经完整验证的原 payload 和布局，不再完整解码或重新分配摘要。
pub struct Lookup {
    payload: Vec<u8>,
    layout: InfoLayout,
}
impl Lookup {
    pub fn info(&self) -> ObjectInfo<'_> {
        ObjectInfo::from_layout(&self.payload[protocol::HEADER_LEN + 4..], &self.layout)
    }
}

pub struct EnumerationPage {
    payload: Vec<u8>,
    layout: PageLayout,
    cursor: Option<EnumerationCursor>,
}
impl EnumerationPage {
    pub fn page(&self) -> Page<'_> {
        Page::from_layout(&self.payload[protocol::HEADER_LEN + 4..], &self.layout)
    }
    pub fn into_cursor(self) -> Option<EnumerationCursor> {
        self.cursor
    }
}

/// 只持应答入口和 opaque cursor；没有 Namespace、路径、目录 grant 或 backing 身份。
pub struct EnumerationCursor {
    entry: Arc<MailboxSender>,
    opaque: Vec<u8>,
}
impl EnumerationCursor {
    pub fn next(
        self,
        client: &mut Client<'_>,
        max_entries: u32,
        deadline: Deadline,
    ) -> Result<EnumerationPage, ClientError> {
        self.next_cancelable(client, max_entries, deadline, None)
    }

    pub fn next_cancelable(
        self,
        client: &mut Client<'_>,
        max_entries: u32,
        deadline: Deadline,
        cancel: Option<&Capability>,
    ) -> Result<EnumerationPage, ClientError> {
        let mut budget = client
            .limits
            .start(libkernel::time::snapshot()?, deadline)?;
        budget.consume_bytes(self.opaque.len())?;
        budget.before_call(libkernel::time::snapshot()?.now_ns)?;
        let request = Request::Enumerate {
            path: "",
            cursor: &self.opaque,
            max_entries,
        };
        let reply = call(
            client.caller,
            &self.entry,
            &request,
            budget.deadline(),
            cancel,
        )?;
        let response = validate_reply(&reply.payload, &reply.handles, &request, budget.deadline())?;
        match response {
            Response::Page(page) => {
                let (layout, cursor) = prepare_page(page, &self.entry, &mut budget)?;
                Ok(EnumerationPage {
                    payload: reply.payload,
                    layout,
                    cursor,
                })
            }
            Response::Error(status) => Err(ClientError::Status(status)),
            // decode_response 已拒绝续页 Boundary；这里同样不会接续或重新选择绑定。
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

    pub fn lookup(&mut self, path: &str, deadline: Deadline) -> Result<Lookup, ClientError> {
        self.lookup_with(path, CallOptions::new(deadline))
    }

    pub fn lookup_with(
        &mut self,
        path: &str,
        options: CallOptions<'_>,
    ) -> Result<Lookup, ClientError> {
        let budget = self
            .limits
            .start(libkernel::time::snapshot()?, options.deadline)?;
        let mut walk = Walk::new(self.namespace, path, options.cwd, budget)?;
        loop {
            walk.before_call(libkernel::time::snapshot()?.now_ns)?;
            let deadline = walk.budget().deadline();
            let (entry, path) = walk.target()?;
            let request = Request::Lookup { path };
            let mut reply = call(self.caller, entry, &request, deadline, options.cancel)?;
            match validate_reply(&reply.payload, &reply.handles, &request, deadline)? {
                Response::Found(info) => {
                    let layout = info.layout();
                    return Ok(Lookup {
                        payload: reply.payload,
                        layout,
                    });
                }
                Response::Boundary(boundary) => {
                    let next = if boundary.capability_count() == 1 {
                        Some(adopt_entry(&mut reply.handles)?)
                    } else {
                        None
                    };
                    walk.follow(boundary, next, libkernel::time::snapshot()?.now_ns)?;
                }
                Response::Error(status) => return Err(ClientError::Status(status)),
                _ => return Err(ClientError::Protocol),
            }
        }
    }

    pub fn enumerate(
        &mut self,
        path: &str,
        max_entries: u32,
        deadline: Deadline,
    ) -> Result<EnumerationPage, ClientError> {
        self.enumerate_with(path, max_entries, CallOptions::new(deadline))
    }

    pub fn enumerate_with(
        &mut self,
        path: &str,
        max_entries: u32,
        options: CallOptions<'_>,
    ) -> Result<EnumerationPage, ClientError> {
        let budget = self
            .limits
            .start(libkernel::time::snapshot()?, options.deadline)?;
        let mut walk = Walk::new(self.namespace, path, options.cwd, budget)?;
        loop {
            walk.before_call(libkernel::time::snapshot()?.now_ns)?;
            let deadline = walk.budget().deadline();
            let (entry, path) = walk.target()?;
            let request = Request::Enumerate {
                path,
                cursor: &[],
                max_entries,
            };
            let mut reply = call(self.caller, entry, &request, deadline, options.cancel)?;
            match validate_reply(&reply.payload, &reply.handles, &request, deadline)? {
                Response::Page(page) => {
                    let entry = Arc::clone(entry);
                    let (layout, cursor) = prepare_page(page, &entry, walk.budget_mut())?;
                    return Ok(EnumerationPage {
                        payload: reply.payload,
                        layout,
                        cursor,
                    });
                }
                Response::Boundary(boundary) => {
                    let next = if boundary.capability_count() == 1 {
                        Some(adopt_entry(&mut reply.handles)?)
                    } else {
                        None
                    };
                    walk.follow(boundary, next, libkernel::time::snapshot()?.now_ns)?;
                }
                Response::Error(status) => return Err(ClientError::Status(status)),
                _ => return Err(ClientError::Protocol),
            }
        }
    }
}

fn call(
    caller: &mut Caller,
    entry: &MailboxSender,
    request: &Request<'_>,
    deadline: Deadline,
    cancel: Option<&Capability>,
) -> Result<Reply, ClientError> {
    let mut bytes = Vec::new();
    let len = request.encoded_len().map_err(|_| ClientError::Protocol)?;
    bytes
        .try_reserve_exact(len)
        .map_err(|_| LocalError::Resource)?;
    bytes.resize(len, 0);
    let used = protocol::encode_request(request, deadline, &mut bytes)
        .map_err(|_| ClientError::Protocol)?;
    let rpc = RpcRequest::new(protocol::ID, &bytes[..used])?;
    caller
        .call_cancelable(entry, deadline, rpc, cancel.map(Capability::as_handle))
        .map_err(ClientError::Transport)
}

fn validate_reply<'a>(
    payload: &'a [u8],
    handles: &libkernel::ipc::capability::HandleSet,
    request: &Request<'_>,
    deadline: Deadline,
) -> Result<Response<'a>, ClientError> {
    if handles.len() > MESSAGE_HANDLE_MAX {
        return Err(ClientError::Protocol);
    }
    let mut slots = 0u64;
    for index in 0..handles.len() {
        if handles.get(index).is_ok() {
            slots |= 1 << index;
        }
    }
    protocol::decode_response(payload, request, deadline, slots).map_err(|_| ClientError::Protocol)
}

fn adopt_entry(
    handles: &mut libkernel::ipc::capability::HandleSet,
) -> Result<Arc<MailboxSender>, ClientError> {
    // 所有槽已按变体核对；先验证真实 role/必需位，再移交该槽的关闭责任。
    let description = handles.get(0)?.description()?;
    let required = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
    if description.kind != ObjectKind::MailboxSender as u32
        || !description.rights.contains(required)
    {
        return Err(ClientError::Protocol);
    }
    let (entry, _) =
        MailboxSender::from_capability(handles.take(0)?).map_err(ClientError::Adopt)?;
    Arc::try_new(entry).map_err(|_| LocalError::Resource.into())
}

fn prepare_page(
    page: Page<'_>,
    entry: &Arc<MailboxSender>,
    budget: &mut FollowBudget,
) -> Result<(PageLayout, Option<EnumerationCursor>), ClientError> {
    let layout = page.layout();
    let cursor = if page.done() {
        None
    } else {
        budget.consume_bytes(
            page.next_cursor()
                .len()
                .checked_add(core::mem::size_of::<EnumerationCursor>())
                .ok_or(LocalError::BudgetExceeded)?,
        )?;
        let mut opaque = Vec::new();
        opaque
            .try_reserve_exact(page.next_cursor().len())
            .map_err(|_| LocalError::Resource)?;
        opaque.extend_from_slice(page.next_cursor());
        Some(EnumerationCursor {
            entry: Arc::clone(entry),
            opaque,
        })
    };
    Ok((layout, cursor))
}
