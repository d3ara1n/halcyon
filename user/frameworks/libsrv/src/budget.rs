//! 资源账户只由服务授权政策建立，派生 grant 共享来源账户而不重置额度。
//! 领域资源分类由领域经 [`Taxonomy`] 自行定义，账户不解释分类含义。

use alloc::{sync::Arc, vec::Vec};
use core::marker::PhantomData;
use erhino_shared::call::SystemCallError;
use metadata_admission::{Counter, Permit, SponsoredPermit};

/// 领域资源分类：把领域枚举映射到有限槽位。
pub trait Taxonomy {
    /// 分类总数；限额数组长度必须等于它。
    const COUNT: usize;
    /// 本分类的数组槽位。
    fn slot(self) -> usize;
}

/// 无领域资源的服务（pm、init 与验收负载）使用的最小分类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreResource {
    Task,
    InputBytes,
}

impl Taxonomy for CoreResource {
    const COUNT: usize = 2;
    fn slot(self) -> usize {
        match self {
            Self::Task => 0,
            Self::InputBytes => 1,
        }
    }
}

impl CoreResource {
    /// 执行核心在 [`CoreResource`] 上的固定槽位。
    pub const EXECUTION_SLOTS: ExecutionSlots = ExecutionSlots {
        task: 0,
        input_bytes: 1,
    };
}

/// 执行核心向账户借用的两个固定槽位：任务数与每来源输入记录字节。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionSlots {
    pub task: usize,
    pub input_bytes: usize,
}

static NEXT_ACCOUNT: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

fn counters<K: Taxonomy>(limits: &[usize]) -> Result<Vec<Option<Arc<Counter>>>, SystemCallError> {
    if limits.len() != K::COUNT {
        return Err(SystemCallError::IllegalArgument);
    }
    let mut counters = Vec::new();
    counters
        .try_reserve_exact(K::COUNT)
        .map_err(|_| SystemCallError::OutOfMemory)?;
    for limit in limits {
        let counter = (*limit != 0).then(|| {
            Arc::try_new(Counter::new(*limit)).map_err(|_| SystemCallError::OutOfMemory)
        });
        let counter = match counter {
            Some(counter) => Some(counter?),
            None => None,
        };
        counters.push(counter);
    }
    Ok(counters)
}

fn usage(counters: &[Option<Arc<Counter>>], slot: usize) -> (usize, usize) {
    counters
        .get(slot)
        .and_then(|counter| counter.as_ref())
        .map_or((0, 0), |counter| (counter.used(), counter.limit()))
}

pub struct Budget<K: Taxonomy> {
    counters: Vec<Option<Arc<Counter>>>,
    accounts: Option<Arc<Counter>>,
    _kind: PhantomData<fn(K)>,
}

