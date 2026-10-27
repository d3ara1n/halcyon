//! 一个可 Claim 席位的唯一占用状态。长期任务和能力壳由服务 backing 持有。

use core::num::NonZeroU64;

use crate::{Error, identity::BindingRef};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Preparing,
    ResourcesReady,
    Stopping,
    Released,
}

/// 上层先在认证 context 表中解析主体及范围；本值只负责同主体去重。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subject(NonZeroU64);

impl Subject {
    pub fn from_authenticated_context(context: u64) -> Result<Self, Error> {
        Ok(Self(NonZeroU64::new(context).ok_or(Error::Permission)?))
    }

    pub fn context(self) -> u64 {
        self.0.get()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub reference: BindingRef,
    pub subject: Subject,
    pub phase: Phase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Claim {
    Vacant,
    Existing(Snapshot),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Commit {
    /// 仅这个结果把预付 Binding/任务的责任移交到席位 backing。
    Created(Snapshot),
    /// 原任务已经拥有责任，新候选必须退款，不得再启动 Prepare。
    Existing(Snapshot),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Closed,
    /// 长期 owner 必须继续推进这一轮；重复 Stop 不创建另一份任务。
    Binding(Snapshot),
}

/// 不可 Claim 的目录不创建 Seat。Released 观察壳独立保留原 Snapshot，
/// 不能把此席位的新 epoch 当作旧控制的当前轮。
pub struct Seat {
    device: crate::identity::DeviceId,
    present: bool,
    epoch: u64,
    epoch_closed: bool,
    active: Option<Snapshot>,
}

impl Seat {
    pub fn new(device: crate::identity::DeviceId) -> Self {
        Self {
            device,
            present: true,
            epoch: 1,
            epoch_closed: false,
            active: None,
        }
    }

    pub fn current_epoch(&self) -> u64 {
        self.epoch
    }

    pub fn present(&self) -> bool {
        self.present
    }

    pub fn active(&self) -> Option<Snapshot> {
        self.active
    }

    fn check_ref(&self, reference: BindingRef) -> Result<(), Error> {
        if reference.device() != self.device {
            return Err(Error::StaleInstance);
        }
        if reference.epoch() > self.epoch {
            return Err(Error::InvalidEpoch);
        }
        if reference.epoch() < self.epoch || self.epoch_closed {
            return Err(Error::StaleEpoch);
        }
        Ok(())
    }

    /// `admitted` 是当前来源及全部祖先准入的最终结果，不得缓存到异步任务中。
    /// 来源封口和 Withdraw 后仍允许原主体取回原控制，不扩大权限。
    pub fn claim(
        &self,
        reference: BindingRef,
        subject: Subject,
        admitted: bool,
    ) -> Result<Claim, Error> {
        self.check_ref(reference)?;
        if let Some(active) = self.active {
            return if active.subject == subject {
                Ok(Claim::Existing(active))
            } else {
                Err(Error::Busy)
            };
        }
        if !self.present || !admitted {
            return Err(Error::Closed);
        }
        Ok(Claim::Vacant)
    }

    /// 调用者已预留实际 Binding、控制出口及准备/停止任务；本调用与将这些
    /// owner 链入 backing 位于同一本地提交中。失败不得消耗 epoch。
    pub fn commit_claim(
        &mut self,
        reference: BindingRef,
        subject: Subject,
        admitted: bool,
    ) -> Result<Commit, Error> {
        match self.claim(reference, subject, admitted)? {
            Claim::Existing(active) => Ok(Commit::Existing(active)),
            Claim::Vacant => {
                let active = Snapshot {
                    reference,
                    subject,
                    phase: Phase::Preparing,
                };
                self.active = Some(active);
                Ok(Commit::Created(active))
            }
        }
    }

    /// 表项全部安装及验证后才调用；停止先到时拒绝迟到 Prepared。
    pub fn resources_ready(&mut self, reference: BindingRef) -> Result<(), Error> {
        self.check_ref(reference)?;
        let active = self.active.as_mut().ok_or(Error::NotReady)?;
        match active.phase {
            Phase::Preparing => {
                active.phase = Phase::ResourcesReady;
                Ok(())
            }
            Phase::ResourcesReady => Ok(()),
            Phase::Stopping | Phase::Released => Err(Error::Closed),
        }
    }

    /// 调用者已验证来源的停止权限与范围；manager 只来自实际管理授权表。
    pub fn stop(
        &mut self,
        reference: BindingRef,
        subject: Subject,
        manager: bool,
    ) -> Result<Stop, Error> {
        match self.check_ref(reference) {
            Err(Error::StaleEpoch) => return Ok(Stop::Closed),
            other => other?,
        }
        if let Some(active) = self.active.as_mut() {
            if active.subject != subject && !manager {
                return Err(Error::Permission);
            }
            active.phase = Phase::Stopping;
            return Ok(Stop::Binding(*active));
        }
        self.advance();
        Ok(Stop::Closed)
    }

    /// 祖先撤出时准入先封口，再由预付遍历逐项调用；本地即刻拒绝新 Claim。
    pub fn withdraw(&mut self) -> Option<Snapshot> {
        self.present = false;
        if let Some(active) = self.active.as_mut() {
            active.phase = Phase::Stopping;
        } else {
            self.epoch_closed = true;
        }
        self.active
    }

    /// 只接收原 Provision 明确 Closed 且本轮迟到任务已收束的事实；
    /// timeout、Unavailable、进程 DONE 均不能调用本方法。
    pub fn release(&mut self, reference: BindingRef) -> Result<Snapshot, Error> {
        self.check_ref(reference)?;
        let mut active = self.active.ok_or(Error::NotReady)?;
        if active.phase != Phase::Stopping {
            return Err(Error::NotReady);
        }
        active.phase = Phase::Released;
        self.active = None;
        self.advance();
        Ok(active)
    }

    fn advance(&mut self) {
        if !self.present {
            self.epoch_closed = true;
            return;
        }
        match self.epoch.checked_add(1) {
            Some(next) => self.epoch = next,
            None => self.epoch_closed = true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::DeviceId;

    fn setup() -> (Seat, BindingRef, Subject, Subject) {
        let device = DeviceId::new(9, 1).unwrap();
        (
            Seat::new(device),
            BindingRef::new(device, 1).unwrap(),
            Subject::from_authenticated_context(21).unwrap(),
            Subject::from_authenticated_context(22).unwrap(),
        )
    }

    #[test]
    fn lost_reply_recovers_same_subject_after_source_seal_and_withdraw() {
        let (mut seat, reference, a, b) = setup();
        seat.commit_claim(reference, a, true).unwrap();
        assert_eq!(seat.claim(reference, b, true), Err(Error::Busy));
        seat.withdraw();
        let active = seat.active().unwrap();
        assert_eq!(active.phase, Phase::Stopping);
        assert_eq!(seat.claim(reference, a, false), Ok(Claim::Existing(active)));
        assert_eq!(seat.resources_ready(reference), Err(Error::Closed));
        assert_eq!(seat.stop(reference, b, false), Err(Error::Permission));
        let shell = seat.release(reference).unwrap();
        assert_eq!(shell.phase, Phase::Released);
        assert_eq!(seat.claim(reference, a, true), Err(Error::StaleEpoch));
        assert_eq!(seat.stop(reference, a, false), Ok(Stop::Closed));
        assert_eq!(
            seat.claim(BindingRef::new(reference.device(), 2).unwrap(), a, true),
            Err(Error::InvalidEpoch)
        );
    }

    #[test]
    fn vacant_cancellation_closes_only_issued_epoch() {
        let (mut seat, reference, a, _) = setup();
        let future = BindingRef::new(reference.device(), 2).unwrap();
        assert_eq!(seat.stop(future, a, true), Err(Error::InvalidEpoch));
        assert_eq!(seat.stop(reference, a, false), Ok(Stop::Closed));
        assert!(seat.active().is_none());
        assert_eq!(seat.claim(reference, a, true), Err(Error::StaleEpoch));
        assert_eq!(seat.claim(future, a, true), Ok(Claim::Vacant));
        seat.commit_claim(future, a, true).unwrap();
        assert_eq!(seat.stop(reference, a, true), Ok(Stop::Closed));
        assert_eq!(seat.active().unwrap().reference, future);
    }

    #[test]
    fn preparation_cannot_skip_safe_stopping_before_release() {
        let (mut seat, reference, a, _) = setup();
        assert_eq!(seat.claim(reference, a, false), Err(Error::Closed));
        seat.commit_claim(reference, a, true).unwrap();
        assert_eq!(seat.release(reference), Err(Error::NotReady));
        seat.resources_ready(reference).unwrap();
        assert_eq!(seat.release(reference), Err(Error::NotReady));
        seat.stop(reference, a, false).unwrap();
        seat.release(reference).unwrap();
        assert!(seat.active().is_none());
    }

    #[test]
    fn final_epoch_can_stop_but_never_wrap_or_reclaim() {
        let (mut seat, reference, a, _) = setup();
        seat.epoch = u64::MAX;
        let last = BindingRef::new(reference.device(), u64::MAX).unwrap();
        seat.commit_claim(last, a, true).unwrap();
        seat.stop(last, a, false).unwrap();
        seat.release(last).unwrap();
        assert_eq!(seat.current_epoch(), u64::MAX);
        assert_eq!(seat.stop(last, a, false), Ok(Stop::Closed));
        assert_eq!(seat.claim(last, a, true), Err(Error::StaleEpoch));
        assert_eq!(seat.claim(reference, a, true), Err(Error::StaleEpoch));
    }

    #[test]
    fn withdrawn_vacant_device_does_not_issue_future_epochs() {
        let (mut seat, reference, a, _) = setup();
        seat.withdraw();
        assert_eq!(seat.stop(reference, a, false), Ok(Stop::Closed));
        let future = BindingRef::new(reference.device(), 2).unwrap();
        assert_eq!(seat.stop(future, a, true), Err(Error::InvalidEpoch));
        assert_eq!(seat.claim(future, a, true), Err(Error::InvalidEpoch));
        assert_eq!(seat.current_epoch(), 1);
    }

    #[test]
    fn duplicate_commit_does_not_transfer_second_preparation_owner() {
        let (mut seat, reference, a, _) = setup();
        assert!(matches!(
            seat.commit_claim(reference, a, true),
            Ok(Commit::Created(_))
        ));
        assert!(matches!(
            seat.commit_claim(reference, a, true),
            Ok(Commit::Existing(_))
        ));
    }
}
