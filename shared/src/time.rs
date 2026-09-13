//! 启动内单调时间域、固定宽期限与精确平台换算。

pub type Timestamp = u64;
pub const NANOS_PER_SECOND: u64 = 1_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct Deadline {
    pub kind: u32,
    pub reserved: u32,
    pub at_ns: u64,
}

impl Deadline {
    pub const INFINITE: Self = Self {
        kind: 0,
        reserved: 0,
        at_ns: 0,
    };

    pub const fn at(at_ns: u64) -> Self {
        Self {
            kind: 1,
            reserved: 0,
            at_ns,
        }
    }

    pub const fn instant(self) -> Result<Option<u64>, TimeError> {
        match (self.kind, self.reserved, self.at_ns) {
            (0, 0, 0) => Ok(None),
            (1, 0, at) => Ok(Some(at)),
            _ => Err(TimeError::InvalidDeadline),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C, align(8))]
pub struct ClockSnapshot {
    pub now_ns: u64,
    pub max_deadline_ns: u64,
    pub resolution_ns: u64,
    pub reserved: u64,
}

impl ClockSnapshot {
    pub const fn after_ns(self, duration_ns: u64) -> Result<Deadline, TimeError> {
        match self.now_ns.checked_add(duration_ns) {
            Some(at) if at <= self.max_deadline_ns => Ok(Deadline::at(at)),
            _ => Err(TimeError::OutOfRange),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeError {
    InvalidFrequency,
    InvalidDeadline,
    OutOfRange,
}

/// 一个不跨硬件计数器回绕的时钟 epoch；最大 tick 留给关闭定时器。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockGeometry {
    frequency_hz: u64,
    origin_ticks: u64,
    max_deadline_ns: u64,
}

impl ClockGeometry {
    pub fn new(frequency_hz: u64, origin_ticks: u64) -> Result<Self, TimeError> {
        if frequency_hz == 0 {
            return Err(TimeError::InvalidFrequency);
        }
        let available = (u64::MAX - 1)
            .checked_sub(origin_ticks)
            .ok_or(TimeError::OutOfRange)?;
        let max_ns =
            u128::from(available) * u128::from(NANOS_PER_SECOND) / u128::from(frequency_hz);
        Ok(Self {
            frequency_hz,
            origin_ticks,
            max_deadline_ns: max_ns.min(u128::from(u64::MAX)) as u64,
        })
    }

    pub const fn frequency_hz(self) -> u64 {
        self.frequency_hz
    }
    pub const fn origin_ticks(self) -> u64 {
        self.origin_ticks
    }
    pub const fn max_deadline_ns(self) -> u64 {
        self.max_deadline_ns
    }

    pub fn resolution_ns(self) -> u64 {
        NANOS_PER_SECOND.div_ceil(self.frequency_hz).max(1)
    }

    pub fn elapsed_ns(self, ticks: u64) -> Result<u64, TimeError> {
        if ticks == u64::MAX {
            return Err(TimeError::OutOfRange);
        }
        let elapsed = ticks
            .checked_sub(self.origin_ticks)
            .ok_or(TimeError::OutOfRange)?;
        let ns = u128::from(elapsed) * u128::from(NANOS_PER_SECOND) / u128::from(self.frequency_hz);
        u64::try_from(ns).map_err(|_| TimeError::OutOfRange)
    }

    pub fn deadline_ticks(self, deadline: Deadline) -> Result<Option<u64>, TimeError> {
        let Some(ns) = deadline.instant()? else {
            return Ok(None);
        };
        if ns > self.max_deadline_ns {
            return Err(TimeError::OutOfRange);
        }
        let offset =
            (u128::from(ns) * u128::from(self.frequency_hz)).div_ceil(u128::from(NANOS_PER_SECOND));
        let ticks = u128::from(self.origin_ticks) + offset;
        if ticks >= u128::from(u64::MAX) {
            return Err(TimeError::OutOfRange);
        }
        Ok(Some(ticks as u64))
    }

    pub fn snapshot(self, ticks: u64) -> Result<ClockSnapshot, TimeError> {
        Ok(ClockSnapshot {
            now_ns: self.elapsed_ns(ticks)?,
            max_deadline_ns: self.max_deadline_ns,
            resolution_ns: self.resolution_ns(),
            reserved: 0,
        })
    }
}

const _: () = {
    assert!(core::mem::size_of::<Deadline>() == 16);
    assert!(core::mem::size_of::<ClockSnapshot>() == 32);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deadline_encoding_is_explicit() {
        assert_eq!(Deadline::INFINITE.instant(), Ok(None));
        assert_eq!(Deadline::at(0).instant(), Ok(Some(0)));
        assert_eq!(
            Deadline {
                kind: 0,
                reserved: 0,
                at_ns: 1
            }
            .instant(),
            Err(TimeError::InvalidDeadline)
        );
        assert_eq!(
            Deadline {
                kind: 1,
                reserved: 1,
                at_ns: 0
            }
            .instant(),
            Err(TimeError::InvalidDeadline)
        );
    }

    #[test]
    fn non_integral_frequency_preserves_rounding() {
        let geometry = ClockGeometry::new(32_768, 123).unwrap();
        assert_eq!(geometry.elapsed_ns(124), Ok(30_517));
        assert_eq!(geometry.deadline_ticks(Deadline::at(30_518)), Ok(Some(125)));
        assert_eq!(geometry.elapsed_ns(123 + 32_768), Ok(NANOS_PER_SECOND));
        assert_eq!(geometry.resolution_ns(), 30_518);
    }

    #[test]
    fn epoch_end_and_checked_duration_are_bounded() {
        let geometry = ClockGeometry::new(NANOS_PER_SECOND, u64::MAX - 3).unwrap();
        assert_eq!(geometry.max_deadline_ns(), 2);
        assert_eq!(
            geometry.deadline_ticks(Deadline::at(2)),
            Ok(Some(u64::MAX - 1))
        );
        assert_eq!(
            geometry.deadline_ticks(Deadline::at(3)),
            Err(TimeError::OutOfRange)
        );
        assert_eq!(geometry.elapsed_ns(0), Err(TimeError::OutOfRange));
        let snapshot = geometry.snapshot(u64::MAX - 2).unwrap();
        assert_eq!(snapshot.after_ns(1), Ok(Deadline::at(2)));
        assert_eq!(snapshot.after_ns(u64::MAX), Err(TimeError::OutOfRange));
    }
}
