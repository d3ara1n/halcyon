//! 独立 DirectoryAccess/NameAuthority 表；真实 context、范围、输出与付款由发行者冻结。
use crate::{
    protocol::{AccessRights, PublishRights, Status, valid_name},
    resource::ServiceResource,
};
use alloc::{rc::Rc, string::String, sync::Arc};
use core::{cell::Cell, mem::ManuallyDrop};
use erhino_shared::{call::SystemCallError, object::Rights};
use libbudget::{AccountView, Charge};
use metadata_admission::{Counter, Permit};
use ordered_table::{OrderedTable, PreparedEntry};
fn status(e: SystemCallError) -> Status {
    match e {
        SystemCallError::QuotaExceeded => Status::Quota,
        SystemCallError::OutOfMemory => Status::Resource,
        _ => Status::Resource,
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Root,
    ExactName(String),
}
impl Scope {
    pub fn permits(&self, name: &str) -> bool {
        match self {
            Self::Root => true,
            Self::ExactName(expected) => expected == name,
        }
    }
}
pub trait RoleRights: Copy {
    fn raw(self) -> u32;
    fn resource() -> ServiceResource;
}
impl RoleRights for AccessRights {
    fn raw(self) -> u32 {
        self.raw()
    }
    fn resource() -> ServiceResource {
        ServiceResource::Access
    }
}
impl RoleRights for PublishRights {
    fn raw(self) -> u32 {
        self.raw()
    }
    fn resource() -> ServiceResource {
        ServiceResource::Authority
    }
}
struct State<R> {
    scope: Scope,
    rights: R,
    output: Rights,
    funding: crate::Funding,
    active: Cell<bool>,
    _slot: Permit,
    _unit: Charge,
    _source: Charge,
    _bytes: Charge,
}
pub struct Snapshot<R> {
    state: Rc<State<R>>,
}
impl<R> Clone for Snapshot<R> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}
impl<R: RoleRights> Snapshot<R> {
    pub fn scope(&self) -> &Scope {
        &self.state.scope
    }
    pub fn rights(&self) -> R {
        self.state.rights
    }
    pub fn output(&self) -> Rights {
        self.state.output
    }
    /// 无显式运输请求的控制出口，只取必需位与上限允许的复制权。
    pub fn control_transport(&self) -> Result<Rights, Status> {
        let required = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
        if !self.output().contains(required) {
            return Err(Status::Permission);
        }
        Ok(required | (self.output() & Rights::DUPLICATE))
    }
    pub fn account(&self) -> &AccountView<ServiceResource> {
        self.state.funding.domain()
    }
    pub fn funding(&self) -> &crate::Funding {
        &self.state.funding
    }
    pub fn active(&self) -> bool {
        self.state.active.get()
    }
    pub fn check(&self, name: Option<&str>, required: R) -> Result<(), Status> {
        if !self.active() {
            return Err(Status::Revoked);
        }
        if self.rights().raw() & required.raw() != required.raw()
            || name.is_some_and(|name| !self.scope().permits(name))
        {
            return Err(Status::Permission);
        }
        Ok(())
    }
}
pub type DirectoryAccess = Snapshot<AccessRights>;
pub type NameAuthority = Snapshot<PublishRights>;
pub type AccessTable = RoleTable<AccessRights>;
pub type AuthorityTable = RoleTable<PublishRights>;
pub struct PreparedRole<R> {
    entry: PreparedEntry<Snapshot<R>, ()>,
}
pub struct InstallFailure<R> {
    pub status: Status,
    pub prepared: PreparedRole<R>,
}
pub struct RoleTable<R> {
    entries: ManuallyDrop<OrderedTable<Snapshot<R>, ()>>,
    slots: Arc<Counter>,
    sealed: bool,
}
impl<R: RoleRights> RoleTable<R> {
    pub fn new(limit: usize) -> Result<Self, Status> {
        if limit == 0 {
            return Err(Status::Invalid);
        }
        Ok(Self {
            entries: ManuallyDrop::new(OrderedTable::new(limit)),
            slots: Arc::try_new(Counter::new(limit)).map_err(|_| Status::Resource)?,
            sealed: false,
        })
    }
    pub fn prepare(
        &self,
        scope: Scope,
        rights: R,
        output: Rights,
        funding: crate::Funding,
    ) -> Result<PreparedRole<R>, Status> {
        if self.sealed {
            return Err(Status::Revoked);
        }
        if !output.is_known() {
            return Err(Status::Invalid);
        }
        if let Scope::ExactName(name) = &scope
            && !valid_name(name)
        {
            return Err(Status::Invalid);
        }
        let account = funding.domain();
        let slot = Counter::try_acquire(&self.slots).map_err(|_| Status::Quota)?;
        let unit = account.acquire(R::resource(), 1).map_err(status)?;
        let source = account
            .acquire(ServiceResource::WaitSource, 1)
            .map_err(status)?;
        let storage = core::mem::size_of::<State<R>>()
            .checked_add(PreparedEntry::<Snapshot<R>, ()>::allocation_bytes())
            .and_then(|n| {
                n.checked_add(match &scope {
                    Scope::Root => 0,
                    Scope::ExactName(name) => name.capacity(),
                })
            })
            .ok_or(Status::Resource)?;
        let bytes = account
            .acquire(ServiceResource::Bytes, storage)
            .map_err(status)?;
        let snapshot = Snapshot {
            state: Rc::try_new(State {
                scope,
                rights,
                output,
                funding,
                active: Cell::new(true),
                _slot: slot,
                _unit: unit,
                _source: source,
                _bytes: bytes,
            })
            .map_err(|_| Status::Resource)?,
        };
        let entry = self
            .entries
            .prepare_insert_candidate(0, snapshot, &libbudget::AllocatorBacked)
            .map_err(|e| match e {
                ordered_table::InsertError::Limit(_) => Status::Quota,
                ordered_table::InsertError::Storage(e, _) => status(libbudget::allocator_error(e)),
            })?;
        Ok(PreparedRole { entry })
    }
    /// actual context 必须由本 mailbox 的 MintedSender description 确认，来源先准入。
    pub fn install(
        &mut self,
        prepared: PreparedRole<R>,
        context: u64,
    ) -> Result<(), InstallFailure<R>> {
        let status = if self.sealed {
            Some(Status::Revoked)
        } else if context == 0 {
            Some(Status::Invalid)
        } else if self.entries.get(context).is_some() {
            Some(Status::Conflict)
        } else {
            None
        };
        if let Some(status) = status {
            return Err(InstallFailure { status, prepared });
        }
        self.entries
            .insert_prepared(prepared.entry.with_key(context));
        Ok(())
    }
    pub fn get(&self, context: u64) -> Result<Snapshot<R>, Status> {
        let snapshot = self.entries.get(context).ok_or(Status::Permission)?;
        if self.sealed || !snapshot.active() {
            return Err(Status::Revoked);
        }
        Ok(snapshot.clone())
    }
    /// 接收准入只解析可信绑定和付款；权限/撤销仍由 Snapshot::check 在操作时复核。
    pub fn binding(&self, context: u64) -> Option<Snapshot<R>> {
        self.entries.get(context).cloned()
    }
    pub fn revoke(&mut self, context: u64) -> bool {
        let Some(snapshot) = self.entries.get(context) else {
            return false;
        };
        snapshot.state.active.set(false);
        true
    }
    pub fn remove(&mut self, context: u64) -> bool {
        // 最后 sender 消散只删除新请求索引；使用引用继续保有权限与费用。
        self.entries.remove(context).is_some()
    }
    pub fn seal(&mut self) {
        self.sealed = true;
        let mut after = None;
        while let Some((&context, state)) = self.entries.next_after(after.as_ref()) {
            state.state.active.set(false);
            after = Some(context);
        }
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub fn close(self) -> Result<(), Self> {
        if self.sealed && self.is_empty() {
            Ok(())
        } else {
            Err(self)
        }
    }
}
impl AuthorityTable {
    pub fn prepare_exact(
        &self,
        parent: &NameAuthority,
        name: &str,
        rights: PublishRights,
    ) -> Result<PreparedRole<PublishRights>, Status> {
        parent.check(None, PublishRights::DELEGATE)?;
        if !matches!(parent.scope(), Scope::Root)
            || rights.raw() & parent.rights().raw() != rights.raw()
        {
            return Err(Status::Permission);
        }
        if !valid_name(name) {
            return Err(Status::Invalid);
        }
        let mut owned = String::new();
        owned
            .try_reserve_exact(name.len())
            .map_err(|_| Status::Resource)?;
        owned.push_str(name);
        self.prepare(
            Scope::ExactName(owned),
            rights,
            parent.output(),
            parent.funding().clone(),
        )
    }
}
impl<R> Drop for RoleTable<R> {
    fn drop(&mut self) {
        if self.entries.is_empty() {
            // SAFETY: 此表未曾析构，空表无授权或来源 owner。
            unsafe {
                ManuallyDrop::drop(&mut self.entries);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libbudget::{Budget, Taxonomy};
    fn account() -> crate::Funding {
        let mut limits = [32; ServiceResource::COUNT + libexecution::ExecutionResource::COUNT];
        limits[ServiceResource::Bytes.slot()] = 128 * 1024;
        let budget = Budget::new(&limits, 1).unwrap();
        let account = budget.account(&limits).unwrap();
        let slots: [_; ServiceResource::COUNT] = core::array::from_fn(|n| budget.slot(n).unwrap());
        let domain = account.view(&slots).unwrap();
        let execution = account
            .view(&core::array::from_fn::<
                _,
                { libexecution::ExecutionResource::COUNT },
                _,
            >(|n| {
                budget.slot(ServiceResource::COUNT + n).unwrap()
            }))
            .unwrap();
        crate::Funding::new(&domain, &execution).unwrap()
    }
    #[test]
    fn prepared_and_snapshots_each_keep_the_same_capacity_responsibility() {
        // 检验纯授权容器预留；不模拟 kernel sender 或声称来源已登记。
        let table = AccessTable::new(1).unwrap();
        let account = account();
        let output = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
        let first = table
            .prepare(Scope::Root, AccessRights::ALL, output, account.clone())
            .unwrap();
        let use_ref = first.entry.value().clone();
        assert!(matches!(
            table.prepare(Scope::Root, AccessRights::ALL, output, account.clone()),
            Err(Status::Quota)
        ));
        drop(first);
        assert!(matches!(
            table.prepare(Scope::Root, AccessRights::ALL, output, account.clone()),
            Err(Status::Quota)
        ));
        drop(use_ref);
        assert!(
            table
                .prepare(Scope::Root, AccessRights::ALL, output, account)
                .is_ok()
        );
    }
    #[test]
    fn delegation_uses_exact_scope_and_cannot_add_publish_rights() {
        // 发行者冻结的 scope/output/付款沿用父引用；名字越界及权限放大在准入前拒绝。
        let parent = AuthorityTable::new(1).unwrap();
        let child = AuthorityTable::new(1).unwrap();
        let rights = PublishRights::CREATE | PublishRights::DELEGATE;
        let output = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
        let prepared = parent
            .prepare(Scope::Root, rights, output, account())
            .unwrap();
        let parent = prepared.entry.value().clone();
        assert!(matches!(
            child.prepare_exact(&parent, "fs", PublishRights::REPLACE),
            Err(Status::Permission)
        ));
        let role = child
            .prepare_exact(&parent, "fs", PublishRights::CREATE)
            .unwrap();
        let exact = role.entry.value();
        assert_eq!(exact.output(), output);
        assert_eq!(exact.account().account_id(), parent.account().account_id());
        assert_eq!(
            exact.check(Some("other"), PublishRights::CREATE),
            Err(Status::Permission)
        );
        assert!(exact.check(Some("fs"), PublishRights::CREATE).is_ok());
        assert!(matches!(
            child.prepare_exact(exact, "fs", PublishRights::CREATE),
            Err(Status::Permission)
        ));
    }
    #[test]
    fn default_control_transport_respects_complete_ceiling_and_never_adds_grant() {
        // 由控制角色合同判定：必需位必须齐全，可选 DUP 受完整上限约束，GRANT 需显式请求。
        let table = AuthorityTable::new(1).unwrap();
        let required = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
        for (ceiling, expected) in [
            (required, Ok(required)),
            (
                required | Rights::DUPLICATE | Rights::GRANT,
                Ok(required | Rights::DUPLICATE),
            ),
            (Rights::WRITE | Rights::WAIT, Err(Status::Permission)),
        ] {
            let role = table
                .prepare(Scope::Root, PublishRights::ALL, ceiling, account())
                .unwrap();
            assert_eq!(role.entry.value().control_transport(), expected);
        }
    }
    #[test]
    fn revoked_binding_preserves_payer_and_is_distinct_from_unknown_context() {
        // 撤销拒绝使用，不删除可信付款；缺索引与显式撤销必须保留不同错误。
        let mut table = AuthorityTable::new(1).unwrap();
        let payer = account();
        let role = table
            .prepare(
                Scope::Root,
                PublishRights::ALL,
                Rights::WRITE | Rights::WAIT | Rights::TRANSIT,
                payer.clone(),
            )
            .unwrap();
        assert!(table.install(role, 1).is_ok());
        assert!(table.revoke(1));
        assert!(matches!(table.get(1), Err(Status::Revoked)));
        let binding = table.binding(1).unwrap();
        assert_eq!(binding.account().account_id(), payer.domain().account_id());
        assert_eq!(
            binding.funding().execution().account_id(),
            payer.execution().account_id()
        );
        assert_eq!(
            binding.check(Some("name"), PublishRights::CREATE),
            Err(Status::Revoked)
        );
        assert!(table.binding(2).is_none());
        assert!(matches!(table.get(2), Err(Status::Permission)));
        table.seal();
        assert!(matches!(table.get(2), Err(Status::Permission)));
        assert!(table.remove(1));
    }
    #[test]
    fn removing_sender_index_preserves_admitted_use_but_explicit_revoke_does_not() {
        // 纯表契约验证：移除索引不是撤销；使用引用持费，显式撤销只影响同一授权。
        let mut table = AuthorityTable::new(1).unwrap();
        let output = Rights::WRITE | Rights::WAIT | Rights::TRANSIT;
        let prepared = table
            .prepare(Scope::Root, PublishRights::ALL, output, account())
            .unwrap();
        let admitted = prepared.entry.value().clone();
        let child = table.prepare_exact(&admitted, "child", PublishRights::CREATE);
        assert!(matches!(child, Err(Status::Quota)));
        assert!(table.install(prepared, 1).is_ok());
        assert!(table.remove(1));
        assert!(matches!(table.get(1), Err(Status::Permission)));
        assert!(admitted.check(Some("name"), PublishRights::CREATE).is_ok());
        assert!(matches!(
            table.prepare(Scope::Root, PublishRights::ALL, output, account()),
            Err(Status::Quota)
        ));
        drop(admitted);
        let prepared = table
            .prepare(Scope::Root, PublishRights::ALL, output, account())
            .unwrap();
        let admitted = prepared.entry.value().clone();
        assert!(table.install(prepared, 2).is_ok());
        let children = AuthorityTable::new(1).unwrap();
        let child = children
            .prepare_exact(&admitted, "child", PublishRights::CREATE)
            .unwrap();
        assert!(table.revoke(2));
        assert_eq!(
            admitted.check(Some("name"), PublishRights::CREATE),
            Err(Status::Revoked)
        );
        assert!(
            child
                .entry
                .value()
                .check(Some("child"), PublishRights::CREATE)
                .is_ok()
        );
        assert!(table.remove(2));
    }
}
