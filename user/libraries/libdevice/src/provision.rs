//! 单个真实供应单元的轮次状态；实际硬件效果与预付任务仍由该 provider 持有。

use crate::Error;
use erhino_shared::object::Rights;
use libkernel::ipc::{capability::Capability, notification::NotificationSignaler};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Preparing,
    Prepared { count: u32 },
    Stopping,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Active {
    pub epoch: u64,
    pub phase: Phase,
    pub failure: Option<Error>,
    pub consumer_quiescent: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prepare {
    Started,
    Existing(Active),
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stop {
    Closed,
    Pending(Active),
}

#[derive(Default)]
pub struct Provision {
    closed_epoch: u64,
    active: Option<Active>,
    signal: Option<(u64, NotificationSignaler)>,
}

impl Provision {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn closed_epoch(&self) -> u64 {
        self.closed_epoch
    }

    pub fn active(&self) -> Option<Active> {
        self.active
    }

    /// 仅接受绑定到本实例的 srv_device context。发行者已提交 Binding，并且
    /// 本地准备/停止 owner 已预付；epoch 可以跳过未曾调用供应者的空闲取消轮。
    fn begin(&mut self, epoch: u64) -> Result<Prepare, Error> {
        Self::valid(epoch)?;
        if epoch <= self.closed_epoch {
            return Ok(Prepare::Closed);
        }
        if let Some(active) = self.active {
            return if active.epoch == epoch {
                Ok(Prepare::Existing(active))
            } else {
                Err(Error::Busy)
            };
        }
        self.closed_epoch = epoch - 1;
        self.active = Some(Active {
            epoch,
            phase: Phase::Preparing,
            failure: None,
            consumer_quiescent: false,
        });
        Ok(Prepare::Started)
    }

    /// 在产生供应效果前固定原轮通知身份；重复 Prepare 只能交同一信号对象。
    pub fn prepare(&mut self, epoch: u64, capability: Capability) -> Result<Prepare, Error> {
        let (signal, description) = NotificationSignaler::from_capability(capability)
            .map_err(|failure| Error::from(failure.error))?;
        if !description.rights.contains(Rights::SIGNAL) {
            return Err(Error::Permission);
        }
        if let Some(active) = self.active
            && active.epoch == epoch
            && self
                .signal
                .as_ref()
                .is_none_or(|(object, _)| *object != description.object_id)
        {
            return Err(Error::Conflict);
        }
        let result = self.begin(epoch)?;
        if result == Prepare::Started {
            self.signal = Some((description.object_id, signal));
        }
        Ok(result)
    }

    fn changed(&self) -> Result<(), Error> {
        if let Some((_, signal)) = &self.signal {
            signal.signal(1)?;
        }
        Ok(())
    }

    /// 整表冻结后才提交；重复返回同一 count，不允许原地改变资源形状。
    pub fn prepared(&mut self, epoch: u64, count: u32) -> Result<(), Error> {
        let active = self.exact(epoch)?;
        match active.phase {
            Phase::Preparing => active.phase = Phase::Prepared { count },
            Phase::Prepared { count: old } if old == count => {}
            Phase::Prepared { .. } => return Err(Error::Conflict),
            Phase::Stopping => return Err(Error::Closed),
        }
        self.changed()
    }

    /// 只有确定没有遗留效果才可 Rejected。已经 Prepared 或停止中的轮次必须
    /// 经过实际清理再 Closed，不能把晚到拒绝当成退款依据。
    pub fn rejected(&mut self, epoch: u64) -> Result<(), Error> {
        if self.exact(epoch)?.phase != Phase::Preparing {
            return Err(Error::NotReady);
        }
        self.close(epoch);
        let notification = self.changed();
        self.signal = None;
        notification
    }

    pub fn failed(&mut self, epoch: u64, reason: Error) -> Result<(), Error> {
        let active = self.exact(epoch)?;
        active.phase = Phase::Stopping;
        // 第一个失败是该轮诊断事实；重复/后续错误不覆盖原原因。
        active.failure.get_or_insert(reason);
        self.changed()
    }

    /// 无 active 时也能关闭已认证发行者指定的轮，封住迟到 Prepare。
    /// 有 active 时不准越轮关闭；不能用未来 Stop 抛弃已有硬件责任。
    pub fn stop(&mut self, epoch: u64, consumer_quiescent: bool) -> Result<Stop, Error> {
        Self::valid(epoch)?;
        if epoch <= self.closed_epoch {
            return Ok(Stop::Closed);
        }
        if let Some(active) = self.active.as_mut() {
            if active.epoch != epoch {
                return Err(Error::Busy);
            }
            active.phase = Phase::Stopping;
            active.consumer_quiescent |= consumer_quiescent;
            let result = Stop::Pending(*active);
            self.changed()?;
            return Ok(result);
        }
        self.closed_epoch = epoch;
        Ok(Stop::Closed)
    }

    /// 由持有实际供应/硬件清理责任的 owner 调用；所有效果及迟到任务已安全结束。
    /// 收到 Terminal、超时或进程死亡本身不满足本方法的调用前提。
    pub fn finish(&mut self, epoch: u64) -> Result<(), Error> {
        Self::valid(epoch)?;
        if epoch <= self.closed_epoch {
            return Ok(());
        }
        if self.exact(epoch)?.phase != Phase::Stopping {
            return Err(Error::NotReady);
        }
        self.close(epoch);
        let notification = self.changed();
        self.signal = None;
        notification
    }

    fn valid(epoch: u64) -> Result<(), Error> {
        if epoch == 0 {
            Err(Error::InvalidEpoch)
        } else {
            Ok(())
        }
    }

    fn exact(&mut self, epoch: u64) -> Result<&mut Active, Error> {
        Self::valid(epoch)?;
        if epoch <= self.closed_epoch {
            return Err(Error::Closed);
        }
        match self.active.as_mut() {
            Some(active) if active.epoch == epoch => Ok(active),
            Some(_) => Err(Error::Busy),
            None => Err(Error::NotReady),
        }
    }

    fn close(&mut self, epoch: u64) {
        self.active = None;
        self.closed_epoch = epoch;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vacant_claim_cancellations_do_not_require_consecutive_prepares() {
        let mut p = Provision::new();
        assert_eq!(p.begin(7), Ok(Prepare::Started));
        assert_eq!(p.closed_epoch(), 6);
        assert_eq!(p.begin(3), Ok(Prepare::Closed));
        assert_eq!(p.stop(3, false), Ok(Stop::Closed));
        assert_eq!(p.begin(8), Err(Error::Busy));
        assert_eq!(p.stop(8, false), Err(Error::Busy));
        p.prepared(7, 2).unwrap();
        p.stop(7, false).unwrap();
        p.finish(7).unwrap();
        assert_eq!(p.begin(19), Ok(Prepare::Started));
    }

    #[test]
    fn stop_before_prepare_blocks_late_effects_including_final_epoch() {
        let mut p = Provision::new();
        assert_eq!(p.stop(4, false), Ok(Stop::Closed));
        assert_eq!(p.begin(4), Ok(Prepare::Closed));
        assert_eq!(p.begin(5), Ok(Prepare::Started));
        p.rejected(5).unwrap();
        assert_eq!(p.stop(u64::MAX, false), Ok(Stop::Closed));
        assert_eq!(p.begin(u64::MAX), Ok(Prepare::Closed));
        assert_eq!(p.begin(0), Err(Error::InvalidEpoch));
    }

    #[test]
    fn failed_or_quiescent_is_not_closed_and_late_prepared_cannot_reopen() {
        let mut p = Provision::new();
        p.begin(1).unwrap();
        p.failed(1, Error::Resource).unwrap();
        assert_eq!(p.closed_epoch(), 0);
        assert_eq!(p.prepared(1, 1), Err(Error::Closed));
        assert_eq!(p.rejected(1), Err(Error::NotReady));
        p.stop(1, true).unwrap();
        assert_eq!(p.closed_epoch(), 0);
        assert!(p.active().unwrap().consumer_quiescent);
        // 真实静止事实可与首次停止一起提交；迟到的较弱意图不得撤销它。
        p.stop(1, false).unwrap();
        assert!(p.active().unwrap().consumer_quiescent);
        p.finish(1).unwrap();
        p.begin(2).unwrap();
        p.stop(1, true).unwrap();
        p.finish(1).unwrap();
        assert!(!p.active().unwrap().consumer_quiescent);
        assert_eq!(p.active().unwrap().epoch, 2);
    }

    #[test]
    fn prepared_shape_is_immutable_and_does_not_imply_safe_release() {
        let mut p = Provision::new();
        p.begin(1).unwrap();
        p.prepared(1, 12).unwrap();
        assert_eq!(p.prepared(1, 12), Ok(()));
        assert_eq!(p.prepared(1, 3), Err(Error::Conflict));
        assert_eq!(p.finish(1), Err(Error::NotReady));
        assert!(!p.active().unwrap().consumer_quiescent);
        p.stop(1, false).unwrap();
        p.finish(1).unwrap();
        assert_eq!(p.begin(1), Ok(Prepare::Closed));
    }

    #[test]
    fn first_stop_can_carry_quiescence_without_a_separate_acknowledgement() {
        let mut p = Provision::new();
        p.begin(4).unwrap();
        p.prepared(4, 2).unwrap();
        let Stop::Pending(active) = p.stop(4, true).unwrap() else {
            panic!("active supply was lost");
        };
        assert_eq!(active.phase, Phase::Stopping);
        assert!(active.consumer_quiescent);
        assert_eq!(p.closed_epoch(), 3);
        p.stop(4, false).unwrap();
        assert!(p.active().unwrap().consumer_quiescent);
        p.finish(4).unwrap();
    }
}
