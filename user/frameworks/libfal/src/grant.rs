//! provider 授权登记；sender 只交付调用者，登记项不持可延长 authority 的母本。

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
    object::{HandleRole, ObjectSignals, Rights},
    wait::WaitItem,
    wait_set::ReadyRecord,
};
use libsrv::budget::{Account, Charge, Resource};
use metadata_admission::{Counter, Permit};
use ordered_table::{OrderedTable, PreparedEntry};
use rinlib::ipc::{capability::Capability, message::Mailbox, wait_set::WaitSet};

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
    output_transport_rights: Rights,
    account: Arc<Account>,
    policy: PolicyState,
    lifetime: Option<Capability>,
    token: Option<u64>,
    _slot: Permit,
    _grant_charge: Charge,
    _observer_charge: Charge,
    _storage_charge: Charge,
}

pub struct GrantTable<'a> {
    set: &'a WaitSet,
    grants: ManuallyDrop<OrderedTable<GrantState>>,
    sources: ManuallyDrop<OrderedTable<u64>>,
    slots: Arc<Counter>,
    cookie: u64,
    mailbox_id: u64,
    sealed: bool,
    cursor: u64,
}

impl<'a> GrantTable<'a> {
    pub fn new(
        set: &'a WaitSet,
        mailbox: &Mailbox,
        cookie: u64,
        limit: usize,
    ) -> Result<Self, SystemCallError> {
        if limit == 0 || cookie == 0 {
            return Err(SystemCallError::IllegalArgument);
        }
        let mailbox_id = rinlib::ipc::object::query(mailbox.as_handle())?.object_id;
        Ok(Self {
            set,
            grants: ManuallyDrop::new(OrderedTable::new(limit)),
            sources: ManuallyDrop::new(OrderedTable::new(limit)),
            slots: Arc::try_new(Counter::new(limit)).map_err(|_| SystemCallError::OutOfMemory)?,
            cookie,
            mailbox_id,
            sealed: false,
            cursor: 0,
        })
    }

    /// 发行者须已验证 root 位于父 grant 内且 rights/output 上限不放大。
    /// 登记、源观察与所有存储完成后才返回 sender，失败零发布。
    pub fn issue(
        &mut self,
        mailbox: &Mailbox,
        root: NodeRef,
        policy: Issuance,
        account: Arc<Account>,
        badge: u64,
    ) -> Result<Capability, SystemCallError> {
        if self.sealed {
            return Err(SystemCallError::ObjectClosed);
        }
        if !policy.output_transport.is_known()
            || !policy.sender_transport.is_known()
            || !policy
                .sender_transport
                .contains(Rights::WRITE | Rights::WAIT | Rights::TRANSIT)
        {
            return Err(SystemCallError::RightsDenied);
        }
        let slot = Counter::try_acquire(&self.slots).map_err(|_| SystemCallError::QuotaExceeded)?;
        let grant_charge = account.acquire(Resource::Grant, 1)?;
        let observer_charge = account.acquire(Resource::WaitSource, 1)?;
        let storage_charge = account.acquire(
            Resource::Bytes,
            PreparedEntry::<GrantState>::allocation_bytes()
                + PreparedEntry::<u64>::allocation_bytes(),
        )?;
        let minted = mailbox.mint(badge, policy.sender_transport)?;
        let description = minted.sender.description()?;
        if description.related_object_id != self.mailbox_id {
            return Err(SystemCallError::WrongObjectType);
        }
        let context = description.object_id;
        let state = GrantState {
            root,
            rights: policy.rights,
            output_transport_rights: policy.output_transport,
            account,
            policy: PolicyState::Active,
            lifetime: Some(minted.lifetime),
            token: None,
            _slot: slot,
            _grant_charge: grant_charge,
            _observer_charge: observer_charge,
            _storage_charge: storage_charge,
        };
        let mut state = self
            .grants
            .prepare_insert(context, state)
            .map_err(insert_error)?;
        let source = self
            .sources
            .prepare_insert(0, context)
            .map_err(insert_error)?;
        let lifetime = state
            .value_mut()
            .lifetime
            .as_ref()
            .expect("new grant lifetime missing");
        let token = self.set.register(WaitItem::new(
            lifetime.as_handle(),
            ObjectSignals::CLOSED,
            self.cookie,
        ))?;
        state.value_mut().token = Some(token);
        self.sources.insert_prepared(source.with_key(token));
        self.grants.insert_prepared(state);
        Ok(minted.sender)
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

    pub fn on_ready(&mut self, record: ReadyRecord) -> Option<u64> {
        if record.arm_generation != 1 {
            return None;
        }
        let context = *self.sources.get(record.token)?;
        if record.error != 0 || record.observed.intersects(ObjectSignals::CLOSED) {
            self.grants.get_mut(context)?.policy = PolicyState::Revoked;
            Some(context)
        } else {
            None
        }
    }

    pub fn revoke(&mut self, context: u64) -> bool {
        let Some(state) = self.grants.get_mut(context) else {
            return false;
        };
        state.policy = PolicyState::Revoked;
        true
    }

    /// 每步注销一个源、关闭一个 observer 或释放一个登记项；失败保留完整状态。
    pub fn retire_step(&mut self, context: u64) -> Result<bool, SystemCallError> {
        let Some(state) = self.grants.get_mut(context) else {
            return Ok(true);
        };
        state.policy = PolicyState::Revoked;
        if let Some(token) = state.token {
            self.set.remove(token)?;
            state.token = None;
            let _ = self.sources.remove(token);
            return Ok(false);
        }
        if let Some(lifetime) = state.lifetime.take() {
            match lifetime.close() {
                Ok(()) => return Ok(false),
                Err((owner, error)) => {
                    state.lifetime = Some(owner);
                    return Err(error);
                }
            }
        }
        let _ = self.grants.remove(context);
        Ok(true)
    }

    pub fn seal(&mut self) {
        self.sealed = true;
    }
    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }

    pub fn drain_step(&mut self) -> Result<bool, SystemCallError> {
        if !self.sealed {
            return Err(SystemCallError::ObjectNotAvailable);
        }
        let mut ids = [0];
        if self.grants.scan_visible(|_| true, self.cursor, &mut ids).0 == 0 {
            return Ok(self.grants.is_empty());
        }
        let context = ids[0];
        if self.retire_step(context)? {
            self.cursor = context;
        }
        Ok(self.grants.is_empty())
    }

    pub fn close(self) -> Result<(), Self> {
        if self.sealed && self.grants.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }
}

fn insert_error<T>(error: ordered_table::InsertError<T>) -> SystemCallError {
    match error {
        ordered_table::InsertError::Limit(_) => SystemCallError::QuotaExceeded,
        ordered_table::InsertError::Allocation(_) => SystemCallError::OutOfMemory,
    }
}

impl Drop for GrantTable<'_> {
    fn drop(&mut self) {
        if self.grants.is_empty() {
            // 所有源与 observer 已退休，空表析构无业务回调。
            unsafe {
                ManuallyDrop::drop(&mut self.grants);
                ManuallyDrop::drop(&mut self.sources);
            }
        } else {
            ABANDONED.fetch_add(self.grants.len(), Ordering::Relaxed);
        }
    }
}

pub fn abandoned_grants() -> usize {
    ABANDONED.load(Ordering::Relaxed)
}
