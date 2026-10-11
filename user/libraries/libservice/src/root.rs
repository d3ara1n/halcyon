//! 根发行者的真实 Lifetime owner；sender 只在来源成功登记后交给启动投递者。
use crate::{
    authority::{PreparedRole, Scope},
    protocol::{AccessRights, PublishRights, Status},
    provider::State,
    resource::ServiceResource,
};
use erhino_shared::{
    call::SystemCallError,
    object::{ObjectSignals, Rights},
    time::Deadline,
};
use libbudget::{AccountView, Charge};
use libexecution::{
    SourceRegistration,
    runtime::{Advance, Input, Requests, SourceId, SourcePlan, Step},
};
use libkernel::ipc::message::MailboxSender;
pub const LIFETIME: u64 = 1;
pub enum Policy {
    Directory {
        scope: Scope,
        rights: AccessRights,
        output: Rights,
    },
    Authority {
        scope: Scope,
        rights: PublishRights,
        output: Rights,
    },
}
enum Prepared {
    Directory(PreparedRole<AccessRights>),
    Authority(PreparedRole<PublishRights>),
}
pub struct Root {
    funding: crate::Funding,
    context: u64,
    prepared: Option<Prepared>,
    directory: bool,
    sender: Option<MailboxSender>,
    lifetime: Option<libkernel::ipc::message::LifetimeObserver>,
    source: SourceRegistration,
    published: bool,
    stopping: bool,
    revoking: bool,
    _bytes: Charge,
}
impl Root {
    pub fn prepare(
        state: &State,
        policy: Policy,
        transport: Rights,
        account: &AccountView<ServiceResource>,
        execution: &AccountView<libexecution::ExecutionResource>,
    ) -> Result<Self, Status> {
        let funding = crate::Funding::new(account, execution).map_err(|_| Status::Invalid)?;
        if !transport.is_known() {
            return Err(Status::Invalid);
        }
        if !transport.contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT) {
            return Err(Status::Permission);
        }
        let bytes = account
            .acquire(ServiceResource::Bytes, core::mem::size_of::<Self>())
            .map_err(|_| Status::Resource)?;
        let prepared = match policy {
            Policy::Directory {
                scope,
                rights,
                output,
            } => Prepared::Directory(state.accesses.prepare(
                scope,
                rights,
                output,
                funding.clone(),
            )?),
            Policy::Authority {
                scope,
                rights,
                output,
            } => Prepared::Authority(state.authorities.prepare(
                scope,
                rights,
                output,
                funding.clone(),
            )?),
        };
        let minted = state
            .mailbox
            .mint(0, transport)
            .map_err(|_| Status::Resource)?;
        let context = minted
            .sender
            .description()
            .map_err(|_| Status::Resource)?
            .object_id;
        Ok(Self {
            funding,
            context,
            directory: matches!(prepared, Prepared::Directory(_)),
            prepared: Some(prepared),
            sender: Some(minted.sender),
            lifetime: Some(minted.lifetime),
            source: SourceRegistration::new(),
            published: false,
            stopping: false,
            revoking: false,
            _bytes: bytes,
        })
    }
    pub fn execution_funding(&self) -> &AccountView<libexecution::ExecutionResource> {
        self.funding.execution()
    }
    /// 启动投递者接管母本；本 owner 从此只持 Lifetime，不在 World 留一份 sender。
    pub fn take_sender(&mut self) -> Option<MailboxSender> {
        if self.published && !self.stopping {
            self.sender.take()
        } else {
            None
        }
    }
    pub fn published(&self) -> bool {
        self.published && !self.stopping
    }
    pub fn stop(&mut self) {
        self.stopping = true;
        self.revoking = true;
    }
    pub fn refused(&mut self, kind: u64) {
        self.source.refused(kind);
        self.stopping = true;
    }
    pub fn registered(&mut self, kind: u64, source: SourceId) {
        self.source.registered(kind, source);
    }
    pub fn unregistered(&mut self, source: SourceId) {
        self.source.unregistered(source);
    }
    pub fn advance<F>(
        &mut self,
        state: &mut State,
        requests: &mut Requests<'_, F>,
        input: &mut Input<'_>,
    ) -> Result<Advance, SystemCallError> {
        while let Some(event) = input.pull() {
            if event.kind == LIFETIME && event.observed.intersects(ObjectSignals::CLOSED) {
                self.stopping = true;
            }
        }
        input.take_timeout();
        if self.stopping {
            if self.published {
                if self.revoking {
                    if self.directory {
                        state.accesses.revoke(self.context);
                    } else {
                        state.authorities.revoke(self.context);
                    }
                }
                state.changed(requests);
            }
            if !self.source.is_empty() {
                let _ = self.source.remove(requests);
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                });
            }
            self.sender = None;
            if let Some(lifetime) = self.lifetime.take()
                && let Err((lifetime, _)) = lifetime.close()
            {
                self.lifetime = Some(lifetime);
                return Ok(Advance {
                    work_done: 1,
                    step: Step::Runnable,
                });
            }
            self.prepared = None;
            if self.published {
                if self.directory {
                    state.accesses.remove(self.context);
                } else {
                    state.authorities.remove(self.context);
                }
                self.published = false;
            }
            return Ok(Advance {
                work_done: 1,
                step: Step::Complete,
            });
        }
        if self.source.is_empty() {
            let lifetime = self
                .lifetime
                .as_ref()
                .expect("root Lifetime missing")
                .as_handle();
            let _ = self.source.install(
                requests,
                SourcePlan::new(lifetime, ObjectSignals::CLOSED),
                LIFETIME,
            );
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
            match prepared {
                Prepared::Directory(role) => {
                    if let Err(f) = state.accesses.install(role, self.context) {
                        self.prepared = Some(Prepared::Directory(f.prepared));
                        self.stopping = true;
                    }
                }
                Prepared::Authority(role) => {
                    if let Err(f) = state.authorities.install(role, self.context) {
                        self.prepared = Some(Prepared::Authority(f.prepared));
                        self.stopping = true;
                    }
                }
            }
            if !self.stopping {
                self.published = true;
            }
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
    pub fn deadline(&self) -> Deadline {
        Deadline::INFINITE
    }
}
