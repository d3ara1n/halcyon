//! 与真实 maintenance::Record 组合核验两项审阅反例，避免以另一套调度模型作预期。
use maintenance::record::{Action, Outcome, Record, State};
use memory_space::{MmioExecutorState, MmioIdle, MmioNotify};

fn running() -> (MmioExecutorState, Record) {
    let mut executor = MmioExecutorState::new();
    let mut record = Record::new();
    assert_eq!(executor.submit(), Some(MmioNotify::Activate));
    assert_eq!(record.activate(), Ok(Action::Link));
    record.pick().unwrap();
    executor.begin_step();
    (executor, record)
}

#[test]
fn enqueue_during_fence_never_activates_blocked_work_or_bypasses_acknowledgement() {
    let (mut executor, mut record) = running();
    let fence = record.arm().unwrap();
    // 发布 fence 与 finish(Blocked) 之间有新队尾：不能置 Running.notified。
    assert_eq!(executor.submit(), Some(MmioNotify::None));
    assert_eq!(record.state(), State::Running { notified: false });
    executor.wait_fence();
    assert_eq!(executor.submit(), Some(MmioNotify::None));
    assert!(!executor.dependency_hint());
    assert_eq!(record.finish(Outcome::Blocked), Ok(Action::None));
    assert_eq!(record.state(), State::Blocked);
    // Frozen 也不能跳过已经发布的 fence。
    assert_eq!(executor.close(), MmioNotify::None);
    assert_eq!(record.state(), State::Blocked);
    assert_eq!(record.wake(fence), Action::Link);
    record.pick().unwrap();
    executor.begin_step();
    assert_eq!(executor.finish_idle(false), MmioIdle::Done);
    assert_eq!(record.finish(Outcome::Done), Ok(Action::Retire));
}

#[test]
fn close_wakes_an_observation_dependency_without_activating_blocked_record() {
    let (mut executor, mut record) = running();
    assert!(executor.arm_dependency());
    let dependency = record.arm().unwrap();
    assert_eq!(record.finish(Outcome::Blocked), Ok(Action::None));
    assert_eq!(executor.submit(), Some(MmioNotify::None));
    assert_eq!(executor.close(), MmioNotify::WakeDependency);
    assert_eq!(record.wake(dependency), Action::Link);
    record.pick().unwrap();
    executor.begin_step();
    assert!(!executor.arm_dependency());
    executor.detach_dependency();
    assert_eq!(executor.finish_idle(false), MmioIdle::Done);
    assert_eq!(record.finish(Outcome::Done), Ok(Action::Retire));
}

#[test]
fn visible_vm_freeze_does_not_finish_before_exit_lease_handoff() {
    let (mut executor, mut record) = running();
    // VM gate 已封但 stop 尚未安装退出租约：本轮只能退到 Idle，不能 Done。
    assert_eq!(executor.finish_idle(false), MmioIdle::Idle);
    assert_eq!(record.finish(Outcome::Idle), Ok(Action::Retire));
    assert_eq!(record.state(), State::Idle);
    // 退出租约安装后在同一队列门内 close + activate，重新取得推进机会。
    assert_eq!(executor.close(), MmioNotify::Activate);
    assert_eq!(record.activate(), Ok(Action::Link));
    record.pick().unwrap();
    executor.begin_step();
    assert_eq!(executor.finish_idle(false), MmioIdle::Done);
    record.finish(Outcome::Done).unwrap();
    assert_eq!(executor.submit(), None);
    assert!(!executor.dependency_hint());
}

#[test]
fn producer_after_idle_probe_is_committed_before_a_terminal_close() {
    let (mut executor, mut record) = running();
    assert_eq!(executor.finish_idle(false), MmioIdle::Idle);
    // 生产者在 finish(Idle) 前进入同一门，必须当场完成维护激活，不能锁外迟发。
    assert_eq!(executor.submit(), Some(MmioNotify::Activate));
    assert_eq!(record.activate(), Ok(Action::None));
    assert_eq!(executor.close(), MmioNotify::None);
    assert_eq!(record.finish(Outcome::Idle), Ok(Action::Link));
    record.pick().unwrap();
    executor.begin_step();
    assert_eq!(executor.finish_idle(true), MmioIdle::Ready);
    record.finish(Outcome::Ready).unwrap();
    record.pick().unwrap();
    executor.begin_step();
    assert_eq!(executor.finish_idle(false), MmioIdle::Done);
    record.finish(Outcome::Done).unwrap();
    // 终态以后拒绝入队，故不存在待提交的 activate(Done)。
    assert_eq!(executor.submit(), None);
    assert_eq!(record.state(), State::Done);
}
