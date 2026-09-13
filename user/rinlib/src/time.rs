//! 公共单调时间与一次转换的绝对期限。

pub use erhino_shared::time::{ClockSnapshot, Deadline};
use erhino_shared::{call::SystemCallError, time::TimeError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Instant(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Duration(u64);

impl Duration {
    pub const fn from_nanos(ns: u64) -> Self {
        Self(ns)
    }
    pub const fn from_millis(ms: u64) -> Result<Self, SystemCallError> {
        match ms.checked_mul(1_000_000) {
            Some(ns) => Ok(Self(ns)),
            None => Err(SystemCallError::ClockRange),
        }
    }
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
}

impl Instant {
    pub fn now() -> Result<Self, SystemCallError> {
        Ok(Self(snapshot()?.now_ns))
    }
    pub const fn as_nanos(self) -> u64 {
        self.0
    }
}

pub(crate) fn map_error(error: TimeError) -> SystemCallError {
    match error {
        TimeError::InvalidDeadline | TimeError::InvalidFrequency => {
            SystemCallError::IllegalArgument
        }
        TimeError::OutOfRange => SystemCallError::ClockRange,
    }
}

pub fn snapshot() -> Result<ClockSnapshot, SystemCallError> {
    let mut output = ClockSnapshot {
        now_ns: 0,
        max_deadline_ns: 0,
        resolution_ns: 0,
        reserved: 0,
    };
    // SAFETY: output 在 ecall 期间有效且可写。
    unsafe { crate::call::sys_monotonic_now(&mut output) }?;
    if output.reserved != 0 || output.resolution_ns == 0 || output.now_ns > output.max_deadline_ns {
        return Err(SystemCallError::InternalError);
    }
    Ok(output)
}

pub fn after(duration: Duration) -> Result<Deadline, SystemCallError> {
    snapshot()?.after_ns(duration.as_nanos()).map_err(map_error)
}

/// 相对毫秒便利入口：只在这里转换一次，零保持既有无限调用约定。
pub fn timeout_millis(ms: u64) -> Result<Deadline, SystemCallError> {
    if ms == 0 {
        return Ok(Deadline::INFINITE);
    }
    after(Duration::from_millis(ms)?)
}

pub fn expired(deadline: Deadline) -> Result<bool, SystemCallError> {
    let Some(at) = deadline.instant().map_err(map_error)? else {
        return Ok(false);
    };
    let clock = snapshot()?;
    if at > clock.max_deadline_ns {
        return Err(SystemCallError::ClockRange);
    }
    Ok(clock.now_ns >= at)
}

pub fn sleep_until(deadline: Deadline) -> Result<(), SystemCallError> {
    // SAFETY: 期限值在 ecall 复制期间有效，不持有跨调用的临时执行责任。
    unsafe { crate::call::sys_sleep_until(&deadline) }
}
