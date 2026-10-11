//! 任务侧单来源登记寿命：命令入队后推进状态，真实token与退休由Runtime持有。

use erhino_shared::call::SystemCallError;

use crate::{ExecutionResource, Requests, SourceFunding, SourceId, SourceKind, SourcePlan};
use libbudget::AccountView;

#[derive(Debug, Default)]
enum Phase {
    #[default]
    Empty,
    Installing(SourceKind),
    CancellingInstall(SourceKind),
    Attached(SourceId),
    RemovePending(SourceId),
    Removing(SourceId),
}

/// 一个任务角色的登记寿命值，不持handle，不接收集合，也不在Drop隐式注销。
/// 安装中取消保留回执责任；Runtime在实际Remove成功后交付unregistered。
#[derive(Debug, Default)]
pub struct SourceRegistration {
    phase: Phase,
    funding: SourceFunding,
}

impl SourceRegistration {
    pub const fn new() -> Self {
        Self {
            phase: Phase::Empty,
            funding: SourceFunding::Task,
        }
    }

    pub fn funded(view: AccountView<ExecutionResource>) -> Self {
        Self::with_funding(SourceFunding::Bound(view))
    }

    /// 只在逻辑观察出生时选择；相位复位不更换该绑定。
    pub fn with_funding(funding: SourceFunding) -> Self {
        Self {
            phase: Phase::Empty,
            funding,
        }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self.phase, Phase::Empty)
    }

    pub fn is_installing(&self) -> bool {
        matches!(
            self.phase,
            Phase::Installing(_) | Phase::CancellingInstall(_)
        )
    }

    pub fn is_removing(&self) -> bool {
        matches!(self.phase, Phase::Removing(_))
    }

    pub fn source(&self) -> Option<SourceId> {
        match self.phase {
            Phase::Attached(source) | Phase::RemovePending(source) | Phase::Removing(source) => {
                Some(source)
            }
            _ => None,
        }
    }

    /// 命令入队拒绝保持Empty；已有责任则不重复安装。
    pub fn install<T>(
        &mut self,
        requests: &mut Requests<'_, T>,
        plan: SourcePlan,
        kind: SourceKind,
    ) -> Result<bool, SystemCallError> {
        if !self.is_empty() {
            return Ok(false);
        }
        // SourcePlan只含借用handle/掩码描述，没有关闭权或付款owner；
        // 拒绝不改变相位，业务仍持原handle，按原add_source合同报告容量错误。
        match &self.funding {
            SourceFunding::Task => requests.register(plan, kind),
            SourceFunding::Bound(view) => requests.register_bound(plan, kind, view),
        }
        .map_err(|_| SystemCallError::ReachLimit)?;
        self.phase = Phase::Installing(kind);
        Ok(true)
    }

    /// 记录取消意图，不伪造实际Remove；迟到安装回执转RemovePending。
    pub fn cancel(&mut self) {
        self.phase = match self.phase {
            Phase::Installing(kind) | Phase::CancellingInstall(kind) => {
                Phase::CancellingInstall(kind)
            }
            Phase::Attached(source) | Phase::RemovePending(source) => Phase::RemovePending(source),
            Phase::Removing(source) => Phase::Removing(source),
            Phase::Empty => Phase::Empty,
        };
    }

    /// 安装未完成时保留取消责任；有token时只在命令入队成功后转Removing。
    pub fn remove<T>(&mut self, requests: &mut Requests<'_, T>) -> Result<bool, SystemCallError> {
        match self.phase {
            Phase::Attached(source) | Phase::RemovePending(source) => {
                requests.remove(source)?;
                self.phase = Phase::Removing(source);
                Ok(true)
            }
            Phase::Installing(_) | Phase::CancellingInstall(_) => {
                self.cancel();
                Ok(false)
            }
            Phase::Empty | Phase::Removing(_) => Ok(false),
        }
    }

    /// 回执来自本任务已入队命令；不匹配的真实token不能静默丢弃。
    pub fn registered(&mut self, kind: SourceKind, source: SourceId) {
        self.phase = match self.phase {
            Phase::Installing(expected) => {
                assert_eq!(kind, expected, "source receipt changed its role");
                Phase::Attached(source)
            }
            Phase::CancellingInstall(expected) => {
                assert_eq!(kind, expected, "cancelled source receipt changed its role");
                Phase::RemovePending(source)
            }
            Phase::Attached(current) | Phase::RemovePending(current) | Phase::Removing(current)
                if current == source =>
            {
                return;
            }
            _ => panic!("source receipt has no matching installation owner"),
        };
    }

    /// 同一token的实际退休回执才清寿命；旧身份迟到不会清除新登记。
    pub fn unregistered(&mut self, source: SourceId) -> bool {
        if self.source() == Some(source) {
            self.phase = Phase::Empty;
            true
        } else {
            false
        }
    }

    /// 安装拒绝返还命令责任；mask政策与错误处置仍归业务。
    pub fn refused(&mut self, kind: SourceKind) -> bool {
        if matches!(self.phase, Phase::Installing(expected) | Phase::CancellingInstall(expected) if expected == kind)
        {
            self.phase = Phase::Empty;
            true
        } else {
            false
        }
    }
}
