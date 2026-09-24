//! 同步调用使用私有 ReplyPort，运输、期限与错误阶段共用 exchange 核心。

use crate::exchange::{CallCause, CallError, Reply, Request, cause};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    time::Deadline,
    wait::{WaitItem, WaitReason},
};
use rinlib::ipc::{
    message::{Mailbox, MailboxSender},
    wait::wait_until,
};

struct ReplyPort {
    owner: Mailbox,
    sender: MailboxSender,
}

impl ReplyPort {
    fn new() -> Result<Self, SystemCallError> {
        let owner = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)?;
        let minted = owner.mint(
            0,
            Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT,
        )?;
        Ok(Self {
            owner,
            sender: minted.sender,
        })
    }
}

#[derive(Debug)]
pub struct ReplyCleanup {
    owner: Option<Mailbox>,
    sender: Option<MailboxSender>,
}

impl ReplyCleanup {
    fn new(port: ReplyPort) -> Self {
        Self {
            owner: Some(port.owner),
            sender: Some(port.sender),
        }
    }

    pub fn has_owners(&self) -> bool {
        self.sender.is_some() || self.owner.is_some()
    }

    pub fn retry_close(&mut self) -> Result<(), SystemCallError> {
        let mut first = None;
        if let Some(sender) = self.sender.take()
            && let Err((sender, error)) = sender.close()
        {
            self.sender = Some(sender);
            first = Some(error);
        }
        if let Some(owner) = self.owner.take()
            && let Err((owner, error)) = owner.close()
        {
            self.owner = Some(owner);
            first.get_or_insert(error);
        }
        first.map_or(Ok(()), Err)
    }
}

pub struct Caller {
    port: Option<ReplyPort>,
}

enum OperationState {
    Unsent(Request),
    Sent,
    Completed,
}

/// 可分步推进的 RPC 调用；发送成功后可在未知业务结果下显式 abort。
pub struct CallOperation<'a> {
    caller: &'a mut Caller,
    service: &'a MailboxSender,
    deadline: Deadline,
    cancel: Option<Handle>,
    state: OperationState,
    protocol: u64,
    txid: u64,
}

impl Caller {
    pub const fn new() -> Self {
        Self { port: None }
    }

    fn ensure_port(&mut self) -> Result<(), SystemCallError> {
        if self.port.is_none() {
            self.port = Some(ReplyPort::new()?)
        }
        Ok(())
    }

    pub fn call(
        &mut self,
        service: &MailboxSender,
        deadline: Deadline,
        request: Request,
    ) -> Result<Reply, CallError> {
        self.call_cancelable(service, deadline, request, None)
    }

    pub fn call_cancelable(
        &mut self,
        service: &MailboxSender,
        deadline: Deadline,
        request: Request,
        cancel: Option<Handle>,
    ) -> Result<Reply, CallError> {
        let mut operation = CallOperation::new(self, service, deadline, request, cancel)?;
        loop {
            match operation.advance() {
                Ok(Some(reply)) => return Ok(reply),
                Ok(None) => {}
                Err(cause) => {
                    // advance 只在返回回复时进入完成态，错误必然仍有待收束操作。
                    return Err(operation.abort(cause).expect("pending call cannot be completed"));
                }
            }
            if let Err(cause) = operation.wait_ready() {
                // wait_ready 只在未完成态调用。
                return Err(operation.abort(cause).expect("pending call cannot be completed"));
            }
        }
    }
}

impl<'a> CallOperation<'a> {
    pub fn new(
        caller: &'a mut Caller,
        service: &'a MailboxSender,
        deadline: Deadline,
        mut request: Request,
        cancel: Option<Handle>,
    ) -> Result<Self, CallError> {
        let setup = (|| {
            request.new_attempt()?;
            let description = service.description()?;
            if !description.rights.contains(Rights::WRITE | Rights::WAIT) {
                return Err(SystemCallError::RightsDenied);
            }
            caller.ensure_port()?;
            request.attach_reply(&caller.port.as_ref().expect("ReplyPort just created").sender)
        })();
        if let Err(error) = setup {
            return Err(CallError::unsent(cause(error), request));
        }
        let protocol = request.protocol();
        let txid = request.txid();
        Ok(Self {
            caller,
            service,
            deadline,
            cancel,
            state: OperationState::Unsent(request),
            protocol,
            txid,
        })
    }

