//! 正式 ClockState 的隔离样本编排；失败实例不污染平台时钟。

use super::*;

pub(crate) fn run() {
    let geometry = ClockGeometry::new(1_000_000_000, 100).unwrap();
    let clock = ClockState::new(100);
    assert_eq!(clock.read(geometry, || 99).unwrap().now_ns, 0);
    assert_eq!(clock.read(geometry, || 120).unwrap().now_ns, 20);
    assert_eq!(clock.read(geometry, || 119).unwrap().now_ns, 20);
    let delayed = clock
        .read(geometry, || {
            let old_sample = 125;
            assert_eq!(clock.read(geometry, || 500).unwrap().now_ns, 400);
            old_sample
        })
        .unwrap();
    assert_eq!(
        delayed.now_ns, 400,
        "delayed old sample regressed the published clock"
    );
    assert_eq!(
        clock.read(geometry, || 498),
        Err(SystemCallError::ClockRange)
    );
    assert_eq!(
        clock.read(geometry, || 600),
        Err(SystemCallError::ClockRange),
        "failed clock recovered after a later valid sample"
    );

    let boundary = ClockGeometry::new(1_000_000_000, u64::MAX - 3).unwrap();
    let clock = ClockState::new(boundary.origin_ticks());
    assert_eq!(clock.read(boundary, || u64::MAX - 1).unwrap().now_ns, 2);
    assert_eq!(
        clock.read(boundary, || u64::MAX),
        Err(SystemCallError::ClockRange)
    );
    assert_eq!(
        clock.read(boundary, || boundary.origin_ticks()),
        Err(SystemCallError::ClockRange)
    );
    let clock = ClockState::new(boundary.origin_ticks());
    assert_eq!(clock.read(boundary, || 0), Err(SystemCallError::ClockRange));

    let clock = ClockState::new(100);
    assert_eq!(
        clock.read(geometry, || {
            assert_eq!(
                clock.read(geometry, || 98),
                Err(SystemCallError::ClockRange)
            );
            101
        }),
        Err(SystemCallError::ClockRange),
        "concurrent failure escaped the final failure check"
    );
    assert_eq!(
        clock.read(geometry, || 700),
        Err(SystemCallError::ClockRange)
    );
    info!(
        Task,
        "Clock state checks passed: origin skew, monotonic watermark, delayed sample, irreversible regression, epoch end, concurrent failure"
    );
}
