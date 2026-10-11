//! 原请求持有的唯一异步派生；运输完成后才释放业务 pin 与临时能力。

use super::*;
use crate::backend::Binding;
use erhino_shared::object::ObjectKind;
use libexecution::runtime::Step;
use libkernel::ipc::message::MailboxSender;
use librpc::dispatcher::Completion;

pub(super) enum Purpose {
    Bind {
        binding: Binding,
        prepared: Option<PreparedMutation>,
    },
    Boundary {
        boundary: Resolved,
        used: usize,
    },
}
pub(super) struct PendingOutbound {
    purpose: Purpose,
    policy: Issuance,
    txid: Option<u64>,
    completion: Option<Completion>,
    status: Option<Status>,
    export: Option<Capability>,
}
impl PendingOutbound {
    pub(super) fn txid(&self) -> Option<u64> {
        self.txid
    }
    fn new(purpose: Purpose, policy: Issuance) -> Self {
        Self {
            purpose,
            policy,
            txid: None,
            completion: None,
            status: None,
            export: None,
        }
    }
    fn start<F>(
        &mut self,
        service: Capability,
        deadline: Deadline,
        request: librpc::Request,
        transport: &mut Transport<'_>,
        funding: &AccountView<libexecution::ExecutionResource>,
        requests: &mut Requests<'_, F>,
    ) {
        let service = match MailboxSender::from_capability(service) {
            Ok((sender, _)) => librpc::dispatcher::ServiceOwner::Owned(sender),
            Err(failed) => {
                self.status = Some(failure(failed.error));
                return;
            }
        };
        match transport.dispatcher.begin_for(
            service,
            deadline,
            request,
            Some(transport.waiter),
            funding,
        ) {
            Ok(txid) => {
                self.txid = Some(txid);
            }
            Err(failed) => {
                self.completion = Some(Completion {
                    service: failed.service,
                    result: Err(failed.error),
                });
            }
        }
        // 命令缓冲满时下一推进重申 wake；已经提交的调用不重放。
        let _ = requests.wake(transport.driver);
    }
    fn inspect(&mut self, backend: &mut MemoryBackend, deadline: Deadline) -> Result<(), Status> {
        let completion = self.completion.as_mut().ok_or(Status::Internal)?;
        let reply = completion
            .result
            .as_mut()
            .map_err(|error| call_status(error.cause))?;
        let request = derive(self.policy);
        let slots = if reply.handles.len() >= 64 {
            return Err(Status::Invalid);
        } else {
            (1u64 << reply.handles.len()) - 1
        };
        match protocol::decode_response(&reply.payload, &request, deadline, slots)? {
            FsResponse::Empty => {}
            FsResponse::Error(status) => return Err(status),
            _ => return Err(Status::Invalid),
        }
        let description = reply
            .handles
            .get(0)
            .map_err(failure)?
            .description()
            .map_err(failure)?;
        if description.kind != ObjectKind::MailboxSender as u32
            || description.rights != self.policy.sender_transport
        {
            return Err(Status::Invalid);
        }
        let capability = reply.handles.take(0).map_err(failure)?;
        match &self.purpose {
            Purpose::Bind { binding, .. } => {
                let sender = match MailboxSender::from_capability(capability) {
                    Ok((sender, _)) => sender,
                    Err(failed) => {
                        return Err(failure(failed.error));
                    }
                };
                if let Err((sender, status)) = backend.adopt_binding_sender(binding, sender, true) {
                    drop(sender);
                    return Err(status);
                }
            }
            Purpose::Boundary { .. } => self.export = Some(capability),
        }
        Ok(())
    }
}
fn derive(policy: Issuance) -> FsRequest<'static> {
    FsRequest::DeriveDirectory {
        path: "",
        rights: policy.rights,
        sender_transport: policy.sender_transport,
        output_transport: policy.output_transport,
    }
}
fn duplicate_sender(sender: &MailboxSender, rights: Rights) -> Result<Capability, Status> {
    let handle = libkernel::ipc::object::duplicate(sender.as_handle(), rights).map_err(failure)?;
    // SAFETY: duplicate 交付本函数独占的新、未映射 handle；立即移交 Capability 的关闭责任。
    Ok(unsafe { Capability::from_raw(handle) })
}
fn derive_request(policy: Issuance, deadline: Deadline) -> Result<librpc::Request, Status> {
    let mut bytes = [0; protocol::HEADER_LEN + 24];
    let used = protocol::encode_request(&derive(policy), deadline, &mut bytes)?;
    librpc::Request::new(protocol::ID, &bytes[..used]).map_err(failure)
}
fn call_status(cause: librpc::exchange::CallCause) -> Status {
    use librpc::exchange::CallCause;
    match cause {
        CallCause::Timeout => Status::Expired,
        CallCause::Cancelled | CallCause::Shutdown => Status::Cancelled,
        CallCause::ServiceClosed => Status::Unavailable,
        CallCause::System(error) => failure(error),
        CallCause::Frame(_) => Status::Invalid,
    }
}
impl Request {
    pub(super) fn prepare_bind<F>(
        &mut self,
        backend: &mut MemoryBackend,
        transport: Option<&mut Transport<'_>>,
        requests: &mut Requests<'_, F>,
    ) -> Result<(), Status> {
        if self.domain != Domain::Management || self.header.op != 1 {
            return Ok(());
        }
        let access = self
            .management
            .as_ref()
            .ok_or(self.denied.unwrap_or(Status::Permission))?;
        let response = self
            .outbox
            .as_mut()
            .ok_or(Status::Internal)?
            .response_mut()
            .map_err(failure)?;
        let (context, _) = response.parts().map_err(failure)?;
        let (_, request) = crate::route::decode_request(&context.payload, slots(context))?;
        let crate::route::Request::Bind {
            path,
            max_rights,
            output_transport,
            replace,
        } = request
        else {
            return Err(Status::Invalid);
        };
        let (parent, name) = backend.management_parent(access, path)?;
        let transport = transport.ok_or(Status::Unavailable)?;
        let issuer = context.handles.get(1).map_err(failure)?;
        let description = issuer.description().map_err(failure)?;
        let required = Rights::WRITE | Rights::WAIT | Rights::TRANSIT | Rights::DUPLICATE;
        if description.kind != ObjectKind::MailboxSender as u32
            || !description.rights.contains(required)
        {
            return Err(Status::Permission);
        }
        let policy = Issuance {
            rights: max_rights,
            sender_transport: required,
            output_transport,
        };
        let rpc = derive_request(policy, self.header.deadline)?;
        let binding =
            backend.reserve_binding(max_rights, output_transport, self.funding.domain())?;
        // 先固定原版本、名字和成员费用，再等待目标派生；准备不暴露名字。
        let prepared = backend.prepare_binding(access, parent, name, binding.clone(), replace)?;
        let mut pending = PendingOutbound::new(
            Purpose::Bind {
                binding: binding.clone(),
                prepared: Some(prepared),
            },
            policy,
        );
        let issuer = context.handles.take(1).map_err(failure)?;
        let sender = match MailboxSender::from_capability(issuer) {
            Ok((sender, _)) => sender,
            Err(failed) => {
                pending.status = Some(failure(failed.error));
                self.outbound = Some(pending);
                return Ok(());
            }
        };
        if let Err((sender, status)) = backend.adopt_binding_sender(&binding, sender, false) {
            drop(sender);
            pending.status = Some(status);
            self.outbound = Some(pending);
            return Ok(());
        }
        let service = duplicate_sender(backend.binding_sender(&binding, false)?, required)?;
        pending.start(
            service,
            self.header.deadline,
            rpc,
            transport,
            self.funding.execution(),
            requests,
        );
        self.outbound = Some(pending);
        Ok(())
    }
    pub(super) fn prepare_boundary<F>(
        &mut self,
        backend: &MemoryBackend,
        boundary: Resolved,
        used: usize,
        transport: Option<&mut Transport<'_>>,
        requests: &mut Requests<'_, F>,
    ) -> Result<(), Status> {
        let transport = transport.ok_or(Status::Unavailable)?;
        let access = self.access.as_ref().ok_or(Status::Permission)?;
        let Resolved::Provider {
            binding, parent, ..
        } = &boundary
        else {
            return Err(Status::Internal);
        };
        backend.check_binding_boundary(access, binding, parent)?;
        let output = access.output_transport() & binding.output_transport();
        let policy = Issuance {
            rights: access.rights() & binding.max_rights(),
            sender_transport: Rights::WRITE
                | Rights::WAIT
                | Rights::TRANSIT
                | (output & Rights::DUPLICATE),
            output_transport: output,
        };
        policy.validate_policy()?;
        let rpc = derive_request(policy, self.header.deadline)?;
        let service = duplicate_sender(
            backend.binding_sender(binding, false)?,
            Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
        )?;
        let mut pending = PendingOutbound::new(Purpose::Boundary { boundary, used }, policy);
        pending.start(
            service,
            self.header.deadline,
            rpc,
            transport,
            self.funding.execution(),
            requests,
        );
        self.outbound = Some(pending);
        Ok(())
    }
    pub(super) fn advance_outbound<F>(
        &mut self,
        backend: &mut MemoryBackend,
        transport: Option<&mut Transport<'_>>,
        requests: &mut Requests<'_, F>,
    ) -> Result<Advance, SystemCallError> {
        let pending = self
            .outbound
            .as_mut()
            .expect("outbound file responsibility missing");
        if let Some(txid) = pending.txid {
            let transport = transport.ok_or(SystemCallError::ObjectClosed)?;
            if self.abort.is_some() {
                transport.dispatcher.cancel(txid);
            }
            let wake_pending = requests.wake(transport.driver).is_err();
            if let Some(completion) = transport.dispatcher.take(txid) {
                pending.txid = None;
                pending.completion = Some(completion);
            } else {
                return Ok(file_step(if wake_pending {
                    Step::Runnable
                } else {
                    Step::Parked
                }));
            }
        }
        if pending.completion.is_some() {
            pending.status = pending.inspect(backend, self.header.deadline).err();
            // Dispatcher 已收束真实来源；一个回复至多 MESSAGE_HANDLE_MAX 个
            // 非映射能力，剩余输出、服务 sender 及未发送 Packet 均必成关闭。
            drop(pending.completion.take());
        }
        if self.abort.is_none() {
            self.abort = pending.status;
        }
        if self.abort.is_none() && libkernel::time::expired(self.header.deadline)? {
            self.abort = Some(Status::Expired);
        }
        if self.abort.is_none() {
            let response = self
                .outbox
                .as_mut()
                .ok_or(SystemCallError::ObjectClosed)?
                .response_mut()?;
            match &mut pending.purpose {
                Purpose::Bind { prepared, .. } => {
                    let used = crate::route::encode_response(
                        self.header.op,
                        Status::Ok,
                        self.header.deadline,
                        response.body_mut()?,
                    )
                    .map_err(|_| SystemCallError::IllegalArgument)?;
                    let prepared = prepared
                        .take()
                        .expect("uncommitted binding preparation missing");
                    match backend.commit(prepared) {
                        Ok(_) => {
                            response.finish_body(used)?;
                            self.encoded = true;
                        }
                        Err(failed) => {
                            self.abort = Some(failed.status);
                        }
                    }
                }
                Purpose::Boundary { boundary, used } => {
                    let access = self.access.as_ref().ok_or(SystemCallError::ObjectClosed)?;
                    let Resolved::Provider {
                        binding, parent, ..
                    } = boundary
                    else {
                        unreachable!("outbound boundary lost provider");
                    };
                    let status = backend
                        .check_binding_boundary(access, binding, parent)
                        .err();
                    if let Some(status) = status {
                        self.abort = Some(status);
                    } else {
                        let capability = pending
                            .export
                            .take()
                            .expect("derived boundary lost output owner");
                        match response.push(capability, pending.policy.sender_transport) {
                            Ok(()) => {
                                response.finish_body(*used)?;
                                self.encoded = true;
                            }
                            Err(failed) => {
                                pending.export = Some(failed.capability);
                                self.abort = Some(failure(failed.error));
                            }
                        }
                    }
                }
            }
        }
        // 未提交/被取消的出口仍由 pending 独占，随原请求本地收束。
        self.outbound = None;
        if let Some(status) = self.abort
            && self
                .outbox
                .as_ref()
                .is_some_and(|outbox| outbox.result().is_none())
        {
            self.encode_error(status)?;
        }
        Ok(file_step(Step::Runnable))
    }
}
