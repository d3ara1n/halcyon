//! 同步调用使用私有 ReplyPort，运输、期限与错误阶段共用 exchange 核心。

use erhino_shared::{
    call::SystemCallError,
    object::{ObjectSignals, Rights},
    time::Deadline,
    wait::{WaitItem, WaitReason},
};
use rinlib::ipc::{
    capability::Capability,
    message::Mailbox,
    wait::wait_until,
};
use crate::exchange::{CallCause, CallError, Reply, Request, cause};

struct ReplyPort {
    owner: Mailbox,
    sender: Capability,
}

impl ReplyPort {
    fn new() -> Result<Self, SystemCallError> {
        let owner = Mailbox::create(Rights::READ | Rights::WAIT | Rights::MANAGE)?;
        let minted = owner.mint(0, Rights::WRITE | Rights::WAIT | Rights::DUPLICATE | Rights::TRANSIT)?;
        Ok(Self { owner, sender: minted.sender })
    }
}

pub struct Caller {
    port: Option<ReplyPort>,
}

impl Caller {
    pub const fn new() -> Self { Self { port: None } }

    fn ensure_port(&mut self) -> Result<(), SystemCallError> {
        if self.port.is_none() { self.port = Some(ReplyPort::new()?) }
        Ok(())
    }

    pub fn call(&mut self, service: &Capability, deadline: Deadline, mut request: Request) -> Result<Reply, CallError> {
        let setup = (|| {
            request.new_attempt()?;
            let description = service.description()?;
            if !description.rights.contains(Rights::WRITE | Rights::WAIT) { return Err(SystemCallError::RightsDenied) }
            self.ensure_port()?;
            request.attach_reply(&self.port.as_ref().expect("ReplyPort just created").sender)
        })();
        if let Err(error) = setup { return Err(CallError::unsent(cause(error), request)) }
        loop {
            match request.try_send(service, deadline) {
                Ok(()) => break,
                Err(SystemCallError::MailboxFull) => (),
                Err(error) => return Err(CallError::unsent(cause(error), request)),
            }
            let result = match wait_until(&[WaitItem::new(service.as_handle(), ObjectSignals::WRITABLE | ObjectSignals::CLOSED, 0)], deadline) {
                Ok(result) => result,
                Err(error) => return Err(CallError::unsent(cause(error), request)),
            };
            if WaitReason::from_u32(result.reason) == Some(WaitReason::Timeout) {
                return Err(CallError::unsent(CallCause::Timeout, request));
            }
            if result.observed.intersects(ObjectSignals::CLOSED) {
                return Err(CallError::unsent(CallCause::ServiceClosed, request));
            }
        }
        let port = self.port.as_ref().expect("ReplyPort survives successful Send");
        let result = (|| {
            let items = [
                WaitItem::new(port.owner.as_handle(), ObjectSignals::READABLE | ObjectSignals::CLOSED, 0),
                WaitItem::new(service.as_handle(), ObjectSignals::CLOSED, 1),
            ];
            loop {
                let result = wait_until(&items, deadline).map_err(cause)?;
                if WaitReason::from_u32(result.reason) == Some(WaitReason::Timeout) { return Err(CallCause::Timeout) }
                if result.observed.intersects(ObjectSignals::CLOSED) { return Err(CallCause::ServiceClosed) }
                match port.owner.receive() {
                    Ok(message) => return Reply::accept(message, request.protocol(), request.txid(), deadline),
                    Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => (),
                    Err(error) => return Err(cause(error)),
                }
            }
        })();
        match result {
            Ok(reply) => Ok(reply),
            Err(cause) => {
                self.port = None;
                Err(CallError::sent(cause))
            }
        }
    }
}

impl Default for Caller {
    fn default() -> Self { Self::new() }
}
