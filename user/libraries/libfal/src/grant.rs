//! provider 授权登记；Lifetime 观察由 Runtime task 拥有，表只保存已发布授权。

use crate::resource::FalResource;
use crate::{
    authority::{AccessSnapshot, FalRights},
    store::NodeRef,
};
use alloc::sync::Arc;
use core::{
    mem::ManuallyDrop,
    sync::atomic::{AtomicUsize, Ordering},
};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, HandleRole, Rights},
};
use libbudget::{AccountView, Charge};
use metadata_admission::{Counter, Permit};
use ordered_table::{OrderedTable, PreparedEntry};
use rinlib::ipc::{
    capability::Capability,
    message::{Mailbox, MailboxSender, MintedSender},
};

static ABANDONED: AtomicUsize = AtomicUsize::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyState {
    Active,
    Revoked,
}

#[derive(Debug, Clone, Copy)]
pub struct Issuance {
    pub rights: FalRights,
    pub sender_transport: Rights,
    pub output_transport: Rights,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantError {
    CrossDevice,
    Revoked,
    WrongRole,
    Transport(SystemCallError),
}

struct GrantState {
    root: NodeRef,
    rights: FalRights,
    sender_transport_rights: Rights,
    output_transport_rights: Rights,
    account: AccountView<FalResource>,
    policy: PolicyState,
    _slot: Permit,
    _grant_charge: Charge,
    _observer_charge: Charge,
    _storage_charge: Charge,
}

pub struct PreparedGrant {
    context: u64,
    state: PreparedEntry<GrantState>,
    sender: MailboxSender,
    lifetime: Capability,
}

impl PreparedGrant {
    pub fn context(&self) -> u64 {
        self.context
    }

    pub fn lifetime_handle(&self) -> Handle {
        self.lifetime.as_handle()
    }
}

pub struct GrantObserver {
    context: u64,
    lifetime: Capability,
}

impl GrantObserver {
    pub fn context(&self) -> u64 {
        self.context
    }

    pub fn lifetime_handle(&self) -> Handle {
        self.lifetime.as_handle()
    }

    pub fn close(self) -> Result<(), (Self, SystemCallError)> {
        let Self { context, lifetime } = self;
        lifetime
            .close()
            .map_err(|(lifetime, error)| (Self { context, lifetime }, error))
    }
}

pub struct GrantInstall {
    pub sender: MailboxSender,
    pub observer: GrantObserver,
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
    grants: ManuallyDrop<OrderedTable<GrantState>>,
    slots: Arc<Counter>,
    mailbox_id: u64,
    sealed: bool,
}

impl GrantTable {
    pub fn new(mailbox: &Mailbox, limit: usize) -> Result<Self, SystemCallError> {
        if limit == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let mailbox_id = rinlib::ipc::object::query(mailbox.as_handle())?.object_id;
        Ok(Self {
            grants: ManuallyDrop::new(OrderedTable::new(limit)),
            slots: Arc::try_new(Counter::new(limit)).map_err(|_| SystemCallError::OutOfMemory)?,
            mailbox_id,
            sealed: false,
        })
    }

    fn validate_policy(policy: Issuance) -> Result<(), SystemCallError> {
        if !policy.output_transport.is_known()
            || !policy.sender_transport.is_known()
            || !policy
                .sender_transport
                .contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        {
            return Err(SystemCallError::RightsDenied);
        }
        Ok(())
    }

