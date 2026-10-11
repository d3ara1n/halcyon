//! 可信发行边界冻结的同账户领域/执行付款配对；线上正文不表达付款。
use crate::resource::ServiceResource;
#[cfg(any(test, target_arch = "riscv64"))]
use erhino_shared::call::SystemCallError;
use libbudget::AccountView;
use libexecution::ExecutionResource;

/// 回复责任结束时释放；长期控制任务的自身存储另持 Charge。
#[cfg(any(test, target_arch = "riscv64"))]
pub(crate) struct RequestFees {
    _request: libbudget::Charge,
    _outbox: libbudget::Charge,
    _bytes: libbudget::Charge,
    _source: libbudget::Charge,
}

#[derive(Clone)]
pub struct Funding {
    domain: AccountView<ServiceResource>,
    execution: AccountView<ExecutionResource>,
}
impl Funding {
    #[cfg(any(test, target_arch = "riscv64"))]
    pub(crate) fn prepare_request<T>(
        &self,
    ) -> Result<(RequestFees, libbudget::Charge), SystemCallError> {
        Ok((
            RequestFees {
                _request: self.domain.acquire(ServiceResource::Request, 1)?,
                _outbox: self.domain.acquire(ServiceResource::Outbox, 1)?,
                _bytes: self.domain.acquire(
                    ServiceResource::Bytes,
                    erhino_shared::message::PAYLOAD_MAX * 2,
                )?,
                _source: self.domain.acquire(ServiceResource::WaitSource, 1)?,
            },
            self.domain
                .acquire(ServiceResource::Bytes, core::mem::size_of::<T>())?,
        ))
    }

    #[cfg(any(test, target_arch = "riscv64"))]
    pub(crate) fn new(
        domain: &AccountView<ServiceResource>,
        execution: &AccountView<ExecutionResource>,
    ) -> Result<Self, SystemCallError> {
        if domain.account_id() != execution.account_id() {
            return Err(SystemCallError::IllegalArgument);
        }
        Ok(Self {
            domain: domain.clone(),
            execution: execution.clone(),
        })
    }
    pub fn domain(&self) -> &AccountView<ServiceResource> {
        &self.domain
    }
    pub fn execution(&self) -> &AccountView<ExecutionResource> {
        &self.execution
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libbudget::{Budget, Taxonomy};
    #[test]
    fn request_admission_rolls_back_and_storage_outlives_reply() {
        use erhino_shared::message::PAYLOAD_MAX;
        let mut limits = [8; ServiceResource::COUNT + ExecutionResource::COUNT];
        limits[ServiceResource::Bytes.slot()] = 2 * (PAYLOAD_MAX * 2 + 13);
        let budget = Budget::new(&limits, 2).unwrap();
        let mut short = limits;
        // 允许全部回复费用，最后的任务存储准入必须失败。
        short[ServiceResource::Bytes.slot()] = PAYLOAD_MAX * 2 + 12;
        let a = budget.account(&short).unwrap();
        let b = budget.account(&limits).unwrap();
        let domain_slots: [_; ServiceResource::COUNT] =
            core::array::from_fn(|n| budget.slot(n).unwrap());
        let execution_slots: [_; ExecutionResource::COUNT] =
            core::array::from_fn(|n| budget.slot(ServiceResource::COUNT + n).unwrap());
        let a_domain = a.view(&domain_slots).unwrap();
        let b_domain = b.view(&domain_slots).unwrap();
        let a = Funding::new(&a_domain, &a.view(&execution_slots).unwrap()).unwrap();
        let b = Funding::new(&b_domain, &b.view(&execution_slots).unwrap()).unwrap();
        assert!(matches!(
            a.prepare_request::<[u8; 13]>(),
            Err(SystemCallError::QuotaExceeded)
        ));
        assert!(budget.is_empty());
        let (reply, storage) = b.prepare_request::<[u8; 13]>().unwrap();
        assert!(!budget.is_empty());
        for kind in [
            ServiceResource::Request,
            ServiceResource::Outbox,
            ServiceResource::WaitSource,
        ] {
            assert_eq!(a.domain().usage(kind).0, 0);
            assert_eq!(b.domain().usage(kind).0, 1);
        }
        assert_eq!(
            b.domain().usage(ServiceResource::Bytes).0,
            PAYLOAD_MAX * 2 + 13
        );
        drop(reply);
        for kind in [
            ServiceResource::Request,
            ServiceResource::Outbox,
            ServiceResource::WaitSource,
        ] {
            assert_eq!(b.domain().usage(kind).0, 0);
        }
        assert_eq!(a.domain().usage(ServiceResource::Bytes).0, 0);
        assert_eq!(b.domain().usage(ServiceResource::Bytes).0, 13);
        drop(storage);
        assert!(budget.is_empty());
    }

    #[test]
    fn issuance_pair_requires_one_real_account_identity() {
        let limits = [8; ServiceResource::COUNT + ExecutionResource::COUNT];
        let budget = Budget::new(&limits, 2).unwrap();
        let a = budget.account(&limits).unwrap();
        let b = budget.account(&limits).unwrap();
        let domain_slots: [_; ServiceResource::COUNT] =
            core::array::from_fn(|n| budget.slot(n).unwrap());
        let execution_slots: [_; ExecutionResource::COUNT] =
            core::array::from_fn(|n| budget.slot(ServiceResource::COUNT + n).unwrap());
        let domain = a.view(&domain_slots).unwrap();
        let own = a.view(&execution_slots).unwrap();
        let foreign = b.view(&execution_slots).unwrap();
        assert!(matches!(
            Funding::new(&domain, &foreign),
            Err(SystemCallError::IllegalArgument)
        ));
        let funding = Funding::new(&domain, &own).unwrap();
        assert_eq!(
            funding.domain().account_id(),
            funding.execution().account_id()
        );
        drop(a);
        drop(b);
        assert_eq!(funding.clone().execution().account_id(), own.account_id());
    }
}
