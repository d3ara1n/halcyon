//! 文件根、释放及 backing 退休的实际来源 owner。
use super::{FileTask, World};
use erhino_shared::{call::SystemCallError, object::ObjectSignals};
use libbudget::Charge;
use libexecution::{
    SourceRegistration,
    runtime::{Advance, Input, Requests, SourceId, SourcePlan, Step},
};
use libfs::grant::PreparedGrant;
use libfs::management::PreparedManagement;
use libkernel::ipc::message::MailboxSender;
pub(super) struct Root {
    prepared: Option<Preparation>,
    management: bool,
    observer: Option<libkernel::ipc::message::LifetimeObserver>,
    sender: Option<MailboxSender>,
    source: SourceRegistration,
    context: u64,
    stopping: bool,
    revoking: bool,
    _bytes: Charge,
}
enum Preparation {
    File(PreparedGrant),
    Management(PreparedManagement),
}
impl Preparation {
    fn context(&self) -> u64 {
        match self {
            Self::File(prepared) => prepared.context(),
            Self::Management(prepared) => prepared.context(),
        }
    }
    fn lifetime_handle(&self) -> erhino_shared::object::Handle {
        match self {
            Self::File(prepared) => prepared.lifetime_handle(),
            Self::Management(prepared) => prepared.lifetime_handle(),
        }
    }
}
impl Root {
    pub fn new(
        prepared: PreparedGrant,
        account: &libbudget::AccountView<libfs::resource::FsResource>,
    ) -> Result<Self, SystemCallError> {
        Self::prepare(Preparation::File(prepared), account)
    }
    pub fn management(
        prepared: PreparedManagement,
        account: &libbudget::AccountView<libfs::resource::FsResource>,
    ) -> Result<Self, SystemCallError> {
        Self::prepare(Preparation::Management(prepared), account)
    }
    fn prepare(
        prepared: Preparation,
        account: &libbudget::AccountView<libfs::resource::FsResource>,
    ) -> Result<Self, SystemCallError> {
        let context = prepared.context();
        let management = matches!(prepared, Preparation::Management(_));
        Ok(Self {
            prepared: Some(prepared),
            management,
            observer: None,
            sender: None,
            source: SourceRegistration::new(),
            context,
            stopping: false,
            revoking: false,
            _bytes: account.acquire(
                libfs::resource::FsResource::Bytes,
                core::mem::size_of::<Self>(),
            )?,
        })
    }
    pub fn take_sender(&mut self) -> Option<MailboxSender> {
        if !self.stopping {
            self.sender.take()
        } else {
            None
        }
    }
    pub fn stop(&mut self) {
        self.stopping = true;
        self.revoking = true;
    }
    pub fn registered(&mut self, kind: u64, source: SourceId) {
        self.source.registered(kind, source);
    }
    pub fn unregistered(&mut self, source: SourceId) {
        self.source.unregistered(source);
    }
    pub fn refused(&mut self, kind: u64) {
        self.source.refused(kind);
        self.stopping = true;
    }
    pub fn advance(
        &mut self,
        world: &mut World,
        requests: &mut Requests<'_, FileTask>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            if event.observed.intersects(ObjectSignals::CLOSED) {
                self.stopping = true;
            }
        }
        input.take_timeout();
        if self.stopping {
            if self.revoking {
                if self.management {
                    world.management.revoke(self.context);
                } else {
                    world.grants.revoke(self.context);
                }
            }
            if !self.source.is_empty() {
                let _ = self.source.remove(requests);
                return Ok(Advance {
                    work_done: 1,
                    step: if self.source.is_removing() || self.source.is_installing() {
                        Step::Parked
                    } else {
                        Step::Runnable
                    },
                });
            }
            self.sender = None;
            if let Some(observer) = self.observer.take()
                && let Err((observer, _)) = observer.close()
            {
                self.observer = Some(observer);
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                });
            }
            if self.management {
                world.management.remove(self.context);
            } else {
                world.grants.remove(self.context);
            }
            self.prepared = None;
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        if self.source.is_empty() {
            let handle = self
                .prepared
                .as_ref()
                .expect("file root preparation missing")
                .lifetime_handle();
            let _ =
                self.source
                    .install(requests, SourcePlan::new(handle, ObjectSignals::CLOSED), 1);
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if self.source.source().is_none() {
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if let Some(prepared) = self.prepared.take() {
            let (sender, lifetime) = match prepared {
                Preparation::File(prepared) => {
                    let (publication, sender, lifetime) = prepared.into_parts();
                    world.grants.install(publication);
                    (sender, lifetime)
                }
                Preparation::Management(prepared) => {
                    let (publication, sender, lifetime) = prepared.into_parts();
                    world.management.install(publication);
                    (sender, lifetime)
                }
            };
            self.observer = Some(lifetime);
            self.sender = Some(sender);
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Parked,
        })
    }
}
pub(super) struct Release {
    owner: libkernel::ipc::notification::NotificationOwner,
    source: SourceRegistration,
    stopping: bool,
    _fee: Charge,
}
impl Release {
    pub fn admitted(&self) -> bool {
        self.source.source().is_some()
    }
    pub fn new(
        owner: libkernel::ipc::notification::NotificationOwner,
        account: &libbudget::AccountView<libfs::resource::FsResource>,
    ) -> Result<Self, SystemCallError> {
        Ok(Self {
            owner,
            source: SourceRegistration::new(),
            stopping: false,
            _fee: account.acquire(libfs::resource::FsResource::WaitSource, 1)?,
        })
    }
    pub fn stop(&mut self) {
        self.stopping = true;
    }
    pub fn registered(&mut self, kind: u64, source: SourceId) {
        self.source.registered(kind, source);
    }
    pub fn unregistered(&mut self, source: SourceId) {
        self.source.unregistered(source);
    }
    pub fn refused(&mut self, kind: u64) {
        self.source.refused(kind);
        self.stopping = true;
    }
    pub fn advance(
        &mut self,
        world: &mut World,
        requests: &mut Requests<'_, FileTask>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            if event
                .observed
                .intersects(ObjectSignals::READABLE | ObjectSignals::CLOSED)
            {
                world.stop = true;
                self.stopping = true;
            }
        }
        input.take_timeout();
        if self.stopping {
            let _ = self.source.remove(requests);
            return Ok(Advance {
                work_done: 1,
                step: if self.source.is_empty() {
                    Step::Complete
                } else {
                    Step::Runnable
                },
            });
        }
        if self.source.is_empty() {
            let _ = self.source.install(
                requests,
                SourcePlan::new(
                    self.owner.as_handle(),
                    ObjectSignals::READABLE | ObjectSignals::CLOSED,
                ),
                1,
            );
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Parked,
        })
    }
}
pub(super) struct Retire {
    owner: libkernel::ipc::notification::NotificationOwner,
    source: SourceRegistration,
    ready: bool,
    deadline: erhino_shared::time::Deadline,
    stopping: bool,
    stream_turn: bool,
    _fee: Charge,
}
impl Retire {
    pub fn admitted(&self) -> bool {
        self.source.source().is_some()
    }
    pub fn new(
        owner: libkernel::ipc::notification::NotificationOwner,
        account: &libbudget::AccountView<libfs::resource::FsResource>,
    ) -> Result<Self, SystemCallError> {
        Ok(Self {
            owner,
            source: SourceRegistration::new(),
            ready: true,
            deadline: erhino_shared::time::Deadline::INFINITE,
            stopping: false,
            stream_turn: true,
            _fee: account.acquire(libfs::resource::FsResource::WaitSource, 1)?,
        })
    }
    pub fn stop(&mut self) {
        self.stopping = true;
        self.ready = true;
    }
    pub fn deadline(&self) -> erhino_shared::time::Deadline {
        self.deadline
    }
    pub fn registered(&mut self, kind: u64, source: SourceId) {
        self.source.registered(kind, source);
    }
    pub fn unregistered(&mut self, source: SourceId) {
        self.source.unregistered(source);
    }
    pub fn refused(&mut self, kind: u64) {
        self.source.refused(kind);
        self.ready = true;
    }
    pub fn advance(
        &mut self,
        world: &mut World,
        requests: &mut Requests<'_, FileTask>,
        input: &mut Input<'_>,
        budget: usize,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            if event.observed.intersects(ObjectSignals::READABLE) {
                self.owner.take(u64::MAX)?;
                self.ready = true;
                world.memory.begin_maintenance();
            }
        }
        if input.take_timeout() {
            self.ready = true;
        }
        if world.memory.take_data_change() {
            world.streams.backend_changed();
        }
        if self.stopping && world.memory.is_empty() && world.streams.is_empty() {
            let _ = self.source.remove(requests);
            return Ok(Advance {
                work_done: 1,
                step: if self.source.is_empty() {
                    Step::Complete
                } else {
                    Step::Runnable
                },
            });
        }
        if self.source.is_empty() {
            let _ = self.source.install(
                requests,
                SourcePlan::new(self.owner.as_handle(), ObjectSignals::READABLE),
                1,
            );
            return Ok(Advance {
                work_done: 1,
                step: Step::Runnable,
            });
        }
        if world.streams.has_wake_work() && (self.stream_turn || !self.ready) {
            world.streams.wake_step(&world.memory, requests);
            self.stream_turn = false;
            return Ok(Advance {
                work_done: 1,
                step: if self.ready || world.streams.has_wake_work() {
                    Step::Runnable
                } else {
                    Step::Parked
                },
            });
        }
        if self.ready {
            self.stream_turn = true;
            match world.memory.maintenance_step_at(input.now_ns(), budget) {
                Ok(progress) => {
                    if let Some((id, error)) = world.memory.take_retire_failure() {
                        libkernel::debug!(
                            "backing retirement deferred: record={}, error={:?}",
                            id.raw(),
                            error
                        );
                    }
                    self.deadline = world.memory.retirement_deadline().map_or(
                        erhino_shared::time::Deadline::INFINITE,
                        erhino_shared::time::Deadline::at,
                    );
                    let state = world.memory.retirement_state();
                    self.ready = state == libfs::backend::RetirementState::Ready;
                    return Ok(Advance {
                        work_done: progress.work_done.max(1),
                        step: if self.ready
                            || world.streams.has_wake_work()
                            || (self.stopping && state == libfs::backend::RetirementState::Empty)
                        {
                            Step::Runnable
                        } else {
                            Step::Parked
                        },
                    });
                }
                Err(_) => {
                    return Ok(Advance {
                        work_done: 1,
                        step: Step::Runnable,
                    });
                }
            }
        }
        Ok(Advance {
            work_done: 1,
            step: Step::Parked,
        })
    }
}