    /// 发行者须已验证 root 位于父 grant 内且 rights/output 上限不放大。
    /// 返回值尚未发布；Runtime task 登记 Lifetime 后才能调用 [`Self::install`]。
    pub fn prepare_issue(
        &mut self,
        mailbox: &Mailbox,
        root: NodeRef,
        policy: Issuance,
        account: AccountView<FalResource>,
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
        self.prepare_minted(root, policy, account, minted)
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
        account: AccountView<FalResource>,
        minted: MintedSender,
    ) -> Result<PreparedGrant, GrantAdoptFailure> {
        let prepare = (|| {
            if self.sealed {
                return Err(SystemCallError::ObjectClosed);
            }
            Self::validate_policy(policy)?;
            let slot =
                Counter::try_acquire(&self.slots).map_err(|_| SystemCallError::QuotaExceeded)?;
            let grant_charge = account.acquire(FalResource::Grant, 1)?;
            let observer_charge = account.acquire(FalResource::WaitSource, 1)?;
            let storage_charge = account.acquire(
                FalResource::Bytes,
                PreparedEntry::<GrantState>::allocation_bytes(),
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
        let state = GrantState {
            root,
            rights: policy.rights,
            sender_transport_rights: policy.sender_transport,
            output_transport_rights: policy.output_transport,
            account,
            policy: PolicyState::Active,
            _slot: slot,
            _grant_charge: grant_charge,
            _observer_charge: observer_charge,
            _storage_charge: storage_charge,
        };
        let state = match self.grants.prepare_insert(context, state) {
            Ok(state) => state,
            Err(error) => {
                let (error, state) = match error {
                    ordered_table::InsertError::Limit(state) => {
                        (SystemCallError::QuotaExceeded, state)
                    }
                    ordered_table::InsertError::Allocation(state) => {
                        (SystemCallError::OutOfMemory, state)
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
    pub fn install(&mut self, prepared: PreparedGrant) -> GrantInstall {
        let PreparedGrant {
            context,
            state,
            sender,
            lifetime,
        } = prepared;
        self.grants.insert_prepared(state);
        GrantInstall {
            sender,
            observer: GrantObserver { context, lifetime },
        }
    }

    /// 从当前活动 grant 派生独立寿命的子 grant。调用者须先在后端确认
    /// `root` 位于父 grant 可达范围内且为目录；此处只负责权限不放大、
    /// 运输 ceiling 继承以及完整的发行退款。
    pub fn prepare_derive(
        &mut self,
        mailbox: &Mailbox,
        parent_context: u64,
        root: NodeRef,
        rights: FalRights,
        badge: u64,
    ) -> Result<PreparedGrant, GrantIssueFailure> {
        let Some(parent) = self
            .grants
            .get(parent_context)
            .filter(|parent| parent.policy == PolicyState::Active)
        else {
            return Err(GrantIssueFailure {
                error: SystemCallError::ObjectClosed,
                root,
            });
        };
        if !parent.rights.contains(rights) {
            return Err(GrantIssueFailure {
                error: SystemCallError::RightsDenied,
                root,
            });
        }
        let policy = Issuance {
            rights,
            sender_transport: parent.sender_transport_rights,
            output_transport: parent.output_transport_rights,
        };
        let account = parent.account.clone();
        self.prepare_issue(mailbox, root, policy, account, badge)
    }

    /// context 来自内核 MessageHeader，客户端 payload 的数字不能进入此入口。
    pub fn snapshot(&self, context: u64) -> Option<AccessSnapshot> {
        if self.sealed {
            return None;
        }
        let state = self.grants.get(context)?;
        if state.policy != PolicyState::Active {
            return None;
        }
        Some(AccessSnapshot {
            root: state.root.clone(),
            rights: state.rights,
            account: state.account.clone(),
        })
    }

    pub fn mailbox_id(&self) -> u64 {
        self.mailbox_id
    }

    pub fn output_rights(&self, context: u64) -> Option<Rights> {
        self.grants
            .get(context)
            .filter(|state| state.policy == PolicyState::Active)
            .map(|state| state.output_transport_rights)
    }

    /// Move 的收到能力必须实际指向本表已登记 sender，不能只按 badge 认领。
    pub fn validate_received(&self, capability: &Capability) -> Result<AccessSnapshot, GrantError> {
        let description = capability.description().map_err(GrantError::Transport)?;
        if description.role != HandleRole::MailboxSender as u32
            || !description.rights.contains(Rights::WRITE)
        {
            return Err(GrantError::WrongRole);
        }
        if description.related_object_id != self.mailbox_id {
            return Err(GrantError::CrossDevice);
        }
        self.snapshot(description.object_id)
            .ok_or(GrantError::Revoked)
    }

    pub fn revoke(&mut self, context: u64) -> bool {
        let Some(state) = self.grants.get_mut(context) else {
            return false;
        };
        state.policy = PolicyState::Revoked;
        true
    }

    /// Runtime 已确认来源注销后移除授权状态；observer 仍由对应 task 独占。
    pub fn remove(&mut self, context: u64) -> bool {
        self.grants.remove(context).is_some()
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
            // 所有源与 observer 已退休，空表析构无业务回调。
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
