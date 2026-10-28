//! Registry mailbox 的接收责任；满载沿原任务发送 Quota，不无限重试派生。
use super::{FileTask, World};
use alloc::boxed::Box;
use erhino_shared::{
    call::SystemCallError, message::PAYLOAD_MAX, object::ObjectSignals, time::Deadline,
};
use libbudget::Charge;
use libexecution::{
    SourceRegistration,
    runtime::{Advance, Input, Requests, SourceId, SourcePlan, Step},
};
use libkernel::ipc::message::{MessageStorage, ReceiveBuffer};
use libservice::{protocol::Status, provider::Request, resource::ServiceResource};
pub(super) struct Ingress {
    source: SourceRegistration,
    buffer: ReceiveBuffer,
    storage: Option<MessageStorage>,
    pending: Option<Box<Request>>,
    refusal: Option<Box<Request>>,
    stopping: bool,
    _source: Charge,
    _bytes: Charge,
}
impl Ingress {
    pub fn admitted(&self) -> bool {
        self.source.source().is_some()
    }
    pub fn new(state: &libservice::provider::State) -> Result<Self, SystemCallError> {
        Ok(Self {
            source: SourceRegistration::new(),
            buffer: ReceiveBuffer::new()?,
            storage: Some(MessageStorage::new()?),
            pending: None,
            refusal: None,
            stopping: false,
            _source: state.account.acquire(ServiceResource::WaitSource, 1)?,
            _bytes: state.account.acquire(
                ServiceResource::Bytes,
                PAYLOAD_MAX * 2 + core::mem::size_of::<Self>(),
            )?,
        })
    }
    pub fn refused_task(&mut self, mut request: Box<Request>) {
        request.reject(Status::Quota);
        assert!(
            self.refusal.is_none(),
            "registry ingress already owns refusal"
        );
        self.refusal = Some(request);
    }
    pub fn stop(&mut self) {
        self.stopping = true;
    }
    pub fn registered(&mut self, kind: u64, source: SourceId) {
        if kind == 0 {
            self.source.registered(kind, source);
        } else if let Some(request) = self.refusal.as_mut() {
            request.registered(kind, source);
        }
    }
    pub fn unregistered(
        &mut self,
        state: &mut libservice::provider::State,
        kind: u64,
        source: SourceId,
    ) {
        if kind == 0 {
            self.source.unregistered(source);
        } else if let Some(request) = self.refusal.as_mut() {
            request.unregistered(state, kind, source);
        }
    }
    pub fn refused<F>(&mut self, failure: libexecution::runtime::RequestFailure<F>) {
        match failure {
            libexecution::runtime::RequestFailure::Source { kind: 0, .. } => {
                self.source.refused(0);
                self.stopping = true;
            }
            failure => {
                if let Some(request) = self.refusal.as_mut() {
                    request.refused(failure);
                }
            }
        }
    }
    pub fn deadline(&self) -> Deadline {
        self.refusal
            .as_ref()
            .or(self.pending.as_ref())
            .map_or(Deadline::INFINITE, |request| request.deadline())
    }
    pub fn advance(
        &mut self,
        task: u64,
        world: &mut World,
        requests: &mut Requests<'_, FileTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        let state = world
            .service
            .as_mut()
            .expect("registry ingress backing missing");
        let mut ready = false;
        while let Some(event) = input.pull() {
            if event.kind == 0 {
                if event.observed.intersects(ObjectSignals::CLOSED) {
                    self.stopping = true;
                    world.stop = true;
                } else {
                    ready = true;
                }
            } else if let Some(refusal) = self.refusal.as_mut() {
                refusal.observe(event);
            }
        }
        if input.take_timeout() {
            self.pending = None;
            if let Some(refusal) = self.refusal.as_mut() {
                refusal.timed_out();
            }
        }
        if self.stopping {
            self.pending = None;
            if let Some(refusal) = self.refusal.as_mut() {
                refusal.stop();
            }
            if !self.source.is_empty() {
                let _ = self.source.remove(requests);
            }
        }
        if let Some(refusal) = self.refusal.as_mut() {
            if !self.source.is_empty() {
                let _ = self.source.remove(requests);
            }
            let advance = refusal.advance(task, state, requests, input, budget)?;
            if advance.step != Step::Complete {
                return Ok(advance);
            }
            self.refusal = None;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if self.stopping {
            return Ok(Advance {
                work_done: 1,
                step: if self.source.is_empty() {
                    Step::Complete
                } else {
                    Step::Runnable
                },
            });
        }
        if let Some(request) = self.pending.take() {
            let funding = request.execution_funding().clone();
            if let Err(returned) = requests.spawn(libexecution::TaskSubmission::new(
                FileTask::ServiceRequest(request),
                3,
                funding,
            )) {
                let FileTask::ServiceRequest(request) = returned.into_parts().0 else {
                    unreachable!("registry spawn returned another task")
                };
                self.refused_task(request);
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if self.source.is_empty() {
            let _ = self.source.install(
                requests,
                SourcePlan::new(
                    state.mailbox.as_handle(),
                    ObjectSignals::READABLE | ObjectSignals::CLOSED,
                ),
                0,
            );
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if !ready {
            return Ok(Advance {
                work_done: 1,
                step: Step::Parked,
            });
        }
        if self.storage.is_none() {
            self.storage = Some(MessageStorage::new()?);
        }
        match self.buffer.receive(state.mailbox.as_handle()) {
            Ok(()) => {}
            Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => {
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            Err(error) => return Err(error),
        }
        let message = self
            .storage
            .take()
            .expect("registry receive storage missing")
            .take(&mut self.buffer)?;
        match Request::accept(message, state) {
            Ok(request) => {
                self.pending =
                    Some(Box::try_new(request).map_err(|_| SystemCallError::OutOfMemory)?)
            }
            Err(rejected) => drop(rejected),
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Runnable,
        })
    }
}
