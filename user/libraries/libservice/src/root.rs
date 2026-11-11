//! 根角色的真实 Lifetime owner；sender 只作身份凭据，安装后由装配者释放。
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
pub struct AdoptFailure {
    pub status: Status,
    pub minted: libkernel::ipc::message::MintedSender,
}
pub struct Root {
    funding: crate::Funding,
    context: u64,
    prepared: Option<Prepared>,
    directory: bool,
    sender: Option<MailboxSender>,
    lifetime: Option<libkernel::ipc::message::LifetimeObserver>,
    source: SourceRegistration,
    installed: bool,
    stopping: bool,
    revoking: bool,
    _bytes: Charge,
}
impl Root {
    /// 采用已存在且可被调用的运输入口；仅在来源登记和角色安装后开放 Ingress。
    /// 失败返还完整 pair，不能把业务政策当作运输授权。
    pub fn prepare_minted(
        state: &State,
        policy: Policy,
        minted: libkernel::ipc::message::MintedSender,
        account: &AccountView<ServiceResource>,
        execution: &AccountView<libexecution::ExecutionResource>,
    ) -> Result<Self, AdoptFailure> {
        let setup = (|| {
            let funding = crate::Funding::new(account, execution).map_err(|_| Status::Invalid)?;
            let description = minted
                .validate(
                    &state.mailbox,
                    Rights::WRITE | Rights::WAIT | Rights::TRANSFER,
                )
                .map_err(|_| Status::Invalid)?;
            let context = description.object_id;
            if state.accesses.contains_context(context)
                || state.authorities.contains_context(context)
            {
                return Err(Status::Conflict);
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
            Ok((funding, context, bytes, prepared))
        })();
        let (funding, context, bytes, prepared) = match setup {
            Ok(setup) => setup,
            Err(status) => return Err(AdoptFailure { status, minted }),
        };
        Ok(Self {
            funding,
            context,
            directory: matches!(prepared, Prepared::Directory(_)),
            prepared: Some(prepared),
            sender: Some(minted.sender),
            lifetime: Some(minted.lifetime),
            source: SourceRegistration::new(),
            installed: false,
            stopping: false,
            revoking: false,
            _bytes: bytes,
        })
    }
    pub fn execution_funding(&self) -> &AccountView<libexecution::ExecutionResource> {
        self.funding.execution()
    }
    /// 角色已安装后交回本地身份凭据；装配者释放它，不再动态导出。
    pub fn take_sender(&mut self) -> Option<MailboxSender> {
        if self.installed && !self.stopping {
            self.sender.take()
        } else {
            None
        }
    }
    pub fn installed(&self) -> bool {
        self.installed && !self.stopping
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
            if self.installed {
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
            if self.installed {
                if self.directory {
                    state.accesses.remove(self.context);
                } else {
                    state.authorities.remove(self.context);
                }
                self.installed = false;
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
        if self.prepared.is_some()
            && (state.accesses.contains_context(self.context)
                || state.authorities.contains_context(self.context))
        {
            // 两个尚未安装的准备角色也可能同号；在领域边界拒绝，不交给底层表断言。
            return Err(SystemCallError::IllegalArgument);
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
            if self.stopping {
                return Err(SystemCallError::IllegalArgument);
            }
            self.installed = true;
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
