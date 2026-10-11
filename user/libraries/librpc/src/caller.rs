//! 同步调用使用私有 ReplyPort，运输、期限与错误阶段共用 exchange 核心。

use crate::exchange::{CallCause, CallError, Reply, Request, cause};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectSignals, Rights},
    time::Deadline,
    wait::WaitReason,
    wait_set::{MemberRequest, ReadyRecord},
};
use libkernel::ipc::{
    message::{Mailbox, MailboxSender},
    wait_set::WaitSet,
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
    Sent(libkernel::ipc::message::ReplyObserver),
    Completed,
}

/// 可分步推进的 RPC 调用；发送成功后可在未知业务结果下显式 abort。
pub struct CallOperation<'a> {
    caller: &'a mut Caller,
    service: &'a MailboxSender,
    deadline: Deadline,
    wait: Option<CallWait>,
    state: OperationState,
    protocol: u64,
    txid: u64,
}

/// 每个调用在发送前出生一个真实扁平集合，跨分步推进持有接收权。
struct CallWait {
    set: WaitSet,
    reply: u64,
    service: Option<u64>,
    cancel: Option<(u64, Handle)>,
    observation: u64,
}

impl CallWait {
    fn new(
        reply: Handle,
        service: Handle,
        cancel: Option<Handle>,
        observation: Handle,
    ) -> Result<Self, SystemCallError> {
        // 发送后只替换一次 service 掩码，额外一槽覆盖正在摘除的旧成员。
        let set = WaitSet::create(5)?;
        let reply = set.register(MemberRequest::new(
            reply,
            ObjectSignals::READABLE | ObjectSignals::CLOSED,
            0,
        ))?;
        let service = set.register(MemberRequest::new(
            service,
            ObjectSignals::WRITABLE | ObjectSignals::CLOSED,
            1,
        ))?;
        let cancel = match cancel {
            Some(handle) => Some((
                set.register(MemberRequest::new(
                    handle,
                    ObjectSignals::READABLE | ObjectSignals::CLOSED,
                    2,
                ))?,
                handle,
            )),
            None => None,
        };
        let observation = set.register(MemberRequest::new(
            observation,
            ObjectSignals::PEER_CLOSED,
            3,
        ))?;
        Ok(Self {
            set,
            reply,
            service: Some(service),
            cancel,
            observation,
        })
    }

    fn sent(&mut self, service: Handle) -> Result<(), SystemCallError> {
        let old = self.service.expect("unsent call lost service member");
        self.set.remove(old)?;
        self.service = None;
        self.service = Some(self.set.register(MemberRequest::new(
            service,
            ObjectSignals::CLOSED,
            1,
        ))?);
        Ok(())
    }

    fn receive(&self, sent: bool) -> Result<Option<(u64, ObjectSignals)>, SystemCallError> {
        let mut records = [ReadyRecord {
            token: 0,
            cookie: 0,
            observed: ObjectSignals::NONE,
            reason: 0,
            reserved: 0,
        }; 4];
        let count = match self.set.receive_into(&mut records) {
            Ok(count) => count,
            Err(SystemCallError::ObjectNotAvailable) => 0,
            Err(error) => return Err(error),
        };
        let mut selected: Option<(u64, ObjectSignals)> = None;
        for record in &records[..count] {
            let current = match record.cookie {
                0 => sent && record.token == self.reply,
                1 => self.service == Some(record.token),
                2 => self.cancel.is_some_and(|(token, _)| token == record.token),
                3 => sent && record.token == self.observation,
                _ => false,
            };
            if !current || record.reserved != 0 {
                continue;
            }
            let signals = match WaitReason::from_u32(record.reason) {
                Some(WaitReason::Signaled) => record.observed,
                Some(WaitReason::Closed) => record.observed | ObjectSignals::CLOSED,
                _ => return Err(SystemCallError::InternalError),
            };
            let priority = |cookie| match (sent, cookie) {
                (true, 0) | (false, 2) => 0,
                (true, 2) | (false, 1) => 1,
                _ => 2,
            };
            if selected.is_none_or(|(cookie, _)| priority(record.cookie) < priority(cookie)) {
                selected = Some((record.cookie, signals));
            }
        }
        Ok(selected)
    }

