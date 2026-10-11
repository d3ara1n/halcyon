//! provider 授权登记；Lifetime 观察由 Runtime task 拥有，表只保存已发布授权。

use crate::Funding;
use crate::resource::FsResource;
use crate::{
    authority::{AccessSnapshot, FsRights, GrantLease, Issuance},
    store::NodeRef,
};
use alloc::{rc::Rc, sync::Arc};
use core::cell::Cell;
use core::{
    mem::ManuallyDrop,
    sync::atomic::{AtomicUsize, Ordering},
};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, ObjectKind, Rights},
};
use libbudget::AccountView;
use libexecution::ExecutionResource;
use libkernel::ipc::{
    capability::Capability,
    message::{Mailbox, MailboxSender, MintedSender},
};
use metadata_admission::Counter;
use ordered_table::{OrderedTable, PreparedEntry};

static ABANDONED: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantError {
    CrossDevice,
    Revoked,
    WrongRole,
    Transport(SystemCallError),
}

pub struct PreparedGrant {
    context: u64,
    state: PreparedEntry<AccessSnapshot, ()>,
    sender: MailboxSender,
    lifetime: libkernel::ipc::message::LifetimeObserver,
}

impl PreparedGrant {
    pub fn execution_funding(&self) -> &AccountView<ExecutionResource> {
        self.state.value().funding.execution()
    }
    pub fn context(&self) -> u64 {
        self.context
    }

    pub fn lifetime_handle(&self) -> Handle {
        self.lifetime.as_handle()
    }
    /// 把真实角色交给回复/寿命 owner，表项仍未公开。
    pub fn into_parts(
        self,
    ) -> (
        GrantPublication,
        MailboxSender,
        libkernel::ipc::message::LifetimeObserver,
    ) {
        let Self {
            context,
            state,
            sender,
            lifetime,
        } = self;
        (GrantPublication { context, state }, sender, lifetime)
    }
}

pub struct GrantPublication {
    context: u64,
    state: PreparedEntry<AccessSnapshot, ()>,
}
impl GrantPublication {
    pub fn context(&self) -> u64 {
        self.context
    }
    pub fn root(&self) -> &NodeRef {
        &self.state.value().root
    }
}

pub struct GrantIssueFailure {
    pub error: SystemCallError,
    pub root: NodeRef,
}

pub struct GrantAdoptFailure {
    pub error: SystemCallError,
    pub root: NodeRef,
    pub minted: MintedSender,
}

pub struct GrantTable {
    grants: ManuallyDrop<OrderedTable<AccessSnapshot, ()>>,
    slots: Arc<Counter>,
    mailbox_id: u64,
    sealed: bool,
}

impl GrantTable {
    pub fn new(mailbox: &Mailbox, limit: usize) -> Result<Self, SystemCallError> {
        if limit == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let mailbox_id = libkernel::ipc::object::query(mailbox.as_handle())?.object_id;
        Ok(Self {
            grants: ManuallyDrop::new(OrderedTable::new(limit)),
            slots: Arc::try_new(Counter::new(limit)).map_err(|_| SystemCallError::OutOfMemory)?,
            mailbox_id,
            sealed: false,
        })
    }

    fn validate_policy(policy: Issuance) -> Result<(), SystemCallError> {
        policy.validate_policy().map_err(|status| match status {
            crate::protocol::Status::Invalid => SystemCallError::IllegalArgument,
            _ => SystemCallError::RightsDenied,
        })
    }

    /// 发行者须已验证 root 位于父 grant 内且 rights/output 上限不放大。
    /// 返回值尚未发布；Runtime task 登记 Lifetime 后才能调用 [`Self::install`]。
    pub fn prepare_issue(
        &mut self,
        mailbox: &Mailbox,
        root: NodeRef,
        policy: Issuance,
        account: &AccountView<FsResource>,
        execution: &AccountView<ExecutionResource>,
        badge: u64,
    ) -> Result<PreparedGrant, GrantIssueFailure> {
        let funding = match Funding::new(account, execution) {
            Ok(funding) => funding,
            Err(error) => return Err(GrantIssueFailure { error, root }),
        };
        self.prepare_issue_funded(mailbox, root, policy, funding, badge)
    }

