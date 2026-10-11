//! 执行准入的固定付款绑定；费用由 Task Slot 或实际 SourceEntry 持有。
use crate::ExecutionResource;
use core::fmt;
use libbudget::AccountView;

/// 准入前完整拥有任务、来源上界和付款视图；失败原样返还。
pub struct TaskSubmission<T> {
    task: T,
    max_sources: usize,
    funding: AccountView<ExecutionResource>,
}
impl<T> TaskSubmission<T> {
    pub fn new(task: T, max_sources: usize, funding: AccountView<ExecutionResource>) -> Self {
        Self {
            task,
            max_sources,
            funding,
        }
    }
    pub fn funding(&self) -> &AccountView<ExecutionResource> {
        &self.funding
    }
    pub fn into_parts(self) -> (T, usize, AccountView<ExecutionResource>) {
        (self.task, self.max_sources, self.funding)
    }
}
impl<T: fmt::Debug> fmt::Debug for TaskSubmission<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskSubmission")
            .field("task", &self.task)
            .field("max_sources", &self.max_sources)
            .field("account", &self.funding.account_id())
            .finish()
    }
}

/// Task 模式在命令暂存时解析；Bound 模式从逻辑观察出生起保留同一视图。
#[derive(Clone, Default)]
pub enum SourceFunding {
    #[default]
    Task,
    Bound(AccountView<ExecutionResource>),
}
impl fmt::Debug for SourceFunding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Task => f.write_str("Task"),
            Self::Bound(view) => f.debug_tuple("Bound").field(&view.account_id()).finish(),
        }
    }
}
