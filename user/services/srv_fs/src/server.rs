//! srv_fs 验收 provider 的长期 Runtime 装配。

use super::{PROTOCOL_ID, REPLY_BODY_MAX, internal_served};
use erhino_shared::{
    call::SystemCallError,
    object::ObjectSignals,
    time::Deadline,
};
use libfal::{FAL_HEADER_LEN, memfs::MemFs, provider};
use librpc::{Outbox, OutboxResult, RequestContext};
use libsrv::{
    budget::{Budget, CoreResource},
    runtime::{
        Advance, DriveState, Input, RequestFailure, Requests, Runtime, SourceId, SourceKind, Step,
        Task,
    },
};
use rinlib::ipc::{
    message::{Mailbox, MessageStorage, ReceiveBuffer},
    wait::wait_until,
    wait_set::WaitSet,
};

pub(super) const STOP_KIND: u64 = 0x4653_5354_4f50;
const KIND_MAILBOX: SourceKind = 1;
const KIND_REPLY: SourceKind = 1;

struct World {
    mailbox: Mailbox,
    provider: MemFs,
    stop: bool,
    failed: bool,
}

struct Ingress {
    buffer: ReceiveBuffer,
    source: Option<SourceId>,
    requested: bool,
    removing: bool,
    stopping: bool,
}

impl Ingress {
    fn new() -> Self {
        Self {
            buffer: ReceiveBuffer::new().expect("provider receive buffer creation failed"),
            source: None,
            requested: false,
            removing: false,
            stopping: false,
        }
    }
}