    fn ready(&self, sent: bool) -> Result<bool, CallCause> {
        let Some((cookie, observed)) = self.receive(sent).map_err(cause)? else {
            return Ok(false);
        };
        if cookie == 2 {
            return Err(CallCause::Cancelled);
        }
        if cookie == 3 || observed.intersects(ObjectSignals::CLOSED) {
            return Err(CallCause::ServiceClosed);
        }
        Ok(true)
    }
    fn cancelled(&self) -> Result<bool, CallCause> {
        let Some((_, handle)) = self.cancel else {
            return Ok(false);
        };
        libkernel::ipc::wait::probe(handle, ObjectSignals::READABLE | ObjectSignals::CLOSED)
            .map(|signals| signals.intersects(ObjectSignals::READABLE | ObjectSignals::CLOSED))
            .map_err(cause)
    }
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
                    return Err(operation
                        .abort(cause)
                        .expect("pending call cannot be completed"));
                }
            }
            if let Err(cause) = operation.wait_ready() {
                // wait_ready 只在未完成态调用。
                return Err(operation
                    .abort(cause)
                    .expect("pending call cannot be completed"));
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
            request.attach_reply(&caller.port.as_ref().expect("ReplyPort just created").sender)?;
            CallWait::new(
                caller
                    .port
                    .as_ref()
                    .expect("ReplyPort just created")
                    .owner
                    .as_handle(),
                service.as_handle(),
                cancel,
                request
                    .observation()
                    .expect("reply preparation lost observer")
                    .as_handle(),
            )
        })();
        let wait = match setup {
            Ok(wait) => wait,
            Err(error) => return Err(CallError::unsent(cause(error), request)),
        };
        let protocol = request.protocol();
        let txid = request.txid();
        Ok(Self {
            caller,
            service,
            deadline,
            wait: Some(wait),
            state: OperationState::Unsent(request),
            protocol,
            txid,
        })
    }

    pub fn advance(&mut self) -> Result<Option<Reply>, CallCause> {
        if matches!(self.state, OperationState::Unsent(_))
            && self
                .wait
                .as_ref()
                .expect("live call lost wait owner")
                .cancelled()?
        {
            return Err(CallCause::Cancelled);
        }
        if let OperationState::Unsent(request) = &mut self.state {
            match request.try_send(self.service, self.deadline) {
                Ok(()) => {
                    self.state = OperationState::Sent(request.take_observation());
                    self.wait
                        .as_mut()
                        .expect("live call lost wait owner")
                        .sent(self.service.as_handle())
                        .map_err(cause)?;
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
                drop(self.wait.take());
                Ok(Some(reply))
            }
            Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => Ok(None),
            Err(error) => Err(cause(error)),
        }
    }

    fn ready_at_deadline(&self) -> Result<bool, CallCause> {
        let wait = self.wait.as_ref().expect("live call lost wait owner");
        let probe = |handle, signals| libkernel::ipc::wait::probe(handle, signals).map_err(cause);
        if let OperationState::Sent(observation) = &self.state {
            let port = self.caller.port.as_ref().expect("sent call lost ReplyPort");
            let reply = probe(
                port.owner.as_handle(),
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
            )?;
            if reply.intersects(ObjectSignals::READABLE) {
                return Ok(true);
            }
            if reply.intersects(ObjectSignals::CLOSED) {
                return Err(CallCause::ServiceClosed);
            }
            if wait.cancelled()? {
                return Err(CallCause::Cancelled);
            }
            if probe(self.service.as_handle(), ObjectSignals::CLOSED)?
                .intersects(ObjectSignals::CLOSED)
                || probe(observation.as_handle(), ObjectSignals::PEER_CLOSED)?
                    .intersects(ObjectSignals::PEER_CLOSED)
            {
                return Err(CallCause::ServiceClosed);
            }
            Ok(false)
        } else {
            if wait.cancelled()? {
                return Err(CallCause::Cancelled);
            }
            let service = probe(
                self.service.as_handle(),
                ObjectSignals::WRITABLE | ObjectSignals::CLOSED,
            )?;
            if service.intersects(ObjectSignals::CLOSED) {
                return Err(CallCause::ServiceClosed);
            }
            Ok(service.intersects(ObjectSignals::WRITABLE))
        }
    }

    pub fn wait_ready(&self) -> Result<(), CallCause> {
        if matches!(self.state, OperationState::Completed) {
            return Err(CallCause::System(SystemCallError::ObjectBusy));
        }
        let wait = self.wait.as_ref().expect("live call lost wait owner");
        loop {
            if wait.ready(matches!(self.state, OperationState::Sent(_)))? {
                return Ok(());
            }
            let result = wait.set.wait(self.deadline).map_err(cause)?;
            match WaitReason::from_u32(result.reason) {
                Some(WaitReason::Timeout) => {
                    // 超时边界仍取一次当前来源；就绪提示不能被延后调度翻成超时。
                    if self.ready_at_deadline()? {
                        return Ok(());
                    }
                    return Err(CallCause::Timeout);
                }
                Some(WaitReason::Signaled) => {}
                Some(WaitReason::Closed) => {
                    return Err(CallCause::System(SystemCallError::ObjectClosed));
                }
                None => return Err(CallCause::System(SystemCallError::InternalError)),
            }
        }
    }

    pub fn abort(&mut self, cause: CallCause) -> Option<CallError> {
        drop(self.wait.take());
        match core::mem::replace(&mut self.state, OperationState::Completed) {
            OperationState::Unsent(request) => Some(CallError::unsent(cause, request)),
            OperationState::Sent(_) => {
                let cleanup = self.caller.port.take().map(ReplyCleanup::new);
                Some(CallError::sent_with_cleanup(cause, cleanup))
            }
            OperationState::Completed => None,
        }
    }

    pub fn phase(&self) -> Option<crate::exchange::CallPhase> {
        match self.state {
            OperationState::Unsent(_) => Some(crate::exchange::CallPhase::Unsent),
            OperationState::Sent(_) => Some(crate::exchange::CallPhase::Sent),
            OperationState::Completed => None,
        }
    }
}

impl Drop for CallOperation<'_> {
    fn drop(&mut self) {
        drop(self.wait.take());
        if matches!(self.state, OperationState::Sent(_)) {
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
