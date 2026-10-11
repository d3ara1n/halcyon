//! 文件会话的业务状态；Runnel/映射退休不改变已冻结的终态计数。
use crate::protocol::{Direction, Status, StreamInfo, StreamReason, StreamState};
use erhino_shared::time::Deadline;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    Offered,
    Active,
    Stopping {
        started: bool,
        outcome: Status,
        reason: StreamReason,
    },
    Terminal {
        outcome: Status,
        reason: StreamReason,
    },
}
#[derive(Clone, Copy)]
pub(crate) struct FinishWaiter {
    pub task: u64,
    pub deadline: Deadline,
}
pub(crate) struct State {
    id: u64,
    direction: Direction,
    phase: Phase,
    accepted: u64,
    transported: u64,
    offer_deadline: Deadline,
    session_deadline: Deadline,
    finish: Option<FinishWaiter>,
}
pub(crate) enum Finish {
    Ready(StreamInfo),
    Wait,
}
impl State {
    pub fn new(
        id: u64,
        direction: Direction,
        offer_deadline: Deadline,
        session_deadline: Deadline,
    ) -> Result<Self, Status> {
        if id == 0
            || offer_deadline
                .instant()
                .map_err(|_| Status::Invalid)?
                .is_none()
        {
            return Err(Status::Invalid);
        }
        session_deadline.instant().map_err(|_| Status::Invalid)?;
        Ok(Self {
            id,
            direction,
            phase: Phase::Offered,
            accepted: 0,
            transported: 0,
            offer_deadline,
            session_deadline,
            finish: None,
        })
    }
    pub fn phase(&self) -> Phase {
        self.phase
    }
    pub fn info(&self) -> StreamInfo {
        let (state, outcome, reason) = match self.phase {
            Phase::Offered | Phase::Stopping { started: false, .. } => {
                (StreamState::Offered, Status::Ok, StreamReason::None)
            }
            Phase::Active | Phase::Stopping { started: true, .. } => {
                (StreamState::Active, Status::Ok, StreamReason::None)
            }
            Phase::Terminal { outcome, reason } => (StreamState::Terminal, outcome, reason),
        };
        StreamInfo {
            session_id: self.id,
            state,
            outcome,
            reason,
            accepted: self.accepted,
            transported: self.transported,
        }
    }
    pub fn deadline(&self) -> Deadline {
        match self.phase {
            Phase::Offered => self.offer_deadline,
            Phase::Active => self.session_deadline,
            Phase::Stopping { .. } | Phase::Terminal { .. } => Deadline::INFINITE,
        }
    }
    pub fn observe_time(&mut self, now: u64) {
        if self
            .deadline()
            .instant()
            .ok()
            .flatten()
            .is_some_and(|at| at <= now)
        {
            self.stop(Status::Expired, StreamReason::Expired);
        }
    }
    pub fn start(&mut self, ready: bool, now: u64) -> Result<StreamInfo, Status> {
        self.observe_time(now);
        match self.phase {
            Phase::Offered if ready => {
                self.phase = Phase::Active;
                Ok(self.info())
            }
            Phase::Offered => Err(Status::Busy),
            Phase::Active => Ok(self.info()),
            Phase::Stopping { .. } | Phase::Terminal { .. } => Err(Status::Conflict),
        }
    }
    pub fn stop(&mut self, outcome: Status, reason: StreamReason) {
        let started = match self.phase {
            Phase::Offered => false,
            Phase::Active => true,
            Phase::Stopping { .. } | Phase::Terminal { .. } => return,
        };
        self.phase = Phase::Stopping {
            started,
            outcome,
            reason,
        };
    }
    pub fn cancel(&mut self) -> StreamInfo {
        self.stop(Status::Cancelled, StreamReason::Cancelled);
        self.info()
    }
    /// 宿主只有在不可能再提交后端数据时调用；映射仍可保留在退休链。
    pub fn freeze(&mut self) -> Option<u64> {
        if let Phase::Stopping {
            outcome, reason, ..
        } = self.phase
        {
            self.phase = Phase::Terminal { outcome, reason };
            return self.finish.map(|waiter| waiter.task);
        }
        None
    }
    pub fn add_transported(&mut self, count: usize) -> Result<(), Status> {
        if self.phase != Phase::Active {
            return Err(Status::Conflict);
        }
        self.transported = self
            .transported
            .checked_add(count as u64)
            .ok_or(Status::Range)?;
        Ok(())
    }
    pub fn accept_write(&mut self, count: usize) -> Result<(), Status> {
        if self.direction != Direction::Write
            || !matches!(
                self.phase,
                Phase::Active | Phase::Stopping { started: true, .. }
            )
        {
            return Err(Status::Conflict);
        }
        let accepted = self
            .accepted
            .checked_add(count as u64)
            .ok_or(Status::Range)?;
        if accepted > self.transported {
            return Err(Status::Invalid);
        }
        self.accepted = accepted;
        Ok(())
    }
    pub fn confirm_read(&mut self, accepted: u64) -> Result<(), Status> {
        if self.direction != Direction::Read
            || !matches!(
                self.phase,
                Phase::Active | Phase::Stopping { started: true, .. }
            )
        {
            return Err(Status::Conflict);
        }
        if accepted < self.accepted || accepted > self.transported {
            return Err(Status::Invalid);
        }
        self.accepted = accepted;
        Ok(())
    }
    pub fn finish(&mut self, waiter: FinishWaiter) -> Result<Finish, Status> {
        match self.phase {
            Phase::Offered | Phase::Stopping { started: false, .. } => Err(Status::Conflict),
            Phase::Terminal { .. } => Ok(Finish::Ready(self.info())),
            Phase::Active | Phase::Stopping { started: true, .. } => {
                if self.finish.is_some() {
                    return Err(Status::Busy);
                }
                self.finish = Some(waiter);
                Ok(Finish::Wait)
            }
        }
    }
    pub fn finish_waiter(&self) -> Option<FinishWaiter> {
        self.finish
    }
    pub fn release_finish(&mut self, task: u64) {
        if self.finish.is_some_and(|waiter| waiter.task == task) {
            self.finish = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn start_gate_cancel_and_terminal_freeze_follow_business_contract() {
        let mut state =
            State::new(7, Direction::Write, Deadline::at(10), Deadline::at(50)).unwrap();
        assert!(matches!(state.start(false, 1), Err(Status::Busy)));
        assert_eq!(state.info().state, StreamState::Offered);
        state.start(true, 2).unwrap();
        state.add_transported(8).unwrap();
        state.accept_write(3).unwrap();
        assert_eq!(state.start(false, 3).unwrap().accepted, 3);
        let info = state.cancel();
        assert_eq!(info.state, StreamState::Active);
        assert_eq!(info.reason, StreamReason::None);
        assert_eq!(state.deadline(), Deadline::INFINITE);
        assert!(matches!(state.add_transported(1), Err(Status::Conflict)));
        state.freeze();
        let frozen = state.info();
        assert_eq!(frozen.state, StreamState::Terminal);
        assert_eq!(frozen.outcome, Status::Cancelled);
        assert_eq!(frozen.reason, StreamReason::Cancelled);
        assert_eq!((frozen.accepted, frozen.transported), (3, 8));
        assert!(matches!(state.accept_write(1), Err(Status::Conflict)));
        assert!(matches!(state.start(true, 4), Err(Status::Conflict)));
        state.stop(Status::Unavailable, StreamReason::ProviderStopped);
        state.cancel();
        state.freeze();
        assert_eq!(state.info(), frozen);
    }
    #[test]
    fn finish_is_single_waiter_cancel_does_not_claim_it_and_old_release_is_safe() {
        let mut state =
            State::new(9, Direction::Read, Deadline::at(10), Deadline::INFINITE).unwrap();
        let waiter = FinishWaiter {
            task: 1,
            deadline: Deadline::at(15),
        };
        assert!(matches!(state.finish(waiter), Err(Status::Conflict)));
        state.start(true, 1).unwrap();
        assert!(matches!(state.finish(waiter), Ok(Finish::Wait)));
        assert_eq!(state.finish_waiter().unwrap().deadline, Deadline::at(15));
        assert!(matches!(
            state.finish(FinishWaiter { task: 2, ..waiter }),
            Err(Status::Busy)
        ));
        state.release_finish(1);
        assert!(matches!(
            state.finish(FinishWaiter { task: 2, ..waiter }),
            Ok(Finish::Wait)
        ));
        state.release_finish(1);
        assert_eq!(state.finish_waiter().unwrap().task, 2);
        state.add_transported(6).unwrap();
        state.confirm_read(4).unwrap();
        assert!(matches!(state.confirm_read(3), Err(Status::Invalid)));
        assert!(matches!(state.confirm_read(7), Err(Status::Invalid)));
        state.cancel();
        assert_eq!(state.freeze(), Some(2));
        let Finish::Ready(info) = state.finish(waiter).unwrap() else {
            panic!("terminal Finish was not immediate");
        };
        assert_eq!((info.accepted, info.transported), (4, 6));
        assert!(matches!(state.confirm_read(6), Err(Status::Conflict)));
    }
    #[test]
    fn offer_and_session_deadlines_do_not_extend_one_another() {
        let mut state =
            State::new(11, Direction::Read, Deadline::at(10), Deadline::at(20)).unwrap();
        state.observe_time(10);
        assert!(matches!(
            state.phase(),
            Phase::Stopping { started: false, .. }
        ));
        assert_eq!(state.info().state, StreamState::Offered);
        state.freeze();
        assert_eq!(state.info().reason, StreamReason::Expired);
        let mut state =
            State::new(12, Direction::Write, Deadline::at(10), Deadline::at(20)).unwrap();
        state.start(true, 5).unwrap();
        assert_eq!(state.deadline(), Deadline::at(20));
        state.observe_time(10);
        assert_eq!(state.phase(), Phase::Active);
        state.observe_time(20);
        state.freeze();
        assert_eq!(state.info().reason, StreamReason::Expired);
    }
}