    pub fn advance(&mut self) -> Result<Option<Reply>, CallCause> {
        if let OperationState::Unsent(request) = &mut self.state {
            match request.try_send(self.service, self.deadline) {
                Ok(()) => {
                    self.state = OperationState::Sent;
                    return Ok(None);
                }
                Err(SystemCallError::MailboxFull) => return Ok(None),
                Err(error) => return Err(cause(error)),
            }
        }
        if matches!(self.state, OperationState::Completed) {
            return Err(CallCause::System(SystemCallError::ObjectBusy));
        }
        let port = self
            .caller
            .port
            .as_ref()
            .ok_or(CallCause::System(SystemCallError::ObjectClosed))?;
        match port.owner.receive() {
            Ok(message) => {
                let reply = Reply::accept(message, self.protocol, self.txid, self.deadline)?;
                self.state = OperationState::Completed;
                Ok(Some(reply))
            }
            Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => Ok(None),
            Err(error) => Err(cause(error)),
        }
    }

    pub fn wait_ready(&self) -> Result<(), CallCause> {
        if matches!(self.state, OperationState::Completed) {
            return Err(CallCause::System(SystemCallError::ObjectBusy));
        }
        let result = if matches!(self.state, OperationState::Sent) {
            let items = [
                WaitItem::new(
                    self.caller
                        .port
                        .as_ref()
                        .ok_or(CallCause::System(SystemCallError::ObjectClosed))?
                        .owner
                        .as_handle(),
                    ObjectSignals::READABLE | ObjectSignals::CLOSED,
                    0,
                ),
                WaitItem::new(self.service.as_handle(), ObjectSignals::CLOSED, 1),
                WaitItem::new(
                    self.cancel.unwrap_or(self.service.as_handle()),
                    ObjectSignals::READABLE | ObjectSignals::CLOSED,
                    2,
                ),
            ];
            wait_until(&items[..if self.cancel.is_some() { 3 } else { 2 }], self.deadline)
        } else {
            let items = [
                WaitItem::new(
                    self.service.as_handle(),
                    ObjectSignals::WRITABLE | ObjectSignals::CLOSED,
                    0,
                ),
                WaitItem::new(
                    self.cancel.unwrap_or(self.service.as_handle()),
                    ObjectSignals::READABLE | ObjectSignals::CLOSED,
                    1,
                ),
            ];
            wait_until(&items[..if self.cancel.is_some() { 2 } else { 1 }], self.deadline)
        }
        .map_err(cause)?;
        let reason = WaitReason::from_u32(result.reason)
            .ok_or(CallCause::System(SystemCallError::InternalError))?;
        if reason == WaitReason::Timeout {
            return Err(CallCause::Timeout);
        }
        if reason == WaitReason::Cancelled {
            return Err(CallCause::Cancelled);
        }
        let cancel_index = if matches!(self.state, OperationState::Sent) { 2 } else { 1 };
        if self.cancel.is_some()
            && result.item_index == cancel_index
            && result.observed.intersects(ObjectSignals::READABLE | ObjectSignals::CLOSED)
        {
            return Err(CallCause::Cancelled);
        }
        if result.observed.intersects(ObjectSignals::CLOSED) {
            return Err(CallCause::ServiceClosed);
        }
        Ok(())
    }

    pub fn abort(&mut self, cause: CallCause) -> Option<CallError> {
        match core::mem::replace(&mut self.state, OperationState::Completed) {
            OperationState::Unsent(request) => Some(CallError::unsent(cause, request)),
            OperationState::Sent => {
                let cleanup = self.caller.port.take().map(ReplyCleanup::new);
                Some(CallError::sent_with_cleanup(cause, cleanup))
            }
            OperationState::Completed => None,
        }
    }

    pub fn phase(&self) -> Option<crate::exchange::CallPhase> {
        match self.state {
            OperationState::Unsent(_) => Some(crate::exchange::CallPhase::Unsent),
            OperationState::Sent => Some(crate::exchange::CallPhase::Sent),
            OperationState::Completed => None,
        }
    }
}

impl Drop for CallOperation<'_> {
    fn drop(&mut self) {
        if matches!(self.state, OperationState::Sent) {
            // 放弃后旧回复端口不得再被后续调用复用。
            drop(self.caller.port.take());
        }
    }
}

impl Default for Caller {
    fn default() -> Self {
        Self::new()
    }
}
