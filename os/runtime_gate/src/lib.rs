//! 多 hart 运行时发布闸门。
//!
//! 闸门只允许 `Preparing -> Ready` 或 `Preparing -> Failed`。失败发布幂等，
//! 便于多个启动失败源竞争广播；Ready 一经发布不可撤销。

#![no_std]
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicU8, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum GateState {
    Preparing = 0,
    Ready = 1,
    Failed = 2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionError {
    AlreadyReady,
    AlreadyFailed,
}

pub struct RuntimeGate {
    state: AtomicU8,
}

impl RuntimeGate {
    pub const fn new() -> Self {
        Self {
            state: AtomicU8::new(GateState::Preparing as u8),
        }
    }

    pub fn state(&self) -> GateState {
        match self.state.load(Ordering::Acquire) {
            1 => GateState::Ready,
            2 => GateState::Failed,
            _ => GateState::Preparing,
        }
    }

    pub fn publish_ready(&self) -> Result<(), TransitionError> {
        match self.state.compare_exchange(
            GateState::Preparing as u8,
            GateState::Ready as u8,
            Ordering::Release,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(()),
            Err(value) if value == GateState::Failed as u8 => Err(TransitionError::AlreadyFailed),
            Err(_) => Err(TransitionError::AlreadyReady),
        }
    }

    pub fn publish_failed(&self) -> Result<(), TransitionError> {
        match self.state.compare_exchange(
            GateState::Preparing as u8,
            GateState::Failed as u8,
            Ordering::Release,
            Ordering::Acquire,
        ) {
            Ok(_) => Ok(()),
            Err(value) if value == GateState::Failed as u8 => Ok(()),
            Err(_) => Err(TransitionError::AlreadyReady),
        }
    }
}

impl Default for RuntimeGate {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use std::{sync::Arc, vec::Vec};

    use super::{GateState, RuntimeGate, TransitionError};

    #[test]
    fn ready_and_failed_are_terminal() {
        let ready = RuntimeGate::new();
        ready.publish_ready().unwrap();
        assert_eq!(ready.state(), GateState::Ready);
        assert_eq!(ready.publish_ready(), Err(TransitionError::AlreadyReady));
        assert_eq!(ready.publish_failed(), Err(TransitionError::AlreadyReady));

        let failed = RuntimeGate::new();
        failed.publish_failed().unwrap();
        failed.publish_failed().unwrap();
        assert_eq!(failed.state(), GateState::Failed);
        assert_eq!(failed.publish_ready(), Err(TransitionError::AlreadyFailed));
    }

    #[test]
    fn ready_racing_failure_has_one_terminal_outcome() {
        for _ in 0..32 {
            let gate = Arc::new(RuntimeGate::new());
            let barrier = Arc::new(std::sync::Barrier::new(3));
            let ready = {
                let gate = gate.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    gate.publish_ready()
                })
            };
            let failure = {
                let gate = gate.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    gate.publish_failed()
                })
            };
            barrier.wait();
            let ready = ready.join().unwrap();
            let failure = failure.join().unwrap();
            match gate.state() {
                GateState::Ready => {
                    assert_eq!(ready, Ok(()));
                    assert_eq!(failure, Err(TransitionError::AlreadyReady));
                    assert_eq!(gate.publish_failed(), Err(TransitionError::AlreadyReady));
                }
                GateState::Failed => {
                    assert_eq!(ready, Err(TransitionError::AlreadyFailed));
                    assert_eq!(failure, Ok(()));
                    assert_eq!(gate.publish_failed(), Ok(()));
                }
                GateState::Preparing => panic!("competing publishers left the gate preparing"),
            }
        }
    }

    #[test]
    fn competing_failure_publishers_converge() {
        let gate = Arc::new(RuntimeGate::new());
        let workers: Vec<_> = (0..8)
            .map(|_| {
                let gate = gate.clone();
                std::thread::spawn(move || gate.publish_failed())
            })
            .collect();
        for worker in workers {
            worker.join().unwrap().unwrap();
        }
        assert_eq!(gate.state(), GateState::Failed);
    }
}
