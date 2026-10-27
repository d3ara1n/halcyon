//! 来源发行时冻结的同账户领域/执行付款，不接受线上正文自报 payer。
use crate::{Error, resource::DeviceResource};
use libbudget::AccountView;
use libexecution::ExecutionResource;

#[derive(Clone)]
pub struct Funding {
    domain: AccountView<DeviceResource>,
    execution: AccountView<ExecutionResource>,
}
impl Funding {
    pub fn new(
        domain: AccountView<DeviceResource>,
        execution: AccountView<ExecutionResource>,
    ) -> Result<Self, Error> {
        if domain.account_id() != execution.account_id() {
            return Err(Error::Permission);
        }
        Ok(Self { domain, execution })
    }
    pub fn domain(&self) -> &AccountView<DeviceResource> {
        &self.domain
    }
    pub fn execution(&self) -> &AccountView<ExecutionResource> {
        &self.execution
    }
}
