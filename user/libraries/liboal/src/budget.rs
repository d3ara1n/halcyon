//! 调用方提供的接续限额；一轮调用的所有领域阶段共享同一个余额和期限。

use erhino_shared::time::{ClockSnapshot, Deadline};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalError {
    InvalidPath,
    NoBinding,
    Exists,
    BudgetExceeded,
    Expired,
    InvalidLimits,
    InvalidDeadline,
    ClockRange,
    Resource,
    Protocol,
}

#[derive(Debug, Clone, Copy)]
pub struct FollowLimits {
    calls: u32,
    expanded_bytes: usize,
    timeout_ns: u64,
}

impl FollowLimits {
    pub fn new(calls: u32, expanded_bytes: usize, timeout_ns: u64) -> Result<Self, LocalError> {
        if calls == 0 || expanded_bytes == 0 || timeout_ns == 0 {
            return Err(LocalError::InvalidLimits);
        }
        Ok(Self {
            calls,
            expanded_bytes,
            timeout_ns,
        })
    }

    /// 每次方法入口重新结合当前时间和调用者期限，不储存可被重复使用的绝对期限。
    pub fn start(
        self,
        clock: ClockSnapshot,
        requested: Deadline,
    ) -> Result<FollowBudget, LocalError> {
        let configured = u128::from(clock.now_ns) + u128::from(self.timeout_ns);
        let at = requested
            .instant()
            .map_err(|_| LocalError::InvalidDeadline)?
            .map_or(configured, |at| u128::from(at).min(configured));
        let at = u64::try_from(at).map_err(|_| LocalError::ClockRange)?;
        if at > clock.max_deadline_ns {
            return Err(LocalError::ClockRange);
        }
        if clock.now_ns >= at {
            return Err(LocalError::Expired);
        }
        FollowBudget::new(self.calls, self.expanded_bytes, Deadline::at(at))
    }
}

#[derive(Debug)]
pub struct FollowBudget {
    calls: u32,
    expanded_bytes: usize,
    deadline: Deadline,
}

impl FollowBudget {
    pub fn new(calls: u32, expanded_bytes: usize, deadline: Deadline) -> Result<Self, LocalError> {
        if calls == 0 || expanded_bytes == 0 {
            return Err(LocalError::InvalidLimits);
        }
        if deadline
            .instant()
            .map_err(|_| LocalError::InvalidDeadline)?
            .is_none()
        {
            return Err(LocalError::InvalidDeadline);
        }
        Ok(Self {
            calls,
            expanded_bytes,
            deadline,
        })
    }

    pub fn deadline(&self) -> Deadline {
        self.deadline
    }
    pub fn remaining_calls(&self) -> u32 {
        self.calls
    }
    pub fn remaining_bytes(&self) -> usize {
        self.expanded_bytes
    }

    pub fn before_call(&mut self, now_ns: u64) -> Result<(), LocalError> {
        self.check_deadline(now_ns)?;
        self.calls = self
            .calls
            .checked_sub(1)
            .ok_or(LocalError::BudgetExceeded)?;
        Ok(())
    }

    pub fn check_deadline(&self, now_ns: u64) -> Result<(), LocalError> {
        if now_ns >= self.deadline.at_ns {
            Err(LocalError::Expired)
        } else {
            Ok(())
        }
    }

    /// 先扣输入扫描/新增路径及帧的容量，之后才准备内存。取消不补回已执行的展开工作。
    pub fn consume_bytes(&mut self, bytes: usize) -> Result<(), LocalError> {
        self.expanded_bytes = self
            .expanded_bytes
            .checked_sub(bytes)
            .ok_or(LocalError::BudgetExceeded)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_call_deadline_uses_fresh_clock_and_earliest_limit() {
        // 宿主有限相对超时与调用者绝对期限求最早值，不跨方法复用旧期限。
        let limits = FollowLimits::new(3, 100, 50).unwrap();
        let clock = |now_ns| ClockSnapshot {
            now_ns,
            max_deadline_ns: 1000,
            resolution_ns: 1,
            reserved: 0,
        };
        assert_eq!(
            limits
                .start(clock(10), Deadline::INFINITE)
                .unwrap()
                .deadline(),
            Deadline::at(60)
        );
        assert_eq!(
            limits
                .start(clock(20), Deadline::INFINITE)
                .unwrap()
                .deadline(),
            Deadline::at(70)
        );
        assert_eq!(
            limits
                .start(clock(10), Deadline::at(30))
                .unwrap()
                .deadline(),
            Deadline::at(30)
        );
        assert_eq!(
            limits.start(clock(10), Deadline::at(10)).unwrap_err(),
            LocalError::Expired
        );
        let edge = ClockSnapshot {
            now_ns: u64::MAX - 10,
            max_deadline_ns: u64::MAX,
            resolution_ns: 1,
            reserved: 0,
        };
        assert_eq!(
            limits
                .start(edge, Deadline::at(u64::MAX - 1))
                .unwrap()
                .deadline(),
            Deadline::at(u64::MAX - 1)
        );
        assert_eq!(
            limits.start(edge, Deadline::INFINITE).unwrap_err(),
            LocalError::ClockRange
        );
    }

    #[test]
    fn calls_and_expansion_are_charged_before_work_and_never_reset() {
        // Boundary 循环和多阶段组合必须耗尽原余额，整数下溢不能重新获得额度。
        let mut budget = FollowBudget::new(1, 4, Deadline::at(10)).unwrap();
        budget.consume_bytes(4).unwrap();
        assert_eq!(budget.consume_bytes(1), Err(LocalError::BudgetExceeded));
        assert_eq!(budget.remaining_bytes(), 0);
        budget.before_call(9).unwrap();
        assert_eq!(budget.before_call(9), Err(LocalError::BudgetExceeded));
        assert_eq!(budget.before_call(10), Err(LocalError::Expired));
        assert_eq!(
            FollowLimits::new(0, 1, 1).unwrap_err(),
            LocalError::InvalidLimits
        );
        assert_eq!(
            FollowBudget::new(1, 1, Deadline::INFINITE).unwrap_err(),
            LocalError::InvalidDeadline
        );
    }
}
