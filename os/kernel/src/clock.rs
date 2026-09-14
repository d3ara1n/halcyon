//! 单一平台时钟 epoch；读取不取对象锁，允许在投递提交临界区检查期限。

use crate::sbi;
use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
};
use erhino_shared::{
    call::SystemCallError,
    time::{ClockGeometry, ClockSnapshot, Deadline, TimeError},
};

struct GeometryCell {
    state: AtomicU8,
    value: UnsafeCell<MaybeUninit<ClockGeometry>>,
}

// SAFETY: 唯一 boot 写入者在 state=2 的 release 发布前完成初始化，之后只读。
unsafe impl Sync for GeometryCell {}

static GEOMETRY: GeometryCell = GeometryCell {
    state: AtomicU8::new(0),
    value: UnsafeCell::new(MaybeUninit::uninit()),
};
pub(crate) mod selftest;

struct ClockState {
    high_ticks: AtomicU64,
    high_ns: AtomicU64,
    failed: AtomicBool,
}

impl ClockState {
    const fn new(origin: u64) -> Self {
        Self {
            high_ticks: AtomicU64::new(origin),
            high_ns: AtomicU64::new(0),
            failed: AtomicBool::new(false),
        }
    }

    fn read(
        &self,
        geometry: ClockGeometry,
        sample: impl FnOnce() -> u64,
    ) -> Result<ClockSnapshot, SystemCallError> {
        if self.failed.load(Ordering::Acquire) {
            return Err(SystemCallError::ClockRange);
        }
        let prior = self.high_ticks.load(Ordering::Acquire);
        let raw = sample();
        if raw < prior && prior - raw > 1 {
            self.failed.store(true, Ordering::Release);
            return Err(SystemCallError::ClockRange);
        }
        // origin 同样是另一 hart 已发布的样本；允许一 tick 的合法差异，启动时间仍为零。
        let raw = raw.max(geometry.origin_ticks());
        let mut snapshot = geometry.snapshot(raw).map_err(|error| {
            self.failed.store(true, Ordering::Release);
            map_error(error)
        })?;
        self.high_ticks.fetch_max(raw, Ordering::AcqRel);
        snapshot.now_ns = self
            .high_ns
            .fetch_max(snapshot.now_ns, Ordering::AcqRel)
            .max(snapshot.now_ns);
        if self.failed.load(Ordering::Acquire) {
            return Err(SystemCallError::ClockRange);
        }
        Ok(snapshot)
    }
}

static STATE: ClockState = ClockState::new(0);

pub(crate) fn is_failed() -> bool {
    STATE.failed.load(Ordering::Acquire)
}

pub fn init(frequency_hz: u64) {
    let origin = sbi::read_time();
    let geometry =
        ClockGeometry::new(frequency_hz, origin).expect("platform clock geometry is invalid");
    assert!(
        GEOMETRY
            .state
            .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
            .is_ok(),
        "platform clock initialized twice"
    );
    STATE.high_ticks.store(origin, Ordering::Relaxed);
    // SAFETY: compare_exchange 授予唯一初始化权，读取方仅接受已发布的 state=2。
    unsafe { (*GEOMETRY.value.get()).write(geometry) };
    GEOMETRY.state.store(2, Ordering::Release);
}

fn geometry() -> ClockGeometry {
    assert_eq!(
        GEOMETRY.state.load(Ordering::Acquire),
        2,
        "platform clock is not initialized"
    );
    // SAFETY: acquire 已确认 boot 完成初始化，几何以后不变。
    unsafe { (*GEOMETRY.value.get()).assume_init() }
}

pub fn map_error(error: TimeError) -> SystemCallError {
    match error {
        TimeError::InvalidDeadline | TimeError::InvalidFrequency => {
            SystemCallError::IllegalArgument
        }
        TimeError::OutOfRange => SystemCallError::ClockRange,
    }
}

pub fn now() -> Result<ClockSnapshot, SystemCallError> {
    STATE.read(geometry(), sbi::read_time)
}

pub(crate) fn now_ticks() -> Result<u64, SystemCallError> {
    let _ = now()?;
    Ok(STATE.high_ticks.load(Ordering::Acquire))
}

pub fn deadline_ticks(deadline: Deadline) -> Result<Option<u64>, SystemCallError> {
    let _ = now()?;
    geometry().deadline_ticks(deadline).map_err(map_error)
}

pub fn check_delivery(deadline: Deadline) -> Result<(), SystemCallError> {
    let snapshot = now()?;
    if let Some(at) = deadline.instant().map_err(map_error)? {
        if at > snapshot.max_deadline_ns {
            return Err(SystemCallError::ClockRange);
        }
        if snapshot.now_ns >= at {
            return Err(SystemCallError::DeadlineExpired);
        }
    }
    Ok(())
}

pub fn after_ns(duration_ns: u64) -> Result<u64, SystemCallError> {
    let deadline = now()?.after_ns(duration_ns).map_err(map_error)?;
    deadline_ticks(deadline)?.ok_or(SystemCallError::InternalError)
}
