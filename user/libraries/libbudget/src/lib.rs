#![no_std]
#![feature(allocator_api)]

//! 用户态公共记账：唯一付款账户、领域计费视图与随 owner 持有的扣账。
//!
//! [`AccountView`] 只为同一 [`Account`] 解释领域资源，不创建账户或计数器；
//! [`Charge`] 固定实际退款目标并保活付款来源。底层固定容量原语由
//! `metadata_admission` 唯一提供。

extern crate alloc;

use alloc::{sync::Arc, vec::Vec};
use core::marker::PhantomData;
use erhino_shared::call::SystemCallError;
use metadata_admission::{Counter, Permit, SponsoredPermit};

/// 领域资源分类：把领域枚举映射到领域视图中的有限槽位。
pub trait Taxonomy {
    const COUNT: usize;
    fn slot(self) -> usize;
}

static NEXT_BUDGET: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);
static NEXT_ACCOUNT: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

fn counters(limits: &[usize]) -> Result<Vec<Option<Arc<Counter>>>, SystemCallError> {
    let mut counters = Vec::new();
    counters
        .try_reserve_exact(limits.len())
        .map_err(|_| SystemCallError::OutOfMemory)?;
    for limit in limits {
        let counter = (*limit != 0)
            .then(|| Arc::try_new(Counter::new(*limit)).map_err(|_| SystemCallError::OutOfMemory));
        counters.push(match counter {
            Some(counter) => Some(counter?),
            None => None,
        });
    }
    Ok(counters)
}

fn usage(counters: &[Option<Arc<Counter>>], slot: usize) -> (usize, usize) {
    counters
        .get(slot)
        .and_then(|counter| counter.as_ref())
        .map_or((0, 0), |counter| (counter.used(), counter.limit()))
}

/// 由具体 Budget 签发的布局槽，不能跨布局绑定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetSlot {
    budget: u64,
    index: usize,
}

/// 一个装配域的全局计费布局。
pub struct Budget {
    id: u64,
    counters: Vec<Option<Arc<Counter>>>,
    accounts: Option<Arc<Counter>>,
}

