//! 资源账户只由服务授权政策建立，派生 grant 共享来源账户而不重置额度。

use alloc::sync::Arc;
use erhino_shared::call::SystemCallError;
use metadata_admission::{Counter, Permit, SponsoredPermit};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub enum Resource {
    Account,
    Task,
    Node,
    Bytes,
    Grant,
    Request,
    Outbox,
    Watch,
    Offer,
    Stream,
    WaitSource,
    ServiceRecord,
}

impl Resource {
    pub const COUNT: usize = Self::ServiceRecord as usize + 1;
}

pub type Limits = [usize; Resource::COUNT];
type Counters = [Option<Arc<Counter>>; Resource::COUNT];
static NEXT_ACCOUNT: monotonic_id::AtomicId64 = monotonic_id::AtomicId64::new(1);

fn counters(limits: Limits) -> Result<Counters, SystemCallError> {
    let mut counters: Counters = core::array::from_fn(|_| None);
    for (counter, limit) in counters.iter_mut().zip(limits) {
        if limit != 0 {
            *counter =
                Some(Arc::try_new(Counter::new(limit)).map_err(|_| SystemCallError::OutOfMemory)?);
        }
    }
    Ok(counters)
}

fn usage(counters: &Counters, resource: Resource) -> (usize, usize) {
    counters[resource as usize]
        .as_ref()
        .map_or((0, 0), |counter| (counter.used(), counter.limit()))
}

pub struct Budget {
    counters: Counters,
}

impl Budget {
    pub fn new(limits: Limits) -> Result<Arc<Self>, SystemCallError> {
        Arc::try_new(Self {
            counters: counters(limits)?,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }
    pub fn usage(&self, resource: Resource) -> (usize, usize) {
        usage(&self.counters, resource)
    }
    pub fn account(self: &Arc<Self>, limits: Limits) -> Result<Arc<Account>, SystemCallError> {
        let counter = self.counters[Resource::Account as usize]
            .as_ref()
            .ok_or(SystemCallError::QuotaExceeded)?;
        let permit = Counter::try_acquire(counter).map_err(|_| SystemCallError::QuotaExceeded)?;
        let id = NEXT_ACCOUNT.allocate().ok_or(SystemCallError::ReachLimit)?;
        Arc::try_new(Account {
            id,
            service: self.clone(),
            counters: counters(limits)?,
            _permit: permit,
        })
        .map_err(|_| SystemCallError::OutOfMemory)
    }
}

pub struct Account {
    id: u64,
    service: Arc<Budget>,
    counters: Counters,
    _permit: Permit,
}

impl Account {
    pub fn id(&self) -> u64 {
        self.id
    }
    pub fn usage(&self, resource: Resource) -> (usize, usize) {
        usage(&self.counters, resource)
    }
    pub fn acquire(
        self: &Arc<Self>,
        resource: Resource,
        units: usize,
    ) -> Result<Charge, SystemCallError> {
        let permit = if units == 0 {
            None
        } else {
            let local = self.counters[resource as usize]
                .as_ref()
                .ok_or(SystemCallError::QuotaExceeded)?;
            let global = self.service.counters[resource as usize]
                .as_ref()
                .ok_or(SystemCallError::QuotaExceeded)?;
            Some(
                SponsoredPermit::try_acquire_many(self, global, local, units)
                    .map_err(|_| SystemCallError::QuotaExceeded)?,
            )
        };
        Ok(Charge {
            account: self.id,
            resource,
            units,
            _permit: permit,
        })
    }
}

pub struct Charge {
    account: u64,
    resource: Resource,
    units: usize,
    _permit: Option<SponsoredPermit<Account>>,
}

impl Charge {
    pub fn account_id(&self) -> u64 {
        self.account
    }
    pub fn resource(&self) -> Resource {
        self.resource
    }
    pub fn units(&self) -> usize {
        self.units
    }
}

impl core::fmt::Debug for Charge {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Charge")
            .field("account", &self.account)
            .field("resource", &self.resource)
            .field("units", &self.units)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_service_limits_refund_together() {
        let mut limits = [0; Resource::COUNT];
        limits[Resource::Account as usize] = 2;
        limits[Resource::Bytes as usize] = 10;
        let budget = Budget::new(limits).unwrap();
        limits[Resource::Bytes as usize] = 8;
        let first = budget.account(limits).unwrap();
        let second = budget.account(limits).unwrap();
        let charge = first.acquire(Resource::Bytes, 7).unwrap();
        assert_eq!(first.usage(Resource::Bytes), (7, 8));
        assert!(matches!(
            first.acquire(Resource::Bytes, 2),
            Err(SystemCallError::QuotaExceeded)
        ));
        assert!(matches!(
            second.acquire(Resource::Bytes, 4),
            Err(SystemCallError::QuotaExceeded)
        ));
        drop(charge);
        assert_eq!(budget.usage(Resource::Bytes), (0, 10));
        assert!(second.acquire(Resource::Bytes, 8).is_ok());
    }

    #[test]
    fn zero_limit_denies_and_zero_charge_needs_no_slot() {
        let mut limits = [0; Resource::COUNT];
        limits[Resource::Account as usize] = 1;
        let budget = Budget::new(limits).unwrap();
        let account = budget.account([0; Resource::COUNT]).unwrap();
        assert!(matches!(
            account.acquire(Resource::Task, 1),
            Err(SystemCallError::QuotaExceeded)
        ));
        assert_eq!(account.acquire(Resource::Bytes, 0).unwrap().units(), 0);
    }
}