impl Task<World> for Ingress {
    type Family = ServiceTask;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World,
        requests: &mut Requests<ServiceTask>,
        input: &mut Input<'_>,
        _budget: usize,
    ) -> Result<Advance, SystemCallError> {
        if self.stopping {
            if let Some(source) = self.source
                && !self.removing
            {
                requests.remove(source)?;
                self.removing = true;
            }
            return Ok(Advance {
                work_done: 1,
                step: if self.source.is_none() && !self.requested {
                    Step::Complete
                } else {
                    Step::Parked
                },
            });
        }
        if self.source.is_none() && !self.requested {
            requests.add_source(
                world.mailbox.as_handle(),
                ObjectSignals::READABLE | ObjectSignals::CLOSED,
                KIND_MAILBOX,
            )?;
            self.requested = true;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        let mut ready = false;
        while let Some(event) = input.pull() {
            if event.error != 0 || event.observed.intersects(ObjectSignals::CLOSED) {
                world.failed = true;
                self.stopping = true;
            } else {
                ready = true;
            }
        }
        if self.stopping {
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
        match self.buffer.receive(world.mailbox.as_handle()) {
            Ok(()) => {}
            Err(SystemCallError::ObjectNotAvailable | SystemCallError::ObjectBusy) => {
                requests.rearm(self.source.expect("provider source remains registered"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
            Err(error) => return Err(error),
        }
        if self.buffer.header().kind == STOP_KIND {
            self.buffer.discard();
            world.stop = true;
            self.stopping = true;
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        let message = MessageStorage::new()?.take(&mut self.buffer)?;
        let context = match RequestContext::decode(message, PROTOCOL_ID) {
            Ok(context) if context.handles.remaining() == 1 => context,
            Ok(_) | Err(_) => {
                requests.rearm(self.source.expect("provider source remains registered"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
        };
        let outbox = match Outbox::prepare(
            context,
            FAL_HEADER_LEN + REPLY_BODY_MAX,
            Deadline::INFINITE,
            KIND_REPLY,
        ) {
            Ok(outbox) => outbox,
            Err(_) => {
                requests.rearm(self.source.expect("provider source remains registered"))?;
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Parked,
                });
            }
        };
        requests
            .spawn(
                ServiceTask::Request(RequestTask {
                    outbox,
                    committed: false,
                }),
                1,
            )
            .map_err(|_| SystemCallError::ReachLimit)?;
        requests.rearm(self.source.expect("provider source remains registered"))?;
        Ok(Advance {
            work_done: 1,
            step: Step::Parked,
        })
    }

    fn refused(&mut self, world: &mut World, failure: RequestFailure<ServiceTask>) {
        match failure {
            RequestFailure::Spawn { .. }
            | RequestFailure::Source { .. }
            | RequestFailure::Wake { .. } => world.failed = true,
        }
    }

    fn registered(&mut self, _world: &mut World, kind: SourceKind, source: SourceId) {
        if kind == KIND_MAILBOX {
            self.requested = false;
            self.source = Some(source);
        }
    }

    fn unregistered(&mut self, _world: &mut World, kind: SourceKind, source: SourceId) {
        if kind == KIND_MAILBOX && self.source == Some(source) {
            self.source = None;
            self.requested = false;
            self.removing = false;
        }
    }

    fn stop(&mut self, _world: &mut World) {
        self.stopping = true;
    }
}

struct RequestTask {
    outbox: Outbox,
    committed: bool,
}

impl Task<World> for RequestTask {
    type Family = ServiceTask;

    fn advance(
        &mut self,
        _id: u64,
        world: &mut World,
        requests: &mut Requests<ServiceTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            self.outbox.observe(event);
        }
        if input.take_timeout() {
            self.outbox.timed_out();
        }
        if !self.committed && self.outbox.result().is_none() {
            if !self.outbox.is_admitted() {
                return Ok(self.outbox.admit(requests));
            }
            let mut body = [0u8; REPLY_BODY_MAX];
            let served = {
                let (context, _) = self.outbox.response_mut()?.parts()?;
                match provider::serve(&mut world.provider, &context.payload, &mut body) {
                    Ok(served) => served,
                    Err(_) => internal_served(&mut body),
                }
            };
            let response = self.outbox.response_mut()?;
            let len = provider::encode_reply(response.body_mut()?, served.kind, &body[..served.len]);
            response.finish_body(len)?;
            self.committed = true;
        }
        let advance = self.outbox.drive(requests, budget)?;
        if advance.step == Step::Complete
            && let Some(OutboxResult::Abandoned(cause)) = self.outbox.result()
        {
            rinlib::debug!("fs: provider response abandoned after commit: {:?}", cause);
        }
        Ok(advance)
    }

    fn refused(&mut self, world: &mut World, failure: RequestFailure<ServiceTask>) {
        self.outbox.refused(world, failure);
    }

    fn registered(&mut self, world: &mut World, kind: SourceKind, source: SourceId) {
        self.outbox.registered(world, kind, source);
    }

    fn unregistered(&mut self, world: &mut World, kind: SourceKind, source: SourceId) {
        self.outbox.unregistered(world, kind, source);
    }

    fn stop(&mut self, world: &mut World) {
        self.outbox.stop(world);
    }

    fn deadline(&self) -> Deadline {
        self.outbox.deadline()
    }
}

enum ServiceTask {
    Ingress(Ingress),
    Request(RequestTask),
}

impl Task<World> for ServiceTask {
    type Family = Self;

    fn advance(
        &mut self,
        id: u64,
        world: &mut World,
        requests: &mut Requests<Self>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        match self {
            Self::Ingress(task) => task.advance(id, world, requests, input, budget),
            Self::Request(task) => task.advance(id, world, requests, input, budget),
        }
    }

    fn refused(&mut self, world: &mut World, failure: RequestFailure<Self>) {
        match self {
            Self::Ingress(task) => task.refused(world, failure),
            Self::Request(task) => task.refused(world, failure),
        }
    }

    fn registered(&mut self, world: &mut World, kind: SourceKind, source: SourceId) {
        match self {
            Self::Ingress(task) => task.registered(world, kind, source),
            Self::Request(task) => task.registered(world, kind, source),
        }
    }

    fn unregistered(&mut self, world: &mut World, kind: SourceKind, source: SourceId) {
        match self {
            Self::Ingress(task) => task.unregistered(world, kind, source),
            Self::Request(task) => task.unregistered(world, kind, source),
        }
    }

    fn stop(&mut self, world: &mut World) {
        match self {
            Self::Ingress(task) => task.stop(world),
            Self::Request(task) => task.stop(world),
        }
    }

    fn deadline(&self) -> Deadline {
        match self {
            Self::Ingress(_) => Deadline::INFINITE,
            Self::Request(task) => task.deadline(),
        }
    }
}

pub(super) fn run(mailbox: Mailbox) {
    let budget = Budget::<CoreResource>::new(&[32, 256 * 1024], 1)
        .expect("provider Runtime budget creation failed");
    let account = budget
        .account(&[32, 256 * 1024])
        .expect("provider Runtime account creation failed");
    let set = WaitSet::create(32).expect("provider Runtime WaitSet creation failed");
    let mut runtime = Runtime::<ServiceTask, WaitSet>::new(
        set,
        16,
        16,
        CoreResource::EXECUTION_SLOTS,
        &account,
    )
    .expect("provider Runtime creation failed");
    runtime
        .spawn(ServiceTask::Ingress(Ingress::new()), 1)
        .map_err(|failure| failure.error)
        .expect("provider ingress admission failed");
    let mut world = World {
        mailbox,
        provider: MemFs::new(),
        stop: false,
        failed: false,
    };
    let mut sealing = false;
    loop {
        assert!(!world.failed, "provider Runtime entered a fatal state");
        if world.stop && !sealing {
            runtime.seal();
            sealing = true;
        }
        let result = if sealing {
            runtime.shutdown_turn(&mut world, 1)
        } else {
            runtime.turn(&mut world, 1)
        };
        result.expect("provider Runtime failed");
        match runtime.drive_state() {
            DriveState::Drained => break,
            DriveState::Runnable => {}
            DriveState::Waiting(deadline) => {
                wait_until(&[runtime.wait_item(0)], deadline)
                    .expect("provider Runtime wait failed");
                runtime.notified();
            }
        }
    }
    runtime
        .close()
        .map_err(|(_, error)| error)
        .expect("provider Runtime close failed");
    assert_eq!(account.usage(CoreResource::Task).0, 0);
    assert_eq!(account.usage(CoreResource::InputBytes).0, 0);
}
