//! 注册入口的 sender-context authority；名称 scope 与付款视图在发行时冻结。

use crate::{
    protocol::{AuthorityInfo, Scope, valid_name},
    resource::ServiceResource,
};
use alloc::{string::String, sync::Arc};
use erhino_shared::call::SystemCallError;
use libbudget::{AccountView, Charge};
use metadata_admission::{Counter, Permit};
use ordered_table::{InsertError, OrderedTable, PreparedEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorityError {
    Closed,
    Invalid,
    NotFound,
    Permission,
    Exists,
    Resource(SystemCallError),
}

impl From<SystemCallError> for AuthorityError {
    fn from(error: SystemCallError) -> Self {
        Self::Resource(error)
    }
}

enum AuthorityScope {
    Root,
    ExactName(String),
}

struct Authority {
    identity: u64,
    scope: AuthorityScope,
    account: AccountView<ServiceResource>,
    _slot: Permit,
    _authority_charge: Charge,
    _bytes_charge: Charge,
}

impl Authority {
    fn info(&self) -> AuthorityInfo {
        AuthorityInfo {
            identity: self.identity,
            scope: match self.scope {
                AuthorityScope::Root => Scope::Root,
                AuthorityScope::ExactName(_) => Scope::ExactName,
            },
        }
    }

    fn permits_name(&self, name: &str) -> bool {
        match &self.scope {
            AuthorityScope::Root => true,
            AuthorityScope::ExactName(expected) => expected == name,
        }
    }
}

pub struct PreparedAuthority {
    entry: PreparedEntry<Authority>,
}

pub struct InstallFailure {
    pub error: AuthorityError,
    pub prepared: PreparedAuthority,
}

/// 注册 authority 的唯一索引。key 必须取 Mailbox sender context identity。
pub struct AuthorityTable {
    entries: OrderedTable<Authority>,
    slots: Arc<Counter>,
    sealed: bool,
}

impl AuthorityTable {
    pub fn new(limit: usize) -> Result<Self, AuthorityError> {
        if limit == 0 {
            return Err(AuthorityError::Invalid);
        }
        Ok(Self {
            entries: OrderedTable::new(limit),
            slots: Arc::try_new(Counter::new(limit))
                .map_err(|_| AuthorityError::Resource(SystemCallError::OutOfMemory))?,
            sealed: false,
        })
    }

    /// 准备一个注册根。调用方应先登记新 sender 的 Lifetime，再以其 context identity 安装。
    pub fn prepare_root(
        &self,
        account: AccountView<ServiceResource>,
    ) -> Result<PreparedAuthority, AuthorityError> {
        self.prepare(AuthorityScope::Root, account)
    }

    /// 从注册根准备不可继续转委派的 exact-name authority。
    pub fn prepare_exact(
        &self,
        parent_identity: u64,
        name: &str,
    ) -> Result<PreparedAuthority, AuthorityError> {
        if !valid_name(name) {
            return Err(AuthorityError::Invalid);
        }
        let parent = self
            .entries
            .get(parent_identity)
            .ok_or(AuthorityError::NotFound)?;
        if !matches!(parent.scope, AuthorityScope::Root) {
            return Err(AuthorityError::Permission);
        }
        let mut owned = String::new();
        owned
            .try_reserve_exact(name.len())
            .map_err(|_| AuthorityError::Resource(SystemCallError::OutOfMemory))?;
        owned.push_str(name);
        self.prepare(AuthorityScope::ExactName(owned), parent.account.clone())
    }

    fn prepare(
        &self,
        scope: AuthorityScope,
        account: AccountView<ServiceResource>,
    ) -> Result<PreparedAuthority, AuthorityError> {
        if self.sealed {
            return Err(AuthorityError::Closed);
        }
        let slot = Counter::try_acquire(&self.slots)
            .map_err(|_| AuthorityError::Resource(SystemCallError::QuotaExceeded))?;
        let authority_charge = account.acquire(ServiceResource::Authority, 1)?;
        let name_bytes = match &scope {
            AuthorityScope::Root => 0,
            AuthorityScope::ExactName(name) => name.len(),
        };
        let bytes = name_bytes
            .checked_add(PreparedEntry::<Authority>::allocation_bytes())
            .ok_or(AuthorityError::Resource(SystemCallError::ReachLimit))?;
        let bytes_charge = account.acquire(ServiceResource::Bytes, bytes)?;
        let authority = Authority {
            identity: 0,
            scope,
            account,
            _slot: slot,
            _authority_charge: authority_charge,
            _bytes_charge: bytes_charge,
        };
        let entry = self
            .entries
            .prepare_insert_candidate(0, authority)
            .map_err(map_insert_error)?;
        Ok(PreparedAuthority { entry })
    }

    /// 安装点是 authority 可被请求使用的线性化点。
    pub fn install(
        &mut self,
        mut prepared: PreparedAuthority,
        identity: u64,
    ) -> Result<AuthorityInfo, InstallFailure> {
        let error = if self.sealed {
            Some(AuthorityError::Closed)
        } else if identity == 0 {
            Some(AuthorityError::Invalid)
        } else if self.entries.get(identity).is_some() {
            Some(AuthorityError::Exists)
        } else {
            None
        };
        if let Some(error) = error {
            return Err(InstallFailure { error, prepared });
        }
        prepared.entry.value_mut().identity = identity;
        prepared.entry = prepared.entry.with_key(identity);
        self.entries.insert_prepared(prepared.entry);
        Ok(self
            .entries
            .get(identity)
            .expect("installed registration authority missing")
            .info())
    }

    pub fn info(&self, identity: u64) -> Result<AuthorityInfo, AuthorityError> {
        self.entries
            .get(identity)
            .map(Authority::info)
            .ok_or(AuthorityError::NotFound)
    }

    /// Register、QueryName 与条件 Withdraw 共用该检查；返回的账户由新注册继承。
    pub fn authorize_name(
        &self,
        identity: u64,
        name: &str,
    ) -> Result<AccountView<ServiceResource>, AuthorityError> {
        if self.sealed {
            return Err(AuthorityError::Closed);
        }
        if !valid_name(name) {
            return Err(AuthorityError::Invalid);
        }
        let authority = self.entries.get(identity).ok_or(AuthorityError::NotFound)?;
        if !authority.permits_name(name) {
            return Err(AuthorityError::Permission);
        }
        Ok(authority.account.clone())
    }

    pub fn remove(&mut self, identity: u64) -> Result<(), AuthorityError> {
        self.entries
            .remove(identity)
            .map(drop)
            .ok_or(AuthorityError::NotFound)
    }

    pub fn seal(&mut self) {
        self.sealed = true;
    }

    pub fn is_sealed(&self) -> bool {
        self.sealed
    }

    /// Lifetime source 已全部撤销后，由退休任务有界释放 authority 元数据。
    pub fn retire_step(&mut self, budget: usize) -> usize {
        if !self.sealed {
            return 0;
        }
        let mut retired = 0;
        while retired < budget && self.entries.pop_first().is_some() {
            retired += 1;
        }
        retired
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn map_insert_error(error: InsertError<Authority>) -> AuthorityError {
    match error {
        InsertError::Limit(_) => AuthorityError::Resource(SystemCallError::QuotaExceeded),
        InsertError::Allocation(_) => AuthorityError::Resource(SystemCallError::OutOfMemory),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libbudget::{Budget, Taxonomy};

    fn account() -> AccountView<ServiceResource> {
        let layout = [0, 1, 2, 3];
        let limits = [4096; ServiceResource::COUNT];
        let budget = Budget::new(&limits, 1).unwrap();
        let account = budget.account(&limits).unwrap();
        let binding = layout.map(|index| budget.slot(index).unwrap());
        account.view(&binding).unwrap()
    }

    fn install(
        table: &mut AuthorityTable,
        prepared: PreparedAuthority,
        identity: u64,
    ) -> AuthorityInfo {
        match table.install(prepared, identity) {
            Ok(info) => info,
            Err(failure) => panic!("authority installation failed: {:?}", failure.error),
        }
    }

    #[test]
    fn exact_name_authority_inherits_payment_and_cannot_delegate() {
        let mut table = AuthorityTable::new(4).unwrap();
        let root = table.prepare_root(account()).unwrap();
        assert_eq!(install(&mut table, root, 11).scope, Scope::Root);
        let exact = table.prepare_exact(11, "fs.secondary").unwrap();
        assert_eq!(install(&mut table, exact, 12).scope, Scope::ExactName);
        assert_eq!(table.retire_step(4), 0);
        assert!(table.authorize_name(12, "fs.secondary").is_ok());
        assert!(table.authorize_name(12, "fs.secondary").is_ok());
        assert!(matches!(
            table.authorize_name(12, "fs.other"),
            Err(AuthorityError::Permission)
        ));
        assert!(matches!(
            table.prepare_exact(12, "fs.child"),
            Err(AuthorityError::Permission)
        ));
    }

    #[test]
    fn installation_is_conditional_and_seal_blocks_new_use() {
        let mut table = AuthorityTable::new(3).unwrap();
        let first = table.prepare_root(account()).unwrap();
        install(&mut table, first, 21);
        let duplicate = table.prepare_root(account()).unwrap();
        let failure = table.install(duplicate, 21).unwrap_err();
        assert_eq!(failure.error, AuthorityError::Exists);
        drop(failure.prepared);
        table.seal();
        assert!(matches!(
            table.authorize_name(21, "service"),
            Err(AuthorityError::Closed)
        ));
        assert_eq!(table.retire_step(1), 1);
        assert!(table.is_empty());
    }
}
