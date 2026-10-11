//! 独立文件路由管理授权；普通文件 grant 的 context 不进入此表。
use crate::{
    Funding,
    authority::{FsRights, GrantLease},
    protocol::Status,
    resource::FsResource,
    store::NodeRef,
};
use alloc::{rc::Rc, sync::Arc};
use core::{cell::Cell, mem::ManuallyDrop};
use erhino_shared::{
    call::SystemCallError,
    object::{Handle, Rights},
};
use libbudget::AccountView;
use libexecution::ExecutionResource;
use libkernel::ipc::message::{LifetimeObserver, Mailbox, MailboxSender, MintedSender};
use metadata_admission::Counter;
use ordered_table::{OrderedTable, PreparedEntry};

#[derive(Clone)]
pub struct ManagementSnapshot {
    pub(crate) root: NodeRef,
    pub(crate) max_rights: FsRights,
    pub(crate) output_transport: Rights,
    pub(crate) funding: Funding,
    pub(crate) lease: Rc<GrantLease>,
}
impl ManagementSnapshot {
    pub fn root(&self) -> &NodeRef {
        &self.root
    }
    pub fn max_rights(&self) -> FsRights {
        self.max_rights
    }
    pub fn output_transport(&self) -> Rights {
        self.output_transport
    }
    pub fn funding(&self) -> &Funding {
        &self.funding
    }
    pub fn account(&self) -> &AccountView<FsResource> {
        self.funding.domain()
    }
    pub fn is_active(&self) -> bool {
        self.lease.active.get()
    }
}

pub struct PreparedManagement {
    context: u64,
    state: PreparedEntry<ManagementSnapshot, ()>,
    sender: MailboxSender,
    lifetime: LifetimeObserver,
}
impl PreparedManagement {
    pub fn context(&self) -> u64 {
        self.context
    }
    pub fn lifetime_handle(&self) -> Handle {
        self.lifetime.as_handle()
    }
    pub fn execution_funding(&self) -> &AccountView<ExecutionResource> {
        self.state.value().funding.execution()
    }
    pub fn into_parts(self) -> (ManagementPublication, MailboxSender, LifetimeObserver) {
        (
            ManagementPublication {
                context: self.context,
                state: self.state,
            },
            self.sender,
            self.lifetime,
        )
    }
}
pub struct ManagementPublication {
    context: u64,
    state: PreparedEntry<ManagementSnapshot, ()>,
}
impl ManagementPublication {
    pub fn context(&self) -> u64 {
        self.context
    }
}
pub struct ManagementAdoptFailure {
    pub error: SystemCallError,
    pub root: NodeRef,
    pub minted: MintedSender,
}

pub struct ManagementTable {
    entries: ManuallyDrop<OrderedTable<ManagementSnapshot, ()>>,
    slots: Arc<Counter>,
    mailbox_id: u64,
    sealed: bool,
}
impl ManagementTable {
    pub fn new(mailbox: &Mailbox, limit: usize) -> Result<Self, SystemCallError> {
        if limit == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        Ok(Self {
            entries: ManuallyDrop::new(OrderedTable::new(limit)),
            slots: Arc::try_new(Counter::new(limit)).map_err(|_| SystemCallError::OutOfMemory)?,
            mailbox_id: libkernel::ipc::object::query(mailbox.as_handle())?.object_id,
            sealed: false,
        })
    }
    /// 可信发行边界接管实际 sender/Lifetime，付款视图必须来自同一个账户。
    pub fn prepare_minted(
        &mut self,
        root: NodeRef,
        max_rights: FsRights,
        output_transport: Rights,
        domain: &AccountView<FsResource>,
        execution: &AccountView<ExecutionResource>,
        minted: MintedSender,
    ) -> Result<PreparedManagement, ManagementAdoptFailure> {
        let setup = (|| {
            if self.sealed {
                return Err(SystemCallError::ObjectClosed);
            }
            if !output_transport.is_known() {
                return Err(SystemCallError::IllegalArgument);
            }
            let funding = Funding::new(domain, execution)?;
            let description = minted.sender.description()?;
            if description.related_object_id != self.mailbox_id
                || !description.rights.contains(Rights::WRITE | Rights::WAIT)
            {
                return Err(SystemCallError::WrongObjectType);
            }
            let slot =
                Counter::try_acquire(&self.slots).map_err(|_| SystemCallError::QuotaExceeded)?;
            let grant = domain.acquire(FsResource::Grant, 1)?;
            let observer = domain.acquire(FsResource::WaitSource, 1)?;
            let bytes = domain.acquire(
                FsResource::Bytes,
                PreparedEntry::<ManagementSnapshot, ()>::allocation_bytes()
                    .checked_add(core::mem::size_of::<GrantLease>())
                    .ok_or(SystemCallError::ReachLimit)?,
            )?;
            let lease = Rc::try_new(GrantLease {
                active: Cell::new(true),
                _slot: slot,
                _grant_charge: grant,
                _observer_charge: observer,
                _storage_charge: bytes,
            })
            .map_err(|_| SystemCallError::OutOfMemory)?;
            Ok((description.object_id, funding, lease))
        })();
        let (context, funding, lease) = match setup {
            Ok(setup) => setup,
            Err(error) => {
                return Err(ManagementAdoptFailure {
                    error,
                    root,
                    minted,
                });
            }
        };
        let state = ManagementSnapshot {
            root,
            max_rights,
            output_transport,
            funding,
            lease,
        };
        let state = match self
            .entries
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
                return Err(ManagementAdoptFailure {
                    error,
                    root: state.root,
                    minted,
                });
            }
        };
        Ok(PreparedManagement {
            context,
            state,
            sender: minted.sender,
            lifetime: minted.lifetime,
        })
    }
    /// Lifetime 来源已实际登记；本操作是管理授权与 sender 可发布的线性化点。
    pub fn install(&mut self, publication: ManagementPublication) {
        assert!(!self.sealed, "management table sealed before publication");
        self.entries.insert_prepared(publication.state);
    }
    pub fn access(&self, context: u64) -> Result<ManagementSnapshot, Status> {
        let state = self.entries.get(context).ok_or(Status::Permission)?;
        if self.sealed || !state.is_active() {
            return Err(Status::Revoked);
        }
        Ok(state.clone())
    }
    pub fn funding(&self, context: u64) -> Option<&Funding> {
        self.entries.get(context).map(|state| &state.funding)
    }
    pub fn revoke(&mut self, context: u64) {
        if let Some(state) = self.entries.get(context) {
            state.lease.active.set(false);
        }
    }
    /// 先实际注销来源并关闭 Lifetime，最后移除索引；已准入 Snapshot 保有自己的 pin。
    pub fn remove(&mut self, context: u64) {
        self.entries.remove(context).map(PreparedEntry::into_value);
    }
    pub fn seal(&mut self) {
        self.sealed = true;
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn close(self) -> Result<(), Self> {
        if self.sealed && self.entries.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }
}
impl Drop for ManagementTable {
    fn drop(&mut self) {
        if self.entries.is_empty() {
            // SAFETY: 空表不持授权或能力，本处是唯一一次表析构。
            unsafe {
                ManuallyDrop::drop(&mut self.entries);
            }
        }
        // 非空表保留实际责任，不以普通 Drop 越过 Lifetime 与未退出来源。
    }
}
