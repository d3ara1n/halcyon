//! DMA 地址暴露与退休仲裁；物理 pin、授权树和执行承载由内核拥有。
#![no_std]
use erhino_shared::{call::SystemCallError as Error, dma::MappingPhase as Phase};
pub struct MappingState {
    phase: Phase,
    stop: bool,
    readers: usize,
}
impl Default for MappingState {
    fn default() -> Self {
        Self::new()
    }
}
impl MappingState {
    pub const fn new() -> Self {
        Self {
            phase: Phase::Preparing,
            stop: false,
            readers: 0,
        }
    }
    pub const fn phase(&self) -> Phase {
        self.phase
    }
    pub const fn readers(&self) -> usize {
        self.readers
    }
    pub const fn stopped(&self) -> bool {
        self.stop
    }
    pub fn prepared(&mut self, success: bool) -> bool {
        assert!(
            self.phase == Phase::Preparing,
            "DMA preparation completed twice"
        );
        self.phase = if success && !self.stop {
            Phase::ReadyUnexposed
        } else {
            Phase::Cancelling
        };
        self.phase == Phase::ReadyUnexposed
    }
    /// Close/Seal 只能自动取消未暴露准备；已暴露状态保持实际责任。
    pub fn stop(&mut self) {
        self.stop = true;
        if self.phase == Phase::ReadyUnexposed {
            self.phase = Phase::Cancelling;
        }
    }
    pub fn expose(&mut self) -> Result<(), Error> {
        if self.stop || !matches!(self.phase, Phase::ReadyUnexposed | Phase::MayBeExposed) {
            return Err(Error::ObjectClosed);
        }
        let readers = self.readers.checked_add(1).ok_or(Error::ReachLimit)?;
        self.phase = Phase::MayBeExposed;
        self.readers = readers;
        Ok(())
    }
    pub fn release_reader(&mut self) {
        assert!(self.readers != 0, "DMA reader count underflow");
        self.readers -= 1;
    }
    /// 仅在外层已核受信退休 authority 后调用。
    pub fn retire(&mut self) -> Result<(), Error> {
        match self.phase {
            Phase::Preparing => return Err(Error::ObjectBusy),
            Phase::ReadyUnexposed | Phase::MayBeExposed => self.phase = Phase::Retiring,
            _ => {}
        }
        self.stop = true;
        Ok(())
    }
    pub const fn releasing(&self) -> bool {
        matches!(self.phase, Phase::Retiring | Phase::Cancelling)
    }
    pub const fn runnable(&self) -> bool {
        self.releasing() && self.readers == 0
    }
    pub fn finish(&mut self) {
        assert!(
            self.runnable(),
            "DMA release completed before its readers or request"
        );
        self.phase = match self.phase {
            Phase::Retiring => Phase::Retired,
            _ => Phase::Cancelled,
        };
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exposure_is_irreversible_even_when_output_fails_and_all_controls_close() {
        let mut state = MappingState::new();
        assert!(state.prepared(true));
        state.expose().unwrap();
        state.release_reader(); // 包含输出 fault 路径。
        state.stop();
        assert_eq!(state.phase(), Phase::MayBeExposed);
        assert!(!state.runnable());
        assert_eq!(state.expose(), Err(Error::ObjectClosed));
        state.retire().unwrap();
        assert!(state.runnable());
        state.finish();
        state.retire().unwrap();
        assert_eq!(state.phase(), Phase::Retired);
    }
    #[test]
    fn retirement_waits_for_every_accepted_delivery() {
        let mut state = MappingState::new();
        state.prepared(true);
        state.expose().unwrap();
        state.expose().unwrap();
        state.retire().unwrap();
        assert!(!state.runnable());
        assert_eq!(state.expose(), Err(Error::ObjectClosed));
        state.release_reader();
        assert!(!state.runnable());
        state.release_reader();
        assert!(state.runnable());
        state.finish();
    }
    #[test]
    fn seal_cancels_preparation_and_ready_but_never_exposed_pins() {
        let mut preparing = MappingState::new();
        preparing.stop();
        assert!(!preparing.runnable());
        assert!(!preparing.prepared(true));
        assert!(preparing.runnable());
        preparing.finish();
        assert_eq!(preparing.phase(), Phase::Cancelled);
        let mut ready = MappingState::new();
        ready.prepared(true);
        ready.stop();
        assert!(ready.runnable());
        ready.finish();
        assert_eq!(ready.phase(), Phase::Cancelled);
    }
}
