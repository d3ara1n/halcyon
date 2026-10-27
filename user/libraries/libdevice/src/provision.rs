//! 单个真实供应单元的轮次状态；实际硬件效果与预付任务仍由该 provider 持有。

use crate::Error;

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
    pub stopped_report: bool,
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
    pub fn prepare(&mut self, epoch: u64) -> Result<Prepare, Error> {
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
            stopped_report: false,
        });
        Ok(Prepare::Started)
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
        Ok(())
    }

    /// 只有确定没有遗留效果才可 Rejected。已经 Prepared 或停止中的轮次必须
    /// 经过实际清理再 Closed，不能把晚到拒绝当成退款依据。
    pub fn rejected(&mut self, epoch: u64) -> Result<(), Error> {
        if self.exact(epoch)?.phase != Phase::Preparing {
            return Err(Error::NotReady);
        }
        self.close(epoch);
        Ok(())
    }

    pub fn failed(&mut self, epoch: u64, reason: Error) -> Result<(), Error> {
        let active = self.exact(epoch)?;
        active.phase = Phase::Stopping;
        // 第一个失败是该轮诊断事实；重复/后续错误不覆盖原原因。
        active.failure.get_or_insert(reason);
        Ok(())
    }

    /// 无 active 时也能关闭已认证发行者指定的轮，封住迟到 Prepare。
    /// 有 active 时不准越轮关闭；不能用未来 Stop 抛弃已有硬件责任。
    pub fn stop(&mut self, epoch: u64) -> Result<Stop, Error> {
        Self::valid(epoch)?;
        if epoch <= self.closed_epoch {
            return Ok(Stop::Closed);
        }
        if let Some(active) = self.active.as_mut() {
            if active.epoch != epoch {
                return Err(Error::Busy);
            }
            active.phase = Phase::Stopping;
            return Ok(Stop::Pending(*active));
        }
        self.closed_epoch = epoch;
        Ok(Stop::Closed)
    }

    /// 本轮消费者访问已结束或从未产生的可信事实，不产生 Closed；
    /// provider 自己的硬件准备、子责任和资源退休仍须实际完成。
    pub fn confirm_stopped(&mut self, epoch: u64) -> Result<(), Error> {
        Self::valid(epoch)?;
        if epoch <= self.closed_epoch {
            return Ok(());
        }
        let active = self.exact(epoch)?;
        if active.phase != Phase::Stopping {
            return Err(Error::NotReady);
        }
        active.stopped_report = true;
        Ok(())
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
        Ok(())
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
        assert_eq!(p.prepare(7), Ok(Prepare::Started));
        assert_eq!(p.closed_epoch(), 6);
        assert_eq!(p.prepare(3), Ok(Prepare::Closed));
        assert_eq!(p.stop(3), Ok(Stop::Closed));
        assert_eq!(p.prepare(8), Err(Error::Busy));
        assert_eq!(p.stop(8), Err(Error::Busy));
        p.prepared(7, 2).unwrap();
        p.stop(7).unwrap();
        p.finish(7).unwrap();
        assert_eq!(p.prepare(19), Ok(Prepare::Started));
    }

    #[test]
    fn stop_before_prepare_blocks_late_effects_including_final_epoch() {
        let mut p = Provision::new();
        assert_eq!(p.stop(4), Ok(Stop::Closed));
        assert_eq!(p.prepare(4), Ok(Prepare::Closed));
        assert_eq!(p.prepare(5), Ok(Prepare::Started));
        p.rejected(5).unwrap();
        assert_eq!(p.stop(u64::MAX), Ok(Stop::Closed));
        assert_eq!(p.prepare(u64::MAX), Ok(Prepare::Closed));
        assert_eq!(p.prepare(0), Err(Error::InvalidEpoch));
    }

    #[test]
    fn failed_or_confirmed_is_not_closed_and_late_prepared_cannot_reopen() {
        let mut p = Provision::new();
        p.prepare(1).unwrap();
        p.failed(1, Error::Resource).unwrap();
        assert_eq!(p.closed_epoch(), 0);
        assert_eq!(p.prepared(1, 1), Err(Error::Closed));
        assert_eq!(p.rejected(1), Err(Error::NotReady));
        p.confirm_stopped(1).unwrap();
        assert_eq!(p.closed_epoch(), 0);
        assert!(p.active().unwrap().stopped_report);
        p.finish(1).unwrap();
        p.prepare(2).unwrap();
        p.confirm_stopped(1).unwrap();
        p.finish(1).unwrap();
        assert!(!p.active().unwrap().stopped_report);
        assert_eq!(p.active().unwrap().epoch, 2);
    }

    #[test]
    fn prepared_shape_is_immutable_and_does_not_imply_safe_release() {
        let mut p = Provision::new();
        p.prepare(1).unwrap();
        p.prepared(1, 12).unwrap();
        assert_eq!(p.prepared(1, 12), Ok(()));
        assert_eq!(p.prepared(1, 3), Err(Error::Conflict));
        assert_eq!(p.finish(1), Err(Error::NotReady));
        assert_eq!(p.confirm_stopped(1), Err(Error::NotReady));
        p.stop(1).unwrap();
        p.finish(1).unwrap();
        assert_eq!(p.prepare(1), Ok(Prepare::Closed));
    }
}