    fn prepare_issue_funded(
        &mut self,
        mailbox: &Mailbox,
        root: NodeRef,
        policy: Issuance,
        funding: Funding,
        badge: u64,
    ) -> Result<PreparedGrant, GrantIssueFailure> {
        if self.sealed {
            return Err(GrantIssueFailure {
                error: SystemCallError::ObjectClosed,
                root,
            });
        }
        if let Err(error) = Self::validate_policy(policy) {
            return Err(GrantIssueFailure { error, root });
        }
        let minted = match mailbox.mint(badge, policy.sender_transport) {
            Ok(minted) => minted,
            Err(error) => return Err(GrantIssueFailure { error, root }),
        };
        self.prepare_minted_funded(root, policy, funding, minted)
            .map_err(|failure| GrantIssueFailure {
                error: failure.error,
                root: failure.root,
            })
    }

    /// 收编装配者已铸造但尚未公开的 sender/Lifetime 对。
    pub fn prepare_minted(
        &mut self,
        root: NodeRef,
        policy: Issuance,
        account: &AccountView<FsResource>,
        execution: &AccountView<ExecutionResource>,
        minted: MintedSender,
    ) -> Result<PreparedGrant, GrantAdoptFailure> {
        let funding = match Funding::new(account, execution) {
            Ok(funding) => funding,
            Err(error) => {
                return Err(GrantAdoptFailure {
                    error,
                    root,
                    minted,
                });
            }
        };
        self.prepare_minted_funded(root, policy, funding, minted)
    }

    fn prepare_minted_funded(
        &mut self,
        root: NodeRef,
        policy: Issuance,
        funding: Funding,
        minted: MintedSender,
    ) -> Result<PreparedGrant, GrantAdoptFailure> {
        let account = funding.domain();
        let prepare = (|| {
            if self.sealed {
                return Err(SystemCallError::ObjectClosed);
            }
            Self::validate_policy(policy)?;
            let slot =
                Counter::try_acquire(&self.slots).map_err(|_| SystemCallError::QuotaExceeded)?;
            let grant_charge = account.acquire(FsResource::Grant, 1)?;
            let observer_charge = account.acquire(FsResource::WaitSource, 1)?;
            let storage_charge = account.acquire(
                FsResource::Bytes,
                PreparedEntry::<AccessSnapshot, ()>::allocation_bytes()
                    .checked_add(core::mem::size_of::<GrantLease>())
                    .ok_or(SystemCallError::OutOfMemory)?,
            )?;
            Ok((slot, grant_charge, observer_charge, storage_charge))
        })();
        let (slot, grant_charge, observer_charge, storage_charge) = match prepare {
            Ok(prepared) => prepared,
            Err(error) => {
                return Err(GrantAdoptFailure {
                    error,
                    root,
                    minted,
                });
            }
        };
        let description = match minted.sender.description() {
            Ok(description) => description,
            Err(error) => {
                return Err(GrantAdoptFailure {
                    error,
                    root,
                    minted,
                });
            }
        };
        if description.related_object_id != self.mailbox_id
            || !policy.sender_transport.is_subset_of(description.rights)
        {
            return Err(GrantAdoptFailure {
                error: SystemCallError::WrongObjectType,
                root,
                minted,
            });
        }
        let context = description.object_id;
        let lease = match Rc::try_new(GrantLease {
            active: Cell::new(true),
            _slot: slot,
            _grant_charge: grant_charge,
            _observer_charge: observer_charge,
            _storage_charge: storage_charge,
        }) {
            Ok(lease) => lease,
            Err(_) => {
                return Err(GrantAdoptFailure {
                    error: SystemCallError::OutOfMemory,
                    root,
                    minted,
                });
            }
        };
        let state = AccessSnapshot {
            root,
            rights: policy.rights,
            output_transport: policy.output_transport,
            funding,
            lease,
        };
        let state = match self
            .grants
            .prepare_insert(context, state, &libbudget::AllocatorBacked)
        {
            Ok(state) => state,
            Err(error) => {
                let (error, state) = match error {
                    ordered_table::InsertError::Limit(state) => {
                        (SystemCallError::QuotaExceeded, state)
                    }
                    ordered_table::InsertError::Storage(error, state) => {
                        (libbudget::allocator_error(error), state)
                    }
                };
                return Err(GrantAdoptFailure {
                    error,
                    root: state.root,
                    minted,
                });
            }
        };
        Ok(PreparedGrant {
            context,
            state,
            sender: minted.sender,
            lifetime: minted.lifetime,
        })
    }