impl<K: Taxonomy> Budget<K> {
    /// `account_limit` 是结构性账户数限额，不属于领域分类。
    pub fn new(limits: &[usize], account_limit: usize) -> Result<Arc<Self>, SystemCallError> {
        let accounts = (account_limit != 0).then(|| {
            Arc::try_new(Counter::new(account_limit)).map_err(|_| SystemCallError::OutOfMemory)
        });
        let accounts = match accounts {
            Some(counter) => Some(counter?),
            None => None,
        };
        Arc::try_new(Self {
            counters: counters::<K>(limits)?,
            accounts,
            _kind: PhantomData,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }

    pub fn usage(&self, kind: K) -> (usize, usize) {
        usage(&self.counters, kind.slot())
    }

    pub fn account(self: &Arc<Self>, limits: &[usize]) -> Result<Arc<Account<K>>, SystemCallError> {
        let counter = self
            .accounts
            .as_ref()
            .ok_or(SystemCallError::QuotaExceeded)?;
        let permit =
            Counter::try_acquire(counter).map_err(|_| SystemCallError::QuotaExceeded)?;
        let id = NEXT_ACCOUNT.allocate().ok_or(SystemCallError::ReachLimit)?;
        Arc::try_new(Account {
            id,
            service: self.clone(),
            counters: counters::<K>(limits)?,
            _permit: permit,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }
}

pub struct Account<K: Taxonomy> {
    id: u64,
    service: Arc<Budget<K>>,
    counters: Vec<Option<Arc<Counter>>>,
    _permit: Permit,
}

impl<K: Taxonomy> Account<K> {
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn usage(&self, kind: K) -> (usize, usize) {
        usage(&self.counters, kind.slot())
    }
    /// 领域代码按分类取得额度；失败不消费任何一层限额。
    pub fn acquire(self: &Arc<Self>, kind: K, units: usize) -> Result<Charge<K>, SystemCallError> {
        self.acquire_at(kind.slot(), units)
    }
    /// 执行核心按 [`Taxonomy`] 槽位取得额度；槽位合法性由调用侧类型约束。
    pub(crate) fn acquire_at(
        self: &Arc<Self>,
        slot: usize,
        units: usize,
    ) -> Result<Charge<K>, SystemCallError> {
        let permit = if units == 0 {
            None
        } else {
            let local = self
                .counters
                .get(slot)
                .and_then(|counter| counter.as_ref())
                .ok_or(SystemCallError::QuotaExceeded)?;
            let global = self
                .service
                .counters
                .get(slot)
                .and_then(|counter| counter.as_ref())
                .ok_or(SystemCallError::QuotaExceeded)?;
            Some(
                SponsoredPermit::try_acquire_many(self, global, local, units)
                    .map_err(|_| SystemCallError::QuotaExceeded)?,
            )
        };
        Ok(Charge {
            account: self.id,
            slot,
            units,
            _permit: permit,
        })
    }
}

pub struct Charge<K: Taxonomy> {
    account: u64,
    slot: usize,
    units: usize,
    _permit: Option<SponsoredPermit<Account<K>>>,
}

impl<K: Taxonomy> Charge<K> {
    pub fn account_id(&self) -> u64 {
        self.account
    }
    pub fn slot(&self) -> usize {
        self.slot
    }
    pub fn units(&self) -> usize {
        self.units
    }
}

impl<K: Taxonomy> core::fmt::Debug for Charge<K> {
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

    fn limits() -> [usize; CoreResource::COUNT] {
        [usize::MAX, 10]
    }

    #[test]
    fn local_and_service_limits_refund_together() {
        let budget = Budget::<CoreResource>::new(&limits(), 2).unwrap();
        let first = budget.account(&[usize::MAX, 8]).unwrap();
        let second = budget.account(&[usize::MAX, 8]).unwrap();
        let charge = first.acquire(CoreResource::InputBytes, 7).unwrap();
        assert_eq!(first.usage(CoreResource::InputBytes), (7, 8));
        assert!(matches!(
            first.acquire(CoreResource::InputBytes, 2),
            Err(SystemCallError::QuotaExceeded)
        ));
        assert!(matches!(
            second.acquire(CoreResource::InputBytes, 4),
            Err(SystemCallError::QuotaExceeded)
        ));
        drop(charge);
        assert_eq!(budget.usage(CoreResource::InputBytes), (0, 10));
        assert!(second.acquire(CoreResource::InputBytes, 8).is_ok());
    }

    #[test]
    fn account_limit_is_structural() {
        let budget = Budget::<CoreResource>::new(&limits(), 1).unwrap();
        let first = budget.account(&[0; CoreResource::COUNT]).unwrap();
        assert!(matches!(
            budget.account(&[0; CoreResource::COUNT]),
            Err(SystemCallError::QuotaExceeded)
        ));
        drop(first);
        assert!(budget.account(&[0; CoreResource::COUNT]).is_ok());
    }

    #[test]
    fn zero_limit_denies_and_zero_charge_needs_no_slot() {
        let budget = Budget::<CoreResource>::new(&[0; CoreResource::COUNT], 1).unwrap();
        let account = budget.account(&[0; CoreResource::COUNT]).unwrap();
        assert!(matches!(
            account.acquire(CoreResource::Task, 1),
            Err(SystemCallError::QuotaExceeded)
        ));
        assert_eq!(
            account
                .acquire(CoreResource::InputBytes, 0)
                .unwrap()
                .units(),
            0
        );
    }

    #[test]
    fn limit_length_mismatch_is_rejected() {
        assert!(matches!(
            Budget::<CoreResource>::new(&[1], 1),
            Err(SystemCallError::IllegalArgument)
        ));
    }
}
