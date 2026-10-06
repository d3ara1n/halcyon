//! 工作记录状态机。状态、链接与执行租约由同一队列锁拥有；本模块只给出
//! 转换与内核须执行的动作。Done 即终结，不再承载新责任。

/// 记录状态。`Running.notified` 表示本次运行期间收到过唤醒，禁止按旧观察停驻。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Idle,
    Queued,
    Running { notified: bool },
    Blocked,
    Done,
}

/// step 返回的推进结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// 本轮无工作，记录保留出生承载以便以后再次激活。
    Idle,
    /// 仍有可执行步骤。
    Ready,
    /// 依赖已在来源侧登记并复检未就绪。
    Blocked,
    /// 承诺的完成边界成立。
    Done,
}

/// 内核须在转换后执行的动作。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    None,
    /// 挂到所属账户 Ready 队尾（锁内常数操作）；首次发布同时接管强根。
    Link,
    /// 锁外清理等待登记并交还强根。
    Retire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionError {
    /// 事件不适用于当前状态（内核接线错误）。
    Invalid,
}

/// 一条记录的执行状态与当前等待代次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    state: State,
    wait: u64,
}

/// 一次等待周期的唤醒身份；迟到 token 不作用于新周期。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitTicket(u64);

impl Default for Record {
    fn default() -> Self {
        Self::new()
    }
}

impl Record {
    pub const fn new() -> Self {
        Self {
            state: State::Idle,
            wait: 0,
        }
    }

    pub const fn state(&self) -> State {
        self.state
    }

    /// 责任成立；正在运行时仅标记下一轮，不交出重复执行权。
    pub fn activate(&mut self) -> Result<Action, TransitionError> {
        match self.state {
            State::Idle => {
                self.state = State::Queued;
                Ok(Action::Link)
            }
            State::Queued => Ok(Action::None),
            State::Running { .. } => {
                self.state = State::Running { notified: true };
                Ok(Action::None)
            }
            State::Blocked | State::Done => Err(TransitionError::Invalid),
        }
    }

    /// 执行者取得独占推进权：Queued → Running{false}。
    pub fn pick(&mut self) -> Result<(), TransitionError> {
        match self.state {
            State::Queued => {
                self.state = State::Running { notified: false };
                Ok(())
            }
            _ => Err(TransitionError::Invalid),
        }
    }

    /// Running 中开始一次等待周期；返回的票据交给来源登记。
    /// 先 arm，再在来源锁内登记与复检。
    pub fn arm(&mut self) -> Result<WaitTicket, TransitionError> {
        match self.state {
            State::Running { .. } => {
                self.wait = self.wait.checked_add(1).ok_or(TransitionError::Invalid)?;
                Ok(WaitTicket(self.wait))
            }
            _ => Err(TransitionError::Invalid),
        }
    }

    /// 来源完成后的唤醒。代次不符或 Done 时无动作。
    pub fn wake(&mut self, ticket: WaitTicket) -> Action {
        if ticket.0 != self.wait {
            return Action::None;
        }
        match self.state {
            State::Blocked => {
                self.state = State::Queued;
                Action::Link
            }
            State::Running { .. } => {
                self.state = State::Running { notified: true };
                Action::None
            }
            State::Idle | State::Queued | State::Done => Action::None,
        }
    }

    /// 本轮 step 结束。
    pub fn finish(&mut self, outcome: Outcome) -> Result<Action, TransitionError> {
        let State::Running { notified } = self.state else {
            return Err(TransitionError::Invalid);
        };
        Ok(match outcome {
            Outcome::Idle if notified => {
                self.state = State::Queued;
                Action::Link
            }
            Outcome::Idle => {
                self.state = State::Idle;
                Action::Retire
            }
            Outcome::Ready => {
                self.state = State::Queued;
                Action::Link
            }
            Outcome::Blocked if notified => {
                self.state = State::Queued;
                Action::Link
            }
            Outcome::Blocked => {
                self.state = State::Blocked;
                Action::None
            }
            Outcome::Done => {
                self.state = State::Done;
                Action::Retire
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> Record {
        let mut r = Record::new();
        assert_eq!(r.activate(), Ok(Action::Link));
        r.pick().unwrap();
        r
    }

    #[test]
    fn wake_during_running_prevents_stale_park() {
        // 登记后、提交 Blocked 前完成：不得丢失唤醒。
        let mut r = running();
        let t = r.arm().unwrap();
        assert_eq!(r.wake(t), Action::None);
        assert_eq!(r.finish(Outcome::Blocked), Ok(Action::Link));
        assert_eq!(r.state(), State::Queued);
    }

    #[test]
    fn wake_after_park_requeues_once() {
        let mut r = running();
        let t = r.arm().unwrap();
        assert_eq!(r.finish(Outcome::Blocked), Ok(Action::None));
        assert_eq!(r.wake(t), Action::Link);
        // 重复唤醒不重复入队。
        assert_eq!(r.wake(t), Action::None);
    }

    #[test]
    fn notified_resets_on_pick() {
        let mut r = running();
        let t = r.arm().unwrap();
        r.wake(t);
        r.finish(Outcome::Ready).unwrap();
        r.pick().unwrap();
        assert_eq!(r.state(), State::Running { notified: false });
    }

    #[test]
    fn stale_ticket_is_ignored() {
        let mut r = running();
        let old = r.arm().unwrap();
        let _new = r.arm().unwrap();
        r.finish(Outcome::Blocked).unwrap();
        assert_eq!(r.wake(old), Action::None);
        assert_eq!(r.state(), State::Blocked);
    }

    #[test]
    fn done_is_terminal() {
        let mut r = running();
        let t = r.arm().unwrap();
        assert_eq!(r.finish(Outcome::Done), Ok(Action::Retire));
        assert_eq!(r.wake(t), Action::None);
        assert_eq!(r.activate(), Err(TransitionError::Invalid));
        assert_eq!(r.pick(), Err(TransitionError::Invalid));
    }

    #[test]
    fn running_is_never_picked_twice() {
        let mut r = running();
        assert_eq!(r.pick(), Err(TransitionError::Invalid));
    }

    #[test]
    fn update_before_idle_finish_keeps_the_next_scan_reachable() {
        let mut record = running();
        assert_eq!(record.activate(), Ok(Action::None));
        assert_eq!(record.pick(), Err(TransitionError::Invalid));
        assert_eq!(record.finish(Outcome::Idle), Ok(Action::Link));
        assert_eq!(record.activate(), Ok(Action::None));
        record.pick().unwrap();
        assert_eq!(record.finish(Outcome::Idle), Ok(Action::Retire));
        assert_eq!(record.state(), State::Idle);
        assert_eq!(record.activate(), Ok(Action::Link));
    }

    #[test]
    fn stale_dependency_after_idle_does_not_wake_a_new_activity() {
        let mut record = running();
        let old = record.arm().unwrap();
        assert_eq!(record.finish(Outcome::Idle), Ok(Action::Retire));
        assert_eq!(record.wake(old), Action::None);
        record.activate().unwrap();
        record.pick().unwrap();
        let current = record.arm().unwrap();
        assert_eq!(record.finish(Outcome::Blocked), Ok(Action::None));
        assert_eq!(record.wake(old), Action::None);
        assert_eq!(record.wake(current), Action::Link);
    }
}