    /// Lifetime source 已由 Runtime 登记；此处是授权可见与 sender 可发布的线性化点。
    pub fn install(&mut self, publication: GrantPublication) {
        self.grants.insert_prepared(publication.state);
    }

    /// 从当前活动 grant 派生独立寿命的子 grant。调用者须先在后端确认
    /// `root` 位于父 grant 可达范围内且为目录；此处只负责权限不放大、
    /// 运输 ceiling 继承以及完整的发行退款。
    pub fn prepare_derive(
        &mut self,
        mailbox: &Mailbox,
        source: &AccessSnapshot,
        root: NodeRef,
        policy: Issuance,
        ceiling: FsRights,
        badge: u64,
    ) -> Result<PreparedGrant, GrantIssueFailure> {
        if policy.validate_derived(source, ceiling).is_err() {
            return Err(GrantIssueFailure {
                error: SystemCallError::RightsDenied,
                root,
            });
        }
        self.prepare_issue_funded(mailbox, root, policy, source.funding().clone(), badge)
    }

    /// 仅从真实 sender context 解析；撤销只阻止业务，不改变已发行的付款身份。
    pub fn funding(&self, context: u64) -> Option<&Funding> {
        self.grants.get(context).map(|state| &state.funding)
    }

    /// context 来自内核 MessageHeader；未知角色与显式撤销保持不同错误。
    pub fn access(&self, context: u64) -> Result<AccessSnapshot, crate::protocol::Status> {
        let state = self
            .grants
            .get(context)
            .ok_or(crate::protocol::Status::Permission)?;
        if self.sealed || !state.lease.active.get() {
            return Err(crate::protocol::Status::Revoked);
        }
        Ok(state.clone())
    }

    pub fn mailbox_id(&self) -> u64 {
        self.mailbox_id
    }

    /// Rename 的收到能力必须实际指向本表已登记 sender，不能只按 badge 认领。
    pub fn validate_received(&self, capability: &Capability) -> Result<AccessSnapshot, GrantError> {
        let description = capability.description().map_err(GrantError::Transport)?;
        if description.kind != ObjectKind::MailboxSender as u32
            || !description.rights.contains(Rights::WRITE)
        {
            return Err(GrantError::WrongRole);
        }
        if description.related_object_id != self.mailbox_id {
            return Err(GrantError::CrossDevice);
        }
        let state = self
            .grants
            .get(description.object_id)
            .ok_or(GrantError::WrongRole)?;
        if self.sealed || !state.lease.active.get() {
            return Err(GrantError::Revoked);
        }
        Ok(state.clone())
    }

    pub fn revoke(&mut self, context: u64) -> bool {
        let Some(state) = self.grants.get_mut(context) else {
            return false;
        };
        state.lease.active.set(false);
        true
    }

    /// Runtime 已确认来源注销后移除授权状态；observer 仍由对应 task 独占。
    pub fn remove(&mut self, context: u64) -> bool {
        self.grants
            .remove(context)
            .map(ordered_table::PreparedEntry::into_value)
            .is_some()
    }

    pub fn seal(&mut self) {
        self.sealed = true;
    }
    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }

    pub fn close(self) -> Result<(), Self> {
        if self.sealed && self.grants.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }
}

impl Drop for GrantTable {
    fn drop(&mut self) {
        if self.grants.is_empty() {
            // SAFETY: ManuallyDrop 表仅在此析构一次；空表不再拥有授权记录。
            unsafe {
                ManuallyDrop::drop(&mut self.grants);
            }
        } else {
            ABANDONED.fetch_add(self.grants.len(), Ordering::Relaxed);
        }
    }
}

pub fn abandoned_grants() -> usize {
    ABANDONED.load(Ordering::Relaxed)
}