impl Budget {
    /// `limits` 定义实际计费槽；`account_limit` 是结构性账户数限额。
    pub fn new(limits: &[usize], account_limit: usize) -> Result<Arc<Self>, SystemCallError> {
        let accounts = (account_limit != 0).then(|| {
            Arc::try_new(Counter::new(account_limit)).map_err(|_| SystemCallError::OutOfMemory)
        });
        let accounts = match accounts {
            Some(counter) => Some(counter?),
            None => None,
        };
        let id = NEXT_BUDGET.allocate().ok_or(SystemCallError::ReachLimit)?;
        Arc::try_new(Self {
            id,
            counters: counters(limits)?,
            accounts,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub fn layout_len(&self) -> usize {
        self.counters.len()
    }

    pub fn slot(&self, index: usize) -> Option<BudgetSlot> {
        (index < self.counters.len()).then_some(BudgetSlot {
            budget: self.id,
            index,
        })
    }

    /// 建立唯一付款账户；本地限额必须与预算布局逐槽对应。
    pub fn account(self: &Arc<Self>, limits: &[usize]) -> Result<Arc<Account>, SystemCallError> {
        if limits.len() != self.counters.len() {
            return Err(SystemCallError::IllegalArgument);
        }
        let counter = self
            .accounts
            .as_ref()
            .ok_or(SystemCallError::QuotaExceeded)?;
        let permit = Counter::try_acquire(counter).map_err(|_| SystemCallError::QuotaExceeded)?;
        let id = NEXT_ACCOUNT.allocate().ok_or(SystemCallError::ReachLimit)?;
        Arc::try_new(Account {
            id,
            budget: self.clone(),
            counters: counters(limits)?,
            _permit: permit,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }
}

/// 唯一付款身份；领域代码只通过 [`AccountView`] 扣账。
pub struct Account {
    id: u64,
    budget: Arc<Budget>,
    counters: Vec<Option<Arc<Counter>>>,
    _permit: Permit,
}

impl Account {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// 将领域槽绑定到本账户布局中的实际计费槽，不创建新的额度。
    pub fn view<K: Taxonomy>(
        self: &Arc<Self>,
        binding: &[BudgetSlot],
    ) -> Result<AccountView<K>, SystemCallError> {
        if binding.len() != K::COUNT
            || binding
                .iter()
                .any(|slot| slot.budget != self.budget.id || slot.index >= self.counters.len())
        {
            return Err(SystemCallError::IllegalArgument);
        }
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(binding.len())
            .map_err(|_| SystemCallError::OutOfMemory)?;
        slots.extend(binding.iter().map(|slot| slot.index));
        let slots = Arc::try_new(slots).map_err(|_| SystemCallError::OutOfMemory)?;
        Ok(AccountView {
            account: self.clone(),
            slots,
            _kind: PhantomData,
        })
    }

    fn acquire(
        &self,
        sponsor: &Arc<Self>,
        slot: usize,
        units: usize,
    ) -> Result<Charge, SystemCallError> {
        let backing = if units == 0 {
            ChargeBacking::Zero(sponsor.clone())
        } else {
            let local = self
                .counters
                .get(slot)
                .and_then(|counter| counter.as_ref())
                .ok_or(SystemCallError::QuotaExceeded)?;
            let global = self
                .budget
                .counters
                .get(slot)
                .and_then(|counter| counter.as_ref())
                .ok_or(SystemCallError::QuotaExceeded)?;
            ChargeBacking::Metered(
                SponsoredPermit::try_acquire_many(sponsor, global, local, units)
                    .map_err(|_| SystemCallError::QuotaExceeded)?,
            )
        };
        Ok(Charge {
            account: self.id,
            slot,
            units,
            backing,
        })
    }
}

/// 同一付款账户对领域资源分类的不可变解释。
pub struct AccountView<K: Taxonomy> {
    account: Arc<Account>,
    slots: Arc<Vec<usize>>,
    _kind: PhantomData<fn(K)>,
}

impl<K: Taxonomy> Clone for AccountView<K> {
    fn clone(&self) -> Self {
        Self {
            account: self.account.clone(),
            slots: self.slots.clone(),
            _kind: PhantomData,
        }
    }
}

impl<K: Taxonomy> AccountView<K> {
    pub fn account_id(&self) -> u64 {
        self.account.id()
    }

    pub fn usage(&self, kind: K) -> (usize, usize) {
        usage(&self.account.counters, self.actual_slot(kind))
    }

    pub fn budget_usage(&self, kind: K) -> (usize, usize) {
        usage(&self.account.budget.counters, self.actual_slot(kind))
    }

    pub fn acquire(&self, kind: K, units: usize) -> Result<Charge, SystemCallError> {
        self.account
            .acquire(&self.account, self.actual_slot(kind), units)
    }

    fn actual_slot(&self, kind: K) -> usize {
        *self
            .slots
            .get(kind.slot())
            .expect("domain resource slot outside declared taxonomy")
    }
}

enum ChargeBacking {
    Zero(Arc<Account>),
    Metered(SponsoredPermit<Account>),
}

/// 实际扣账 owner；析构退还全局与账户两层额度。
pub struct Charge {
    account: u64,
    slot: usize,
    units: usize,
    backing: ChargeBacking,
}

impl Charge {
    pub fn account_id(&self) -> u64 {
        self.account
    }

    pub fn slot(&self) -> usize {
        self.slot
    }

    pub fn units(&self) -> usize {
        self.units
    }

    pub fn shrink_to(&mut self, units: usize) {
        assert!(units <= self.units, "budget charge cannot grow");
        match &mut self.backing {
            ChargeBacking::Zero(_account) => {
                assert_eq!(units, 0, "zero budget charge cannot retain units");
            }
            ChargeBacking::Metered(permit) => permit.shrink_to(units),
        }
        self.units = units;
    }
}

impl core::fmt::Debug for Charge {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Charge")
            .field("account", &self.account)
            .field("slot", &self.slot)
            .field("units", &self.units)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    enum StorageResource {
        Objects,
        Bytes,
    }

    impl Taxonomy for StorageResource {
        const COUNT: usize = 2;
        fn slot(self) -> usize {
            match self {
                Self::Objects => 0,
                Self::Bytes => 1,
            }
        }
    }

    #[derive(Clone, Copy)]
    enum ExecutionResource {
        InputBytes,
    }

    impl Taxonomy for ExecutionResource {
        const COUNT: usize = 1;
        fn slot(self) -> usize {
            0
        }
    }

    #[test]
    fn views_share_account_identity_and_actual_limit() {
        let budget = Budget::new(&[4, 10], 1).unwrap();
        let account = budget.account(&[4, 10]).unwrap();
        let storage = account
            .view::<StorageResource>(&[budget.slot(0).unwrap(), budget.slot(1).unwrap()])
            .unwrap();
        let execution = account
            .view::<ExecutionResource>(&[budget.slot(1).unwrap()])
            .unwrap();
        assert_eq!(storage.account_id(), execution.account_id());
        let _stored = storage.acquire(StorageResource::Bytes, 7).unwrap();
        assert!(matches!(
            execution.acquire(ExecutionResource::InputBytes, 4),
            Err(SystemCallError::QuotaExceeded)
        ));
        assert_eq!(execution.usage(ExecutionResource::InputBytes), (7, 10));
    }

    #[test]
    fn cloned_view_does_not_open_an_account_or_reset_limits() {
        let budget = Budget::new(&[1, 8], 1).unwrap();
        let account = budget.account(&[1, 8]).unwrap();
        let view = account
            .view::<StorageResource>(&[budget.slot(0).unwrap(), budget.slot(1).unwrap()])
            .unwrap();
        let alias = view.clone();
        let _charge = view.acquire(StorageResource::Objects, 1).unwrap();
        assert!(matches!(
            alias.acquire(StorageResource::Objects, 1),
            Err(SystemCallError::QuotaExceeded)
        ));
        assert!(matches!(
            budget.account(&[1, 8]),
            Err(SystemCallError::QuotaExceeded)
        ));
    }

    #[test]
    fn failed_second_layer_acquire_refunds_local_limit() {
        let budget = Budget::new(&[1], 2).unwrap();
        let first = budget.account(&[1]).unwrap();
        let second = budget.account(&[1]).unwrap();
        let binding = [budget.slot(0).unwrap()];
        let first_view = first.view::<ExecutionResource>(&binding).unwrap();
        let second_view = second.view::<ExecutionResource>(&binding).unwrap();
        let charge = first_view
            .acquire(ExecutionResource::InputBytes, 1)
            .unwrap();
        assert!(matches!(
            second_view.acquire(ExecutionResource::InputBytes, 1),
            Err(SystemCallError::QuotaExceeded)
        ));
        drop(charge);
        assert!(
            second_view
                .acquire(ExecutionResource::InputBytes, 1)
                .is_ok()
        );
    }

    #[test]
    fn shrink_refunds_shared_and_local_limits() {
        let budget = Budget::new(&[8], 1).unwrap();
        let account = budget.account(&[8]).unwrap();
        let view = account
            .view::<ExecutionResource>(&[budget.slot(0).unwrap()])
            .unwrap();
        let mut charge = view.acquire(ExecutionResource::InputBytes, 7).unwrap();
        charge.shrink_to(2);
        assert_eq!(view.usage(ExecutionResource::InputBytes), (2, 8));
        assert_eq!(view.budget_usage(ExecutionResource::InputBytes), (2, 8));
    }

    #[test]
    fn zero_charge_keeps_account_slot_until_resource_release() {
        let budget = Budget::new(&[1], 1).unwrap();
        let charge = {
            let account = budget.account(&[1]).unwrap();
            let view = account
                .view::<ExecutionResource>(&[budget.slot(0).unwrap()])
                .unwrap();
            view.acquire(ExecutionResource::InputBytes, 0).unwrap()
        };
        assert!(matches!(
            budget.account(&[1]),
            Err(SystemCallError::QuotaExceeded)
        ));
        drop(charge);
        assert!(budget.account(&[1]).is_ok());
    }

    #[test]
    fn invalid_layout_binding_is_rejected() {
        let budget = Budget::new(&[1], 1).unwrap();
        let account = budget.account(&[1]).unwrap();
        assert!(matches!(
            account.view::<StorageResource>(&[budget.slot(0).unwrap()]),
            Err(SystemCallError::IllegalArgument)
        ));
        let other = Budget::new(&[1], 1).unwrap();
        assert!(matches!(
            account.view::<ExecutionResource>(&[other.slot(0).unwrap()]),
            Err(SystemCallError::IllegalArgument)
        ));
    }
}
